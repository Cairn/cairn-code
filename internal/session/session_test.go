package session

import (
        "encoding/json"
        "os"
        "path/filepath"
        "strings"
        "testing"
        "time"

        "github.com/cairn/cairn-code/internal/llm"
)

// TestNewSessionID verifies UUID generation.
func TestNewSessionID(t *testing.T) {
        id := NewSessionID()
        if id == "" {
                t.Error("session ID should not be empty")
        }
        // UUID format: 8-4-4-4-12
        parts := strings.Split(id, "-")
        if len(parts) != 5 {
                t.Errorf("UUID should have 5 parts, got %d: %q", len(parts), id)
        }
}

// TestSaveAndLoadRoundTrip verifies save → load preserves all fields.
func TestSaveAndLoadRoundTrip(t *testing.T) {
        dir := t.TempDir()
        original := &Session{
                ID:        "test-id-123",
                CreatedAt: time.Date(2026, 1, 15, 10, 0, 0, 0, time.UTC),
                UpdatedAt: time.Date(2026, 1, 15, 11, 0, 0, 0, time.UTC),
                Messages: []SessionMsg{
                        {Role: "user", Content: "hello"},
                        {Role: "assistant", Content: []llm.ContentBlock{
                                {Type: "text", Text: "hi there"},
                        }},
                },
                Model:    "claude-sonnet-4-20250514",
                Provider: "anthropic",
                Summary:  "Test conversation",
                TokensIn: 100,
                TokensOut: 50,
        }

        err := SaveSession(dir, original)
        if err != nil {
                t.Fatalf("SaveSession error: %v", err)
        }

        loaded, err := LoadSession(dir, "test-id-123")
        if err != nil {
                t.Fatalf("LoadSession error: %v", err)
        }

        if loaded.ID != original.ID {
                t.Errorf("ID = %q, want %q", loaded.ID, original.ID)
        }
        if loaded.Provider != original.Provider {
                t.Errorf("Provider = %q, want %q", loaded.Provider, original.Provider)
        }
        if loaded.TokensIn != original.TokensIn {
                t.Errorf("TokensIn = %d, want %d", loaded.TokensIn, original.TokensIn)
        }
        if len(loaded.Messages) != 2 {
                t.Fatalf("Messages = %d, want 2", len(loaded.Messages))
        }
        if loaded.Messages[0].Content != "hello" {
                t.Errorf("first message content = %v, want 'hello'", loaded.Messages[0].Content)
        }
}

// TestLoadSessionPathTraversal verifies path traversal prevention.
func TestLoadSessionPathTraversal(t *testing.T) {
        dir := t.TempDir()

        tests := []struct {
                name string
                id   string
        }{
                {"dot dot slash", "../etc/passwd"},
                {"forward slash", "foo/bar"},
                {"backslash", "foo\\bar"},
        }

        for _, tt := range tests {
                t.Run(tt.name, func(t *testing.T) {
                        _, err := LoadSession(dir, tt.id)
                        if err == nil {
                                t.Fatal("expected error for path traversal attempt")
                        }
                        if err.Error() != "invalid session ID" {
                                t.Errorf("expected 'invalid session ID', got: %v", err)
                        }
                })
        }
}

// TestLoadSessionNotFound verifies error for non-existent session.
func TestLoadSessionNotFound(t *testing.T) {
        dir := t.TempDir()
        _, err := LoadSession(dir, "nonexistent")
        if err == nil {
                t.Fatal("expected error for non-existent session")
        }
}

// TestLoadSessionInvalidJSON verifies error for corrupted JSON.
func TestLoadSessionInvalidJSON(t *testing.T) {
        dir := t.TempDir()
        path := filepath.Join(dir, "corrupt.json")
        os.WriteFile(path, []byte(`{not valid json}`), 0644)

        _, err := LoadSession(dir, "corrupt")
        if err == nil {
                t.Fatal("expected error for invalid JSON")
        }
}

// TestListSessionsEmpty verifies empty directory returns empty list.
func TestListSessionsEmpty(t *testing.T) {
        dir := t.TempDir()
        sessions, err := ListSessions(dir)
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if len(sessions) != 0 {
                t.Errorf("expected 0 sessions, got %d", len(sessions))
        }
}

// TestListSessionsSorted verifies sessions are sorted by UpdatedAt descending.
func TestListSessionsSorted(t *testing.T) {
        dir := t.TempDir()

        // Create sessions in specific time order
        old := &Session{
                ID: "old-session", UpdatedAt: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
                CreatedAt: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
        }
        mid := &Session{
                ID: "mid-session", UpdatedAt: time.Date(2026, 3, 1, 0, 0, 0, 0, time.UTC),
                CreatedAt: time.Date(2026, 3, 1, 0, 0, 0, 0, time.UTC),
        }
        new := &Session{
                ID: "new-session", UpdatedAt: time.Date(2026, 6, 1, 0, 0, 0, 0, time.UTC),
                CreatedAt: time.Date(2026, 6, 1, 0, 0, 0, 0, time.UTC),
        }

        // Save them — SaveSession overwrites UpdatedAt with time.Now()
        // So we need to write the JSON directly to preserve the timestamps
        writeSessionJSON := func(s *Session) {
                data, _ := json.Marshal(s)
                os.WriteFile(filepath.Join(dir, s.ID+".json"), data, 0644)
        }
        writeSessionJSON(old)
        writeSessionJSON(mid)
        writeSessionJSON(new)

        sessions, err := ListSessions(dir)
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if len(sessions) != 3 {
                t.Fatalf("expected 3 sessions, got %d", len(sessions))
        }
        if sessions[0].ID != "new-session" {
                t.Errorf("first session should be newest, got %q", sessions[0].ID)
        }
        if sessions[2].ID != "old-session" {
                t.Errorf("last session should be oldest, got %q", sessions[2].ID)
        }
}

// TestListSessionsIgnoresNonJSON verifies non-.json files are skipped.
func TestListSessionsIgnoresNonJSON(t *testing.T) {
        dir := t.TempDir()
        os.WriteFile(filepath.Join(dir, "not-a-session.txt"), []byte("hello"), 0644)
        os.WriteFile(filepath.Join(dir, ".gitkeep"), []byte{}, 0644)

        sessions, err := ListSessions(dir)
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if len(sessions) != 0 {
                t.Errorf("expected 0 sessions, got %d", len(sessions))
        }
}

// TestListSessionsSkipsCorrupted verifies corrupted JSON files are silently skipped.
func TestListSessionsSkipsCorrupted(t *testing.T) {
        dir := t.TempDir()
        os.WriteFile(filepath.Join(dir, "good.json"), []byte(`{"id":"good","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","messages":[]}`), 0644)
        os.WriteFile(filepath.Join(dir, "bad.json"), []byte(`{corrupted}`), 0644)

        sessions, err := ListSessions(dir)
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if len(sessions) != 1 {
                t.Errorf("expected 1 valid session, got %d", len(sessions))
        }
        if sessions[0].ID != "good" {
                t.Errorf("expected session 'good', got %q", sessions[0].ID)
        }
}

// TestToMessages verifies conversion to llm.Message slice.
func TestToMessages(t *testing.T) {
        s := &Session{
                Messages: []SessionMsg{
                        {Role: "user", Content: "hello"},
                        {Role: "assistant", Content: "hi"},
                },
        }

        msgs := s.ToMessages()
        if len(msgs) != 2 {
                t.Fatalf("expected 2 messages, got %d", len(msgs))
        }
        if msgs[0].Role != llm.RoleUser {
                t.Errorf("first role = %q, want 'user'", msgs[0].Role)
        }
        if msgs[1].Role != llm.RoleAssistant {
                t.Errorf("second role = %q, want 'assistant'", msgs[1].Role)
        }
        if msgs[0].Content != "hello" {
                t.Errorf("first content = %v", msgs[0].Content)
        }
}

// TestFromMessagesAndToMessagesRoundTrip verifies conversion round-trip.
func TestFromMessagesAndToMessagesRoundTrip(t *testing.T) {
        original := []llm.Message{
                {Role: llm.RoleUser, Content: "test input"},
                {Role: llm.RoleAssistant, Content: []llm.ContentBlock{
                        {Type: "text", Text: "test output"},
                }},
        }

        session := FromMessages("sess-1", original, "claude", "anthropic", 10, 20)
        if session.ID != "sess-1" {
                t.Errorf("ID = %q, want 'sess-1'", session.ID)
        }
        if session.Model != "claude" {
                t.Errorf("Model = %q", session.Model)
        }
        if session.TokensIn != 10 || session.TokensOut != 20 {
                t.Errorf("tokens = %d/%d, want 10/20", session.TokensIn, session.TokensOut)
        }
        if session.CreatedAt != session.UpdatedAt {
                t.Error("CreatedAt should equal UpdatedAt for new sessions")
        }

        // Convert back
        msgs := session.ToMessages()
        if len(msgs) != 2 {
                t.Fatalf("expected 2 messages, got %d", len(msgs))
        }
        if msgs[0].Content != "test input" {
                t.Errorf("first message content lost")
        }
}

// TestSaveSessionUpdatesUpdatedAt verifies SaveSession sets UpdatedAt.
func TestSaveSessionUpdatesUpdatedAt(t *testing.T) {
        dir := t.TempDir()
        before := time.Now().Add(-1 * time.Hour)

        s := &Session{
                ID:        "update-test",
                CreatedAt: before,
                UpdatedAt: before,
                Messages:  []SessionMsg{},
        }

        SaveSession(dir, s)

        loaded, _ := LoadSession(dir, "update-test")
        if !loaded.UpdatedAt.After(before) {
                t.Error("UpdatedAt should be set to current time on save")
        }
}

// TestDefaultSessionDir verifies the default path is non-empty.
func TestDefaultSessionDir(t *testing.T) {
        dir := DefaultSessionDir()
        if dir == "" {
                t.Error("DefaultSessionDir should not be empty")
        }
        if !strings.Contains(dir, "cairn-code") {
                t.Errorf("path should contain 'cairn-code', got %q", dir)
        }
}
