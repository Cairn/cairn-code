package tools

import (
        "bytes"
        "context"
        "encoding/json"
        "fmt"
        "os"
        "os/exec"
        "strings"
        "text/template"
)

// PullRequestTool creates GitHub pull requests via the gh CLI.
type PullRequestTool struct{}

func NewPullRequestTool() *PullRequestTool {
        return &PullRequestTool{}
}

func (t *PullRequestTool) Name() string { return "create_pull_request" }

func (t *PullRequestTool) Description() string {
        return "Creates a GitHub pull request. Gathers git context (diff, status, branch) automatically, " +
                "generates a structured PR with summary, test plan, and attribution. Requires gh CLI and an " +
                "authenticated GitHub session. Will create a new branch if currently on the default branch. " +
                "Never force pushes. Respects conventional commit style."
}

func (t *PullRequestTool) InputSchema() map[string]any {
        return map[string]any{
                "type": "object",
                "properties": map[string]any{
                        "title": map[string]any{
                                "type":        "string",
                                "description": "Short, descriptive PR title (under 72 characters). If empty, auto-generates from the commit/diff.",
                        },
                        "summary": map[string]any{
                                "type":        "string",
                                "description": "1-3 bullet points summarizing the changes. If empty, auto-generates from the diff.",
                        },
                        "test_plan": map[string]any{
                                "type":        "string",
                                "description": "Bulleted checklist of how to test the changes. Include build steps, verification commands, and manual checks. If empty, auto-generates reasonable defaults.",
                        },
                        "branch": map[string]any{
                                "type":        "string",
                                "description": "Target branch name for the PR (e.g. 'fix/my-bug'). If empty and on default branch, auto-creates a branch from the title.",
                        },
                        "base": map[string]any{
                                "type":        "string",
                                "description": "The base branch to target (default: auto-detected from the repo's default branch).",
                        },
                        "reviewers": map[string]any{
                                "type":        "string",
                                "description": "Comma-separated list of GitHub reviewer usernames to request reviews from.",
                        },
                        "labels": map[string]any{
                                "type":        "string",
                                "description": "Comma-separated list of labels to apply to the PR.",
                        },
                        "commit_message": map[string]any{
                                "type":        "string",
                                "description": "Commit message for the changes. If empty, uses the PR title.",
                        },
                        "co_author": map[string]any{
                                "type":        "string",
                                "description": "Co-authored-by trailer for the commit (e.g. 'Cairn Code <cairn@example.com>').",
                        },
                },
        }
}

func (t *PullRequestTool) NeedsPermission() bool { return true }

type pullRequestInput struct {
        Title        string `json:"title,omitempty"`
        Summary      string `json:"summary,omitempty"`
        TestPlan     string `json:"test_plan,omitempty"`
        Branch       string `json:"branch,omitempty"`
        Base         string `json:"base,omitempty"`
        Reviewers    string `json:"reviewers,omitempty"`
        Labels       string `json:"labels,omitempty"`
        CommitMessage string `json:"commit_message,omitempty"`
        CoAuthor     string `json:"co_author,omitempty"`
}

func (t *PullRequestTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
        var params pullRequestInput
        if err := json.Unmarshal(input, &params); err != nil {
                return "", fmt.Errorf("invalid input: %w", err)
        }

        // Step 1: Gather git context
        runGit := func(args ...string) string {
                cmd := exec.CommandContext(ctx, "git", args...)
                cmd.Dir = "" // use CWD
                out, _ := cmd.CombinedOutput()
                return strings.TrimSpace(string(out))
        }

        currentBranch := runGit("branch", "--show-current")
        if currentBranch == "" {
                return "", fmt.Errorf("not inside a git repository")
        }

        // Detect default branch
        defaultBranch := ""
        // Try git symbolic-ref first (fastest, works for local refs)
        if out, err := exec.CommandContext(ctx, "git", "symbolic-ref", "refs/remotes/origin/HEAD", "--short").CombinedOutput(); err == nil {
                defaultBranch = strings.TrimPrefix(strings.TrimSpace(string(out)), "origin/")
        }
        // Fallback: try common defaults
        if defaultBranch == "" {
                for _, candidate := range []string{"main", "master"} {
                        if out, err := exec.CommandContext(ctx, "git", "rev-parse", "--verify", candidate).CombinedOutput(); err == nil && strings.TrimSpace(string(out)) != "" {
                                defaultBranch = candidate
                                break
                        }
                }
        }

        if params.Base == "" {
                params.Base = defaultBranch
        }

        // Check if PR already exists for this branch
        var prCheck string
        if out, err := exec.CommandContext(ctx, "gh", "pr", "view", "--json", "number,title,url").CombinedOutput(); err == nil {
                prCheck = strings.TrimSpace(string(out))
        }

        // Step 2: Auto-create branch if on default branch
        onDefaultBranch := currentBranch == defaultBranch
        if onDefaultBranch && params.Branch == "" {
                // Generate branch name from title if possible
                if params.Title != "" {
                        params.Branch = branchNameFromTitle(params.Title)
                } else {
                        params.Branch = "fix/cairn-code-changes"
                }
        }

        if onDefaultBranch && params.Branch != "" {
                // Create and checkout new branch
                if out, err := exec.CommandContext(ctx, "git", "checkout", "-b", params.Branch).CombinedOutput(); err != nil {
                        return "", fmt.Errorf("failed to create branch %s: %s", params.Branch, string(out))
                }
                currentBranch = params.Branch
        }

        // Step 3: Gather diff info for auto-generation
        stagedFiles := runGit("diff", "--cached", "--name-only")
        unstagedFiles := runGit("diff", "--name-only")
        untrackedFiles := runGit("ls-files", "--others", "--exclude-standard")

        // Stage all changes if nothing is staged
        if stagedFiles == "" && (unstagedFiles != "" || untrackedFiles != "") {
                exec.CommandContext(ctx, "git", "add", "-A").Run()
                stagedFiles = runGit("diff", "--cached", "--name-only")
        }

        if stagedFiles == "" {
                return "", fmt.Errorf("no changes to commit. Make some changes first.")
        }

        // Get the full diff for context
        diff := runGit("diff", "--cached", params.Base)

        // Step 4: Build commit message
        commitMsg := params.CommitMessage
        if commitMsg == "" && params.Title != "" {
                commitMsg = params.Title
        }
        if commitMsg == "" {
                commitMsg = "feat: apply changes"
        }

        // Add co-author trailer
        if params.CoAuthor != "" {
                commitMsg += fmt.Sprintf("\n\nCo-Authored-By: %s", params.CoAuthor)
        }

        // Step 5: Commit
        commitArgs := []string{"commit", "-m", commitMsg}
        if out, err := exec.CommandContext(ctx, "git", commitArgs...).CombinedOutput(); err != nil {
                return "", fmt.Errorf("failed to commit: %s", string(out))
        }

        // Step 6: Push
        pushArgs := []string{"push", "-u", "origin", currentBranch}
        if out, err := exec.CommandContext(ctx, "git", pushArgs...).CombinedOutput(); err != nil {
                return "", fmt.Errorf("failed to push: %s", string(out))
        }

        // Step 7: Build PR body
        prBody, err := buildPRBody(params, diff, stagedFiles)
        if err != nil {
                return "", fmt.Errorf("failed to build PR body: %w", err)
        }

        // Auto-detect title from commit if not provided
        prTitle := params.Title
        if prTitle == "" {
                // Use the first line of the commit message
                prTitle = runGit("log", "-1", "--pretty=%s")
                if prTitle == "" {
                        prTitle = "Update code"
                }
        }

        // Truncate title if too long
        if len(prTitle) > 72 {
                prTitle = prTitle[:69] + "..."
        }

        // Step 8: Create or update PR
        if prCheck != "" {
                // Update existing PR
                editArgs := []string{"pr", "edit", "--title", prTitle, "--body", prBody}
                if out, err := exec.CommandContext(ctx, "gh", editArgs...).CombinedOutput(); err != nil {
                        return "", fmt.Errorf("failed to update PR: %s", string(out))
                }
                return fmt.Sprintf("Updated existing PR successfully.\n\n%s", prCheck), nil
        }

        // Create new PR
        createArgs := []string{"pr", "create", "--title", prTitle, "--body", prBody}
        if params.Base != "" {
                createArgs = append(createArgs, "--base", params.Base)
        }
        if params.Reviewers != "" {
                for _, r := range strings.Split(params.Reviewers, ",") {
                        createArgs = append(createArgs, "--reviewer", strings.TrimSpace(r))
                }
        }
        if params.Labels != "" {
                for _, l := range strings.Split(params.Labels, ",") {
                        createArgs = append(createArgs, "--label", strings.TrimSpace(l))
                }
        }

        out, err := exec.CommandContext(ctx, "gh", createArgs...).CombinedOutput()
        result := string(out)
        if err != nil {
                // Check if it's just a URL output (gh sometimes returns non-zero but prints URL)
                if strings.Contains(result, "https://") {
                        return fmt.Sprintf("PR created: %s", strings.TrimSpace(result)), nil
                }
                return "", fmt.Errorf("failed to create PR: %s", result)
        }

        return fmt.Sprintf("PR created successfully.\n\n%s", strings.TrimSpace(result)), nil
}

// buildPRBody constructs the PR description markdown.
func buildPRBody(params pullRequestInput, diff, files string) (string, error) {
        // Auto-generate summary if not provided
        summary := params.Summary
        if summary == "" {
                summary = autoSummary(files)
        }

        // Auto-generate test plan if not provided
        testPlan := params.TestPlan
        if testPlan == "" {
                testPlan = autoTestPlan(files)
        }

        // Parse changed files list
        fileList := strings.Split(strings.TrimSpace(files), "\n")

        // Build the PR body using a template
        tmpl := `## Summary

{{ .Summary }}

## Changes

{{ range .Files }}- {{ . }}
{{ end }}
## Test plan

{{ .TestPlan }}

---
⚡ Generated with [Cairn Code](https://github.com/Cairn/cairn-code)`

        t, err := template.New("pr").Parse(tmpl)
        if err != nil {
                return "", err
        }

        var buf bytes.Buffer
        data := struct {
                Summary  string
                Files    []string
                TestPlan string
        }{
                Summary:  summary,
                Files:    fileList,
                TestPlan: testPlan,
        }

        if err := t.Execute(&buf, data); err != nil {
                return "", err
        }

        return buf.String(), nil
}

// autoSummary generates a summary from the list of changed files.
func autoSummary(files string) string {
        fileList := strings.Split(strings.TrimSpace(files), "\n")
        if len(fileList) == 0 {
                return "- Updated project files"
        }

        // Categorize files
        var goFiles, configFiles, docFiles, otherFiles []string
        for _, f := range fileList {
                switch {
                case strings.HasSuffix(f, ".go"):
                        goFiles = append(goFiles, f)
                case strings.HasSuffix(f, ".json"), strings.HasSuffix(f, ".yaml"), strings.HasSuffix(f, ".yml"), strings.HasSuffix(f, ".toml"), strings.HasSuffix(f, ".mod"), strings.HasSuffix(f, ".sum"):
                        configFiles = append(configFiles, f)
                case strings.HasSuffix(f, ".md"), strings.HasSuffix(f, ".txt"), strings.HasSuffix(f, ".rst"):
                        docFiles = append(docFiles, f)
                default:
                        otherFiles = append(otherFiles, f)
                }
        }

        var parts []string
        if len(goFiles) > 0 {
                parts = append(parts, fmt.Sprintf("- Updated %d Go source file(s): %s", len(goFiles), strings.Join(truncateList(goFiles, 5), ", ")))
        }
        if len(configFiles) > 0 {
                parts = append(parts, fmt.Sprintf("- Modified configuration: %s", strings.Join(truncateList(configFiles, 5), ", ")))
        }
        if len(docFiles) > 0 {
                parts = append(parts, fmt.Sprintf("- Updated documentation: %s", strings.Join(truncateList(docFiles, 3), ", ")))
        }
        if len(otherFiles) > 0 {
                parts = append(parts, fmt.Sprintf("- Changed %d other file(s): %s", len(otherFiles), strings.Join(truncateList(otherFiles, 5), ", ")))
        }
        if len(parts) == 0 {
                parts = append(parts, "- Updated project files")
        }

        return strings.Join(parts, "\n")
}

// autoTestPlan generates reasonable test steps from the changed files.
func autoTestPlan(files string) string {
        fileList := strings.Split(strings.TrimSpace(files), "\n")
        var plan []string

        // Check for Go files
        hasGo := false
        for _, f := range fileList {
                if strings.HasSuffix(f, ".go") {
                        hasGo = true
                        break
                }
        }

        if hasGo {
                plan = append(plan,
                        "- `go build ./...` compiles without errors",
                        "- `go vet ./...` passes static analysis",
                        "- `go test ./...` passes all tests",
                )
        }

        plan = append(plan,
                "- Verify the changes work as described in the summary",
                "- Check for any unintended side effects",
        )

        return strings.Join(plan, "\n")
}

// branchNameFromTitle converts a commit/PR title to a branch name.
func branchNameFromTitle(title string) string {
        branch := strings.ToLower(title)
        // Remove common prefixes
        for _, prefix := range []string{"fix:", "feat:", "chore:", "docs:", "refactor:", "test:", "build:", "ci:"} {
                branch = strings.TrimPrefix(branch, prefix)
        }
        // Clean up
        branch = strings.TrimSpace(branch)
        branch = strings.ReplaceAll(branch, " ", "-")
        branch = strings.ReplaceAll(branch, "/", "-")
        // Remove non-alphanumeric chars except hyphens
        var clean strings.Builder
        for _, c := range branch {
                if (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-' {
                        clean.WriteRune(c)
                }
        }
        branch = clean.String()
        // Trim and limit length
        branch = strings.Trim(branch, "-")
        if len(branch) > 60 {
                branch = branch[:57] + "..."
        }
        if branch == "" {
                branch = "fix/cairn-code-changes"
        }
        return branch
}

// truncateList shortens a file list if it's too long.
func truncateList(list []string, max int) []string {
        if len(list) <= max {
                return list
        }
        return append(list[:max], fmt.Sprintf("and %d more", len(list)-max))
}

func init() {
        // Ensure gh CLI is available
        if _, err := exec.LookPath("gh"); err != nil {
                fmt.Fprintln(os.Stderr, "Warning: 'gh' CLI not found. create_pull_request tool requires GitHub CLI (gh). Install it from https://cli.github.com/")
        }
}
