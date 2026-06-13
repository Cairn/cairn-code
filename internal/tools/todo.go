package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// TodoStore holds the shared todo list state between the agent and tools.
type TodoStore struct {
	Items []TodoItem
}

// TodoItem represents a single todo item.
type TodoItem struct {
	Content string `json:"content"`
	Status  string `json:"status"` // "pending", "in_progress", "completed"
}

// TodoWriteTool manages the agent's in-memory todo list.
type TodoWriteTool struct {
	store *TodoStore
}

// NewTodoWriteTool creates a new TodoWrite tool.
func NewTodoWriteTool(store *TodoStore) *TodoWriteTool {
	return &TodoWriteTool{store: store}
}

func (t *TodoWriteTool) Name() string { return "todo_write" }

func (t *TodoWriteTool) Description() string {
	return "Manages the agent's in-memory todo list. Use this to track progress on complex multi-step tasks."
}

func (t *TodoWriteTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"todos": map[string]any{
				"type":        "array",
				"description": "The updated todo list.",
				"items": map[string]any{
					"type": "object",
					"properties": map[string]any{
						"content": map[string]any{
							"type":        "string",
							"description": "Description of the task.",
						},
						"status": map[string]any{
							"type":        "string",
							"enum":        []string{"pending", "in_progress", "completed"},
							"description": "Status of the task.",
						},
					},
					"required": []string{"content", "status"},
				},
			},
		},
		"required":             []string{"todos"},
	}
}

func (t *TodoWriteTool) NeedsPermission() bool { return false }

type todoWriteInput struct {
	Todos []TodoItem `json:"todos"`
}

func (t *TodoWriteTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params todoWriteInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	// Update the shared store
	t.store.Items = params.Todos

	// Return formatted todo list
	return formatTodos(t.store.Items), nil
}

// formatTodos formats a todo list as a readable string.
func formatTodos(items []TodoItem) string {
	if len(items) == 0 {
		return "Todo list is empty."
	}

	var buf strings.Builder
	buf.WriteString("Todo list updated:\n")
	for i, item := range items {
		marker := "[ ]"
		switch item.Status {
		case "in_progress":
			marker = "[●]"
		case "completed":
			marker = "[✓]"
		}
		fmt.Fprintf(&buf, "  %d. %s %s\n", i+1, marker, item.Content)
	}
	return buf.String()
}
