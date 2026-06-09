package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// GitTool executes safe git operations, blocking dangerous commands.
type GitTool struct{}

func NewGitTool() *GitTool {
	return &GitTool{}
}

func (t *GitTool) Name() string { return "git" }

func (t *GitTool) Description() string {
	return "Executes safe git operations. Use this instead of bash for git commands to enforce safety rules: no force pushes, no config changes, no --no-verify, no hard resets, no branch deletion. Supports: status, diff, log, branch (create/switch/list), add, commit, push (safe), pull, fetch, stash, tag, show, rev-parse, remote -v."
}

func (t *GitTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"command": map[string]any{
				"type":        "string",
				"description": "The git subcommand to run (e.g. 'status', 'diff', 'log', 'commit').",
			},
			"args": map[string]any{
				"type":        "string",
				"description": "Arguments to pass to the git command.",
			},
			"path": map[string]any{
				"type":        "string",
				"description": "Repository path. Use this instead of cd.",
			},
			"description": map[string]any{
				"type":        "string",
				"description": "What this git operation does.",
			},
		},
		"required": []string{"command"},
	}
}

func (t *GitTool) NeedsPermission() bool { return true }

type gitInput struct {
	Command     string `json:"command"`
	Args        string `json:"args,omitempty"`
	Path        string `json:"path,omitempty"`
	Description string `json:"description,omitempty"`
}

func (t *GitTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params gitInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.Command == "" {
		return "", fmt.Errorf("command is required")
	}

	args := strings.Fields(params.Args)

	// Safety check: block dangerous operations
	if blocked, reason := isBlocked(params.Command, args); blocked {
		return "", fmt.Errorf("blocked: %s is not allowed. Use safe git operations only.", reason)
	}

	// Build the git command
	gitArgs := []string{params.Command}
	gitArgs = append(gitArgs, args...)

	var cmd *exec.Cmd
	if params.Path != "" {
		cmd = exec.CommandContext(ctx, "git", "-C", params.Path)
		cmd.Args = append(cmd.Args, gitArgs...)
	} else {
		cmd = exec.CommandContext(ctx, "git")
		cmd.Args = append(cmd.Args, gitArgs...)
	}

	output, err := cmd.CombinedOutput()

	exitCode := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		} else {
			return "", fmt.Errorf("executing git command: %w", err)
		}
	}

	// Truncate output if > 50KB
	const maxOutput = 50 * 1024
	result := string(output)
	if len(result) > maxOutput {
		result = result[:maxOutput] + "\n... [output truncated]"
	}

	return fmt.Sprintf("Exit code: %d\n\n%s", exitCode, result), nil
}

// isBlocked checks whether a git command/argument combination is on the blocklist.
func isBlocked(command string, args []string) (bool, string) {
	argsStr := strings.Join(args, " ")

	// --- force push ---
	if command == "push" {
		for _, arg := range args {
			if arg == "--force" || arg == "-f" || arg == "--force-with-lease" {
				return true, fmt.Sprintf("git push %s", arg)
			}
		}
	}

	// --- no-verify (anywhere) ---
	for _, arg := range args {
		if arg == "--no-verify" {
			return true, "git --no-verify"
		}
	}

	// --- config ---
	if command == "config" {
		return true, "git config"
	}

	// --- clean -f / --force ---
	if command == "clean" {
		for _, arg := range args {
			if arg == "-f" || arg == "--force" || strings.HasPrefix(arg, "-f") || strings.HasPrefix(arg, "--force") {
				return true, "git clean --force"
			}
		}
	}

	// --- reset --hard ---
	if command == "reset" {
		for _, arg := range args {
			if arg == "--hard" {
				return true, "git reset --hard"
			}
		}
	}

	// --- branch -D / --delete (uppercase D = force delete) ---
	if command == "branch" {
		for _, arg := range args {
			if arg == "-D" || strings.HasPrefix(arg, "-D") {
				return true, "git branch -D (force delete)"
			}
		}
		// also block --delete flag to prevent accidental deletions
		if strings.Contains(argsStr, "--delete") {
			return true, "git branch --delete"
		}
	}

	// --- rebase ---
	if command == "rebase" {
		return true, "git rebase"
	}

	// --- filter-branch ---
	if command == "filter-branch" {
		return true, "git filter-branch"
	}

	// --- reflog expire/delete ---
	if command == "reflog" {
		for _, arg := range args {
			if arg == "expire" || arg == "delete" {
				return true, fmt.Sprintf("git reflog %s", arg)
			}
		}
	}

	// --- checkout -f / --force ---
	if command == "checkout" {
		for _, arg := range args {
			if arg == "-f" || arg == "--force" || strings.HasPrefix(arg, "-f") || strings.HasPrefix(arg, "--force") {
				return true, "git checkout --force"
			}
		}
	}

	return false, ""
}
