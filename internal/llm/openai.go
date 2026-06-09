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

// OpenAIProvider implements the Provider interface for OpenAI's API.
type OpenAIProvider struct {
        apiKey  string
        baseURL string
        orgID   string
        client  *http.Client
}

// NewOpenAIProvider creates a new OpenAI provider.
func NewOpenAIProvider(cfg *config.Config) *OpenAIProvider {
        return &OpenAIProvider{
                apiKey:  cfg.GetOpenAIAPIKey(),
                baseURL: cfg.GetOpenAIBaseURL(),
                orgID:   cfg.OpenAI.OrgID,
                client:  &http.Client{Timeout: 120 * time.Second},
        }
}

// Name returns the provider name.
func (p *OpenAIProvider) Name() string {
        return "openai"
}

// AvailableModels returns the list of available OpenAI models.
func (p *OpenAIProvider) AvailableModels() []ModelInfo {
        return []ModelInfo{
                {ID: "gpt-4o", Name: "GPT-4o", MaxCtx: 128000},
                {ID: "gpt-4o-mini", Name: "GPT-4o Mini", MaxCtx: 128000},
                {ID: "gpt-4-turbo", Name: "GPT-4 Turbo", MaxCtx: 128000},
                {ID: "gpt-3.5-turbo", Name: "GPT-3.5 Turbo", MaxCtx: 16385},
        }
}

// openaiRequest is the request format for OpenAI's API.
type openaiRequest struct {
        Model       string            `json:"model"`
        Messages    []openaiMessage   `json:"messages"`
        Tools       []openaiTool      `json:"tools,omitempty"`
        MaxTokens   int               `json:"max_tokens,omitempty"`
        Temperature float64           `json:"temperature,omitempty"`
        Stream      bool              `json:"stream,omitempty"`
}

type openaiMessage struct {
        Role       string    `json:"role"`
        Content    any       `json:"content,omitempty"`
        ToolCalls  []openaiToolCall `json:"tool_calls,omitempty"`
        ToolCallID string    `json:"tool_call_id,omitempty"`
}

type openaiToolCall struct {
        ID       string       `json:"id"`
        Type     string       `json:"type"`
        Function openaiFuncCall `json:"function"`
}

type openaiFuncCall struct {
        Name      string `json:"name"`
        Arguments string `json:"arguments"`
}

type openaiTool struct {
        Type     string         `json:"type"`
        Function openaiFuncDef  `json:"function"`
}

type openaiFuncDef struct {
        Name        string         `json:"name"`
        Description string         `json:"description"`
        Parameters  map[string]any `json:"parameters"`
}

// openaiResponse is the response format from OpenAI's API.
type openaiResponse struct {
        ID      string         `json:"id"`
        Choices []openaiChoice `json:"choices"`
        Model   string         `json:"model"`
        Usage   openaiUsage    `json:"usage"`
}

type openaiChoice struct {
        Index        int              `json:"index"`
        Message      openaiMessage    `json:"message"`
        FinishReason string           `json:"finish_reason"`
}

type openaiUsage struct {
        PromptTokens     int `json:"prompt_tokens"`
        CompletionTokens int `json:"completion_tokens"`
        TotalTokens      int `json:"total_tokens"`
}

// SendMessage sends a message to the OpenAI API.
func (p *OpenAIProvider) SendMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string) (*Response, error) {
        if model == "" {
                model = "gpt-4o"
        }

        oaiMessages := convertMessagesToOpenAI(messages, system)

        reqBody := openaiRequest{
                Model:     model,
                Messages:  oaiMessages,
                MaxTokens: 8192,
        }

        if len(tools) > 0 {
                oaiTools := make([]openaiTool, 0, len(tools))
                for _, t := range tools {
                        oaiTools = append(oaiTools, openaiTool{
                                Type: "function",
                                Function: openaiFuncDef{
                                        Name:        t.Name,
                                        Description: t.Description,
                                        Parameters:  t.InputSchema,
                                },
                        })
                }
                reqBody.Tools = oaiTools
        }

        jsonBody, err := json.Marshal(reqBody)
        if err != nil {
                return nil, fmt.Errorf("marshaling request: %w", err)
        }

        url := p.baseURL + "/chat/completions"
        req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(jsonBody))
        if err != nil {
                return nil, fmt.Errorf("creating request: %w", err)
        }

        req.Header.Set("Authorization", "Bearer "+p.apiKey)
        req.Header.Set("Content-Type", "application/json")
        if p.orgID != "" {
                req.Header.Set("OpenAI-Organization", p.orgID)
        }

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
                return nil, fmt.Errorf("openai API error (status %d): %s", resp.StatusCode, string(body))
        }

        var oaiResp openaiResponse
        if err := json.Unmarshal(body, &oaiResp); err != nil {
                return nil, fmt.Errorf("parsing response: %w", err)
        }

        if len(oaiResp.Choices) == 0 {
                return nil, fmt.Errorf("no choices in response")
        }

        return convertOpenAIResponse(&oaiResp.Choices[0], oaiResp.Model, &oaiResp.Usage), nil
}

// convertOpenAIResponse converts an OpenAI response to our format.
func convertOpenAIResponse(choice *openaiChoice, model string, usage *openaiUsage) *Response {
        var blocks []ContentBlock

        // Add text content
        if choice.Message.Content != nil {
                switch c := choice.Message.Content.(type) {
                case string:
                        if c != "" {
                                blocks = append(blocks, ContentBlock{Type: "text", Text: c})
                        }
                }
        }

        // Convert tool calls
        for _, tc := range choice.Message.ToolCalls {
                var input any
                if tc.Function.Arguments != "" {
                        json.Unmarshal([]byte(tc.Function.Arguments), &input)
                }
                blocks = append(blocks, ContentBlock{
                        Type:  "tool_use",
                        ID:    tc.ID,
                        Name:  tc.Function.Name,
                        Input: input,
                })
        }

        // Map finish reason
        stopReason := "end_turn"
        if len(choice.Message.ToolCalls) > 0 {
                stopReason = "tool_use"
        }
        if choice.FinishReason == "length" {
                stopReason = "max_tokens"
        }

        return &Response{
                Content:    blocks,
                StopReason: stopReason,
                Model:      model,
                Usage: Usage{
                        InputTokens:  usage.PromptTokens,
                        OutputTokens: usage.CompletionTokens,
                },
        }
}

// openaiStreamChunk represents a single SSE chunk in the OpenAI streaming format.
type openaiStreamChunk struct {
        ID      string                `json:"id"`
        Choices []openaiStreamChoice  `json:"choices"`
        Model   string                `json:"model"`
        Usage   *openaiUsage          `json:"usage,omitempty"`
}

type openaiStreamChoice struct {
        Index        int                `json:"index"`
        Delta        openaiStreamDelta  `json:"delta"`
        FinishReason string             `json:"finish_reason"`
}

type openaiStreamDelta struct {
        Role      string             `json:"role,omitempty"`
        Content   string             `json:"content,omitempty"`
        ToolCalls []openaiStreamToolCall `json:"tool_calls,omitempty"`
}

type openaiStreamToolCall struct {
        Index    int    `json:"index"`
        ID       string `json:"id,omitempty"`
        Type     string `json:"type,omitempty"`
        Function struct {
                Name      string `json:"name,omitempty"`
                Arguments string `json:"arguments,omitempty"`
        } `json:"function"`
}

// Ensure OpenAIProvider satisfies StreamingProvider.
var _ StreamingProvider = (*OpenAIProvider)(nil)

// StreamMessage sends a streaming request to the OpenAI API.
func (p *OpenAIProvider) StreamMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string, cb StreamingCallback) (*Response, error) {
        return streamOpenAIFormat(ctx, p.baseURL+"/chat/completions", p.apiKey, p.orgID, messages, tools, system, model, cb, p.client)
}

// streamOpenAIFormat is a shared streaming implementation for OpenAI-compatible APIs.
func streamOpenAIFormat(ctx context.Context, url, apiKey, orgID string, messages []Message, tools []ToolDefinition, system string, model string, cb StreamingCallback, client *http.Client) (*Response, error) {
        if model == "" {
                model = "gpt-4o"
        }

        oaiMessages := convertMessagesToOpenAI(messages, system)

        reqBody := openaiRequest{
                Model:     model,
                Messages:  oaiMessages,
                MaxTokens: 8192,
                Stream:    true,
        }

        if len(tools) > 0 {
                oaiTools := make([]openaiTool, 0, len(tools))
                for _, t := range tools {
                        oaiTools = append(oaiTools, openaiTool{
                                Type: "function",
                                Function: openaiFuncDef{
                                        Name:        t.Name,
                                        Description: t.Description,
                                        Parameters:  t.InputSchema,
                                },
                        })
                }
                reqBody.Tools = oaiTools
        }

        jsonBody, err := json.Marshal(reqBody)
        if err != nil {
                return nil, fmt.Errorf("marshaling request: %w", err)
        }

        req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(jsonBody))
        if err != nil {
                return nil, fmt.Errorf("creating request: %w", err)
        }

        req.Header.Set("Content-Type", "application/json")
        if apiKey != "" {
                req.Header.Set("Authorization", "Bearer "+apiKey)
        }
        if orgID != "" {
                req.Header.Set("OpenAI-Organization", orgID)
        }

        resp, err := client.Do(req)
        if err != nil {
                return nil, fmt.Errorf("sending request: %w", err)
        }
        defer resp.Body.Close()

        if resp.StatusCode != http.StatusOK {
                body, _ := io.ReadAll(resp.Body)
                return nil, fmt.Errorf("API error (status %d): %s", resp.StatusCode, string(body))
        }

        // Parse SSE stream
        var accumulatedText string
        var accumulatedToolCalls []openaiToolCall
        var toolCallArgs []*strings.Builder  // accumulate arguments by index
        var finishReason string
        var responseModel string
        var usage openaiUsage

        scanner := bufio.NewScanner(resp.Body)
        for scanner.Scan() {
                line := scanner.Text()
                if !strings.HasPrefix(line, "data: ") {
                        continue
                }
                data := strings.TrimPrefix(line, "data: ")
                if data == "[DONE]" {
                        break
                }

                var chunk openaiStreamChunk
                if err := json.Unmarshal([]byte(data), &chunk); err != nil {
                        continue // skip malformed chunks
                }

                if chunk.Model != "" {
                        responseModel = chunk.Model
                }

                if chunk.Usage != nil {
                        usage = *chunk.Usage
                }

                if len(chunk.Choices) == 0 {
                        continue
                }

                choice := chunk.Choices[0]
                if choice.FinishReason != "" {
                        finishReason = choice.FinishReason
                }

                // Handle text delta
                if choice.Delta.Content != "" {
                        accumulatedText += choice.Delta.Content
                        if cb != nil {
                                cb(choice.Delta.Content, false)
                        }
                }

                // Handle tool call deltas
                for _, tc := range choice.Delta.ToolCalls {
                        // Ensure tool call entry exists
                        for len(accumulatedToolCalls) <= tc.Index {
                                accumulatedToolCalls = append(accumulatedToolCalls, openaiToolCall{
                                        Type: "function",
                                })
                                toolCallArgs = append(toolCallArgs, &strings.Builder{})
                        }
                        if tc.ID != "" {
                                accumulatedToolCalls[tc.Index].ID = tc.ID
                        }
                        if tc.Function.Name != "" {
                                accumulatedToolCalls[tc.Index].Function.Name = tc.Function.Name
                        }
                        if tc.Function.Arguments != "" {
                                toolCallArgs[tc.Index].WriteString(tc.Function.Arguments)
                        }
                }
        }

        // Signal done
        if cb != nil {
                cb("", true)
        }

        // Assemble tool calls with accumulated arguments
        for i, tc := range accumulatedToolCalls {
                if i < len(toolCallArgs) && toolCallArgs[i] != nil {
                        tc.Function.Arguments = toolCallArgs[i].String()
                        accumulatedToolCalls[i] = tc
                }
        }

        // Build content blocks
        var blocks []ContentBlock
        if accumulatedText != "" {
                blocks = append(blocks, ContentBlock{Type: "text", Text: accumulatedText})
        }

        for _, tc := range accumulatedToolCalls {
                var input any
                if tc.Function.Arguments != "" {
                        json.Unmarshal([]byte(tc.Function.Arguments), &input)
                }
                blocks = append(blocks, ContentBlock{
                        Type:  "tool_use",
                        ID:    tc.ID,
                        Name:  tc.Function.Name,
                        Input: input,
                })
        }

        // Map finish reason
        stopReason := "end_turn"
        if len(accumulatedToolCalls) > 0 {
                stopReason = "tool_use"
        }
        if finishReason == "length" {
                stopReason = "max_tokens"
        }

        return &Response{
                Content:    blocks,
                StopReason: stopReason,
                Model:      responseModel,
                Usage: Usage{
                        InputTokens:  usage.PromptTokens,
                        OutputTokens: usage.CompletionTokens,
                },
        }, nil
}

// convertMessagesToOpenAI converts messages to the OpenAI message format (shared by send and stream).
func convertMessagesToOpenAI(messages []Message, system string) []openaiMessage {
        oaiMessages := make([]openaiMessage, 0, len(messages)+1)

        if system != "" {
                oaiMessages = append(oaiMessages, openaiMessage{
                        Role:    "system",
                        Content: system,
                })
        }

        for _, msg := range messages {
                if msg.Role == RoleSystem {
                        continue
                }

                oaiMsg := openaiMessage{
                        Role: string(msg.Role),
                }

                switch c := msg.Content.(type) {
                case string:
                        oaiMsg.Content = c
                case []ContentBlock:
                        var textParts []string
                        var toolCalls []openaiToolCall
                        for _, block := range c {
                                switch block.Type {
                                case "text":
                                        textParts = append(textParts, block.Text)
                                case "tool_use":
                                        argsJSON, _ := json.Marshal(block.Input)
                                        toolCalls = append(toolCalls, openaiToolCall{
                                                ID:   block.ID,
                                                Type: "function",
                                                Function: openaiFuncCall{
                                                        Name:      block.Name,
                                                        Arguments: string(argsJSON),
                                                },
                                        })
                                case "tool_result":
                                        oaiMsg.Role = "tool"
                                        oaiMsg.Content = block.Content
                                        oaiMsg.ToolCallID = block.ID
                                }
                        }
                        if oaiMsg.Role != "tool" {
                                if len(textParts) > 0 {
                                        oaiMsg.Content = joinStrings(textParts)
                                }
                                if len(toolCalls) > 0 {
                                        oaiMsg.ToolCalls = toolCalls
                                }
                        }
                default:
                        oaiMsg.Content = c
                }

                oaiMessages = append(oaiMessages, oaiMsg)
        }

        return oaiMessages
}

// joinStrings joins non-empty strings with newlines.
func joinStrings(parts []string) string {
        result := ""
        for i, s := range parts {
                if s == "" {
                        continue
                }
                if i > 0 {
                        result += "\n"
                }
                result += s
        }
        return result
}
