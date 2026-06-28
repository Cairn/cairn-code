package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"time"
)

type GoTool struct{}

func NewGoTool() *GoTool {
	return &GoTool{}
}

func (t *GoTool) Name() string { return "go" }

func (t *GoTool) Description() string {
	return "Executes a 'go' command in a subprocess with combined stdout/stderr. Restricts usage to common go operations."
}

func (t *GoTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"args": map[string]any{
				"type":        "array",
				"items":       map[string]any{"type": "string"},
				"description": "The specific go command arguments to execute (e.g., ['build', './...'], ['test', './...']).",
			},
			"timeout": map[string]any{
				"type":        "integer",
				"description": "Timeout in milliseconds (default 120000, max 600000).",
			},
		},
		"required": []string{"args"},
	}
}

func (t *GoTool) NeedsPermission() bool { return true }

type goInput struct {
	Args    []string `json:"args"`
	Timeout *int     `json:"timeout,omitempty"`
}

func (t *GoTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params goInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if len(params.Args) == 0 {
		return "", fmt.Errorf("args are required")
	}

	timeoutMs := 120000
	if params.Timeout != nil {
		if *params.Timeout > 600000 {
			timeoutMs = 600000
		} else if *params.Timeout > 0 {
			timeoutMs = *params.Timeout
		}
	}

	execCtx, cancel := context.WithTimeout(ctx, time.Duration(timeoutMs)*time.Millisecond)
	defer cancel()

	cmd := exec.CommandContext(execCtx, "go", params.Args...)
	cmd.Dir = ""

	output, err := cmd.CombinedOutput()

	exitCode := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		} else {
			return "", fmt.Errorf("executing go command: %w", err)
		}
	}

	const maxOutput = 50 * 1024
	result := string(output)
	if len(result) > maxOutput {
		result = result[:maxOutput] + "\n... [output truncated]"
	}

	return fmt.Sprintf("Exit code: %d\n\n%s", exitCode, result), nil
}
