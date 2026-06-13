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

// AnthropicProvider implements the Provider interface for Anthropic's API.
type AnthropicProvider struct {
	apiKey  string
	baseURL string
	client  *http.Client
}

// NewAnthropicProvider creates a new Anthropic provider.
func NewAnthropicProvider(cfg *config.Config) *AnthropicProvider {
	return &AnthropicProvider{
		apiKey:  cfg.GetAnthropicAPIKey(),
		baseURL: cfg.GetAnthropicBaseURL(),
		client:  &http.Client{Timeout: 120 * time.Second},
	}
}

// Name returns the provider name.
func (p *AnthropicProvider) Name() string {
	return "anthropic"
}

// AvailableModels returns the list of available Anthropic models.
func (p *AnthropicProvider) AvailableModels() []ModelInfo {
	return []ModelInfo{
		{ID: "claude-sonnet-4-20250514", Name: "Claude Sonnet 4", MaxCtx: 200000},
		{ID: "claude-3-5-sonnet-20241022", Name: "Claude 3.5 Sonnet", MaxCtx: 200000},
		{ID: "claude-3-5-haiku-20241022", Name: "Claude 3.5 Haiku", MaxCtx: 200000},
		{ID: "claude-3-opus-20240229", Name: "Claude 3 Opus", MaxCtx: 200000},
	}
}

// anthropicRequest is the request format for Anthropic's API.
type anthropicRequest struct {
	Model     string              `json:"model"`
	MaxTokens int                 `json:"max_tokens"`
	System    string              `json:"system,omitempty"`
	Messages  []anthropicMessage  `json:"messages"`
	Tools     []anthropicTool     `json:"tools,omitempty"`
}

type anthropicMessage struct {
	Role    string `json:"role"`
	Content any    `json:"content"`
}

type anthropicTool struct {
	Name        string         `json:"name"`
	Description string         `json:"description"`
	InputSchema map[string]any `json:"input_schema"`
}

// anthropicResponse is the response format from Anthropic's API.
type anthropicResponse struct {
	ID           string              `json:"id"`
	Type         string              `json:"type"`
	Role         string              `json:"role"`
	Content      []anthropicContent   `json:"content"`
	Model        string              `json:"model"`
	StopReason   string              `json:"stop_reason"`
	Usage        anthropicUsage      `json:"usage"`
}

type anthropicContent struct {
	Type    string         `json:"type"`
	Text    string         `json:"text,omitempty"`
	ID      string         `json:"id,omitempty"`
	Name    string         `json:"name,omitempty"`
	Input   json.RawMessage `json:"input,omitempty"`
	Content string         `json:"content,omitempty"`
	IsError bool           `json:"is_error,omitempty"`
}

type anthropicUsage struct {
	InputTokens              int `json:"input_tokens"`
	OutputTokens             int `json:"output_tokens"`
	CacheReadInputTokens     int `json:"cache_read_input_tokens"`
	CacheCreationInputTokens int `json:"cache_creation_input_tokens"`
}

// SendMessage sends a message to the Anthropic API.
func (p *AnthropicProvider) SendMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string) (*Response, error) {
	if model == "" {
		model = "claude-sonnet-4-20250514"
	}

	// Convert messages to Anthropic format
	anthMessages := make([]anthropicMessage, 0, len(messages))
	for _, msg := range messages {
		if msg.Role == RoleSystem {
			// Anthropic handles system separately
			continue
		}
		anthMsg := anthropicMessage{
			Role:    string(msg.Role),
			Content: convertContentToAnthropic(msg.Content),
		}
		anthMessages = append(anthMessages, anthMsg)
	}

	// Build request
	reqBody := anthropicRequest{
		Model:     model,
		MaxTokens: 8192,
		System:    system,
		Messages:  anthMessages,
	}

	// Convert tools
	if len(tools) > 0 {
		anthTools := make([]anthropicTool, 0, len(tools))
		for _, t := range tools {
			anthTools = append(anthTools, anthropicTool{
				Name:        t.Name,
				Description: t.Description,
				InputSchema: t.InputSchema,
			})
		}
		reqBody.Tools = anthTools
	}

	jsonBody, err := json.Marshal(reqBody)
	if err != nil {
		return nil, fmt.Errorf("marshaling request: %w", err)
	}

	// Create HTTP request
	url := p.baseURL + "/v1/messages"
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(jsonBody))
	if err != nil {
		return nil, fmt.Errorf("creating request: %w", err)
	}

	req.Header.Set("x-api-key", p.apiKey)
	req.Header.Set("anthropic-version", "2023-06-01")
	req.Header.Set("content-type", "application/json")

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
		return nil, fmt.Errorf("anthropic API error (status %d): %s", resp.StatusCode, string(body))
	}

	// Parse response
	var anthResp anthropicResponse
	if err := json.Unmarshal(body, &anthResp); err != nil {
		return nil, fmt.Errorf("parsing response: %w", err)
	}

	// Convert to our format
	return p.convertResponse(&anthResp), nil
}

// convertContentToAnthropic converts message content to Anthropic format.
func convertContentToAnthropic(content any) any {
	switch c := content.(type) {
	case string:
		return c
	case []ContentBlock:
		blocks := make([]anthropicContent, 0, len(c))
		for _, b := range c {
			ab := anthropicContent{
				Type:    b.Type,
				Text:    b.Text,
				ID:      b.ID,
				Name:    b.Name,
				Content: b.Content,
				IsError: b.IsError,
			}
			if b.Input != nil {
				inputJSON, _ := json.Marshal(b.Input)
				ab.Input = inputJSON
			}
			blocks = append(blocks, ab)
		}
		return blocks
	default:
		return c
	}
}

// convertResponse converts an Anthropic response to our format.
func (p *AnthropicProvider) convertResponse(anthResp *anthropicResponse) *Response {
	blocks := make([]ContentBlock, 0, len(anthResp.Content))
	for _, c := range anthResp.Content {
		block := ContentBlock{
			Type:    c.Type,
			Text:    c.Text,
			ID:      c.ID,
			Name:    c.Name,
			Content: c.Content,
			IsError: c.IsError,
		}
		if c.Input != nil {
			var input any
			json.Unmarshal(c.Input, &input)
			block.Input = input
		}
		blocks = append(blocks, block)
	}

	return &Response{
		Content:    blocks,
		StopReason: anthResp.StopReason,
		Model:      anthResp.Model,
		Usage: Usage{
			InputTokens:  anthResp.Usage.InputTokens,
			OutputTokens: anthResp.Usage.OutputTokens,
			CacheRead:    anthResp.Usage.CacheReadInputTokens,
			CacheCreate:  anthResp.Usage.CacheCreationInputTokens,
		},
	}
}
