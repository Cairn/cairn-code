package tools

import (
	"context"
	"encoding/json"
	"testing"
)

// TestTodoWriteInvalidInput verifies error for invalid JSON.
func TestTodoWriteInvalidInput(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)
	_, err := tool.Execute(context.Background(), json.RawMessage(`{invalid}`))
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

// TestTodoWriteEmpty verifies empty todo list.
func TestTodoWriteEmpty(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"todos":[]}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "Todo list is empty." {
		t.Errorf("expected 'Todo list is empty.', got: %q", result)
	}
	if len(store.Items) != 0 {
		t.Errorf("store should have 0 items, got %d", len(store.Items))
	}
}

// TestTodoWritePending verifies pending status marker.
func TestTodoWritePending(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"todos":[{"content":"do thing","status":"pending"}]}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !contains(result, "[ ]") {
		t.Errorf("expected pending marker '[ ]', got: %q", result)
	}
	if len(store.Items) != 1 || store.Items[0].Content != "do thing" {
		t.Errorf("store not updated correctly")
	}
}

// TestTodoWriteInProgress verifies in_progress status marker.
func TestTodoWriteInProgress(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"todos":[{"content":"active task","status":"in_progress"}]}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !contains(result, "[●]") {
		t.Errorf("expected in_progress marker '[●]', got: %q", result)
	}
}

// TestTodoWriteCompleted verifies completed status marker.
func TestTodoWriteCompleted(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)

	result, err := tool.Execute(context.Background(), json.RawMessage(`{"todos":[{"content":"done task","status":"completed"}]}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !contains(result, "[✓]") {
		t.Errorf("expected completed marker '[✓]', got: %q", result)
	}
}

// TestTodoWriteMixed verifies mixed statuses.
func TestTodoWriteMixed(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)

	_, err := tool.Execute(context.Background(), json.RawMessage(`{"todos":[{"content":"first","status":"completed"},{"content":"second","status":"in_progress"},{"content":"third","status":"pending"}]}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(store.Items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(store.Items))
	}
	if store.Items[0].Status != "completed" || store.Items[1].Status != "in_progress" || store.Items[2].Status != "pending" {
		t.Error("statuses not preserved in order")
	}
}

// TestTodoWriteStoreMutation verifies the store is actually mutated.
func TestTodoWriteStoreMutation(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)

	tool.Execute(context.Background(), json.RawMessage(`{"todos":[{"content":"task1","status":"pending"},{"content":"task2","status":"completed"}]}`))

	if len(store.Items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(store.Items))
	}
	if store.Items[0].Content != "task1" {
		t.Errorf("first item = %q, want 'task1'", store.Items[0].Content)
	}
}

// TestFormatTodos verifies the formatTodos helper directly.
func TestFormatTodos(t *testing.T) {
	items := []TodoItem{
		{Content: "first", Status: "pending"},
		{Content: "second", Status: "completed"},
		{Content: "third", Status: "in_progress"},
	}

	result := formatTodos(items)

	if !contains(result, "1. [ ] first") {
		t.Errorf("missing pending item: %q", result)
	}
	if !contains(result, "2. [✓] second") {
		t.Errorf("missing completed item: %q", result)
	}
	if !contains(result, "3. [●] third") {
		t.Errorf("missing in_progress item: %q", result)
	}
}

// TestTodoNeedsPermission verifies TodoWriteTool doesn't need permission.
func TestTodoNeedsPermission(t *testing.T) {
	store := &TodoStore{}
	tool := NewTodoWriteTool(store)
	if tool.NeedsPermission() {
		t.Error("TodoWriteTool should not need permission")
	}
}
