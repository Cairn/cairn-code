package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// MemoryTool provides cross-session memory for storing and recalling project context,
// user preferences, learned patterns, and important decisions.
type MemoryTool struct {
	memoryDir string
}

func NewMemoryTool() *MemoryTool {
	dir := filepath.Join(os.Getenv("HOME"), ".config", "cairn-code", "memory")
	os.MkdirAll(dir, 0755)
	return &MemoryTool{memoryDir: dir}
}

func (t *MemoryTool) Name() string { return "memory" }

func (t *MemoryTool) Description() string {
	return "Save and recall information across sessions. Stores project context, user preferences, learned patterns, and important decisions. Each memory has a key and content. Use 'save' to store, 'recall' to retrieve, 'list' to show all, 'delete' to remove, 'search' to find by content."
}

func (t *MemoryTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"action": map[string]any{
				"type":        "string",
				"description": "The memory action: 'save', 'recall', 'list', 'delete', or 'search'.",
			},
			"key": map[string]any{
				"type":        "string",
				"description": "Unique identifier for the memory (e.g. 'project-style', 'user-pref-indent').",
			},
			"content": map[string]any{
				"type":        "string",
				"description": "The content to save. Used with 'save' action.",
			},
			"query": map[string]any{
				"type":        "string",
				"description": "Search query. Used with 'search' action to find memories containing this text.",
			},
		},
		"required": []string{"action"},
	}
}

func (t *MemoryTool) NeedsPermission() bool { return false }

type memoryEntry struct {
	Key       string `json:"key"`
	Content   string `json:"content"`
	CreatedAt string `json:"created_at"`
	UpdatedAt string `json:"updated_at"`
}

type memoryInput struct {
	Action  string `json:"action"`
	Key     string `json:"key,omitempty"`
	Content string `json:"content,omitempty"`
	Query   string `json:"query,omitempty"`
}

func (t *MemoryTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params memoryInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	switch params.Action {
	case "save":
		return t.save(params)
	case "recall":
		return t.recall(params)
	case "list":
		return t.list()
	case "delete":
		return t.delete(params)
	case "search":
		return t.search(params)
	default:
		return "", fmt.Errorf("unknown action: %s (must be 'save', 'recall', 'list', 'delete', or 'search')", params.Action)
	}
}

func (t *MemoryTool) save(params memoryInput) (string, error) {
	if params.Key == "" {
		return "", fmt.Errorf("key is required for save action")
	}
	if params.Content == "" {
		return "", fmt.Errorf("content is required for save action")
	}

	now := time.Now().UTC().Format(time.RFC3339)
	filePath := filepath.Join(t.memoryDir, params.Key+".json")

	entry := memoryEntry{
		Key:       params.Key,
		Content:   params.Content,
		CreatedAt: now,
		UpdatedAt: now,
	}

	// If file already exists, preserve CreatedAt
	if data, err := os.ReadFile(filePath); err == nil {
		var existing memoryEntry
		if json.Unmarshal(data, &existing) == nil {
			entry.CreatedAt = existing.CreatedAt
		}
	}

	data, err := json.MarshalIndent(entry, "", "  ")
	if err != nil {
		return "", fmt.Errorf("marshaling memory: %w", err)
	}

	if err := os.WriteFile(filePath, data, 0644); err != nil {
		return "", fmt.Errorf("writing memory file: %w", err)
	}

	return fmt.Sprintf("Memory saved: %s", params.Key), nil
}

func (t *MemoryTool) recall(params memoryInput) (string, error) {
	if params.Key == "" {
		return "", fmt.Errorf("key is required for recall action")
	}

	filePath := filepath.Join(t.memoryDir, params.Key+".json")
	data, err := os.ReadFile(filePath)
	if err != nil {
		if os.IsNotExist(err) {
			return "", fmt.Errorf("Memory not found: %s", params.Key)
		}
		return "", fmt.Errorf("reading memory file: %w", err)
	}

	var entry memoryEntry
	if err := json.Unmarshal(data, &entry); err != nil {
		return "", fmt.Errorf("parsing memory file: %w", err)
	}

	return entry.Content, nil
}

func (t *MemoryTool) list() (string, error) {
	entries, err := os.ReadDir(t.memoryDir)
	if err != nil {
		return "", fmt.Errorf("reading memory directory: %w", err)
	}

	var lines []string
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}

		data, err := os.ReadFile(filepath.Join(t.memoryDir, e.Name()))
		if err != nil {
			continue
		}

		var entry memoryEntry
		if err := json.Unmarshal(data, &entry); err != nil {
			continue
		}

		preview := entry.Content
		if len(preview) > 80 {
			preview = preview[:80] + "..."
		}

		lines = append(lines, fmt.Sprintf("- %s (updated %s): %s", entry.Key, entry.UpdatedAt, preview))
	}

	if len(lines) == 0 {
		return "No memories stored.", nil
	}

	return strings.Join(lines, "\n"), nil
}

func (t *MemoryTool) delete(params memoryInput) (string, error) {
	if params.Key == "" {
		return "", fmt.Errorf("key is required for delete action")
	}

	filePath := filepath.Join(t.memoryDir, params.Key+".json")
	if err := os.Remove(filePath); err != nil {
		if os.IsNotExist(err) {
			return "", fmt.Errorf("Memory not found: %s", params.Key)
		}
		return "", fmt.Errorf("deleting memory file: %w", err)
	}

	return fmt.Sprintf("Memory deleted: %s", params.Key), nil
}

func (t *MemoryTool) search(params memoryInput) (string, error) {
	if params.Query == "" {
		return "", fmt.Errorf("query is required for search action")
	}

	query := strings.ToLower(params.Query)
	entries, err := os.ReadDir(t.memoryDir)
	if err != nil {
		return "", fmt.Errorf("reading memory directory: %w", err)
	}

	var lines []string
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}

		data, err := os.ReadFile(filepath.Join(t.memoryDir, e.Name()))
		if err != nil {
			continue
		}

		var entry memoryEntry
		if err := json.Unmarshal(data, &entry); err != nil {
			continue
		}

		if !strings.Contains(strings.ToLower(entry.Content), query) && !strings.Contains(strings.ToLower(entry.Key), query) {
			continue
		}

		preview := entry.Content
		if len(preview) > 80 {
			preview = preview[:80] + "..."
		}

		lines = append(lines, fmt.Sprintf("- %s (updated %s): %s", entry.Key, entry.UpdatedAt, preview))
	}

	if len(lines) == 0 {
		return fmt.Sprintf("No memories found matching '%s'", params.Query), nil
	}

	return strings.Join(lines, "\n"), nil
}
