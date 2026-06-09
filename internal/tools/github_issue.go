package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// GitHubIssueTool manages GitHub issues via the gh CLI.
type GitHubIssueTool struct{}

func NewGitHubIssueTool() *GitHubIssueTool {
	return &GitHubIssueTool{}
}

func (t *GitHubIssueTool) Name() string { return "github_issue" }

func (t *GitHubIssueTool) Description() string {
	return "Manage GitHub issues via the gh CLI. Supports creating, reading, updating, closing, and listing issues. Requires gh CLI to be installed and authenticated."
}

func (t *GitHubIssueTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"action": map[string]any{
				"type":        "string",
				"description": "The issue action: 'create', 'list', 'view', 'update', 'close', 'comment'.",
			},
			"title": map[string]any{
				"type":        "string",
				"description": "Issue title (for create).",
			},
			"body": map[string]any{
				"type":        "string",
				"description": "Issue body/description (for create/update).",
			},
			"number": map[string]any{
				"type":        "string",
				"description": "Issue number (for view/update/close/comment).",
			},
			"labels": map[string]any{
				"type":        "string",
				"description": "Comma-separated labels to add.",
			},
			"assignees": map[string]any{
				"type":        "string",
				"description": "Comma-separated assignee usernames.",
			},
			"state": map[string]any{
				"type":        "string",
				"description": "Filter by state for list: 'open', 'closed', 'all'. Default: 'open'.",
			},
			"limit": map[string]any{
				"type":        "integer",
				"description": "Max issues to return for list (default 10).",
			},
			"repo": map[string]any{
				"type":        "string",
				"description": "Target repo in owner/repo format. Default: auto-detected.",
			},
		},
		"required": []string{"action"},
	}
}

func (t *GitHubIssueTool) NeedsPermission() bool { return true }

type githubIssueInput struct {
	Action    string `json:"action"`
	Title     string `json:"title,omitempty"`
	Body      string `json:"body,omitempty"`
	Number    string `json:"number,omitempty"`
	Labels    string `json:"labels,omitempty"`
	Assignees string `json:"assignees,omitempty"`
	State     string `json:"state,omitempty"`
	Limit     *int   `json:"limit,omitempty"`
	Repo      string `json:"repo,omitempty"`
}

const maxIssueOutput = 50 * 1024

func truncateOutput(s string) string {
	if len(s) > maxIssueOutput {
		return s[:maxIssueOutput] + "\n... [output truncated]"
	}
	return s
}

func (t *GitHubIssueTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params githubIssueInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.Action == "" {
		return "", fmt.Errorf("action is required")
	}

	switch params.Action {
	case "create":
		return t.createIssue(ctx, &params)
	case "list":
		return t.listIssues(ctx, &params)
	case "view":
		return t.viewIssue(ctx, &params)
	case "update":
		return t.updateIssue(ctx, &params)
	case "close":
		return t.closeIssue(ctx, &params)
	case "comment":
		return t.commentIssue(ctx, &params)
	default:
		return "", fmt.Errorf("unknown action: %q (valid actions: create, list, view, update, close, comment)", params.Action)
	}
}

func (t *GitHubIssueTool) createIssue(ctx context.Context, p *githubIssueInput) (string, error) {
	if p.Title == "" {
		return "", fmt.Errorf("title is required for create action")
	}

	args := []string{"issue", "create", "--title", p.Title}
	if p.Body != "" {
		args = append(args, "--body", p.Body)
	}
	for _, label := range splitCSV(p.Labels) {
		args = append(args, "--label", label)
	}
	for _, assignee := range splitCSV(p.Assignees) {
		args = append(args, "--assignee", assignee)
	}
	if p.Repo != "" {
		args = append(args, "--repo", p.Repo)
	}

	output, err := runGh(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("creating issue: %w", err)
	}

	return truncateOutput(strings.TrimSpace(output)), nil
}

func (t *GitHubIssueTool) listIssues(ctx context.Context, p *githubIssueInput) (string, error) {
	state := "open"
	if p.State != "" {
		state = p.State
	}
	limit := 10
	if p.Limit != nil && *p.Limit > 0 {
		limit = *p.Limit
	}

	args := []string{
		"issue", "list",
		"--state", state,
		"--json", "number,title,state,labels,assignees,createdAt",
		"--limit", fmt.Sprintf("%d", limit),
	}
	if p.Repo != "" {
		args = append(args, "--repo", p.Repo)
	}

	output, err := runGh(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("listing issues: %w", err)
	}

	return truncateOutput(output), nil
}

func (t *GitHubIssueTool) viewIssue(ctx context.Context, p *githubIssueInput) (string, error) {
	if p.Number == "" {
		return "", fmt.Errorf("number is required for view action")
	}

	args := []string{
		"issue", "view", p.Number,
		"--json", "number,title,body,state,labels,assignees,comments,createdAt",
	}
	if p.Repo != "" {
		args = append(args, "--repo", p.Repo)
	}

	output, err := runGh(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("viewing issue #%s: %w", p.Number, err)
	}

	return truncateOutput(output), nil
}

func (t *GitHubIssueTool) updateIssue(ctx context.Context, p *githubIssueInput) (string, error) {
	if p.Number == "" {
		return "", fmt.Errorf("number is required for update action")
	}

	args := []string{"issue", "edit", p.Number}
	if p.Title != "" {
		args = append(args, "--title", p.Title)
	}
	if p.Body != "" {
		args = append(args, "--body", p.Body)
	}
	for _, label := range splitCSV(p.Labels) {
		args = append(args, "--add-label", label)
	}
	if p.Repo != "" {
		args = append(args, "--repo", p.Repo)
	}

	output, err := runGh(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("updating issue #%s: %w", p.Number, err)
	}

	result := strings.TrimSpace(output)
	if result == "" {
		result = fmt.Sprintf("Issue #%s updated.", p.Number)
	}

	return truncateOutput(result), nil
}

func (t *GitHubIssueTool) closeIssue(ctx context.Context, p *githubIssueInput) (string, error) {
	if p.Number == "" {
		return "", fmt.Errorf("number is required for close action")
	}

	args := []string{"issue", "close", p.Number}
	if p.Repo != "" {
		args = append(args, "--repo", p.Repo)
	}

	_, err := runGh(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("closing issue #%s: %w", p.Number, err)
	}

	return fmt.Sprintf("Issue #%s closed.", p.Number), nil
}

func (t *GitHubIssueTool) commentIssue(ctx context.Context, p *githubIssueInput) (string, error) {
	if p.Number == "" {
		return "", fmt.Errorf("number is required for comment action")
	}
	if p.Body == "" {
		return "", fmt.Errorf("body is required for comment action")
	}

	args := []string{"issue", "comment", p.Number, "--body", p.Body}
	if p.Repo != "" {
		args = append(args, "--repo", p.Repo)
	}

	_, err := runGh(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("commenting on issue #%s: %w", p.Number, err)
	}

	return fmt.Sprintf("Comment added to issue #%s.", p.Number), nil
}

// runGh executes a gh CLI command with the given arguments and returns its combined output.
func runGh(ctx context.Context, args ...string) (string, error) {
	cmd := exec.CommandContext(ctx, "gh", args...)
	cmd.Dir = "" // use current working directory
	output, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("gh %s: %s", strings.Join(args, " "), strings.TrimSpace(string(output)))
	}
	return string(output), nil
}

// splitCSV splits a comma-separated string into trimmed, non-empty parts.
func splitCSV(s string) []string {
	if s == "" {
		return nil
	}
	parts := strings.Split(s, ",")
	result := make([]string, 0, len(parts))
	for _, p := range parts {
		p = strings.TrimSpace(p)
		if p != "" {
			result = append(result, p)
		}
	}
	return result
}
