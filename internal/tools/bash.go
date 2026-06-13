package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"time"
)

// BashTool executes shell commands.
type BashTool struct{}

func NewBashTool() *BashTool {
	return &BashTool{}
}

func (t *BashTool) Name() string { return "bash" }

func (t *BashTool) Description() string {
	return "Executes a bash command in a subprocess with combined stdout/stderr. Respects timeout and captures exit code."
}

func (t *BashTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"command": map[string]any{
				"type":        "string",
				"description": "The shell command to execute.",
			},
			"timeout": map[string]any{
				"type":        "integer",
				"description": "Timeout in milliseconds (default 120000, max 600000).",
			},
			"description": map[string]any{
				"type":        "string",
				"description": "A short description of what this command does.",
			},
		},
		"required": []string{"command"},
	}
}

func (t *BashTool) NeedsPermission() bool { return true }

type bashInput struct {
	Command     string `json:"command"`
	Timeout     *int   `json:"timeout,omitempty"`
	Description string `json:"description,omitempty"`
}

func (t *BashTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params bashInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.Command == "" {
		return "", fmt.Errorf("command is required")
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

	// Execute command using bash
	cmd := exec.CommandContext(execCtx, "bash", "-c", params.Command)
	cmd.Dir = "" // use current working directory

	output, err := cmd.CombinedOutput()

	exitCode := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		} else {
			return "", fmt.Errorf("executing command: %w", err)
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
