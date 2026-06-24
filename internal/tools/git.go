package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"time"
)

// GitTool executes git commands.
type GitTool struct{}

func NewGitTool() *GitTool {
	return &GitTool{}
}

func (t *GitTool) Name() string { return "git" }

func (t *GitTool) Description() string {
	return "Executes a 'git' command in a subprocess with combined stdout/stderr. Used for checking source control status."
}

func (t *GitTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"args": map[string]any{
				"type":        "array",
				"items":       map[string]any{"type": "string"},
				"description": "The specific git command arguments to execute (e.g., ['status'], ['diff', 'main'], ['log']).",
			},
			"timeout": map[string]any{
				"type":        "integer",
				"description": "Timeout in milliseconds (default 120000, max 600000).",
			},
		},
		"required": []string{"args"},
	}
}

func (t *GitTool) NeedsPermission() bool { return true }

type gitInput struct {
	Args    []string `json:"args"`
	Timeout *int     `json:"timeout,omitempty"`
}

func (t *GitTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params gitInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if len(params.Args) == 0 {
		return "", fmt.Errorf("args are required")
	}

	// Default timeout: 120 seconds, max: 600 seconds
	timeoutMs := 120000
	if params.Timeout != nil {
		if *params.Timeout > 600000 {
			timeoutMs = 600000
		} else if *params.Timeout > 0 {
			timeoutMs = *params.Timeout
		}
	}

	// Create context with timeout
	execCtx, cancel := context.WithTimeout(ctx, time.Duration(timeoutMs)*time.Millisecond)
	defer cancel()

	// Execute command using git directly to avoid command injection vulnerabilities
	cmd := exec.CommandContext(execCtx, "git", params.Args...)
	cmd.Dir = "" // use current working directory

	output, err := cmd.CombinedOutput()

	exitCode := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		} else {
			return "", fmt.Errorf("executing git command: %w", err)
		}
	}

	// Check if output is too large (truncate to 50KB)
	const maxOutput = 50 * 1024
	result := string(output)
	if len(result) > maxOutput {
		result = result[:maxOutput] + "\n... [output truncated]"
	}

	return fmt.Sprintf("Exit code: %d\n\n%s", exitCode, result), nil
}
