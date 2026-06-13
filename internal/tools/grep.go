package tools

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// GrepTool searches for patterns in files.
type GrepTool struct{}

func NewGrepTool() *GrepTool {
	return &GrepTool{}
}

func (t *GrepTool) Name() string { return "grep" }

func (t *GrepTool) Description() string {
	return "A powerful search tool that uses regular expressions to search for patterns in files. Can walk directory trees, match files by glob pattern, and return results in multiple output modes."
}

func (t *GrepTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"pattern": map[string]any{
				"type":        "string",
				"description": "The regular expression pattern to search for.",
			},
			"path": map[string]any{
				"type":        "string",
				"description": "File or directory to search in. Defaults to current directory.",
			},
			"glob": map[string]any{
				"type":        "string",
				"description": "Glob pattern to filter files (e.g., '*.go', '**/*.ts').",
			},
			"output_mode": map[string]any{
				"type":        "string",
				"description": "Output mode: 'content' (matching lines), 'files_with_matches' (file paths only), 'count' (match counts).",
				"enum":        []string{"content", "files_with_matches", "count"},
			},
			"i": map[string]any{
				"type":        "boolean",
				"description": "Case insensitive search.",
			},
			"n": map[string]any{
				"type":        "boolean",
				"description": "Show line numbers in output.",
			},
			"C": map[string]any{
				"type":        "integer",
				"description": "Number of lines of context to show around matches.",
			},
			"A": map[string]any{
				"type":        "integer",
				"description": "Number of lines of context after each match.",
			},
			"B": map[string]any{
				"type":        "integer",
				"description": "Number of lines of context before each match.",
			},
		},
		"required": []string{"pattern"},
	}
}

func (t *GrepTool) NeedsPermission() bool { return false }

type grepInput struct {
	Pattern    string `json:"pattern"`
	Path       string `json:"path,omitempty"`
	Glob       string `json:"glob,omitempty"`
	OutputMode string `json:"output_mode,omitempty"`
	IgnoreCase bool   `json:"i,omitempty"`
	ShowLineNum bool  `json:"n,omitempty"`
	Context    int    `json:"C,omitempty"`
	After      int    `json:"A,omitempty"`
	Before     int    `json:"B,omitempty"`
}

type fileMatch struct {
	Path     string
	Lines    []matchLine
	Count    int
}

type matchLine struct {
	LineNum  int
	Line     string
	Matches  []int // column positions of matches
}

func (t *GrepTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params grepInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.Pattern == "" {
		return "", fmt.Errorf("pattern is required")
	}

	// Determine output mode
	outputMode := params.OutputMode
	if outputMode == "" {
		outputMode = "content"
	}

	// Compile regex
	flags := ""
	if params.IgnoreCase {
		flags = "(?i)"
	}
	re, err := regexp.Compile(flags + params.Pattern)
	if err != nil {
		return "", fmt.Errorf("invalid regex pattern: %w", err)
	}

	// Determine search path
	searchPath := "."
	if params.Path != "" {
		searchPath = params.Path
	}

	// Compile glob pattern if provided
	var globRe *regexp.Regexp
	if params.Glob != "" {
		globPattern := params.Glob
		globPattern = strings.ReplaceAll(globPattern, ".", "\\.")
		globPattern = strings.ReplaceAll(globPattern, "*", ".*")
		globPattern = strings.ReplaceAll(globPattern, "?", ".")
		globRe, err = regexp.Compile(globPattern)
		if err != nil {
			return "", fmt.Errorf("invalid glob pattern: %w", err)
		}
	}

	// Determine context lines
	afterLines := params.After
	beforeLines := params.Before
	if params.Context > 0 {
		if afterLines == 0 {
			afterLines = params.Context
		}
		if beforeLines == 0 {
			beforeLines = params.Context
		}
	}

	// Walk directory tree
	var matches []fileMatch

	info, err := os.Stat(searchPath)
	if err != nil {
		return "", fmt.Errorf("accessing path: %w", err)
	}

	var files []string
	if info.IsDir() {
		err = filepath.WalkDir(searchPath, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				return nil // skip errors
			}
			if d.IsDir() {
				// Skip hidden directories and common skip dirs
				name := d.Name()
				if strings.HasPrefix(name, ".") && name != "." {
					return filepath.SkipDir
				}
				if name == "node_modules" || name == "vendor" || name == "__pycache__" || name == ".git" {
					return filepath.SkipDir
				}
				return nil
			}
			// Apply glob filter
			if globRe != nil && !globRe.MatchString(path) {
				return nil
			}
			files = append(files, path)
			return nil
		})
		if err != nil {
			return "", fmt.Errorf("walking directory: %w", err)
		}
	} else {
		files = []string{searchPath}
	}

	// Search each file
	for _, file := range files {
		f, err := os.Open(file)
		if err != nil {
			continue
		}

		var fm fileMatch
		fm.Path = file

		scanner := bufio.NewScanner(f)
		lineNum := 0
		for scanner.Scan() {
			lineNum++
			line := scanner.Text()
			if re.MatchString(line) {
				locs := re.FindAllStringIndex(line, -1)
				ml := matchLine{
					LineNum: lineNum,
					Line:    line,
					Matches: make([]int, 0),
				}
				for _, loc := range locs {
					ml.Matches = append(ml.Matches, loc[0])
				}
				fm.Lines = append(fm.Lines, ml)
				fm.Count++
			}
		}
		f.Close()

		if fm.Count > 0 {
			matches = append(matches, fm)
		}
	}

	if len(matches) == 0 {
		return "No matches found.", nil
	}

	// Format output
	var buf strings.Builder

	switch outputMode {
	case "files_with_matches":
		for _, m := range matches {
			rel, _ := filepath.Rel(".", m.Path)
			fmt.Fprintln(&buf, rel)
		}
		fmt.Fprintf(&buf, "\n%d file(s) matched", len(matches))

	case "count":
		for _, m := range matches {
			rel, _ := filepath.Rel(".", m.Path)
			fmt.Fprintf(&buf, "%s:%d\n", rel, m.Count)
		}
		fmt.Fprintf(&buf, "\n%d file(s) matched", len(matches))

	case "content":
		fallthrough
	default:
		for _, m := range matches {
			rel, _ := filepath.Rel(".", m.Path)
			if len(matches) > 1 {
				fmt.Fprintf(&buf, "%s:\n", rel)
			}
			for _, ml := range m.Lines {
				prefix := ""
				if params.ShowLineNum || outputMode == "content" {
					prefix = fmt.Sprintf("%d:", ml.LineNum)
				}
				fmt.Fprintf(&buf, "  %s%s\n", prefix, ml.Line)
			}
		}
		fmt.Fprintf(&buf, "\n%d file(s) matched", len(matches))
	}

	return buf.String(), nil
}
