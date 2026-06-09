package llm

import (
	"encoding/json"
	"testing"
)

// TestConvertMessagesToOpenAI_EmptyAssistantSkipped verifies that empty
// assistant messages (no content, no tool_calls) are dropped.
func TestConvertMessagesToOpenAI_EmptyAssistantSkipped(t *testing.T) {
	messages := []Message{
		{Role: RoleUser, Content: "hello"},
		{Role: RoleAssistant, Content: []ContentBlock{}},
		{Role: RoleAssistant, Content: []ContentBlock{{Type: "text", Text: "hi there"}}},
	}

	result := convertMessagesToOpenAI(messages, "system prompt")

	// Should have: system, user, assistant(text) — the empty assistant is skipped
	if len(result) != 3 {
		t.Errorf("expected 3 messages, got %d", len(result))
		for i, m := range result {
			b, _ := json.Marshal(m)
			t.Errorf("  msg[%d]: %s", i, string(b))
		}
	}
}

// TestConvertMessagesToOpenAI_ToolUseConversion verifies tool_use blocks
// become proper OpenAI tool_calls in the message.
func TestConvertMessagesToOpenAI_ToolUseConversion(t *testing.T) {
	messages := []Message{
		{Role: RoleUser, Content: "run ls"},
		{Role: RoleAssistant, Content: []ContentBlock{
			{Type: "tool_use", ID: "call_1", Name: "bash", Input: map[string]any{"command": "ls -la"}},
		}},
	}

	result := convertMessagesToOpenAI(messages, "")

	if len(result) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(result))
	}

	assistant := result[1]
	if assistant.Role != "assistant" {
		t.Errorf("expected role 'assistant', got '%s'", assistant.Role)
	}
	if len(assistant.ToolCalls) != 1 {
		t.Fatalf("expected 1 tool_call, got %d", len(assistant.ToolCalls))
	}
	if assistant.ToolCalls[0].ID != "call_1" {
		t.Errorf("expected tool_call ID 'call_1', got '%s'", assistant.ToolCalls[0].ID)
	}
	if assistant.ToolCalls[0].Function.Name != "bash" {
		t.Errorf("expected function name 'bash', got '%s'", assistant.ToolCalls[0].Function.Name)
	}
}

// TestConvertMessagesToOpenAI_ToolResultConversion verifies tool_result blocks
// become proper OpenAI "tool" role messages.
func TestConvertMessagesToOpenAI_ToolResultConversion(t *testing.T) {
	messages := []Message{
		{Role: RoleAssistant, Content: []ContentBlock{
			{Type: "tool_result", ID: "call_1", Content: "file1.txt\nfile2.txt"},
		}},
	}

	result := convertMessagesToOpenAI(messages, "")

	if len(result) != 1 {
		t.Fatalf("expected 1 message, got %d", len(result))
	}

	toolMsg := result[0]
	if toolMsg.Role != "tool" {
		t.Errorf("expected role 'tool', got '%s'", toolMsg.Role)
	}
	if toolMsg.ToolCallID != "call_1" {
		t.Errorf("expected tool_call_id 'call_1', got '%s'", toolMsg.ToolCallID)
	}
	if toolMsg.Content != "file1.txt\nfile2.txt" {
		t.Errorf("unexpected content: %v", toolMsg.Content)
	}
}

// TestConvertMessagesToOpenAI_AnyFromJSONDeserialization verifies that Content
// loaded from JSON (which produces []any instead of []ContentBlock) is handled
// correctly. This was the root cause of the "unknown variant `tool_use`" bug.
func TestConvertMessagesToOpenAI_AnyFromJSONDeserialization(t *testing.T) {
	// Simulate what JSON unmarshal produces: []interface{} with map[string]interface{} items
	rawJSON := `{"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"bash","input":{"command":"ls"}}]}`

	var msg Message
	if err := json.Unmarshal([]byte(rawJSON), &msg); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	// Verify Content is []any, not []ContentBlock
	if _, ok := msg.Content.([]ContentBlock); ok {
		t.Fatal("expected Content to be []any (not []ContentBlock) after JSON unmarshal")
	}
	if _, ok := msg.Content.([]any); !ok {
		t.Fatalf("expected Content to be []any, got %T", msg.Content)
	}

	// This should NOT panic or produce invalid output
	result := convertMessagesToOpenAI([]Message{msg}, "")

	if len(result) != 1 {
		t.Fatalf("expected 1 message, got %d", len(result))
	}

	assistant := result[0]
	if assistant.Role != "assistant" {
		t.Errorf("expected role 'assistant', got '%s'", assistant.Role)
	}
	if len(assistant.ToolCalls) != 1 {
		t.Fatalf("expected 1 tool_call, got %d", len(assistant.ToolCalls))
	}
	if assistant.ToolCalls[0].Function.Name != "bash" {
		t.Errorf("expected function name 'bash', got '%s'", assistant.ToolCalls[0].Function.Name)
	}

	// Verify no raw "tool_use" leaks into the content field
	b, _ := json.Marshal(assistant)
	s := string(b)
	if contains(s, `"content":[{"type":"tool_use"`) || contains(s, `"content":[{"type":"tool_result"`) {
		t.Errorf("raw tool_use/tool_result leaked into content field: %s", s)
	}
}

// TestConvertMessagesToOpenAI_ToolResultFromJSON verifies tool_result blocks
// from JSON deserialization are correctly converted to "tool" role messages.
func TestConvertMessagesToOpenAI_ToolResultFromJSON(t *testing.T) {
	rawJSON := `{"role":"assistant","content":[{"type":"tool_result","id":"call_1","content":"output here","is_error":false}]}`

	var msg Message
	if err := json.Unmarshal([]byte(rawJSON), &msg); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	result := convertMessagesToOpenAI([]Message{msg}, "")

	if len(result) != 1 {
		t.Fatalf("expected 1 message, got %d", len(result))
	}

	if result[0].Role != "tool" {
		t.Errorf("expected role 'tool', got '%s'", result[0].Role)
	}
	if result[0].Content != "output here" {
		t.Errorf("expected content 'output here', got %v", result[0].Content)
	}
}

// TestConvertMessagesToOpenAI_MixedContentAndToolCalls verifies a message
// with both text and tool_calls is handled correctly.
func TestConvertMessagesToOpenAI_MixedContentAndToolCalls(t *testing.T) {
	messages := []Message{
		{Role: RoleAssistant, Content: []ContentBlock{
			{Type: "text", Text: "I'll check that."},
			{Type: "tool_use", ID: "call_1", Name: "bash", Input: map[string]any{"command": "cat file.txt"}},
		}},
	}

	result := convertMessagesToOpenAI(messages, "")

	if len(result) != 1 {
		t.Fatalf("expected 1 message, got %d", len(result))
	}

	assistant := result[0]
	if assistant.Content != "I'll check that." {
		t.Errorf("expected text content, got %v", assistant.Content)
	}
	if len(assistant.ToolCalls) != 1 {
		t.Fatalf("expected 1 tool_call, got %d", len(assistant.ToolCalls))
	}
}

// TestExtractText verifies text extraction from various content formats.
func TestExtractText(t *testing.T) {
	tests := []struct {
		name     string
		content  any
		expected string
	}{
		{"string", "hello world", "hello world"},
		{"content blocks", []ContentBlock{{Type: "text", Text: "abc"}, {Type: "text", Text: "def"}}, "abcdef"},
		{"single block", []ContentBlock{{Type: "text", Text: "solo"}}, "solo"},
		{"mixed blocks", []ContentBlock{{Type: "text", Text: "text1"}, {Type: "tool_use", ID: "c1", Name: "bash"}}, "text1"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := ExtractText(tt.content)
			if got != tt.expected {
				t.Errorf("ExtractText(%v) = %q, want %q", tt.content, got, tt.expected)
			}
		})
	}
}

// TestAsTextBlocks verifies the AsTextBlocks normalization handles []any from JSON.
func TestAsTextBlocks(t *testing.T) {
	// Simulate JSON-unmarshaled content: []any with map items
	raw := `[{"type":"text","text":"hello"},{"type":"tool_use","id":"c1","name":"bash"}]`

	var rawAny []any
	if err := json.Unmarshal([]byte(raw), &rawAny); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	blocks := AsTextBlocks(rawAny)

	if len(blocks) != 2 {
		t.Fatalf("expected 2 blocks, got %d", len(blocks))
	}
	if blocks[0].Type != "text" || blocks[0].Text != "hello" {
		t.Errorf("block[0]: expected text 'hello', got type=%s text=%s", blocks[0].Type, blocks[0].Text)
	}
	if blocks[1].Type != "tool_use" || blocks[1].Name != "bash" {
		t.Errorf("block[1]: expected tool_use 'bash', got type=%s name=%s", blocks[1].Type, blocks[1].Name)
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > 0 && containsStr(s, substr))
}

func containsStr(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
