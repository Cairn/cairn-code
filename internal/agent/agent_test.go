package agent

import (
        "context"
        "encoding/json"
        "testing"
        "time"

        "github.com/cairn/cairn-code/internal/config"
        "github.com/cairn/cairn-code/internal/llm"
        "github.com/cairn/cairn-code/internal/tools"
)

// mockProvider implements llm.Provider for testing.
type mockProvider struct {
        sendFn func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error)
        name   string
}

func (m *mockProvider) SendMessage(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
        return m.sendFn(ctx, messages, toolDefs, system, model)
}

func (m *mockProvider) Name() string {
        if m.name != "" {
                return m.name
        }
        return "mock"
}

func (m *mockProvider) AvailableModels() []llm.ModelInfo {
        return []llm.ModelInfo{{ID: "mock-model", Name: "Mock Model", MaxCtx: 4096}}
}

// mockStreamingProvider wraps a mockProvider with streaming capability.
type mockStreamingProvider struct {
        *mockProvider
        streamFn func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string, cb llm.StreamingCallback) (*llm.Response, error)
}

func (m *mockStreamingProvider) StreamMessage(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string, cb llm.StreamingCallback) (*llm.Response, error) {
        return m.streamFn(ctx, messages, toolDefs, system, model, cb)
}

// mockTool is a simple tool for testing.
type mockTool struct {
        name        string
        description string
        execFn      func(ctx context.Context, input json.RawMessage) (string, error)
}

func (m *mockTool) Name() string        { return m.name }
func (m *mockTool) Description() string  { return m.description }
func (m *mockTool) InputSchema() map[string]any {
        return map[string]any{"type": "object", "properties": map[string]any{}}
}
func (m *mockTool) NeedsPermission() bool { return false }
func (m *mockTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
        if m.execFn != nil {
                return m.execFn(ctx, input)
        }
        return "mock output", nil
}

// newTestAgentWithTools creates an agent with a mock provider and specified tools.
func newTestAgentWithTools(provider llm.Provider, toolList []tools.Tool) *Agent {
        reg := tools.NewRegistry()
        for _, t := range toolList {
                reg.Register(t)
        }
        cfg := &config.Config{
                DefaultModel: "mock-model",
                MaxTurns:     10,
        }
        return NewAgent(provider, reg, cfg, &tools.TodoStore{})
}
func newTestAgent(provider llm.Provider) *Agent {
        cfg := &config.Config{
                DefaultModel: "mock-model",
                MaxTurns:     10,
        }
        return NewAgent(provider, tools.NewRegistry(), cfg, &tools.TodoStore{})
}

// TestAgentStopsOnTextResponse verifies the agent loop terminates when the LLM
// returns a plain text response (no tool use).
func TestAgentStopsOnTextResponse(t *testing.T) {
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        return &llm.Response{
                                Content: []llm.ContentBlock{
                                        {Type: "text", Text: "Hello!"},
                                },
                                StopReason: "end_turn",
                                Usage:       llm.Usage{InputTokens: 10, OutputTokens: 5},
                        }, nil
                },
        }

        a := newTestAgent(provider)
        err := a.Run(context.Background(), "hi")
        if err != nil {
                t.Fatalf("agent.Run returned error: %v", err)
        }

        if a.TurnCount() != 1 {
                t.Errorf("expected 1 turn, got %d", a.TurnCount())
        }

        // Should have user message + assistant message
        if len(a.History()) != 2 {
                t.Errorf("expected 2 messages in history, got %d", len(a.History()))
        }
}

// TestAgentStopsOnMaxTokens verifies the agent terminates when stop reason is max_tokens.
func TestAgentStopsOnMaxTokens(t *testing.T) {
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "cutoff..."}},
                                StopReason: "max_tokens",
                                Usage:      llm.Usage{InputTokens: 10, OutputTokens: 8192},
                        }, nil
                },
        }

        a := newTestAgent(provider)
        err := a.Run(context.Background(), "test")
        if err != nil {
                t.Fatalf("agent.Run returned error: %v", err)
        }
        if a.TurnCount() != 1 {
                t.Errorf("expected 1 turn, got %d", a.TurnCount())
        }
}

// TestAgentStopsOnMaxTurns verifies the agent terminates after max turns.
func TestAgentStopsOnMaxTurns(t *testing.T) {
        callCount := 0
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        callCount++
                        // Always return tool use to try to keep looping
                        return &llm.Response{
                                Content: []llm.ContentBlock{
                                        {Type: "tool_use", ID: "call_1", Name: "bash", Input: map[string]any{"command": "echo hi"}},
                                },
                                StopReason: "tool_use",
                        }, nil
                },
        }

        // Set max turns to 3
        cfg := &config.Config{DefaultModel: "mock", MaxTurns: 3}
        a := NewAgent(provider, tools.NewRegistry(), cfg, &tools.TodoStore{})

        err := a.Run(context.Background(), "test")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        if callCount != 3 {
                t.Errorf("expected 3 LLM calls (max turns), got %d", callCount)
        }
        if a.TurnCount() != 3 {
                t.Errorf("expected turn count 3, got %d", a.TurnCount())
        }
}

// TestAgentContextCancellation verifies the agent stops when context is cancelled.
func TestAgentContextCancellation(t *testing.T) {
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        // Simulate a slow/hanging API call
                        select {
                        case <-ctx.Done():
                                return nil, ctx.Err()
                        case <-time.After(30 * time.Second):
                                return &llm.Response{
                                        Content:    []llm.ContentBlock{{Type: "text", Text: "late response"}},
                                        StopReason: "end_turn",
                                }, nil
                        }
                },
        }

        a := newTestAgent(provider)

        ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
        defer cancel()

        err := a.Run(ctx, "test")
        if err == nil {
                t.Fatal("expected error from cancelled context, got nil")
        }
}

// TestAgentEmptyResponseDoesNotPanic verifies the agent handles empty LLM responses
// without panicking or getting stuck.
func TestAgentEmptyResponseDoesNotPanic(t *testing.T) {
        callCount := 0
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        callCount++
                        // First call: empty response, second call: text response
                        if callCount == 1 {
                                return &llm.Response{
                                        Content:    []llm.ContentBlock{},
                                        StopReason: "end_turn",
                                }, nil
                        }
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "ok"}},
                                StopReason: "end_turn",
                        }, nil
                },
        }

        a := newTestAgent(provider)
        err := a.Run(context.Background(), "test")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        // Should have stopped after the empty response (no tool use → break)
        if a.TurnCount() != 1 {
                t.Errorf("expected 1 turn (empty response should break), got %d", a.TurnCount())
        }
}

// TestAgentToolUseLoop verifies the agent correctly handles a tool_use → tool_result cycle.
func TestAgentToolUseLoop(t *testing.T) {
        callCount := 0
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        callCount++
                        if callCount == 1 {
                                return &llm.Response{
                                        Content: []llm.ContentBlock{
                                                {Type: "text", Text: "Let me check."},
                                                {Type: "tool_use", ID: "call_1", Name: "bash", Input: map[string]any{"command": "ls"}},
                                        },
                                        StopReason: "tool_use",
                                }, nil
                        }
                        return &llm.Response{
                                Content: []llm.ContentBlock{
                                        {Type: "text", Text: "Here are the results."},
                                },
                                StopReason: "end_turn",
                        }, nil
                },
        }

        a := newTestAgentWithTools(provider, []tools.Tool{
                &mockTool{name: "bash", description: "run commands"},
        })

        var toolName string
        a.SetCallbacks(Callbacks{
                OnToolUse: func(name string, input any) {
                        toolName = name
                },
                OnToolResult: func(name string, output string, duration time.Duration) {
                        _ = output // tool result simulated
                },
        })

        err := a.Run(context.Background(), "list files")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        if a.TurnCount() != 2 {
                t.Errorf("expected 2 turns, got %d", a.TurnCount())
        }
        if toolName != "bash" {
                t.Errorf("expected tool name 'bash', got '%s'", toolName)
        }
}

// TestStreamingProviderFiresCallbacks verifies OnStreamChunk is called per-token
// and OnText is called with the full accumulated text.
func TestStreamingProviderFiresCallbacks(t *testing.T) {
        provider := &mockStreamingProvider{
                mockProvider: &mockProvider{name: "mock-stream"},
                streamFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string, cb llm.StreamingCallback) (*llm.Response, error) {
                        // Simulate streaming tokens
                        chunks := []string{"Hello", " ", "world", "!"}
                        for _, c := range chunks {
                                cb(c, "text", false)
                        }
                        cb("", "text", true)
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "Hello world!"}},
                                StopReason: "end_turn",
                                Usage:      llm.Usage{InputTokens: 5, OutputTokens: 4},
                        }, nil
                },
        }

        cfg := &config.Config{DefaultModel: "mock-model", MaxTurns: 10}
        a := NewAgent(provider, tools.NewRegistry(), cfg, &tools.TodoStore{})

        var chunkCalls []string
        var fullText string
        a.SetCallbacks(Callbacks{
                OnStreamChunk: func(chunk string) {
                        chunkCalls = append(chunkCalls, chunk)
                },
                OnText: func(text string) {
                        fullText = text
                },
        })

        err := a.Run(context.Background(), "test")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        if len(chunkCalls) != 4 {
                t.Errorf("expected 4 OnStreamChunk calls, got %d", len(chunkCalls))
        }
        if fullText != "Hello world!" {
                t.Errorf("expected full text 'Hello world!', got '%s'", fullText)
        }
}

// TestAgentHistoryGrowth verifies messages accumulate correctly across turns.
func TestAgentHistoryGrowth(t *testing.T) {
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "response"}},
                                StopReason: "end_turn",
                        }, nil
                },
        }

        a := newTestAgent(provider)

        // First run
        a.Run(context.Background(), "prompt1")
        if len(a.History()) != 2 {
                t.Errorf("expected 2 messages after first run, got %d", len(a.History()))
        }

        // Second run
        a.Run(context.Background(), "prompt2")
        if len(a.History()) != 4 {
                t.Errorf("expected 4 messages after second run, got %d", len(a.History()))
        }
}

// TestAgentReset verifies Reset clears history and turn count.
func TestAgentReset(t *testing.T) {
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "ok"}},
                                StopReason: "end_turn",
                        }, nil
                },
        }

        a := newTestAgent(provider)
        a.Run(context.Background(), "test")

        if len(a.History()) == 0 {
                t.Error("expected non-empty history before reset")
        }

        a.Reset()

        if len(a.History()) != 0 {
                t.Errorf("expected empty history after reset, got %d messages", len(a.History()))
        }
        if a.TurnCount() != 0 {
                t.Errorf("expected turn count 0 after reset, got %d", a.TurnCount())
        }
}
