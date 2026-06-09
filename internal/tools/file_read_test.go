package tools

import (
        "context"
        "encoding/json"
        "os"
        "path/filepath"
        "strings"
        "testing"
)

// TestFileReadInvalidInput verifies error handling for invalid JSON input.
func TestFileReadInvalidInput(t *testing.T) {
        tool := NewFileReadTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
        if err == nil {
                t.Fatal("expected error for invalid JSON")
        }
}

// TestFileReadEmptyPath verifies error when file_path is empty.
func TestFileReadEmptyPath(t *testing.T) {
        tool := NewFileReadTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":""}`))
        if err == nil || err.Error() != "file_path is required" {
                t.Errorf("expected 'file_path is required' error, got: %v", err)
        }
}

// TestFileReadNotFound verifies error when file doesn't exist.
func TestFileReadNotFound(t *testing.T) {
        tool := NewFileReadTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"/nonexistent/file.txt"}`))
        if err == nil {
                t.Fatal("expected error for non-existent file")
        }
}

// TestFileReadDirectory verifies error when path is a directory.
func TestFileReadDirectory(t *testing.T) {
        tool := NewFileReadTool()
        dir := t.TempDir()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+dir+`"}`))
        if err == nil {
                t.Fatal("expected error for directory")
        }
}

// TestFileReadBinary verifies binary file detection.
func TestFileReadBinary(t *testing.T) {
        tool := NewFileReadTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "binary.bin")
        os.WriteFile(path, []byte{0x00, 0x01, 0x02}, 0644)

        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`"}`))
        if err == nil {
                t.Fatal("expected error for binary file")
        }
}

// TestFileReadTooLarge verifies error when file exceeds 1MB.
func TestFileReadTooLarge(t *testing.T) {
        tool := NewFileReadTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "huge.txt")
        data := make([]byte, 1<<20+1)
        for i := range data {
                data[i] = 'a'
        }
        os.WriteFile(path, data, 0644)

        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`"}`))
        if err == nil {
                t.Fatal("expected error for too-large file")
        }
}

// TestFileReadBasic verifies basic file reading with line numbers.
func TestFileReadBasic(t *testing.T) {
        tool := NewFileReadTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("line1\nline2\nline3\n"), 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        // Output has trailing newline from fmt.Fprintf
        if !contains(result, "line1") || !contains(result, "line2") || !contains(result, "line3") {
                t.Errorf("expected all lines in output, got: %q", result)
        }
}

// TestFileReadWithOffset verifies pagination with offset.
func TestFileReadWithOffset(t *testing.T) {
        tool := NewFileReadTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("line1\nline2\nline3\nline4\nline5\n"), 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","offset":3}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if !contains(result, "line3") || !contains(result, "line4") || !contains(result, "line5") {
                t.Errorf("expected lines 3-5 in output, got: %q", result)
        }
        if contains(result, "line1") || contains(result, "line2") {
                t.Errorf("should not contain lines 1-2, got: %q", result)
        }
}

// TestFileReadWithLimit verifies pagination with limit.
func TestFileReadWithLimit(t *testing.T) {
        tool := NewFileReadTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("line1\nline2\nline3\nline4\nline5\n"), 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","offset":2,"limit":2}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if !contains(result, "line2") || !contains(result, "line3") {
                t.Errorf("expected lines 2-3 in output, got: %q", result)
        }
        if contains(result, "line4") || contains(result, "line1") {
                t.Errorf("should not contain lines 1 or 4, got: %q", result)
        }
}

// TestFileReadOffsetExceedsLength verifies behavior when offset exceeds file length.
func TestFileReadOffsetExceedsLength(t *testing.T) {
        tool := NewFileReadTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("line1\nline2\n"), 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","offset":10}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if !contains(result, "offset 10 exceeds file length") {
                t.Errorf("unexpected output: %q", result)
        }
}

// TestFileReadEmptyFile verifies reading an empty file.
func TestFileReadEmptyFile(t *testing.T) {
        tool := NewFileReadTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "empty.txt")
        os.WriteFile(path, []byte(""), 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if result != "" {
                t.Errorf("expected empty output, got: %q", result)
        }
}

// TestIsBinary verifies binary detection logic.
func TestIsBinary(t *testing.T) {
        tests := []struct {
                name     string
                data     []byte
                expected bool
        }{
                {"text", []byte("hello world"), false},
                {"binary with null", []byte{0x00}, true},
                {"binary in middle", []byte("hello\x00world"), true},
                {"empty data", []byte{}, false},
                {"512 bytes non-null", []byte(strings.Repeat("a", 512)), false},
                {"513 bytes no null", []byte(strings.Repeat("b", 513)), false},
        }

        for _, tt := range tests {
                t.Run(tt.name, func(t *testing.T) {
                        got := isBinary(tt.data)
                        if got != tt.expected {
                                t.Errorf("isBinary() = %v, want %v", got, tt.expected)
                        }
                })
        }
}

// TestAbsPath verifies absolute path resolution.
func TestAbsPath(t *testing.T) {
        if absPath("/tmp/test") != "/tmp/test" {
                t.Errorf("absolute path should be returned as-is")
        }
        rel := absPath("test.txt")
        if rel == "" {
                t.Error("absPath should not return empty string")
        }
}

// TestFileReadNeedsPermission verifies FileReadTool doesn't need permission.
func TestFileReadNeedsPermission(t *testing.T) {
        tool := NewFileReadTool()
        if tool.NeedsPermission() {
                t.Error("FileReadTool should not need permission")
        }
}

// TestFileReadToolMetadata verifies tool metadata.
func TestFileReadToolMetadata(t *testing.T) {
        tool := NewFileReadTool()
        if tool.Name() != "file_read" {
                t.Errorf("expected name 'file_read', got %q", tool.Name())
        }
        if tool.Description() == "" {
                t.Error("description should not be empty")
        }
        schema := tool.InputSchema()
        if schema["type"] != "object" {
                t.Errorf("expected schema type 'object', got %v", schema["type"])
        }
}
