package llm

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

// TestOpenCodeProviderInterface verifies OpenCodeProvider satisfies both
// Provider and StreamingProvider at compile time.
func TestOpenCodeProviderInterface(t *testing.T) {
	var _ Provider = (*OpenCodeProvider)(nil)
	var _ StreamingProvider = (*OpenCodeProvider)(nil)
}

// TestOpenCodeProviderName verifies the provider reports the correct name.
func TestOpenCodeProviderName(t *testing.T) {
	p := NewOpenCodeProvider()
	if p.Name() != "opencode" {
		t.Errorf("Name() = %q, want %q", p.Name(), "opencode")
	}
}

// TestOpenCodeAvailableModels verifies the model list has the expected entries
// with correct IDs, names, and non-zero MaxCtx values.
func TestOpenCodeAvailableModels(t *testing.T) {
	p := NewOpenCodeProvider()
	models := p.AvailableModels()

	if len(models) == 0 {
		t.Fatal("AvailableModels() returned empty list")
	}

	expectedIDs := map[string]bool{
		"big-pickle": false, "deepseek-v4-flash-free": false,
		"mimo-v2.5-free": false, "minimax-m3-free": false,
		"nemotron-3-ultra-free": false, "qwen3.6-plus-free": false,
	}

	for _, m := range models {
		if _, ok := expectedIDs[m.ID]; !ok {
			t.Errorf("unexpected model ID %q", m.ID)
		}
		expectedIDs[m.ID] = true
		if m.Name == "" {
			t.Errorf("model %q has empty Name", m.ID)
		}
		if m.MaxCtx <= 0 {
			t.Errorf("model %q has non-positive MaxCtx: %d", m.ID, m.MaxCtx)
		}
	}

	for id, found := range expectedIDs {
		if !found {
			t.Errorf("expected model %q not found", id)
		}
	}
}

// TestOpenCodeNemotronLargeContext verifies the Nemotron model reports a
// larger MaxCtx (1M tokens) than other models.
func TestOpenCodeNemotronLargeContext(t *testing.T) {
	p := NewOpenCodeProvider()
	models := p.AvailableModels()

	for _, m := range models {
		if m.ID == "nemotron-3-ultra-free" {
			if m.MaxCtx < 1_000_000 {
				t.Errorf("nemotron MaxCtx = %d, want >= 1000000", m.MaxCtx)
			}
			return
		}
	}
	t.Error("nemotron-3-ultra-free not found in model list")
}

// newTestProvider creates an OpenCodeProvider wired to a test server.
func newTestProvider(server *httptest.Server) *OpenCodeProvider {
	return &OpenCodeProvider{
		client:  server.Client(),
		baseURL: strings.TrimSuffix(server.URL, "/"),
	}
}

// --- Non-streaming SendMessage tests ---

// TestOpenCodeSendMessage_HappyPath verifies a successful non-streaming response
// is correctly parsed.
func TestOpenCodeSendMessage_HappyPath(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Verify no auth headers (OpenCode requires no auth)
		if auth := r.Header.Get("Authorization"); auth != "" {
			t.Errorf("unexpected Authorization header: %q", auth)
		}
		if org := r.Header.Get("OpenAI-Organization"); org != "" {
			t.Errorf("unexpected OpenAI-Organization header: %q", org)
		}

		resp := openaiResponse{
			ID:    "resp_happy",
			Model: "big-pickle",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "stop",
				Message: openaiMessage{
					Role:    "assistant",
					Content: "Hello! How can I help you today?",
				},
			}},
			Usage: openaiUsage{PromptTokens: 15, CompletionTokens: 8},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	result, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "hi"}},
		nil, "You are helpful.", "big-pickle")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.Model != "big-pickle" {
		t.Errorf("Model = %q, want %q", result.Model, "big-pickle")
	}
	if result.StopReason != "end_turn" {
		t.Errorf("StopReason = %q, want %q", result.StopReason, "end_turn")
	}
	if result.Usage.InputTokens != 15 {
		t.Errorf("InputTokens = %d, want 15", result.Usage.InputTokens)
	}
	if result.Usage.OutputTokens != 8 {
		t.Errorf("OutputTokens = %d, want 8", result.Usage.OutputTokens)
	}
	if len(result.Content) != 1 {
		t.Fatalf("expected 1 content block, got %d", len(result.Content))
	}
	if result.Content[0].Text != "Hello! How can I help you today?" {
		t.Errorf("text = %q, want %q", result.Content[0].Text, "Hello! How can I help you today?")
	}
}

// TestOpenCodeSendMessage_DefaultModel verifies that an empty model string
// falls back to "big-pickle" and the request is correctly formed.
func TestOpenCodeSendMessage_DefaultModel(t *testing.T) {
	var receivedModel string

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req openaiRequest
		json.NewDecoder(r.Body).Decode(&req)
		receivedModel = req.Model

		resp := openaiResponse{
			ID:    "resp_def",
			Model: "big-pickle",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "stop",
				Message:      openaiMessage{Role: "assistant", Content: "ok"},
			}},
			Usage: openaiUsage{PromptTokens: 5, CompletionTokens: 1},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	_, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "test"}},
		nil, "", "")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if receivedModel != "big-pickle" {
		t.Errorf("model = %q, want %q", receivedModel, "big-pickle")
	}
}

// TestOpenCodeSendMessage_ToolUse verifies tool_calls in the response are
// correctly parsed into ContentBlock tool_use blocks.
func TestOpenCodeSendMessage_ToolUse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := openaiResponse{
			ID:    "resp_tool",
			Model: "deepseek-v4-flash-free",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "tool_calls",
				Message: openaiMessage{
					Role:    "assistant",
					Content: "I'll run that command.",
					ToolCalls: []openaiToolCall{{
						ID:   "call_abc123",
						Type: "function",
						Function: openaiFuncCall{
							Name:      "bash",
							Arguments: `{"command":"ls -la"}`,
						},
					}},
				},
			}},
			Usage: openaiUsage{PromptTokens: 20, CompletionTokens: 15},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	result, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "list files"}},
		[]ToolDefinition{{
			Name: "bash", Description: "Run a shell command",
			InputSchema: map[string]any{"type": "object", "properties": map[string]any{"command": map[string]any{"type": "string"}}},
		}},
		"", "deepseek-v4-flash-free")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.StopReason != "tool_use" {
		t.Errorf("StopReason = %q, want %q", result.StopReason, "tool_use")
	}

	// Should have 2 content blocks: text + tool_use
	if len(result.Content) != 2 {
		t.Fatalf("expected 2 content blocks, got %d", len(result.Content))
	}

	if result.Content[0].Type != "text" {
		t.Errorf("block[0] Type = %q, want %q", result.Content[0].Type, "text")
	}

	toolBlock := result.Content[1]
	if toolBlock.Type != "tool_use" {
		t.Errorf("block[1] Type = %q, want %q", toolBlock.Type, "tool_use")
	}
	if toolBlock.ID != "call_abc123" {
		t.Errorf("tool_use ID = %q, want %q", toolBlock.ID, "call_abc123")
	}
	if toolBlock.Name != "bash" {
		t.Errorf("tool_use Name = %q, want %q", toolBlock.Name, "bash")
	}

	inputMap, ok := toolBlock.Input.(map[string]any)
	if !ok {
		t.Fatalf("tool_use Input is %T, want map[string]any", toolBlock.Input)
	}
	if inputMap["command"] != "ls -la" {
		t.Errorf("tool_use input command = %v, want %q", inputMap["command"], "ls -la")
	}
}

// TestOpenCodeSendMessage_MultipleToolUse verifies multiple tool_calls are all parsed.
func TestOpenCodeSendMessage_MultipleToolUse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := openaiResponse{
			ID:    "resp_multi_tool",
			Model: "mimo-v2.5-free",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "tool_calls",
				Message: openaiMessage{
					Role: "assistant",
					ToolCalls: []openaiToolCall{
						{
							ID:   "call_1",
							Type: "function",
							Function: openaiFuncCall{Name: "bash", Arguments: `{"command":"ls"}`},
						},
						{
							ID:   "call_2",
							Type: "function",
							Function: openaiFuncCall{Name: "read_file", Arguments: `{"path":"main.go"}`},
						},
					},
				},
			}},
			Usage: openaiUsage{PromptTokens: 25, CompletionTokens: 20},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	result, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "explore project"}},
		[]ToolDefinition{
			{Name: "bash", Description: "Run command", InputSchema: map[string]any{"type": "object"}},
			{Name: "read_file", Description: "Read file", InputSchema: map[string]any{"type": "object"}},
		},
		"", "mimo-v2.5-free")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(result.Content) != 2 {
		t.Fatalf("expected 2 tool_use blocks, got %d", len(result.Content))
	}
	if result.Content[0].Name != "bash" {
		t.Errorf("tool[0] Name = %q, want %q", result.Content[0].Name, "bash")
	}
	if result.Content[1].Name != "read_file" {
		t.Errorf("tool[1] Name = %q, want %q", result.Content[1].Name, "read_file")
	}
}

// TestOpenCodeSendMessage_MaxTokensStopReason verifies that finish_reason "length"
// maps to stop_reason "max_tokens".
func TestOpenCodeSendMessage_MaxTokensStopReason(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := openaiResponse{
			ID:    "resp_length",
			Model: "big-pickle",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "length",
				Message: openaiMessage{
					Role:    "assistant",
					Content: "truncated...",
				},
			}},
			Usage: openaiUsage{PromptTokens: 8192, CompletionTokens: 8192},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	result, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "long request"}},
		nil, "", "big-pickle")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.StopReason != "max_tokens" {
		t.Errorf("StopReason = %q, want %q", result.StopReason, "max_tokens")
	}
}

// TestOpenCodeSendMessage_EmptyChoices verifies that a response with no choices
// returns an error.
func TestOpenCodeSendMessage_EmptyChoices(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := openaiResponse{
			ID:      "resp_empty",
			Model:   "big-pickle",
			Choices: []openaiChoice{},
			Usage:   openaiUsage{PromptTokens: 10, CompletionTokens: 0},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	_, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "test"}},
		nil, "", "big-pickle")

	if err == nil {
		t.Fatal("expected error for empty choices, got nil")
	}
	if !strings.Contains(err.Error(), "no choices") {
		t.Errorf("error = %q, want to contain %q", err.Error(), "no choices")
	}
}

// TestOpenCodeSendMessage_ErrorStatus verifies non-200 status codes return errors.
func TestOpenCodeSendMessage_ErrorStatus(t *testing.T) {
	tests := []struct {
		name    string
		status  int
		body    string
		wantErr string
	}{
		{"400 bad request", 400, `{"error":"invalid request"}`, "status 400"},
		{"401 unauthorized", 401, `{"error":"unauthorized"}`, "status 401"},
		{"429 rate limited", 429, `{"error":"rate limited"}`, "status 429"},
		{"500 internal server", 500, `{"error":"internal error"}`, "status 500"},
		{"502 bad gateway", 502, `{"error":"bad gateway"}`, "status 502"},
		{"503 service unavailable", 503, `{"error":"unavailable"}`, "status 503"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(tt.status)
				w.Write([]byte(tt.body))
			}))
			defer server.Close()

			p := newTestProvider(server)
			ctx := context.Background()

			_, err := p.SendMessage(ctx,
				[]Message{{Role: RoleUser, Content: "test"}},
				nil, "", "big-pickle")

			if err == nil {
				t.Fatal("expected error, got nil")
			}
			if !strings.Contains(err.Error(), tt.wantErr) {
				t.Errorf("error = %q, want to contain %q", err.Error(), tt.wantErr)
			}
		})
	}
}

// TestOpenCodeSendMessage_SystemPrompt verifies that a system prompt is injected
// as the first message in the request body.
func TestOpenCodeSendMessage_SystemPrompt(t *testing.T) {
	var receivedMessages []openaiMessage

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req openaiRequest
		json.NewDecoder(r.Body).Decode(&req)
		receivedMessages = req.Messages

		resp := openaiResponse{
			ID:    "resp_sys",
			Model: "big-pickle",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "stop",
				Message:      openaiMessage{Role: "assistant", Content: "ok"},
			}},
			Usage: openaiUsage{PromptTokens: 10, CompletionTokens: 1},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	_, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "hello"}},
		nil, "You are a helpful coding assistant.", "big-pickle")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(receivedMessages) < 2 {
		t.Fatalf("expected at least 2 messages (system + user), got %d", len(receivedMessages))
	}
	if receivedMessages[0].Role != "system" {
		t.Errorf("first message role = %q, want %q", receivedMessages[0].Role, "system")
	}
	sysContent, ok := receivedMessages[0].Content.(string)
	if !ok {
		t.Fatalf("system message content is %T, want string", receivedMessages[0].Content)
	}
	if sysContent != "You are a helpful coding assistant." {
		t.Errorf("system prompt = %q, want %q", sysContent, "You are a helpful coding assistant.")
	}
	if receivedMessages[1].Role != "user" {
		t.Errorf("second message role = %q, want %q", receivedMessages[1].Role, "user")
	}
}

// TestOpenCodeSendMessage_NoSystemPrompt verifies that when no system prompt
// is given, no system message is injected.
func TestOpenCodeSendMessage_NoSystemPrompt(t *testing.T) {
	var receivedMessages []openaiMessage

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req openaiRequest
		json.NewDecoder(r.Body).Decode(&req)
		receivedMessages = req.Messages

		resp := openaiResponse{
			ID:    "resp_nosys",
			Model: "big-pickle",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "stop",
				Message:      openaiMessage{Role: "assistant", Content: "ok"},
			}},
			Usage: openaiUsage{PromptTokens: 5, CompletionTokens: 1},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	_, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "hello"}},
		nil, "", "big-pickle")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(receivedMessages) != 1 {
		t.Errorf("expected 1 message (no system), got %d", len(receivedMessages))
	}
	if receivedMessages[0].Role != "user" {
		t.Errorf("message role = %q, want %q", receivedMessages[0].Role, "user")
	}
}

// TestOpenCodeSendMessage_ToolsIncluded verifies that tool definitions are
// correctly serialized in the request body.
func TestOpenCodeSendMessage_ToolsIncluded(t *testing.T) {
	var receivedTools []openaiTool

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req openaiRequest
		json.NewDecoder(r.Body).Decode(&req)
		receivedTools = req.Tools

		resp := openaiResponse{
			ID:    "resp_tools",
			Model: "big-pickle",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "stop",
				Message:      openaiMessage{Role: "assistant", Content: "ok"},
			}},
			Usage: openaiUsage{PromptTokens: 15, CompletionTokens: 1},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	tools := []ToolDefinition{
		{
			Name:        "bash",
			Description: "Execute a shell command",
			InputSchema: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"command": map[string]any{"type": "string", "description": "The command to run"},
					"cwd":     map[string]any{"type": "string", "description": "Working directory"},
				},
				"required": []any{"command"},
			},
		},
		{
			Name:        "read_file",
			Description: "Read a file's contents",
			InputSchema: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"path": map[string]any{"type": "string"},
				},
			},
		},
	}

	_, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "explore"}},
		tools, "", "big-pickle")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(receivedTools) != 2 {
		t.Fatalf("expected 2 tools in request, got %d", len(receivedTools))
	}
	if receivedTools[0].Function.Name != "bash" {
		t.Errorf("tool[0] name = %q, want %q", receivedTools[0].Function.Name, "bash")
	}
	if receivedTools[1].Function.Name != "read_file" {
		t.Errorf("tool[1] name = %q, want %q", receivedTools[1].Function.Name, "read_file")
	}
	if receivedTools[0].Type != "function" {
		t.Errorf("tool type = %q, want %q", receivedTools[0].Type, "function")
	}
}

// TestOpenCodeSendMessage_NoToolsOmitted verifies that when no tools are provided,
// the "tools" field is omitted from the request (omitempty).
func TestOpenCodeSendMessage_NoToolsOmitted(t *testing.T) {
	var rawBody map[string]any

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		json.NewDecoder(r.Body).Decode(&rawBody)

		resp := openaiResponse{
			ID:    "resp_notools",
			Model: "big-pickle",
			Choices: []openaiChoice{{
				Index:        0,
				FinishReason: "stop",
				Message:      openaiMessage{Role: "assistant", Content: "ok"},
			}},
			Usage: openaiUsage{PromptTokens: 5, CompletionTokens: 1},
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	_, err := p.SendMessage(ctx,
		[]Message{{Role: RoleUser, Content: "hello"}},
		nil, "", "big-pickle")

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if _, ok := rawBody["tools"]; ok {
		t.Error("tools field should be omitted when empty (omitempty)")
	}
}

// --- Streaming StreamMessage tests ---

// TestOpenCodeStreamMessage_Callbacks verifies that streaming correctly fires
// the callback for each text delta and signals done at the end.
func TestOpenCodeStreamMessage_Callbacks(t *testing.T) {
	var mu sync.Mutex
	var chunks []string
	var doneCount int

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Verify stream:true in the request
		var req openaiRequest
		json.NewDecoder(r.Body).Decode(&req)
		if !req.Stream {
			t.Error("expected stream=true in request")
		}

		// Simulate SSE stream
		w.Header().Set("Content-Type", "text/event-stream")
		w.Header().Set("Cache-Control", "no-cache")
		w.Header().Set("Connection", "keep-alive")

		sseData := []string{
			`data: {"id":"chunk_1","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":""}],"model":"big-pickle"}`,
			`data: {"id":"chunk_2","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":""}],"model":"big-pickle"}`,
			`data: {"id":"chunk_3","choices":[{"index":0,"delta":{"content":"!"},"finish_reason":"stop"}],"model":"big-pickle","usage":{"prompt_tokens":10,"completion_tokens":3}}`,
			`data: [DONE]`,
		}
		for _, line := range sseData {
			w.Write([]byte(line + "\n"))
		}
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	cb := func(chunk string, done bool) {
		mu.Lock()
		defer mu.Unlock()
		if done {
			doneCount++
		} else if chunk != "" {
			chunks = append(chunks, chunk)
		}
	}

	result, err := p.StreamMessage(ctx,
		[]Message{{Role: RoleUser, Content: "hi"}},
		nil, "", "big-pickle", cb)

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	mu.Lock()
	defer mu.Unlock()

	if len(chunks) != 3 {
		t.Errorf("expected 3 text chunks, got %d: %v", len(chunks), chunks)
	}
	fullText := strings.Join(chunks, "")
	if fullText != "Hello world!" {
		t.Errorf("accumulated text = %q, want %q", fullText, "Hello world!")
	}
	if doneCount != 1 {
		t.Errorf("expected 1 done signal, got %d", doneCount)
	}
	if len(result.Content) == 0 {
		t.Fatal("expected non-empty content in response")
	}
	if result.Content[0].Text != "Hello world!" {
		t.Errorf("response text = %q, want %q", result.Content[0].Text, "Hello world!")
	}
}

// TestOpenCodeStreamMessage_ToolUseStreaming verifies tool_call deltas are
// accumulated correctly during streaming.
func TestOpenCodeStreamMessage_ToolUseStreaming(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		w.Header().Set("Cache-Control", "no-cache")

		sseData := []string{
			// Initial text delta
			`data: {"id":"tc_1","choices":[{"index":0,"delta":{"role":"assistant","content":"Running..."},"finish_reason":""}],"model":"big-pickle"}`,
			// Tool call start
			`data: {"id":"tc_2","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_tool_1","type":"function","function":{"name":"bash","arguments":""}}]},"finish_reason":""}],"model":"big-pickle"}`,
			// Tool call arguments (split across chunks)
			`data: {"id":"tc_3","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"co"}}]},"finish_reason":""}],"model":"big-pickle"}`,
			`data: {"id":"tc_4","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mmand\":\"ls\"}"}}]},"finish_reason":""}],"model":"big-pickle"}`,
			// Finish
			`data: {"id":"tc_5","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"model":"big-pickle","usage":{"prompt_tokens":20,"completion_tokens":15}}`,
			`data: [DONE]`,
		}
		for _, line := range sseData {
			w.Write([]byte(line + "\n"))
		}
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	result, err := p.StreamMessage(ctx,
		[]Message{{Role: RoleUser, Content: "list files"}},
		nil, "", "big-pickle",
		func(s string, done bool) {})

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.StopReason != "tool_use" {
		t.Errorf("StopReason = %q, want %q", result.StopReason, "tool_use")
	}

	// Should have text + tool_use
	if len(result.Content) != 2 {
		t.Fatalf("expected 2 content blocks, got %d", len(result.Content))
	}

	if result.Content[0].Text != "Running..." {
		t.Errorf("text = %q, want %q", result.Content[0].Text, "Running...")
	}

	toolBlock := result.Content[1]
	if toolBlock.Type != "tool_use" {
		t.Errorf("block[1] Type = %q, want %q", toolBlock.Type, "tool_use")
	}
	if toolBlock.ID != "call_tool_1" {
		t.Errorf("tool ID = %q, want %q", toolBlock.ID, "call_tool_1")
	}

	inputMap, ok := toolBlock.Input.(map[string]any)
	if !ok {
		t.Fatalf("Input is %T, want map[string]any", toolBlock.Input)
	}
	if inputMap["command"] != "ls" {
		t.Errorf("command = %v, want %q", inputMap["command"], "ls")
	}
}

// TestOpenCodeStreamMessage_ContextCancellation verifies that a cancelled context
// produces an error (not a hang).
func TestOpenCodeStreamMessage_ContextCancellation(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		// Slow server — never writes data
		w.(http.Flusher).Flush()
		time.Sleep(5 * time.Second)
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	_, err := p.StreamMessage(ctx,
		[]Message{{Role: RoleUser, Content: "test"}},
		nil, "", "big-pickle",
		func(s string, done bool) {})

	if err == nil {
		t.Error("expected error from cancelled context, got nil")
	}
}

// TestOpenCodeStreamMessage_MalformedChunksSkipped verifies that malformed SSE
// data lines are gracefully skipped without crashing.
func TestOpenCodeStreamMessage_MalformedChunksSkipped(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")

		sseData := []string{
			`data: {"id":"m_1","choices":[{"index":0,"delta":{"content":"good"},"finish_reason":""}],"model":"big-pickle"}`,
			`data: {malformed json`,
			`data: {"id":"m_2","choices":[{"index":0,"delta":{"content":" data"},"finish_reason":"stop"}],"model":"big-pickle"}`,
			`data: [DONE]`,
		}
		for _, line := range sseData {
			w.Write([]byte(line + "\n"))
		}
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	result, err := p.StreamMessage(ctx,
		[]Message{{Role: RoleUser, Content: "test"}},
		nil, "", "big-pickle",
		func(s string, done bool) {})

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Should still get valid response despite malformed chunk
	if len(result.Content) == 0 {
		t.Error("expected non-empty content")
	}
}

// TestOpenCodeStreamMessage_NoAuthHeaders verifies streaming also sends no auth.
func TestOpenCodeStreamMessage_NoAuthHeaders(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if auth := r.Header.Get("Authorization"); auth != "" {
			t.Errorf("unexpected Authorization header in stream: %q", auth)
		}
		if org := r.Header.Get("OpenAI-Organization"); org != "" {
			t.Errorf("unexpected OpenAI-Organization header in stream: %q", org)
		}

		// Respond with valid SSE
		w.Header().Set("Content-Type", "text/event-stream")
		w.Write([]byte(`data: {"id":"s_1","choices":[{"index":0,"delta":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"model":"big-pickle","usage":{"prompt_tokens":5,"completion_tokens":1}}` + "\n"))
		w.Write([]byte("data: [DONE]\n"))
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	_, err := p.StreamMessage(ctx,
		[]Message{{Role: RoleUser, Content: "test"}},
		nil, "", "big-pickle",
		func(s string, done bool) {})

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// TestOpenCodeStreamMessage_ErrorStatus verifies streaming errors on non-200.
func TestOpenCodeStreamMessage_ErrorStatus(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		w.Write([]byte(`{"error":"internal"}`))
	}))
	defer server.Close()

	p := newTestProvider(server)
	ctx := context.Background()

	_, err := p.StreamMessage(ctx,
		[]Message{{Role: RoleUser, Content: "test"}},
		nil, "", "big-pickle",
		func(s string, done bool) {})

	if err == nil {
		t.Fatal("expected error for 500 status, got nil")
	}
	if !strings.Contains(err.Error(), "500") {
		t.Errorf("error = %q, want to contain %q", err.Error(), "500")
	}
}
