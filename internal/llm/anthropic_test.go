package llm

import (
	"encoding/json"
	"strings"
	"testing"
)

// TestAnthropicStreamingProviderInterface verifies AnthropicProvider satisfies StreamingProvider.
func TestAnthropicStreamingProviderInterface(t *testing.T) {
	var _ StreamingProvider = (*AnthropicProvider)(nil)
	var _ Provider = (*AnthropicProvider)(nil)
}

// TestAnthropicStreamEventParsing verifies the SSE event types parse correctly.
func TestAnthropicStreamEventParsing(t *testing.T) {
	tests := []struct {
		name     string
		data     string
		wantType string
	}{
		{
			name: "message_start",
			data: `{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}`,
			wantType: "message_start",
		},
		{
			name: "content_block_start text",
			data: `{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
			wantType: "content_block_start",
		},
		{
			name: "content_block_start tool_use",
			data: `{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"bash","input":""}}`,
			wantType: "content_block_start",
		},
		{
			name:     "content_block_delta text",
			data:     `{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}`,
			wantType: "content_block_delta",
		},
		{
			name: "content_block_delta input_json",
			data: `{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"co"}}`,
			wantType: "content_block_delta",
		},
		{
			name:     "content_block_stop",
			data:     `{"type":"content_block_stop","index":0}`,
			wantType: "content_block_stop",
		},
		{
			name: "message_delta stop_reason",
			data: `{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}`,
			wantType: "message_delta",
		},
		{
			name:     "message_stop",
			data:     `{"type":"message_stop"}`,
			wantType: "message_stop",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var event anthropicStreamEvent
			if err := json.Unmarshal([]byte(tt.data), &event); err != nil {
				t.Fatalf("failed to parse: %v", err)
			}
			if event.Type != tt.wantType {
				t.Errorf("got type %q, want %q", event.Type, tt.wantType)
			}
		})
	}
}

// TestAnthropicStreamTextAccumulation verifies text chunks accumulate correctly
// through the streaming callback pattern.
func TestAnthropicStreamTextAccumulation(t *testing.T) {
	// Simulate a stream of SSE events
	sseEvents := []string{
		`{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}`,
		`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
		`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}`,
		`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}`,
		`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"!"}}`,
		`{"type":"content_block_stop","index":0}`,
		`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}`,
		`{"type":"message_stop"}`,
	}

	var chunkCalls []string
	var doneCallCount int

	// Simulate the callback pattern used by parseAnthropicStream
	for _, raw := range sseEvents {
		data := strings.TrimPrefix(raw, "data: ")
		var event anthropicStreamEvent
		if err := json.Unmarshal([]byte(data), &event); err != nil {
			t.Fatalf("failed to parse event: %v", err)
		}

		switch event.Type {
		case "content_block_delta":
			if event.Delta != nil && event.Delta.Type == "text_delta" {
				chunkCalls = append(chunkCalls, event.Delta.Text)
			}
		case "message_stop":
			doneCallCount++
		}
	}

	if len(chunkCalls) != 3 {
		t.Errorf("expected 3 text chunks, got %d", len(chunkCalls))
	}
	if doneCallCount != 1 {
		t.Errorf("expected 1 done signal, got %d", doneCallCount)
	}

	fullText := strings.Join(chunkCalls, "")
	if fullText != "Hello world!" {
		t.Errorf("expected 'Hello world!', got '%s'", fullText)
	}
}

// TestAnthropicStreamToolUseParsing verifies tool_use blocks accumulate
// input_json_delta correctly.
func TestAnthropicStreamToolUseParsing(t *testing.T) {
	sseEvents := []string{
		`{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_1","name":"bash","input":""}}`,
		`{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"co"}}`,
		`{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"mmand\":\"ls\"}"}}`,
		`{"type":"content_block_stop","index":0}`,
	}

	var toolArgs strings.Builder
	var toolName string

	for _, raw := range sseEvents {
		data := strings.TrimPrefix(raw, "data: ")
		var event anthropicStreamEvent
		if err := json.Unmarshal([]byte(data), &event); err != nil {
			t.Fatalf("failed to parse event: %v", err)
		}

		switch event.Type {
		case "content_block_start":
			if event.ContentBlock != nil && event.ContentBlock.Type == "tool_use" {
				toolName = event.ContentBlock.Name
			}
		case "content_block_delta":
			if event.Delta != nil && event.Delta.Type == "input_json_delta" {
				toolArgs.WriteString(event.Delta.PartialJSON)
			}
		}
	}

	if toolName != "bash" {
		t.Errorf("expected tool name 'bash', got '%s'", toolName)
	}

	// Verify the accumulated JSON is valid
	var parsed map[string]any
	if err := json.Unmarshal([]byte(toolArgs.String()), &parsed); err != nil {
		t.Errorf("accumulated tool args is not valid JSON: %s (error: %v)", toolArgs.String(), err)
	}
	if parsed["command"] != "ls" {
		t.Errorf("expected command 'ls', got %v", parsed["command"])
	}
}

// TestAnthropicStreamRequestEncoding verifies the stream flag is correctly
// serialized in the JSON request.
func TestAnthropicStreamRequestEncoding(t *testing.T) {
	req := anthropicStreamRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 8192,
		System:    "You are helpful.",
		Messages:  []anthropicMessage{{Role: "user", Content: "hi"}},
		Stream:    true,
	}

	data, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	s := string(data)
	if !strings.Contains(s, `"stream":true`) {
		t.Errorf("expected stream:true in JSON, got: %s", s)
	}
	if !strings.Contains(s, `"model":"claude-sonnet-4-20250514"`) {
		t.Errorf("model missing from JSON: %s", s)
	}
}

// TestAnthropicStreamRequestNonStreaming verifies stream:false is omitted
// when streaming is disabled (omitempty).
func TestAnthropicStreamRequestNonStreaming(t *testing.T) {
	req := anthropicStreamRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 8192,
		Stream:    false,
	}

	data, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	s := string(data)
	if strings.Contains(s, `"stream"`) {
		t.Errorf("stream field should be omitted when false (omitempty), got: %s", s)
	}
}
