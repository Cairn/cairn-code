package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

// FileWriteTool creates or overwrites a file.
type FileWriteTool struct{}

func NewFileWriteTool() *FileWriteTool {
	return &FileWriteTool{}
}

func (t *FileWriteTool) Name() string { return "file_write" }

func (t *FileWriteTool) Description() string {
	return "Creates a new file or overwrites an existing file with the given content. Creates parent directories if they don't exist."
}

func (t *FileWriteTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"file_path": map[string]any{
				"type":        "string",
				"description": "The absolute path to the file to write.",
			},
			"content": map[string]any{
				"type":        "string",
				"description": "The content to write to the file.",
			},
		},
		"required": []string{"file_path", "content"},
	}
}

func (t *FileWriteTool) NeedsPermission() bool { return true }

type fileWriteInput struct {
	FilePath string `json:"file_path"`
	Content  string `json:"content"`
}

func (t *FileWriteTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params fileWriteInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.FilePath == "" {
		return "", fmt.Errorf("file_path is required")
	}

	absPath := absPath(params.FilePath)

	// Create parent directories
	dir := filepath.Dir(absPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return "", fmt.Errorf("creating parent directories: %w", err)
	}

	// Write file
	if err := os.WriteFile(absPath, []byte(params.Content), 0644); err != nil {
		return "", fmt.Errorf("writing file: %w", err)
	}

	// Count lines
	lineCount := 0
	for _, b := range params.Content {
		if b == '\n' {
			lineCount++
		}
	}
	if len(params.Content) > 0 && params.Content[len(params.Content)-1] != '\n' {
		lineCount++
	}

	return fmt.Sprintf("Successfully wrote %d bytes (%d lines) to %s", len(params.Content), lineCount, absPath), nil
}
