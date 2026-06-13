package config

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
)

// Config holds the full configuration for Cairn Code.
type Config struct {
	DefaultProvider string            `json:"default_provider"`
	DefaultModel    string            `json:"default_model"`
	Anthropic       AnthropicConfig   `json:"anthropic"`
	OpenAI          OpenAIConfig       `json:"openai"`
	Ollama          OllamaConfig       `json:"ollama"`
	Permissions     PermissionsConfig `json:"permissions"`
	MaxTurns        int               `json:"max_turns"`
	MaxTokens       int               `json:"max_tokens"`
	SystemPromptFile string           `json:"system_prompt_file"`
	ContextFiles    []string          `json:"context_files"`
}

type AnthropicConfig struct {
	APIKey  string `json:"api_key"`
	BaseURL string `json:"base_url"`
}

type OpenAIConfig struct {
	APIKey  string `json:"api_key"`
	BaseURL string `json:"base_url"`
	OrgID   string `json:"org_id"`
}

type OllamaConfig struct {
	BaseURL string `json:"base_url"`
}

type PermissionsConfig struct {
	AutoAllow []string `json:"auto_allow"`
	Ask       []string `json:"ask"`
	Deny      []string `json:"deny"`
}

// DefaultConfig returns a config with sensible defaults.
func DefaultConfig() *Config {
	return &Config{
		DefaultProvider:  "anthropic",
		DefaultModel:     "claude-sonnet-4-20250514",
		Anthropic:        AnthropicConfig{BaseURL: "https://api.anthropic.com"},
		OpenAI:           OpenAIConfig{BaseURL: "https://api.openai.com/v1"},
		Ollama:           OllamaConfig{BaseURL: "http://localhost:11434"},
		MaxTurns:         100,
		MaxTokens:        8192,
		SystemPromptFile: "CAIRN.md",
		ContextFiles:      []string{},
		Permissions: PermissionsConfig{
			AutoAllow: []string{},
			Ask:       []string{"file_write", "bash", "file_edit"},
			Deny:      []string{},
		},
	}
}

// globalConfigPath returns the path to the global config directory.
func globalConfigPath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		home = ""
	}
	switch runtime.GOOS {
	case "darwin", "linux":
		return filepath.Join(home, ".config", "cairn-code")
	case "windows":
		return filepath.Join(home, "AppData", "Roaming", "cairn-code")
	default:
		return filepath.Join(home, ".config", "cairn-code")
	}
}

// LoadConfig loads and merges global and project-local configuration.
// Project config overrides global config.
func LoadConfig() (*Config, error) {
	cfg := DefaultConfig()

	// Load global config
	globalPath := filepath.Join(globalConfigPath(), "config.json")
	globalCfg, err := loadConfigFile(globalPath)
	if err != nil && !os.IsNotExist(err) {
		return nil, fmt.Errorf("loading global config %s: %w", globalPath, err)
	}
	if globalCfg != nil {
		mergeConfig(cfg, globalCfg)
	}

	// Load project-local config
	projectCfg, err := loadConfigFile(".cairn/config.json")
	if err != nil && !os.IsNotExist(err) {
		return nil, fmt.Errorf("loading project config: %w", err)
	}
	if projectCfg != nil {
		mergeConfig(cfg, projectCfg)
	}

	return cfg, nil
}

// loadConfigFile reads and parses a JSON config file. Returns nil if file doesn't exist.
func loadConfigFile(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	cfg := &Config{}
	if err := json.Unmarshal(data, cfg); err != nil {
		return nil, fmt.Errorf("parsing config %s: %w", path, err)
	}
	return cfg, nil
}

// mergeConfig overlays the source config onto the destination.
// Only non-zero values from source override destination.
func mergeConfig(dst, src *Config) {
	if src.DefaultProvider != "" {
		dst.DefaultProvider = src.DefaultProvider
	}
	if src.DefaultModel != "" {
		dst.DefaultModel = src.DefaultModel
	}
	if src.Anthropic.APIKey != "" {
		dst.Anthropic.APIKey = src.Anthropic.APIKey
	}
	if src.Anthropic.BaseURL != "" {
		dst.Anthropic.BaseURL = src.Anthropic.BaseURL
	}
	if src.OpenAI.APIKey != "" {
		dst.OpenAI.APIKey = src.OpenAI.APIKey
	}
	if src.OpenAI.BaseURL != "" {
		dst.OpenAI.BaseURL = src.OpenAI.BaseURL
	}
	if src.OpenAI.OrgID != "" {
		dst.OpenAI.OrgID = src.OpenAI.OrgID
	}
	if src.Ollama.BaseURL != "" {
		dst.Ollama.BaseURL = src.Ollama.BaseURL
	}
	if src.MaxTurns != 0 {
		dst.MaxTurns = src.MaxTurns
	}
	if src.MaxTokens != 0 {
		dst.MaxTokens = src.MaxTokens
	}
	if src.SystemPromptFile != "" {
		dst.SystemPromptFile = src.SystemPromptFile
	}
	if len(src.ContextFiles) > 0 {
		dst.ContextFiles = src.ContextFiles
	}
	if len(src.Permissions.AutoAllow) > 0 {
		dst.Permissions.AutoAllow = src.Permissions.AutoAllow
	}
	if len(src.Permissions.Ask) > 0 {
		dst.Permissions.Ask = src.Permissions.Ask
	}
	if len(src.Permissions.Deny) > 0 {
		dst.Permissions.Deny = src.Permissions.Deny
	}
}

// GetAnthropicAPIKey returns the Anthropic API key, checking the config
// and falling back to the ANTHROPIC_API_KEY environment variable.
func (c *Config) GetAnthropicAPIKey() string {
	if c.Anthropic.APIKey != "" {
		return c.Anthropic.APIKey
	}
	return os.Getenv("ANTHROPIC_API_KEY")
}

// GetOpenAIAPIKey returns the OpenAI API key, checking the config
// and falling back to the OPENAI_API_KEY environment variable.
func (c *Config) GetOpenAIAPIKey() string {
	if c.OpenAI.APIKey != "" {
		return c.OpenAI.APIKey
	}
	return os.Getenv("OPENAI_API_KEY")
}

// GetAnthropicBaseURL returns the Anthropic base URL with default fallback.
func (c *Config) GetAnthropicBaseURL() string {
	if c.Anthropic.BaseURL != "" {
		return c.Anthropic.BaseURL
	}
	return "https://api.anthropic.com"
}

// GetOpenAIBaseURL returns the OpenAI base URL with default fallback.
func (c *Config) GetOpenAIBaseURL() string {
	if c.OpenAI.BaseURL != "" {
		return c.OpenAI.BaseURL
	}
	return "https://api.openai.com/v1"
}

// GetOllamaBaseURL returns the Ollama base URL with default fallback.
func (c *Config) GetOllamaBaseURL() string {
	if c.Ollama.BaseURL != "" {
		return c.Ollama.BaseURL
	}
	return "http://localhost:11434"
}

// SaveConfig writes the config to the global config file.
func SaveConfig(cfg *Config) error {
	configDir := globalConfigPath()
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("creating config directory: %w", err)
	}

	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return fmt.Errorf("marshaling config: %w", err)
	}

	path := filepath.Join(configDir, "config.json")
	if err := os.WriteFile(path, data, 0644); err != nil {
		return fmt.Errorf("writing config: %w", err)
	}

	return nil
}

// IsToolAllowed checks if a tool is allowed based on permission config.
// Returns true if allowed, false if denied.
func (c *Config) IsToolAllowed(toolName string) bool {
	// Check deny list first
	for _, name := range c.Permissions.Deny {
		if name == toolName {
			return false
		}
	}
	return true
}

// IsToolAutoAllowed checks if a tool is in the auto-allow list.
func (c *Config) IsToolAutoAllowed(toolName string) bool {
	for _, name := range c.Permissions.AutoAllow {
		if name == toolName {
			return true
		}
	}
	return false
}
