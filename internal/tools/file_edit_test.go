package tools

import (
        "context"
        "encoding/json"
        "os"
        "path/filepath"
        "strings"
        "testing"
)

// TestFileEditInvalidInput verifies error for invalid JSON.
func TestFileEditInvalidInput(t *testing.T) {
        tool := NewFileEditTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
        if err == nil {
                t.Fatal("expected error for invalid JSON")
        }
}

// TestFileEditEmptyPath verifies error when file_path is empty.
func TestFileEditEmptyPath(t *testing.T) {
        tool := NewFileEditTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"","old_string":"a","new_string":"b"}`))
        if err == nil || err.Error() != "file_path is required" {
                t.Errorf("expected 'file_path is required', got: %v", err)
        }
}

// TestFileEditEmptyOldString verifies error when old_string is empty.
func TestFileEditEmptyOldString(t *testing.T) {
        tool := NewFileEditTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"/tmp/test.txt","old_string":"","new_string":"b"}`))
        if err == nil || err.Error() != "old_string is required" {
                t.Errorf("expected 'old_string is required', got: %v", err)
        }
}

// TestFileEditFileNotFound verifies error when file doesn't exist.
func TestFileEditFileNotFound(t *testing.T) {
        tool := NewFileEditTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"/nonexistent/test.txt","old_string":"a","new_string":"b"}`))
        if err == nil {
                t.Fatal("expected error for non-existent file")
        }
}

// TestFileEditOldStringNotFound verifies error when old_string doesn't exist.
func TestFileEditOldStringNotFound(t *testing.T) {
        tool := NewFileEditTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("hello world\n"), 0644)

        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","old_string":"foo","new_string":"bar"}`))
        if err == nil {
                t.Fatal("expected error when old_string not found")
        }
        if !strings.Contains(err.Error(), "old_string not found") {
                t.Errorf("unexpected error: %v", err)
        }
}

// TestFileEditAmbiguousMatch verifies error when old_string matches multiple times.
func TestFileEditAmbiguousMatch(t *testing.T) {
        tool := NewFileEditTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("hello hello hello\n"), 0644)

        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","old_string":"hello","new_string":"world"}`))
        if err == nil {
                t.Fatal("expected error for ambiguous match")
        }
        if !strings.Contains(err.Error(), "found 3 times") {
                t.Errorf("unexpected error: %v", err)
        }
}

// TestFileEditSingleReplace verifies basic single replacement.
func TestFileEditSingleReplace(t *testing.T) {
        tool := NewFileEditTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("hello world\n"), 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","old_string":"world","new_string":"Go"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        data, _ := os.ReadFile(path)
        if string(data) != "hello Go\n" {
                t.Errorf("file content = %q, want %q", string(data), "hello Go\n")
        }
        if !strings.Contains(result, "File edited successfully") {
                t.Errorf("result should mention success: %q", result)
        }
}

// TestFileEditReplaceAll verifies replace_all mode replaces all occurrences.
func TestFileEditReplaceAll(t *testing.T) {
        tool := NewFileEditTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("a b a b a\n"), 0644)

        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","old_string":"a","new_string":"x","replace_all":true}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        data, _ := os.ReadFile(path)
        if string(data) != "x b x b x\n" {
                t.Errorf("file content = %q, want %q", string(data), "x b x b x\n")
        }
}

// TestFileEditDelete verifies replacing with empty string (deletion).
func TestFileEditDelete(t *testing.T) {
        tool := NewFileEditTool()
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "test.txt")
        os.WriteFile(path, []byte("hello world\n"), 0644)

        _, err := tool.Execute(context.Background(), json.RawMessage(`{"file_path":"`+path+`","old_string":"world","new_string":""}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        data, _ := os.ReadFile(path)
        if string(data) != "hello \n" {
                t.Errorf("file content = %q, want %q", string(data), "hello \n")
        }
}

// TestFileEditNeedsPermission verifies FileEditTool needs permission.
func TestFileEditNeedsPermission(t *testing.T) {
        tool := NewFileEditTool()
        if !tool.NeedsPermission() {
                t.Error("FileEditTool should need permission")
        }
}

// TestComputeSimpleDiff verifies the simple diff computation.
func TestComputeSimpleDiff(t *testing.T) {
        tests := []struct {
                name         string
                old          string
                newStr       string
                wantRemoved  int
                wantAdded    int
        }{
                {"identical", "line1\nline2\n", "line1\nline2\n", 0, 0},
                {"single change", "line1\nline2\n", "line1\nchanged\n", 1, 1},
                {"add line", "line1\n", "line1\nline2\n", 0, 1},
                {"remove line", "line1\nline2\n", "line1\n", 1, 0},
                {"empty both", "", "", 0, 0},
                {"empty to content", "", "new\n", 0, 1},
                {"content to empty", "old\n", "", 1, 0},
        }

        for _, tt := range tests {
                t.Run(tt.name, func(t *testing.T) {
                        result := computeSimpleDiff(tt.old, tt.newStr, "test.txt")
                        if !strings.Contains(result, "original") || !strings.Contains(result, "modified") {
                                t.Errorf("diff should have headers, got: %q", result)
                        }
                        if tt.wantRemoved > 0 && !strings.Contains(result, "removed") {
                                t.Errorf("expected 'removed' in diff for %s", tt.name)
                        }
                        if tt.wantAdded > 0 && !strings.Contains(result, "added") {
                                t.Errorf("expected 'added' in diff for %s", tt.name)
                        }
                })
        }
}
