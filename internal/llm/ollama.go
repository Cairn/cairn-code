package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/cairn/cairn-code/internal/config"
)

// OllamaProvider implements the Provider interface for Ollama's local API.
type OllamaProvider struct {
	baseURL string
	client  *http.Client
}

// NewOllamaProvider creates a new Ollama provider.
func NewOllamaProvider(cfg *config.Config) *OllamaProvider {
	return &OllamaProvider{
		baseURL: cfg.GetOllamaBaseURL(),
		client:  &http.Client{Timeout: 300 * time.Second},
	}
}

// Name returns the provider name.
func (p *OllamaProvider) Name() string {
	return "ollama"
}

// AvailableModels returns a placeholder list for Ollama models.
func (p *OllamaProvider) AvailableModels() []ModelInfo {
	return []ModelInfo{
		{ID: "llama3", Name: "Llama 3", MaxCtx: 8192},
		{ID: "codellama", Name: "Code Llama", MaxCtx: 16384},
		{ID: "mistral", Name: "Mistral", MaxCtx: 32768},
	}
}

// ollamaRequest is the request format for Ollama's API.
type ollamaRequest struct {
	Model    string           `json:"model"`
	Messages []ollamaMessage  `json:"messages"`
	Tools    []ollamaTool     `json:"tools,omitempty"`
	Stream   bool             `json:"stream"`
	Options  map[string]any   `json:"options,omitempty"`
}

type ollamaMessage struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

type ollamaTool struct {
	Type     string         `json:"type"`
	Function ollamaFuncDef  `json:"function"`
}

type ollamaFuncDef struct {
	Name        string         `json:"name"`
	Description string         `json:"description"`
	Parameters  map[string]any `json:"parameters"`
}

// ollamaResponse is the response format from Ollama's API.
type ollamaResponse struct {
	Model     string          `json:"model"`
	Message   ollamaMessage   `json:"message"`
	ToolCalls []ollamaToolCall `json:"tool_calls,omitempty"`
	Done      bool            `json:"done"`
}

type ollamaToolCall struct {
	Function ollamaToolCallFunc `json:"function"`
}

type ollamaToolCallFunc struct {
	Name      string `json:"name"`
	Arguments any    `json:"arguments"`
}

// SendMessage sends a message to the Ollama API.
func (p *OllamaProvider) SendMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string) (*Response, error) {
	if model == "" {
		model = "llama3"
	}

	// Convert messages to Ollama format
	ollMessages := make([]ollamaMessage, 0, len(messages)+1)

	// Add system message if provided
	if system != "" {
		ollMessages = append(ollMessages, ollamaMessage{
			Role:    "system",
			Content: system,
		})
	}

	for _, msg := range messages {
		if msg.Role == RoleSystem {
			continue
		}
		text := ExtractText(msg.Content)
		ollMessages = append(ollMessages, ollamaMessage{
			Role:    string(msg.Role),
			Content: text,
		})
	}

	// Build request
	reqBody := ollamaRequest{
		Model:    model,
		Messages: ollMessages,
		Stream:   false,
	}

	// Convert tools
	if len(tools) > 0 {
		ollTools := make([]ollamaTool, 0, len(tools))
		for _, t := range tools {
			ollTools = append(ollTools, ollamaTool{
				Type: "function",
				Function: ollamaFuncDef{
					Name:        t.Name,
					Description: t.Description,
					Parameters:  t.InputSchema,
				},
			})
		}
		reqBody.Tools = ollTools
	}

	jsonBody, err := json.Marshal(reqBody)
	if err != nil {
		return nil, fmt.Errorf("marshaling request: %w", err)
	}

	// Create HTTP request
	url := p.baseURL + "/api/chat"
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(jsonBody))
	if err != nil {
		return nil, fmt.Errorf("creating request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")

	// Send request
	resp, err := p.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("sending request: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("ollama API error (status %d): %s", resp.StatusCode, string(body))
	}

	// Parse response
	var ollResp ollamaResponse
	if err := json.Unmarshal(body, &ollResp); err != nil {
		return nil, fmt.Errorf("parsing response: %w", err)
	}

	// Convert to our format
	return p.convertResponse(&ollResp), nil
}

// convertResponse converts an Ollama response to our format.
func (p *OllamaProvider) convertResponse(ollResp *ollamaResponse) *Response {
	var blocks []ContentBlock

	// Add text content
	if ollResp.Message.Content != "" {
		blocks = append(blocks, ContentBlock{Type: "text", Text: ollResp.Message.Content})
	}

	// Convert tool calls
	for _, tc := range ollResp.ToolCalls {
		blocks = append(blocks, ContentBlock{
			Type:  "tool_use",
			ID:    fmt.Sprintf("call_%s", tc.Function.Name),
			Name:  tc.Function.Name,
			Input: tc.Function.Arguments,
		})
	}

	stopReason := "end_turn"
	if len(ollResp.ToolCalls) > 0 {
		stopReason = "tool_use"
	}

	return &Response{
		Content:    blocks,
		StopReason: stopReason,
		Model:      ollResp.Model,
		Usage: Usage{
			InputTokens:  0, // Ollama doesn't reliably report tokens
			OutputTokens: 0,
		},
	}
}
