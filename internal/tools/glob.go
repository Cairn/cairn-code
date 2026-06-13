package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/bmatcuk/doublestar/v4"
)

// GlobTool finds files matching a glob pattern.
type GlobTool struct{}

func NewGlobTool() *GlobTool {
	return &GlobTool{}
}

func (t *GlobTool) Name() string { return "glob" }

func (t *GlobTool) Description() string {
	return "Fast file pattern matching tool using glob patterns. Supports patterns like '**/*.go', 'src/**/*.ts', '*.json'. Returns matching file paths sorted alphabetically."
}

func (t *GlobTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"pattern": map[string]any{
				"type":        "string",
				"description": "The glob pattern to match files against (e.g., '**/*.go').",
			},
			"path": map[string]any{
				"type":        "string",
				"description": "The directory to search in. Defaults to current working directory.",
			},
		},
		"required": []string{"pattern"},
	}
}

func (t *GlobTool) NeedsPermission() bool { return false }

type globInput struct {
	Pattern string `json:"pattern"`
	Path    string `json:"path,omitempty"`
}

func (t *GlobTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params globInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.Pattern == "" {
		return "", fmt.Errorf("pattern is required")
	}

	searchPath := "."
	if params.Path != "" {
		searchPath = params.Path
	}

	absPath, err := filepath.Abs(searchPath)
	if err != nil {
		return "", fmt.Errorf("resolving path: %w", err)
	}

	// Use doublestar for glob matching
	matches, err := doublestar.Glob(os.DirFS(absPath), params.Pattern, doublestar.WithFilesOnly())
	if err != nil {
		return "", fmt.Errorf("glob matching: %w", err)
	}

	// Filter out hidden files and directories (unless pattern starts with .)
	var filtered []string
	showHidden := strings.HasPrefix(params.Pattern, ".")
	for _, match := range matches {
		if !showHidden {
			base := filepath.Base(match)
			if strings.HasPrefix(base, ".") {
				continue
			}
		}
		// Make paths relative to search path
		rel, err := filepath.Rel(".", filepath.Join(absPath, match))
		if err != nil {
			rel = match
		}
		filtered = append(filtered, rel)
	}

	if len(filtered) == 0 {
		return "No files matched the pattern.", nil
	}

	result := strings.Join(filtered, "\n")
	return fmt.Sprintf("Found %d matching file(s):\n%s", len(filtered), result), nil
}
