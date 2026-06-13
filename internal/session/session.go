package session

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/cairn/cairn-code/internal/llm"
)

// Session represents a persisted conversation session.
type Session struct {
	ID        string         `json:"id"`
	CreatedAt time.Time      `json:"created_at"`
	UpdatedAt time.Time      `json:"updated_at"`
	Messages  []SessionMsg   `json:"messages"`
	Model     string         `json:"model"`
	Provider  string         `json:"provider"`
	Summary   string         `json:"summary,omitempty"`
	TokensIn  int            `json:"tokens_in"`
	TokensOut int            `json:"tokens_out"`
}

// SessionMsg is a JSON-friendly representation of a conversation message.
// Content can be a string or []llm.ContentBlock.
type SessionMsg struct {
	Role    string `json:"role"`
	Content any    `json:"content"`
}

// DefaultSessionDir returns the default directory for session storage.
func DefaultSessionDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		home = ""
	}
	return filepath.Join(home, ".cache", "cairn-code", "sessions")
}

// NewSessionID generates a new unique session ID.
func NewSessionID() string {
	return uuid.New().String()
}

// SaveSession saves a session to disk.
func SaveSession(dir string, s *Session) error {
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("creating session directory: %w", err)
	}

	s.UpdatedAt = time.Now()

	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return fmt.Errorf("marshaling session: %w", err)
	}

	path := filepath.Join(dir, s.ID+".json")
	if err := os.WriteFile(path, data, 0644); err != nil {
		return fmt.Errorf("writing session: %w", err)
	}

	return nil
}

// LoadSession loads a session from disk by ID.
func LoadSession(dir string, id string) (*Session, error) {
	// Sanitize ID to prevent path traversal
	if strings.Contains(id, "..") || strings.Contains(id, "/") || strings.Contains(id, "\\") {
		return nil, fmt.Errorf("invalid session ID")
	}

	path := filepath.Join(dir, id+".json")
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("reading session: %w", err)
	}

	var s Session
	if err := json.Unmarshal(data, &s); err != nil {
		return nil, fmt.Errorf("parsing session: %w", err)
	}

	return &s, nil
}

// ListSessions returns all saved sessions, sorted by most recently updated.
func ListSessions(dir string) ([]Session, error) {
	if err := os.MkdirAll(dir, 0755); err != nil {
		return nil, fmt.Errorf("creating session directory: %w", err)
	}

	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("reading session directory: %w", err)
	}

	var sessions []Session
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}

		path := filepath.Join(dir, entry.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue // skip unreadable files
		}

		var s Session
		if err := json.Unmarshal(data, &s); err != nil {
			continue // skip unparseable files
		}

		sessions = append(sessions, s)
	}

	// Sort by most recently updated
	sort.Slice(sessions, func(i, j int) bool {
		return sessions[i].UpdatedAt.After(sessions[j].UpdatedAt)
	})

	return sessions, nil
}

// ToMessages converts SessionMsg to llm.Message.
func (s *Session) ToMessages() []llm.Message {
	msgs := make([]llm.Message, 0, len(s.Messages))
	for _, sm := range s.Messages {
		msgs = append(msgs, llm.Message{
			Role:    llm.MessageRole(sm.Role),
			Content: sm.Content,
		})
	}
	return msgs
}

// FromMessages creates a Session from an agent's message history.
func FromMessages(id string, msgs []llm.Message, model, provider string, tokensIn, tokensOut int) *Session {
	sMsgs := make([]SessionMsg, 0, len(msgs))
	for _, m := range msgs {
		sMsgs = append(sMsgs, SessionMsg{
			Role:    string(m.Role),
			Content: m.Content,
		})
	}

	now := time.Now()
	return &Session{
		ID:        id,
		CreatedAt: now,
		UpdatedAt: now,
		Messages:  sMsgs,
		Model:     model,
		Provider:  provider,
		TokensIn:  tokensIn,
		TokensOut: tokensOut,
	}
}
