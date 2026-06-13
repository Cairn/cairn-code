package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// FileReadTool reads file contents.
type FileReadTool struct{}

func NewFileReadTool() *FileReadTool {
	return &FileReadTool{}
}

func (t *FileReadTool) Name() string { return "file_read" }

func (t *FileReadTool) Description() string {
	return "Reads a file from the filesystem. Returns content with line numbers. Supports offset and limit for pagination. Detects binary files."
}

func (t *FileReadTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"file_path": map[string]any{
				"type":        "string",
				"description": "The absolute path to the file to read.",
			},
			"offset": map[string]any{
				"type":        "integer",
				"description": "Line number to start reading from (1-based).",
			},
			"limit": map[string]any{
				"type":        "integer",
				"description": "Maximum number of lines to read.",
			},
		},
		"required": []string{"file_path"},
	}
}

func (t *FileReadTool) NeedsPermission() bool { return false }

type fileReadInput struct {
	FilePath string `json:"file_path"`
	Offset   *int   `json:"offset,omitempty"`
	Limit    *int   `json:"limit,omitempty"`
}

func (t *FileReadTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params fileReadInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.FilePath == "" {
		return "", fmt.Errorf("file_path is required")
	}

	// Check if file exists
	info, err := os.Stat(params.FilePath)
	if err != nil {
		if os.IsNotExist(err) {
			return "", fmt.Errorf("file not found: %s", params.FilePath)
		}
		return "", fmt.Errorf("accessing file: %w", err)
	}

	// Check file size (max 1MB)
	const maxSize = 1 << 20 // 1MB
	if info.Size() > maxSize {
		return "", fmt.Errorf("file too large (%d bytes, max %d bytes). Use offset/limit to read portions", info.Size(), maxSize)
	}

	// Check if file is a regular file
	if !info.Mode().IsRegular() {
		return "", fmt.Errorf("not a regular file: %s", params.FilePath)
	}

	data, err := os.ReadFile(params.FilePath)
	if err != nil {
		return "", fmt.Errorf("reading file: %w", err)
	}

	// Detect binary file
	if isBinary(data) {
		return "", fmt.Errorf("file appears to be binary: %s", params.FilePath)
	}

	lines := strings.Split(string(data), "\n")

	// Remove trailing empty line from split
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}

	offset := 1
	if params.Offset != nil && *params.Offset > 0 {
		offset = *params.Offset
	}

	limit := len(lines)
	if params.Limit != nil && *params.Limit > 0 {
		limit = *params.Limit
	}

	// Apply offset
	if offset > len(lines)+1 {
		return fmt.Sprintf("offset %d exceeds file length (%d lines)", offset, len(lines)), nil
	}
	if offset < 1 {
		offset = 1
	}

	startIdx := offset - 1
	endIdx := startIdx + limit
	if endIdx > len(lines) {
		endIdx = len(lines)
	}

	// Build output with line numbers
	var buf bytes.Buffer
	for i := startIdx; i < endIdx; i++ {
		fmt.Fprintf(&buf, "%6d\t%s\n", i+1, lines[i])
	}

	return buf.String(), nil
}

// isBinary checks if data appears to be binary content.
func isBinary(data []byte) bool {
	const limit = 512
	if len(data) > limit {
		data = data[:limit]
	}
	for _, b := range data {
		if b == 0 {
			return true
		}
	}
	return false
}

// absPath resolves a file path to an absolute path.
func absPath(p string) string {
	if filepath.IsAbs(p) {
		return p
	}
	abs, err := filepath.Abs(p)
	if err != nil {
		return p
	}
	return abs
}
