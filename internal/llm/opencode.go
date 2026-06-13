package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// OpenCodeProvider implements the Provider interface for OpenCode's free API.
// No authentication required — uses OpenAI-compatible format at https://opencode.ai/zen/v1
type OpenCodeProvider struct {
	client *http.Client
}

// NewOpenCodeProvider creates a new OpenCode provider targeting the free OpenCode API.
func NewOpenCodeProvider() *OpenCodeProvider {
	return &OpenCodeProvider{
		client: &http.Client{Timeout: 120 * time.Second},
	}
}

// Name returns the provider name.
func (p *OpenCodeProvider) Name() string {
	return "opencode"
}

// AvailableModels returns the list of available free OpenCode models.
func (p *OpenCodeProvider) AvailableModels() []ModelInfo {
	return []ModelInfo{
		{ID: "big-pickle", Name: "Big Pickle", MaxCtx: 200000},
		{ID: "deepseek-v4-flash-free", Name: "DeepSeek V4 Flash Free", MaxCtx: 200000},
		{ID: "mimo-v2.5-free", Name: "MiMo V2.5 Free", MaxCtx: 200000},
		{ID: "minimax-m3-free", Name: "MiniMax M3 Free", MaxCtx: 200000},
		{ID: "nemotron-3-ultra-free", Name: "Nemotron 3 Ultra Free", MaxCtx: 1048576},
		{ID: "qwen3.6-plus-free", Name: "Qwen3.6 Plus Free", MaxCtx: 262144},
	}
}

// opencodeBaseURL is the base URL for the OpenCode API.
const opencodeBaseURL = "https://opencode.ai/zen/v1"

// SendMessage sends a message to the OpenCode API using the OpenAI-compatible format.
func (p *OpenCodeProvider) SendMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string) (*Response, error) {
	if model == "" {
		model = "big-pickle"
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

	url := opencodeBaseURL + "/chat/completions"
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(jsonBody))
	if err != nil {
		return nil, fmt.Errorf("creating request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")

	resp, err := p.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("sending request to opencode: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("opencode API error (status %d): %s", resp.StatusCode, string(body))
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

// Ensure OpenCodeProvider satisfies StreamingProvider.
var _ StreamingProvider = (*OpenCodeProvider)(nil)

// StreamMessage sends a streaming request to the OpenCode API using the shared OpenAI format.
func (p *OpenCodeProvider) StreamMessage(ctx context.Context, messages []Message, tools []ToolDefinition, system string, model string, cb StreamingCallback) (*Response, error) {
	return streamOpenAIFormat(ctx, opencodeBaseURL+"/chat/completions", "", "", messages, tools, system, model, cb, p.client)
}

// Ensure interface satisfaction
var _ Provider = (*OpenCodeProvider)(nil)
