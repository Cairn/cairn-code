package llm

import (
        "bufio"
        "bytes"
        "context"
        "encoding/json"
        "fmt"
        "io"
        "net/http"
        "strings"
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

// anthropicStreamRequest is the request format for Anthropic's API (with optional streaming).
type anthropicStreamRequest struct {
        Model     string              `json:"model"`
        MaxTokens int                 `json:"max_tokens"`
        System    string              `json:"system,omitempty"`
        Messages  []anthropicMessage  `json:"messages"`
        Tools     []anthropicTool     `json:"tools,omitempty"`
        Stream    bool                `json:"stream,omitempty"`
}

// anthropicStreamEvent represents a single SSE event from Anthropic's streaming API.
type anthropicStreamEvent struct {
        Type         string               `json:"type"`
        Message       *anthropicResponse   `json:"message,omitempty"`
        ContentBlock *anthropicContent    `json:"content_block,omitempty"`
        Delta        *anthropicStreamDelta `json:"delta,omitempty"`
        Usage        *anthropicStreamUsage `json:"usage,omitempty"`
}

// anthropicStreamDelta represents the delta in a streaming event.
type anthropicStreamDelta struct {
        Type       string `json:"type"`                  // "text_delta", "input_json_delta"
        Text       string `json:"text,omitempty"`        // for text_delta
        PartialJSON string `json:"partial_json,omitempty"` // for input_json_delta
        StopReason string `json:"stop_reason,omitempty"`  // for message_delta
}

// anthropicStreamUsage represents partial usage in streaming events.
type anthropicStreamUsage struct {
        OutputTokens int `json:"output_tokens"`
}

// Ensure AnthropicProvider satisfies StreamingProvider.
var _ StreamingProvider = (*AnthropicProvider)(nil)

// SendMessage sends a message to the Anthropic API (non-streaming).
func (p *AnthropicProvider) SendMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string) (*Response, error) {
        return p.StreamMessage(ctx, messages, tools, system, model, nil)
}

// StreamMessage sends a streaming request to the Anthropic API using SSE.
// If cb is nil, it falls back to non-streaming (collects full response then returns).
func (p *AnthropicProvider) StreamMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string, cb StreamingCallback) (*Response, error) {
        if model == "" {
                model = "claude-sonnet-4-20250514"
        }

        // Convert messages to Anthropic format
        anthMessages := make([]anthropicMessage, 0, len(messages))
        for _, msg := range messages {
                if msg.Role == RoleSystem {
                        continue
                }
                anthMessages = append(anthMessages, anthropicMessage{
                        Role:    string(msg.Role),
                        Content: convertContentToAnthropic(msg.Content),
                })
        }

        // Build request — use streaming request struct
        reqBody := anthropicStreamRequest{
                Model:     model,
                MaxTokens: 8192,
                System:    system,
                Messages:  anthMessages,
                Stream:    cb != nil,
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

        url := p.baseURL + "/v1/messages"
        req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(jsonBody))
        if err != nil {
                return nil, fmt.Errorf("creating request: %w", err)
        }

        req.Header.Set("x-api-key", p.apiKey)
        req.Header.Set("anthropic-version", "2023-06-01")
        req.Header.Set("content-type", "application/json")
        req.Header.Set("Accept", "text/event-stream")

        resp, err := p.client.Do(req)
        if err != nil {
                return nil, fmt.Errorf("sending request: %w", err)
        }
        defer resp.Body.Close()

        if resp.StatusCode != http.StatusOK {
                body, _ := io.ReadAll(resp.Body)
                return nil, fmt.Errorf("anthropic API error (status %d): %s", resp.StatusCode, string(body))
        }

        // If no callback, just read the full response as JSON (non-streaming fallback)
        if cb == nil {
                body, err := io.ReadAll(resp.Body)
                if err != nil {
                        return nil, fmt.Errorf("reading response: %w", err)
                }
                var anthResp anthropicResponse
                if err := json.Unmarshal(body, &anthResp); err != nil {
                        return nil, fmt.Errorf("parsing response: %w", err)
                }
                return p.convertResponse(&anthResp), nil
        }

        // Parse SSE stream
        return p.parseAnthropicStream(resp.Body, cb)
}

// parseAnthropicStream reads Anthropic's SSE stream and fires the callback per chunk.
func (p *AnthropicProvider) parseAnthropicStream(body io.Reader, cb StreamingCallback) (*Response, error) {
        var accumulatedText string
        var contentBlocks []anthropicContent
        var currentBlock *anthropicContent
        var toolCallArgs map[int]*strings.Builder
        var stopReason string
        var responseModel string
        var usage anthropicUsage

        scanner := bufio.NewScanner(body)
        // Increase buffer to 1MB — tool call arguments can exceed the default 64KB
        scanner.Buffer(make([]byte, 0, 1024*1024), 1024*1024)
        for scanner.Scan() {
                line := scanner.Text()
                if !strings.HasPrefix(line, "data: ") {
                        continue
                }
                data := strings.TrimPrefix(line, "data: ")

                var event anthropicStreamEvent
                if err := json.Unmarshal([]byte(data), &event); err != nil {
                        continue
                }

                switch event.Type {
                case "message_start":
                        if event.Message != nil {
                                responseModel = event.Message.Model
                                usage = event.Message.Usage
                        }

                case "content_block_start":
                        if event.ContentBlock != nil {
                                block := *event.ContentBlock
                                contentBlocks = append(contentBlocks, block)
                                currentBlock = &contentBlocks[len(contentBlocks)-1]
                                if block.Type == "tool_use" && toolCallArgs == nil {
                                        toolCallArgs = make(map[int]*strings.Builder)
                                }
                                if block.Type == "tool_use" {
                                        toolCallArgs[len(contentBlocks)-1] = &strings.Builder{}
                                }
                        }

                case "content_block_delta":
                        if event.Delta != nil {
                                switch event.Delta.Type {
                                case "text_delta":
                                        if event.Delta.Text != "" {
                                                accumulatedText += event.Delta.Text
                                                cb(event.Delta.Text, false)
                                        }
                                case "input_json_delta":
                                        if currentBlock != nil && event.Delta.PartialJSON != "" {
                                                idx := len(contentBlocks) - 1
                                                if builder, ok := toolCallArgs[idx]; ok {
                                                        builder.WriteString(event.Delta.PartialJSON)
                                                }
                                        }
                                }
                        }

                case "content_block_stop":
                        // Finalize tool_use arguments
                        if currentBlock != nil && currentBlock.Type == "tool_use" {
                                idx := len(contentBlocks) - 1
                                if builder, ok := toolCallArgs[idx]; ok {
                                        currentBlock.Input = json.RawMessage(builder.String())
                                }
                        }
                        currentBlock = nil

                case "message_delta":
                        if event.Delta != nil {
                                stopReason = event.Delta.StopReason
                        }
                        if event.Usage != nil {
                                usage.OutputTokens = event.Usage.OutputTokens
                        }

                case "message_stop":
                        cb("", true)
                }
        }

        // Safety net: if stream ended without message_stop, still signal done
        if cb != nil {
                cb("", true)
        }

        // Build content blocks from accumulated stream data
        blocks := make([]ContentBlock, 0, len(contentBlocks))
        for _, c := range contentBlocks {
                block := ContentBlock{
                        Type:    c.Type,
                        Text:    c.Text,
                        ID:      c.ID,
                        Name:    c.Name,
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
                StopReason: stopReason,
                Model:      responseModel,
                Usage: Usage{
                        InputTokens:  usage.InputTokens,
                        OutputTokens: usage.OutputTokens,
                        CacheRead:    usage.CacheReadInputTokens,
                        CacheCreate:  usage.CacheCreationInputTokens,
                },
        }, nil
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
                                ab.Input = json.RawMessage(inputJSON)
                        }
                        blocks = append(blocks, ab)
                }
                return blocks
        case []any:
                // Handle []any from JSON deserialization (session resume)
                return convertContentToAnthropic(AsTextBlocks(c))
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
