package tools

import (
        "context"
        "encoding/json"
        "os"
        "path/filepath"
        "strings"
        "testing"
)

// contains checks if s contains substr.
func contains(s, substr string) bool {
        return strings.Contains(s, substr)
}

// TestGlobInvalidInput verifies error for invalid JSON.
func TestGlobInvalidInput(t *testing.T) {
        tool := NewGlobTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
        if err == nil {
                t.Fatal("expected error for invalid JSON")
        }
}

// TestGlobEmptyPattern verifies error when pattern is empty.
func TestGlobEmptyPattern(t *testing.T) {
        tool := NewGlobTool()
        _, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":""}`))
        if err == nil || err.Error() != "pattern is required" {
                t.Errorf("expected 'pattern is required', got: %v", err)
        }
}

// TestGlobNoMatches verifies output when nothing matches.
func TestGlobNoMatches(t *testing.T) {
        tool := NewGlobTool()
        tmpDir := t.TempDir()
        result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"*.nonexistent","path":"`+tmpDir+`"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if result != "No files matched the pattern." {
                t.Errorf("expected no matches message, got: %q", result)
        }
}

// TestGlobSimplePattern verifies basic glob matching.
func TestGlobSimplePattern(t *testing.T) {
        tool := NewGlobTool()
        tmpDir := t.TempDir()
        os.WriteFile(filepath.Join(tmpDir, "a.go"), []byte{}, 0644)
        os.WriteFile(filepath.Join(tmpDir, "b.go"), []byte{}, 0644)
        os.WriteFile(filepath.Join(tmpDir, "c.txt"), []byte{}, 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"*.go","path":"`+tmpDir+`"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if !contains(result, "Found 2 matching file(s)") {
                t.Errorf("expected 2 matches, got: %q", result)
        }
        if !contains(result, "a.go") || !contains(result, "b.go") {
                t.Errorf("expected a.go and b.go, got: %q", result)
        }
}

// TestGlobRecursivePattern verifies recursive ** matching.
func TestGlobRecursivePattern(t *testing.T) {
        tool := NewGlobTool()
        tmpDir := t.TempDir()
        os.MkdirAll(filepath.Join(tmpDir, "sub", "deep"), 0755)
        os.WriteFile(filepath.Join(tmpDir, "root.go"), []byte{}, 0644)
        os.WriteFile(filepath.Join(tmpDir, "sub", "nested.go"), []byte{}, 0644)
        os.WriteFile(filepath.Join(tmpDir, "sub", "deep", "deep.go"), []byte{}, 0644)

        result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"**/*.go","path":"`+tmpDir+`"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if !contains(result, "Found 3 matching file(s)") {
                t.Errorf("expected 3 matches, got: %q", result)
        }
}

// TestGlobHiddenFiles verifies hidden files are filtered unless pattern starts with dot.
func TestGlobHiddenFiles(t *testing.T) {
        tool := NewGlobTool()
        tmpDir := t.TempDir()
        os.WriteFile(filepath.Join(tmpDir, "visible.go"), []byte{}, 0644)
        os.WriteFile(filepath.Join(tmpDir, ".hidden.go"), []byte{}, 0644)

        // Without dot prefix — should hide .hidden.go
        result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"*.go","path":"`+tmpDir+`"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if contains(result, ".hidden.go") {
                t.Errorf("hidden files should be filtered: %q", result)
        }
        if !contains(result, "Found 1 matching file(s)") {
                t.Errorf("expected 1 match, got: %q", result)
        }

        // With dot prefix — should show .hidden.go
        result2, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":".*","path":"`+tmpDir+`"}`))
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if !contains(result2, ".hidden.go") {
                t.Errorf("hidden files should be visible with dot pattern: %q", result2)
        }
}

// TestGlobNeedsPermission verifies GlobTool doesn't need permission.
func TestGlobNeedsPermission(t *testing.T) {
        tool := NewGlobTool()
        if tool.NeedsPermission() {
                t.Error("GlobTool should not need permission")
        }
}
