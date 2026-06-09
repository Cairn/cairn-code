package tools

import (
	"context"
	"encoding/json"
	"testing"
)

// mockToolForRegistry is a minimal tool implementation for registry tests.
type mockToolForRegistry struct {
	name        string
	description string
	execFn      func(ctx context.Context, input json.RawMessage) (string, error)
	needsPerm   bool
}

func (m *mockToolForRegistry) Name() string        { return m.name }
func (m *mockToolForRegistry) Description() string  { return m.description }
func (m *mockToolForRegistry) InputSchema() map[string]any {
	return map[string]any{"type": "object"}
}
func (m *mockToolForRegistry) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	if m.execFn != nil {
		return m.execFn(ctx, input)
	}
	return "ok", nil
}
func (m *mockToolForRegistry) NeedsPermission() bool { return m.needsPerm }

// TestNewRegistry verifies empty registry creation.
func TestNewRegistry(t *testing.T) {
	r := NewRegistry()
	if len(r.All()) != 0 {
		t.Error("new registry should be empty")
	}
	if len(r.Names()) != 0 {
		t.Error("new registry names should be empty")
	}
	if len(r.ToolDefinitions()) != 0 {
		t.Error("new registry tool definitions should be empty")
	}
}

// TestRegistryRegisterAndGet verifies registration and retrieval.
func TestRegistryRegisterAndGet(t *testing.T) {
	r := NewRegistry()
	tool := &mockToolForRegistry{name: "bash", description: "run commands"}
	r.Register(tool)

	got, ok := r.Get("bash")
	if !ok {
		t.Fatal("expected to find tool 'bash'")
	}
	if got.Name() != "bash" {
		t.Errorf("got name %q, want 'bash'", got.Name())
	}

	// Missing tool
	_, ok = r.Get("nonexistent")
	if ok {
		t.Error("expected false for missing tool")
	}
}

// TestRegistryOverwrite verifies registering same name twice overwrites.
func TestRegistryOverwrite(t *testing.T) {
	r := NewRegistry()
	r.Register(&mockToolForRegistry{name: "tool", description: "first"})
	r.Register(&mockToolForRegistry{name: "tool", description: "second"})

	tool, _ := r.Get("tool")
	if tool.Description() != "second" {
		t.Errorf("expected 'second', got %q", tool.Description())
	}
	if len(r.All()) != 1 {
		t.Errorf("expected 1 tool, got %d", len(r.All()))
	}
}

// TestRegistryAllSorted verifies All() returns tools sorted by name.
func TestRegistryAllSorted(t *testing.T) {
	r := NewRegistry()
	r.Register(&mockToolForRegistry{name: "charlie"})
	r.Register(&mockToolForRegistry{name: "alpha"})
	r.Register(&mockToolForRegistry{name: "bravo"})

	all := r.All()
	if len(all) != 3 {
		t.Fatalf("expected 3 tools, got %d", len(all))
	}
	if all[0].Name() != "alpha" || all[1].Name() != "bravo" || all[2].Name() != "charlie" {
		t.Errorf("tools not sorted: %v", []string{all[0].Name(), all[1].Name(), all[2].Name()})
	}
}

// TestRegistryNames verifies Names() returns sorted names.
func TestRegistryNames(t *testing.T) {
	r := NewRegistry()
	r.Register(&mockToolForRegistry{name: "z_tool"})
	r.Register(&mockToolForRegistry{name: "a_tool"})
	r.Register(&mockToolForRegistry{name: "m_tool"})

	names := r.Names()
	if len(names) != 3 {
		t.Fatalf("expected 3 names, got %d", len(names))
	}
	if names[0] != "a_tool" || names[1] != "m_tool" || names[2] != "z_tool" {
		t.Errorf("names not sorted: %v", names)
	}
}

// TestRegistryToolDefinitions verifies ToolDefinitions() returns correct definitions.
func TestRegistryToolDefinitions(t *testing.T) {
	r := NewRegistry()
	r.Register(&mockToolForRegistry{name: "bash", description: "run commands"})
	r.Register(&mockToolForRegistry{name: "grep", description: "search files"})

	defs := r.ToolDefinitions()
	if len(defs) != 2 {
		t.Fatalf("expected 2 definitions, got %d", len(defs))
	}
	if defs[0].Name != "bash" || defs[0].Description != "run commands" {
		t.Errorf("first def = %+v", defs[0])
	}
	if defs[1].Name != "grep" || defs[1].Description != "search files" {
		t.Errorf("second def = %+v", defs[1])
	}
}
