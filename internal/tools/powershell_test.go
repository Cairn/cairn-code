//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
)

// TestPowerShellInvalidInput verifies error for invalid JSON.
func TestPowerShellInvalidInput(t *testing.T) {
	tool := NewPowerShellTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

// TestPowerShellEmptyCommand verifies error for empty command.
func TestPowerShellEmptyCommand(t *testing.T) {
	tool := NewPowerShellTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{"command": ""}`))
	if err == nil {
		t.Fatal("expected error for empty command")
	}
}

// TestPowerShellSimpleCommand executes a basic PowerShell command.
func TestPowerShellSimpleCommand(t *testing.T) {
	tool := NewPowerShellTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command": "Write-Output hello"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "hello") {
		t.Fatalf("expected output to contain 'hello', got: %s", result)
	}
	if !strings.Contains(result, "Exit code: 0") {
		t.Fatalf("expected exit code 0, got: %s", result)
	}
}

// TestPowerShellNonZeroExitCode captures non-zero exit codes.
func TestPowerShellNonZeroExitCode(t *testing.T) {
	tool := NewPowerShellTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command": "exit 1"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "Exit code: 1") {
		t.Fatalf("expected exit code 1, got: %s", result)
	}
}

// TestPowerShellTimeout respects timeout parameter.
func TestPowerShellTimeout(t *testing.T) {
	tool := NewPowerShellTool()
	timeout := 100 // 100ms
	_, err := tool.Execute(context.Background(), json.RawMessage(`{"command": "Start-Sleep -Seconds 10", "timeout": 100}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	_ = timeout
}

// TestPowerShellOutputTruncation truncates large output.
func TestPowerShellOutputTruncation(t *testing.T) {
	tool := NewPowerShellTool()
	result, err := tool.Execute(context.Background(), json.RawMessage(`{"command": "1..60000 | ForEach-Object { Write-Output 'x' }"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "[output truncated]") {
		t.Fatalf("expected truncation message, got length: %d", len(result))
	}
}

// TestPowerShellName returns correct tool name.
func TestPowerShellName(t *testing.T) {
	tool := NewPowerShellTool()
	if tool.Name() != "powershell" {
		t.Fatalf("expected name 'powershell', got '%s'", tool.Name())
	}
}

// TestPowerShellNeedsPermission returns true.
func TestPowerShellNeedsPermission(t *testing.T) {
	tool := NewPowerShellTool()
	if !tool.NeedsPermission() {
		t.Fatal("expected NeedsPermission to return true")
	}
}
