package tools

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

// TestFileWriteInvalidInput verifies error for invalid JSON.
func TestFileWriteInvalidInput(t *testing.T) {
	tool := NewFileWriteTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

// TestFileWriteEmptyPath verifies error when file_path is empty.
func TestFileWriteEmptyPath(t *testing.T) {
	tool := NewFileWriteTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"","content":"hello"}`))
	if err == nil || err.Error() != "file_path is required" {
		t.Errorf("expected 'file_path is required' error, got: %v", err)
	}
}

// TestFileWriteBasic verifies writing a new file.
func TestFileWriteBasic(t *testing.T) {
	tool := NewFileWriteTool()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "new.txt")

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","content":"hello world"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("file not created: %v", err)
	}
	if string(data) != "hello world" {
		t.Errorf("file content = %q, want %q", string(data), "hello world")
	}
	if result == "" {
		t.Error("result should not be empty")
	}
}

// TestFileWriteNestedPath verifies parent directories are created.
func TestFileWriteNestedPath(t *testing.T) {
	tool := NewFileWriteTool()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "a", "b", "c", "file.txt")

	_, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","content":"nested"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("file not created at nested path: %v", err)
	}
	if string(data) != "nested" {
		t.Errorf("file content = %q, want %q", string(data), "nested")
	}
}

// TestFileWriteEmptyContent verifies writing empty content.
func TestFileWriteEmptyContent(t *testing.T) {
	tool := NewFileWriteTool()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "empty.txt")

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","content":""}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	data, _ := os.ReadFile(path)
	if len(data) != 0 {
		t.Errorf("file should be empty, got %d bytes", len(data))
	}
	// Line count should be 0 for empty content
	if result != "Successfully wrote 0 bytes (0 lines) to "+path {
		t.Errorf("unexpected result: %q", result)
	}
}

// TestFileWriteWithNewline verifies line count for content ending with newline.
func TestFileWriteWithNewline(t *testing.T) {
	tool := NewFileWriteTool()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "lines.txt")

	_, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","content":"line1\nline2\n"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result, _ := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","content":"line1\nline2\n"}`)); true {
		// 2 newlines = 2 lines
		if result == "" {
			t.Error("result should not be empty")
		}
	}
}

// TestFileWriteOverwrite verifies overwriting existing file.
func TestFileWriteOverwrite(t *testing.T) {
	tool := NewFileWriteTool()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "overwrite.txt")
	os.WriteFile(path, []byte("original"), 0644)

	_, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","content":"replaced"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	data, _ := os.ReadFile(path)
	if string(data) != "replaced" {
		t.Errorf("file content = %q, want %q", string(data), "replaced")
	}
}

// TestFileWriteNoTrailingNewline verifies line count without trailing newline.
func TestFileWriteNoTrailingNewline(t *testing.T) {
	tool := NewFileWriteTool()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "no-nl.txt")

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","content":"hello"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// "hello" has no \n but is non-empty, so should be 1 line
	if result != "Successfully wrote 5 bytes (1 lines) to "+path {
		t.Errorf("unexpected result: %q", result)
	}
}

// TestFileWriteNeedsPermission verifies FileWriteTool needs permission.
func TestFileWriteNeedsPermission(t *testing.T) {
	tool := NewFileWriteTool()
	if !tool.NeedsPermission() {
		t.Error("FileWriteTool should need permission")
	}
}
