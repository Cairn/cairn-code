package tools

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
)

// TestBashInvalidInput verifies error for invalid JSON.
func TestBashInvalidInput(t *testing.T) {
	tool := NewBashTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

// TestBashEmptyCommand verifies error when command is empty.
func TestBashEmptyCommand(t *testing.T) {
	tool := NewBashTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{"command":""}`))
	if err == nil || err.Error() != "command is required" {
		t.Errorf("expected 'command is required', got: %v", err)
	}
}

// TestBashSuccessfulCommand verifies a simple successful command.
func TestBashSuccessfulCommand(t *testing.T) {
	tool := NewBashTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command":"echo hello"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "Exit code: 0") {
		t.Errorf("expected exit code 0, got: %q", result)
	}
	if !strings.Contains(result, "hello") {
		t.Errorf("expected 'hello' in output, got: %q", result)
	}
}

// TestBashFailedCommand verifies a command that exits with non-zero.
func TestBashFailedCommand(t *testing.T) {
	tool := NewBashTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command":"exit 42"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "Exit code: 42") {
		t.Errorf("expected exit code 42, got: %q", result)
	}
}

// TestBashStderr verifies stderr is captured.
func TestBashStderr(t *testing.T) {
	tool := NewBashTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command":"echo error_msg >&2"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "error_msg") {
		t.Errorf("expected stderr output, got: %q", result)
	}
}

// TestBashTimeoutCap verifies timeout is capped at 600000ms.
func TestBashTimeoutCap(t *testing.T) {
	tool := NewBashTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command":"echo fast","timeout":999999}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "Exit code: 0") {
		t.Errorf("expected success with capped timeout, got: %q", result)
	}
}

// TestBashZeroTimeout verifies zero timeout defaults to 120000ms.
func TestBashZeroTimeout(t *testing.T) {
	tool := NewBashTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command":"echo ok","timeout":0}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "Exit code: 0") {
		t.Errorf("expected success, got: %q", result)
	}
}

// TestBashNeedsPermission verifies BashTool needs permission.
func TestBashNeedsPermission(t *testing.T) {
	tool := NewBashTool()
	if !tool.NeedsPermission() {
		t.Error("BashTool should need permission")
	}
}

// TestBashToolMetadata verifies tool metadata.
func TestBashToolMetadata(t *testing.T) {
	tool := NewBashTool()
	if tool.Name() != "bash" {
		t.Errorf("expected name 'bash', got %q", tool.Name())
	}
	if tool.Description() == "" {
		t.Error("description should not be empty")
	}
}
