package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

// FileEditTool performs find-and-replace edits in files.
type FileEditTool struct{}

func NewFileEditTool() *FileEditTool {
	return &FileEditTool{}
}

func (t *FileEditTool) Name() string { return "file_edit" }

func (t *FileEditTool) Description() string {
	return "Performs find-and-replace edits in a file. Finds old_string and replaces it with new_string. Verifies the old_string exists and is unique (unless replace_all is true)."
}

func (t *FileEditTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"file_path": map[string]any{
				"type":        "string",
				"description": "The absolute path to the file to edit.",
			},
			"old_string": map[string]any{
				"type":        "string",
				"description": "The text to find in the file.",
			},
			"new_string": map[string]any{
				"type":        "string",
				"description": "The text to replace old_string with.",
			},
			"replace_all": map[string]any{
				"type":        "boolean",
				"description": "Replace all occurrences instead of just one (default false).",
			},
		},
		"required": []string{"file_path", "old_string", "new_string"},
	}
}

func (t *FileEditTool) NeedsPermission() bool { return true }

type fileEditInput struct {
	FilePath   string `json:"file_path"`
	OldString  string `json:"old_string"`
	NewString  string `json:"new_string"`
	ReplaceAll bool   `json:"replace_all"`
}

func (t *FileEditTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params fileEditInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.FilePath == "" {
		return "", fmt.Errorf("file_path is required")
	}
	if params.OldString == "" {
		return "", fmt.Errorf("old_string is required")
	}

	absPath := absPath(params.FilePath)

	// Read existing file
	data, err := os.ReadFile(absPath)
	if err != nil {
		return "", fmt.Errorf("reading file: %w", err)
	}

	content := string(data)

	// Check if old_string exists
	if !strings.Contains(content, params.OldString) {
		return "", fmt.Errorf("old_string not found in file %s", absPath)
	}

	if !params.ReplaceAll {
		// Check uniqueness
		count := strings.Count(content, params.OldString)
		if count > 1 {
			return "", fmt.Errorf("old_string found %d times in file (provide more context to make it unique, or use replace_all)", count)
		}
	}

	// Perform replacement
	var newContent string
	if params.ReplaceAll {
		newContent = strings.ReplaceAll(content, params.OldString, params.NewString)
	} else {
		newContent = strings.Replace(content, params.OldString, params.NewString, 1)
	}

	// Write back
	if err := os.WriteFile(absPath, []byte(newContent), 0644); err != nil {
		return "", fmt.Errorf("writing file: %w", err)
	}

	// Build diff output
	diff := computeSimpleDiff(content, newContent, absPath)

	return fmt.Sprintf("File edited successfully: %s\n\n%s", absPath, diff), nil
}

// computeSimpleDiff generates a simple unified diff between old and new content.
func computeSimpleDiff(old, new, filename string) string {
	oldLines := strings.Split(old, "\n")
	newLines := strings.Split(new, "\n")

	var buf strings.Builder
	buf.WriteString(fmt.Sprintf("--- %s (original)\n", filename))
	buf.WriteString(fmt.Sprintf("+++ %s (modified)\n", filename))

	// Simple line-by-line comparison
	maxLen := len(oldLines)
	if len(newLines) > maxLen {
		maxLen = len(newLines)
	}

	added := 0
	removed := 0

	i := 0
	for i < maxLen {
		oldLine := ""
		newLine := ""
		if i < len(oldLines) {
			oldLine = oldLines[i]
		}
		if i < len(newLines) {
			newLine = newLines[i]
		}

		if oldLine == newLine {
			// Skip context lines, but show a few around changes
			i++
			continue
		}

		// Find the extent of the change
		oldStart := i
		newStart := i
		for i < len(oldLines) && i < len(newLines) && oldLines[i] != newLines[i] {
			i++
		}

		// Output removed lines
		for j := oldStart; j < i && j < len(oldLines); j++ {
			fmt.Fprintf(&buf, "-%s\n", oldLines[j])
			removed++
		}

		// Output added lines
		for j := newStart; j < i && j < len(newLines); j++ {
			fmt.Fprintf(&buf, "+%s\n", newLines[j])
			added++
		}
	}

	// Handle trailing new lines
	for i < len(oldLines) {
		fmt.Fprintf(&buf, "-%s\n", oldLines[i])
		removed++
		i++
	}
	jIdx := len(newLines)
	if len(oldLines) < len(newLines) {
		jIdx = len(oldLines)
	}
	for jIdx < len(newLines) {
		fmt.Fprintf(&buf, "+%s\n", newLines[jIdx])
		added++
		jIdx++
	}

	buf.WriteString(fmt.Sprintf("\n%d line(s) removed, %d line(s) added\n", removed, added))

	return buf.String()
}
