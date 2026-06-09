package tools

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestGrepInvalidInput verifies error for invalid JSON.
func TestGrepInvalidInput(t *testing.T) {
	tool := NewGrepTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

// TestGrepEmptyPattern verifies error when pattern is empty.
func TestGrepEmptyPattern(t *testing.T) {
	tool := NewGrepTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":""}`))
	if err == nil || err.Error() != "pattern is required" {
		t.Errorf("expected 'pattern is required', got: %v", err)
	}
}

// TestGrepInvalidRegex verifies error for invalid regex.
func TestGrepInvalidRegex(t *testing.T) {
	tool := NewGrepTool()
	_, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"[invalid"}`))
	if err == nil {
		t.Fatal("expected error for invalid regex")
	}
	if !strings.Contains(err.Error(), "invalid regex pattern") {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestGrepNoMatches verifies output when nothing matches.
func TestGrepNoMatches(t *testing.T) {
	tool := NewGrepTool()
	tmpDir := t.TempDir()
	os.WriteFile(filepath.Join(tmpDir, "test.go"), []byte("package main\n"), 0644)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"ZZZZZ","path":"`+tmpDir+`"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "No matches found." {
		t.Errorf("expected no matches, got: %q", result)
	}
}

// TestGrepSingleFile verifies searching a single file.
func TestGrepSingleFile(t *testing.T) {
	tool := NewGrepTool()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "test.go")
	os.WriteFile(path, []byte("package main\nfunc main() {\n\tprintln(\"hello\")\n}\n"), 0644)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"main","path":"`+path+`"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "matched") {
		t.Errorf("expected match, got: %q", result)
	}
}

// TestGrepDirectory verifies searching a directory.
func TestGrepDirectory(t *testing.T) {
	tool := NewGrepTool()
	tmpDir := t.TempDir()
	os.WriteFile(filepath.Join(tmpDir, "a.go"), []byte("package a\nfunc A() {}\n"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "b.go"), []byte("package b\nfunc B() {}\n"), 0644)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"func","path":"`+tmpDir+`","output_mode":"files_with_matches"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "2 file(s) matched") {
		t.Errorf("expected 2 files matched, got: %q", result)
	}
}

// TestGrepCaseInsensitive verifies case-insensitive search.
func TestGrepCaseInsensitive(t *testing.T) {
	tool := NewGrepTool()
	tmpDir := t.TempDir()
	os.WriteFile(filepath.Join(tmpDir, "test.txt"), []byte("Hello WORLD\n"), 0644)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"hello","path":"`+tmpDir+`","i":true}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, "matched") {
		t.Errorf("expected match with case insensitive, got: %q", result)
	}
}

// TestGrepCountMode verifies count output mode.
func TestGrepCountMode(t *testing.T) {
	tool := NewGrepTool()
	tmpDir := t.TempDir()
	os.WriteFile(filepath.Join(tmpDir, "test.txt"), []byte("aaa\nbbb\naaa\n"), 0644)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"aaa","path":"`+tmpDir+`","output_mode":"count"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.Contains(result, ":2") {
		t.Errorf("expected count of 2, got: %q", result)
	}
}

// TestGrepNeedsPermission verifies GrepTool doesn't need permission.
func TestGrepNeedsPermission(t *testing.T) {
	tool := NewGrepTool()
	if tool.NeedsPermission() {
		t.Error("GrepTool should not need permission")
	}
}

// TestGrepSkipsDotGit verifies .git directory is skipped.
func TestGrepSkipsDotGit(t *testing.T) {
	tool := NewGrepTool()
	tmpDir := t.TempDir()
	gitDir := filepath.Join(tmpDir, ".git")
	os.MkdirAll(gitDir, 0755)
	os.WriteFile(filepath.Join(gitDir, "config"), []byte("matchme\n"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "main.go"), []byte("matchme\n"), 0644)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"matchme","path":"`+tmpDir+`","output_mode":"files_with_matches"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if strings.Contains(result, ".git") {
		t.Errorf(".git should be skipped: %q", result)
	}
	if !strings.Contains(result, "1 file(s) matched") {
		t.Errorf("expected 1 match (excluding .git), got: %q", result)
	}
}

// TestGrepSkipsNodeModules verifies node_modules is skipped.
func TestGrepSkipsNodeModules(t *testing.T) {
	tool := NewGrepTool()
	tmpDir := t.TempDir()
	nmDir := filepath.Join(tmpDir, "node_modules")
	os.MkdirAll(nmDir, 0755)
	os.WriteFile(filepath.Join(nmDir, "pkg.js"), []byte("matchme\n"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "app.go"), []byte("matchme\n"), 0644)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"pattern":"matchme","path":"`+tmpDir+`","output_mode":"files_with_matches"}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if strings.Contains(result, "node_modules") {
		t.Errorf("node_modules should be skipped: %q", result)
	}
}
