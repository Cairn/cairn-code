package agent

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/cairn/cairn-code/internal/config"
	"github.com/cairn/cairn-code/internal/llm"
	"github.com/cairn/cairn-code/internal/tools"
)

// Agent is the core coding agent that orchestrates LLM interactions and tool execution.
type Agent struct {
	provider  llm.Provider
	tools     *tools.Registry
	config    *config.Config
	messages  []llm.Message
	system    string
	todoStore *tools.TodoStore
	turnCount int
	model     string
	callbacks Callbacks
}

// TodoItem is an alias for the shared TodoItem type.
type TodoItem = tools.TodoItem

// Callbacks provides hooks for the UI to observe agent behavior.
type Callbacks struct {
	OnText       func(text string)
	OnToolUse    func(name string, input any)
	OnToolResult func(name string, output string, duration time.Duration)
	OnTurnStart  func(turn int)
	OnTurnEnd    func(turn int, usage llm.Usage)
	OnError      func(err error)
	OnPermission func(tool string, input any) bool // return true to allow
}

// NewAgent creates a new Agent with the given provider, tools, and config.
func NewAgent(provider llm.Provider, toolRegistry *tools.Registry, cfg *config.Config, todoStore *tools.TodoStore) *Agent {
	return &Agent{
		provider:  provider,
		tools:     toolRegistry,
		config:    cfg,
		todoStore: todoStore,
		model:     cfg.DefaultModel,
	}
}

// SetModel sets the model to use for inference.
func (a *Agent) SetModel(model string) {
	a.model = model
}

// Run executes the main agent loop with the given prompt.
func (a *Agent) Run(ctx context.Context, prompt string) error {
	// Append user message
	a.messages = append(a.messages, llm.Message{
		Role:    llm.RoleUser,
		Content: prompt,
	})

	maxTurns := a.config.MaxTurns
	if maxTurns <= 0 {
		maxTurns = 100
	}

	for {
		if a.turnCount >= maxTurns {
			if a.callbacks.OnError != nil {
				a.callbacks.OnError(fmt.Errorf("max turns (%d) reached", maxTurns))
			}
			break
		}

		// Build system prompt
		system := a.buildSystemPrompt()

		// Notify turn start
		if a.callbacks.OnTurnStart != nil {
			a.callbacks.OnTurnStart(a.turnCount + 1)
		}

		// Call provider — use streaming if available
		toolDefs := a.tools.ToolDefinitions()
		var resp *llm.Response
		var err error

		if sp, ok := a.provider.(llm.StreamingProvider); ok {
			// Streaming path: accumulate text chunks via callback
			resp, err = a.callStreaming(ctx, sp, toolDefs, system)
		} else {
			// Non-streaming fallback
			resp, err = a.provider.SendMessage(ctx, a.messages, toolDefs, system, a.model)
		}

		if err != nil {
			if a.callbacks.OnError != nil {
				a.callbacks.OnError(fmt.Errorf("LLM error: %w", err))
			}
			return fmt.Errorf("provider error: %w", err)
		}

		a.turnCount++

		// Append assistant message FIRST (with tool_use blocks, before tool results)
		assistantBlocks := make([]llm.ContentBlock, 0, len(resp.Content))
		for _, block := range resp.Content {
			if block.Type == "text" || block.Type == "tool_use" {
				assistantBlocks = append(assistantBlocks, block)
			}
		}
		a.messages = append(a.messages, llm.Message{
			Role:    llm.RoleAssistant,
			Content: assistantBlocks,
		})

		// Process response (text display + tool execution)
		hasToolUse := false
		for _, block := range resp.Content {
			switch block.Type {
			case "text":
				if block.Text != "" {
					if a.callbacks.OnText != nil {
						a.callbacks.OnText(block.Text)
					}
				}

			case "tool_use":
				hasToolUse = true
				if err := a.processToolUse(ctx, block); err != nil {
					if a.callbacks.OnError != nil {
						a.callbacks.OnError(err)
					}
					// Append error as tool result
					a.messages = append(a.messages, llm.Message{
						Role: llm.RoleAssistant,
						Content: []llm.ContentBlock{
							ToolResultBlock(block.ID, fmt.Sprintf("Error: %v", err), true),
						},
					})
					continue
				}
			}
		}

		// Notify turn end
		if a.callbacks.OnTurnEnd != nil {
			a.callbacks.OnTurnEnd(a.turnCount, resp.Usage)
		}

		// If no tool use, we're done
		if !hasToolUse {
			break
		}

		// Check stop reason
		if resp.StopReason == "end_turn" || resp.StopReason == "max_tokens" {
			break
		}
	}

	return nil
}

// callStreaming invokes the streaming provider and feeds text chunks to the callback.
func (a *Agent) callStreaming(ctx context.Context, sp llm.StreamingProvider, tools []llm.ToolDefinition, system string) (*llm.Response, error) {
	var streamedText strings.Builder

	resp, err := sp.StreamMessage(ctx, a.messages, tools, system, a.model, func(chunk string, done bool) {
		if !done && chunk != "" {
			streamedText.WriteString(chunk)
			if a.callbacks.OnText != nil {
				a.callbacks.OnText(chunk)
			}
		}
	})
	if err != nil {
		return nil, err
	}

	// For streaming responses, the text was already sent via callback chunk-by-chunk.
	// We still need to make sure the response Content blocks have the full text.
	// The StreamMessage implementation already assembles the full response, so resp.Content
	// should contain the complete text block(s) and tool_use blocks.

	// Handle tool_use notifications from the final response
	for _, block := range resp.Content {
		if block.Type == "tool_use" && a.callbacks.OnToolUse != nil {
			a.callbacks.OnToolUse(block.Name, block.Input)
		}
	}

	return resp, nil
}

// processToolUse handles a tool use content block.
func (a *Agent) processToolUse(ctx context.Context, block llm.ContentBlock) error {
	toolName := block.Name
	toolInput := block.Input

	// Check if tool exists
	t, ok := a.tools.Get(toolName)
	if !ok {
		errMsg := fmt.Sprintf("unknown tool: %s", toolName)
		a.messages = append(a.messages, llm.Message{
			Role: llm.RoleAssistant,
			Content: []llm.ContentBlock{
				llm.ToolResultBlock(block.ID, errMsg, true),
			},
		})
		return fmt.Errorf("%s", errMsg)
	}

	// Check permissions
	if t.NeedsPermission() && a.callbacks.OnPermission != nil {
		allowed := a.callbacks.OnPermission(toolName, toolInput)
		if !allowed {
			errMsg := fmt.Sprintf("permission denied for tool: %s", toolName)
			a.messages = append(a.messages, llm.Message{
				Role: llm.RoleAssistant,
				Content: []llm.ContentBlock{
					llm.ToolResultBlock(block.ID, errMsg, true),
				},
			})
			return fmt.Errorf("%s", errMsg)
		}
	}

	// Notify tool use (skip if streaming already notified)
	if a.callbacks.OnToolUse != nil {
		a.callbacks.OnToolUse(toolName, toolInput)
	}

	// Execute tool
	inputJSON, _ := json.Marshal(toolInput)
	start := time.Now()
	output, err := t.Execute(ctx, inputJSON)
	duration := time.Since(start)

	if err != nil {
		errMsg := fmt.Sprintf("Error: %v", err)
		if a.callbacks.OnToolResult != nil {
			a.callbacks.OnToolResult(toolName, errMsg, duration)
		}
		a.messages = append(a.messages, llm.Message{
			Role: llm.RoleAssistant,
			Content: []llm.ContentBlock{
				llm.ToolResultBlock(block.ID, errMsg, true),
			},
		})
		return nil // Don't propagate error; we've already recorded it
	}

	// Notify tool result
	if a.callbacks.OnToolResult != nil {
		a.callbacks.OnToolResult(toolName, output, duration)
	}

	// Append tool result to messages
	a.messages = append(a.messages, llm.Message{
		Role: llm.RoleAssistant,
		Content: []llm.ContentBlock{
			llm.ToolResultBlock(block.ID, output, false),
		},
	})

	return nil
}

// buildSystemPrompt constructs the system prompt from config, CAIRN.md, and context.
func (a *Agent) buildSystemPrompt() string {
	var parts []string

	// Load CAIRN.md if it exists
	promptFile := a.config.SystemPromptFile
	if promptFile == "" {
		promptFile = "CAIRN.md"
	}

	data, err := os.ReadFile(promptFile)
	if err == nil {
		parts = append(parts, string(data))
	}

	// Add todo state
	if len(a.todoStore.Items) > 0 {
		var todoText strings.Builder
		todoText.WriteString("\n## Current Todo List\n")
		for i, item := range a.todoStore.Items {
			marker := "○"
			switch item.Status {
			case "in_progress":
				marker = "●"
			case "completed":
				marker = "✓"
			}
			fmt.Fprintf(&todoText, "%d. %s %s\n", i+1, marker, item.Content)
		}
		parts = append(parts, todoText.String())
	}

	// Add tool descriptions summary
	var toolDesc strings.Builder
	toolDesc.WriteString("\n## Available Tools\n")
	for _, t := range a.tools.All() {
		needsPerm := ""
		if t.NeedsPermission() {
			needsPerm = " (requires permission)"
		}
		fmt.Fprintf(&toolDesc, "- **%s**%s: %s\n", t.Name(), needsPerm, t.Description())
	}
	parts = append(parts, toolDesc.String())

	return strings.Join(parts, "\n\n")
}

// History returns the conversation history.
func (a *Agent) History() []llm.Message {
	return a.messages
}

// Reset clears the conversation history and turn counter.
func (a *Agent) Reset() {
	a.messages = nil
	a.turnCount = 0
}

// SetMessages replaces the agent's message history (used for session resume).
func (a *Agent) SetMessages(msgs []llm.Message) {
	a.messages = msgs
}

// TurnCount returns the current turn count.
func (a *Agent) TurnCount() int {
	return a.turnCount
}

// SetCallbacks sets the agent callbacks.
func (a *Agent) SetCallbacks(cb Callbacks) {
	a.callbacks = cb
}

// Model returns the current model name.
func (a *Agent) Model() string {
	return a.model
}

// Provider returns the current LLM provider name.
func (a *Agent) ProviderName() string {
	return a.provider.Name()
}

// Compact summarizes the conversation history using the LLM and replaces it with a compact summary.
func (a *Agent) Compact(ctx context.Context) error {
	if len(a.messages) == 0 {
		return nil
	}

	// Build a representation of the conversation for summarization
	var conv strings.Builder
	for _, msg := range a.messages {
		switch msg.Role {
		case llm.RoleUser:
			conv.WriteString(fmt.Sprintf("User: %s\n", llm.ExtractText(msg.Content)))
		case llm.RoleAssistant:
			blocks := llm.AsTextBlocks(msg.Content)
			for _, b := range blocks {
				switch b.Type {
				case "text":
					conv.WriteString(fmt.Sprintf("Assistant: %s\n", b.Text))
				case "tool_use":
					conv.WriteString(fmt.Sprintf("Assistant: [tool call: %s]\n", b.Name))
				case "tool_result":
					// Truncate long tool results
					content := b.Content
					if len(content) > 200 {
						content = content[:200] + "..."
					}
					conv.WriteString(fmt.Sprintf("Tool result: %s\n", content))
				}
			}
		}
	}

	compactPrompt := fmt.Sprintf(
		"Please summarize the following conversation concisely, preserving:\n"+
			"- Key decisions and conclusions reached\n"+
			"- Important context about the codebase or task\n"+
			"- Any files that were read or modified (names only)\n"+
			"- The current state of the task and what remains to be done\n\n"+
			"Keep the summary under 500 words. Focus on actionable information.\n\n"+
			"Conversation:\n%s",
		conv.String(),
	)

	// Create a temporary set of messages for the summarization request
	compactMessages := []llm.Message{
		{Role: llm.RoleUser, Content: compactPrompt},
	}

	resp, err := a.provider.SendMessage(ctx, compactMessages, nil, "You are a helpful assistant that summarizes conversations concisely.", a.model)
	if err != nil {
		return fmt.Errorf("compaction failed: %w", err)
	}

	// Extract summary text
	summary := llm.ExtractText(resp.Content)
	if summary == "" {
		return fmt.Errorf("compaction failed: empty summary")
	}

	// Replace messages with a summary
	a.messages = []llm.Message{
		{Role: llm.RoleUser, Content: "[Previous conversation was compacted into a summary]"},
		{Role: llm.RoleAssistant, Content: summary},
	}

	return nil
}

// ToolResultBlock is an alias for clarity.
func ToolResultBlock(toolUseID, content string, isError bool) llm.ContentBlock {
	return llm.ToolResultBlock(toolUseID, content, isError)
}
