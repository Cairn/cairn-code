package llm

import (
        "context"
        "fmt"
)

// MessageRole represents the role of a message sender.
type MessageRole string

const (
        RoleUser      MessageRole = "user"
        RoleAssistant MessageRole = "assistant"
        RoleSystem    MessageRole = "system"
)

// Message represents a single message in the conversation.
type Message struct {
        Role    MessageRole `json:"role"`
        Content any         `json:"content"` // string or []ContentBlock
}

// ContentBlock represents a structured content block within a message.
type ContentBlock struct {
        Type    string `json:"type"` // "text", "tool_use", "tool_result", "image"
        Text    string `json:"text,omitempty"`
        ID      string `json:"id,omitempty"`
        Name    string `json:"name,omitempty"`
        Input   any    `json:"input,omitempty"`
        Content string `json:"content,omitempty"`
        IsError bool   `json:"is_error,omitempty"`
}

// ToolDefinition describes a tool available to the LLM.
type ToolDefinition struct {
        Name        string         `json:"name"`
        Description string         `json:"description"`
        InputSchema map[string]any `json:"input_schema"`
}

// Response represents an LLM response.
type Response struct {
        Content    []ContentBlock `json:"content"`
        StopReason string         `json:"stop_reason"` // "end_turn", "tool_use", "max_tokens"
        Usage      Usage          `json:"usage"`
        Model      string         `json:"model"`
}

// Usage represents token usage information.
type Usage struct {
        InputTokens  int `json:"input_tokens"`
        OutputTokens int `json:"output_tokens"`
        CacheRead    int `json:"cache_read_input_tokens,omitempty"`
        CacheCreate  int `json:"cache_creation_input_tokens,omitempty"`
}

// ModelInfo provides metadata about an available model.
type ModelInfo struct {
        ID     string
        Name   string
        MaxCtx int
}

// StreamingCallback is called for each chunk during streaming.
// chunk is the incremental text delta, done is true on the final call.
type StreamingCallback func(chunk string, done bool)

// StreamingProvider is an optional interface for providers that support SSE streaming.
type StreamingProvider interface {
        StreamMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string, cb StreamingCallback) (*Response, error)
}

// Provider is the interface that all LLM providers must implement.
type Provider interface {
        SendMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string) (*Response, error)
        Name() string
        AvailableModels() []ModelInfo
}

// TextBlock creates a text content block.
func TextBlock(text string) ContentBlock {
        return ContentBlock{Type: "text", Text: text}
}

// ToolUseBlock creates a tool_use content block.
func ToolUseBlock(id, name string, input any) ContentBlock {
        return ContentBlock{Type: "tool_use", ID: id, Name: name, Input: input}
}

// ToolResultBlock creates a tool_result content block.
func ToolResultBlock(toolUseID, content string, isError bool) ContentBlock {
        return ContentBlock{Type: "tool_result", ID: toolUseID, Content: content, IsError: isError}
}

// AsTextBlocks extracts the text from a message content.
// Handles both string content and []ContentBlock content.
func AsTextBlocks(content any) []ContentBlock {
        switch c := content.(type) {
        case string:
                return []ContentBlock{{Type: "text", Text: c}}
        case []ContentBlock:
                return c
        case []any:
                blocks := make([]ContentBlock, 0, len(c))
                for _, item := range c {
                        switch v := item.(type) {
                        case ContentBlock:
                                blocks = append(blocks, v)
                        case map[string]any:
                                blocks = append(blocks, mapToContentBlock(v))
                        }
                }
                return blocks
        default:
                return []ContentBlock{{Type: "text", Text: fmt.Sprintf("%v", c)}}
        }
}

// ExtractText returns the concatenated text from content blocks.
func ExtractText(content any) string {
        blocks := AsTextBlocks(content)
        result := ""
        for _, b := range blocks {
                if b.Type == "text" {
                        result += b.Text
                }
        }
        return result
}

// mapToContentBlock converts a generic map to a ContentBlock.
func mapToContentBlock(m map[string]any) ContentBlock {
        cb := ContentBlock{}
        if t, ok := m["type"].(string); ok {
                cb.Type = t
        }
        if t, ok := m["text"].(string); ok {
                cb.Text = t
        }
        if id, ok := m["id"].(string); ok {
                cb.ID = id
        }
        if n, ok := m["name"].(string); ok {
                cb.Name = n
        }
        if c, ok := m["content"].(string); ok {
                cb.Content = c
        }
        if i, ok := m["input"]; ok {
                cb.Input = i
        }
        if ie, ok := m["is_error"].(bool); ok {
                cb.IsError = ie
        }
        return cb
}
