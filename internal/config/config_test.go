package config

import (
        "encoding/json"
        "os"
        "path/filepath"
        "testing"
)

// TestDefaultConfig verifies all default values.
func TestDefaultConfig(t *testing.T) {
        cfg := DefaultConfig()

        if cfg.DefaultProvider != "opencode" {
                t.Errorf("DefaultProvider = %q, want 'opencode'", cfg.DefaultProvider)
        }
        if cfg.DefaultModel != "big-pickle" {
                t.Errorf("DefaultModel = %q, want 'big-pickle'", cfg.DefaultModel)
        }
        if cfg.MaxTurns != 100 {
                t.Errorf("MaxTurns = %d, want 100", cfg.MaxTurns)
        }
        if cfg.MaxTokens != 8192 {
                t.Errorf("MaxTokens = %d, want 8192", cfg.MaxTokens)
        }
        if cfg.SystemPromptFile != "CAIRN.md" {
                t.Errorf("SystemPromptFile = %q, want 'CAIRN.md'", cfg.SystemPromptFile)
        }
        if cfg.Anthropic.BaseURL != "https://api.anthropic.com" {
                t.Errorf("Anthropic.BaseURL = %q", cfg.Anthropic.BaseURL)
        }
        if cfg.OpenAI.BaseURL != "https://api.openai.com/v1" {
                t.Errorf("OpenAI.BaseURL = %q", cfg.OpenAI.BaseURL)
        }
        if cfg.Ollama.BaseURL != "http://localhost:11434" {
                t.Errorf("Ollama.BaseURL = %q", cfg.Ollama.BaseURL)
        }
        if len(cfg.Permissions.Ask) != 3 {
                t.Errorf("Permissions.Ask = %v, want 3 items", cfg.Permissions.Ask)
        }
}

// TestLoadConfigFileNonExistent verifies error returned for missing file.
func TestLoadConfigFileNonExistent(t *testing.T) {
        tmpDir := t.TempDir()
        cfg, err := loadConfigFile(filepath.Join(tmpDir, "nonexistent.json"))
        if err == nil {
                t.Fatal("expected error for non-existent file")
        }
        if cfg != nil {
                t.Error("expected nil config for non-existent file")
        }
}

// TestLoadConfigFileInvalidJSON verifies error for malformed JSON.
func TestLoadConfigFileInvalidJSON(t *testing.T) {
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "config.json")
        os.WriteFile(path, []byte(`{invalid json}`), 0644)

        _, err := loadConfigFile(path)
        if err == nil {
                t.Fatal("expected error for invalid JSON")
        }
}

// TestLoadConfigFileValid verifies successful loading.
func TestLoadConfigFileValid(t *testing.T) {
        tmpDir := t.TempDir()
        path := filepath.Join(tmpDir, "config.json")
        data := `{"default_provider":"anthropic","max_turns":50}`
        os.WriteFile(path, []byte(data), 0644)

        cfg, err := loadConfigFile(path)
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }
        if cfg.DefaultProvider != "anthropic" {
                t.Errorf("DefaultProvider = %q, want 'anthropic'", cfg.DefaultProvider)
        }
        if cfg.MaxTurns != 50 {
                t.Errorf("MaxTurns = %d, want 50", cfg.MaxTurns)
        }
}

// TestMergeConfigEmptySource verifies destination unchanged with empty source.
func TestMergeConfigEmptySource(t *testing.T) {
        dst := DefaultConfig()
        originalProvider := dst.DefaultProvider

        src := &Config{}
        mergeConfig(dst, src)

        if dst.DefaultProvider != originalProvider {
                t.Errorf("DefaultProvider should not change, got %q", dst.DefaultProvider)
        }
}

// TestMergeConfigPartialOverride verifies only non-zero fields override.
func TestMergeConfigPartialOverride(t *testing.T) {
        dst := DefaultConfig()
        src := &Config{
                DefaultProvider: "ollama",
                MaxTurns:        50,
        }

        mergeConfig(dst, src)

        if dst.DefaultProvider != "ollama" {
                t.Errorf("DefaultProvider = %q, want 'ollama'", dst.DefaultProvider)
        }
        if dst.MaxTurns != 50 {
                t.Errorf("MaxTurns = %d, want 50", dst.MaxTurns)
        }
        // Unchanged fields should remain at defaults
        if dst.MaxTokens != 8192 {
                t.Errorf("MaxTokens should stay at default 8192, got %d", dst.MaxTokens)
        }
}

// TestMergeConfigZeroDoesNotOverride verifies zero-value ints don't override.
func TestMergeConfigZeroDoesNotOverride(t *testing.T) {
        dst := &Config{MaxTurns: 100, MaxTokens: 8192}
        src := &Config{MaxTurns: 0, MaxTokens: 0}

        mergeConfig(dst, src)

        if dst.MaxTurns != 100 {
                t.Errorf("MaxTurns should not be overridden by zero, got %d", dst.MaxTurns)
        }
        if dst.MaxTokens != 8192 {
                t.Errorf("MaxTokens should not be overridden by zero, got %d", dst.MaxTokens)
        }
}

// TestMergeConfigPermissions verifies permission lists are merged.
func TestMergeConfigPermissions(t *testing.T) {
        dst := &Config{
                Permissions: PermissionsConfig{
                        AutoAllow: []string{},
                        Ask:       []string{},
                        Deny:      []string{},
                },
        }
        src := &Config{
                Permissions: PermissionsConfig{
                        AutoAllow: []string{"FileRead"},
                        Deny:      []string{"Bash"},
                },
        }

        mergeConfig(dst, src)

        if len(dst.Permissions.AutoAllow) != 1 || dst.Permissions.AutoAllow[0] != "FileRead" {
                t.Errorf("AutoAllow = %v, want [FileRead]", dst.Permissions.AutoAllow)
        }
        if len(dst.Permissions.Deny) != 1 || dst.Permissions.Deny[0] != "Bash" {
                t.Errorf("Deny = %v, want [Bash]", dst.Permissions.Deny)
        }
}

// TestGetAnthropicAPIKeyFromConfig verifies config key takes priority.
func TestGetAnthropicAPIKeyFromConfig(t *testing.T) {
        cfg := &Config{Anthropic: AnthropicConfig{APIKey: "cfg-key"}}
        if cfg.GetAnthropicAPIKey() != "cfg-key" {
                t.Errorf("expected config key, got %q", cfg.GetAnthropicAPIKey())
        }
}

// TestGetAnthropicAPIKeyFromEnv verifies env var fallback.
func TestGetAnthropicAPIKeyFromEnv(t *testing.T) {
        os.Setenv("ANTHROPIC_API_KEY", "env-key")
        defer os.Unsetenv("ANTHROPIC_API_KEY")

        cfg := &Config{Anthropic: AnthropicConfig{APIKey: ""}}
        if cfg.GetAnthropicAPIKey() != "env-key" {
                t.Errorf("expected env key, got %q", cfg.GetAnthropicAPIKey())
        }
}

// TestGetOpenAIAPIKeyFromConfig verifies config key takes priority.
func TestGetOpenAIAPIKeyFromConfig(t *testing.T) {
        cfg := &Config{OpenAI: OpenAIConfig{APIKey: "cfg-oai"}}
        if cfg.GetOpenAIAPIKey() != "cfg-oai" {
                t.Errorf("expected config key, got %q", cfg.GetOpenAIAPIKey())
        }
}

// TestGetOpenAIAPIKeyFromEnv verifies env var fallback.
func TestGetOpenAIAPIKeyFromEnv(t *testing.T) {
        os.Setenv("OPENAI_API_KEY", "env-oai")
        defer os.Unsetenv("OPENAI_API_KEY")

        cfg := &Config{OpenAI: OpenAIConfig{APIKey: ""}}
        if cfg.GetOpenAIAPIKey() != "env-oai" {
                t.Errorf("expected env key, got %q", cfg.GetOpenAIAPIKey())
        }
}

// TestGetAnthropicBaseURLFallback verifies default URL.
func TestGetAnthropicBaseURLFallback(t *testing.T) {
        cfg := &Config{Anthropic: AnthropicConfig{BaseURL: ""}}
        if cfg.GetAnthropicBaseURL() != "https://api.anthropic.com" {
                t.Errorf("expected default URL, got %q", cfg.GetAnthropicBaseURL())
        }
}

// TestGetAnthropicBaseURLCustom verifies custom URL.
func TestGetAnthropicBaseURLCustom(t *testing.T) {
        cfg := &Config{Anthropic: AnthropicConfig{BaseURL: "https://custom.api.com"}}
        if cfg.GetAnthropicBaseURL() != "https://custom.api.com" {
                t.Errorf("expected custom URL, got %q", cfg.GetAnthropicBaseURL())
        }
}

// TestGetOpenAIBaseURLFallback verifies default URL.
func TestGetOpenAIBaseURLFallback(t *testing.T) {
        cfg := &Config{OpenAI: OpenAIConfig{BaseURL: ""}}
        if cfg.GetOpenAIBaseURL() != "https://api.openai.com/v1" {
                t.Errorf("expected default URL, got %q", cfg.GetOpenAIBaseURL())
        }
}

// TestGetOllamaBaseURLFallback verifies default URL.
func TestGetOllamaBaseURLFallback(t *testing.T) {
        cfg := &Config{Ollama: OllamaConfig{BaseURL: ""}}
        if cfg.GetOllamaBaseURL() != "http://localhost:11434" {
                t.Errorf("expected default URL, got %q", cfg.GetOllamaBaseURL())
        }
}

// TestIsToolAllowed verifies deny list check.
func TestIsToolAllowed(t *testing.T) {
        cfg := &Config{Permissions: PermissionsConfig{Deny: []string{"Bash", "rm"}}}

        if cfg.IsToolAllowed("Bash") {
                t.Error("Bash should be denied")
        }
        if cfg.IsToolAllowed("rm") {
                t.Error("rm should be denied")
        }
        if !cfg.IsToolAllowed("FileRead") {
                t.Error("FileRead should be allowed")
        }
}

// TestIsToolAllowedEmptyDeny verifies all tools allowed with empty deny list.
func TestIsToolAllowedEmptyDeny(t *testing.T) {
        cfg := &Config{}
        if !cfg.IsToolAllowed("anything") {
                t.Error("all tools should be allowed with empty deny list")
        }
}

// TestIsToolAutoAllowed verifies auto-allow check.
func TestIsToolAutoAllowed(t *testing.T) {
        cfg := &Config{Permissions: PermissionsConfig{AutoAllow: []string{"Glob", "Grep"}}}

        if !cfg.IsToolAutoAllowed("Glob") {
                t.Error("Glob should be auto-allowed")
        }
        if cfg.IsToolAutoAllowed("Bash") {
                t.Error("Bash should not be auto-allowed")
        }
}

// TestSaveConfig verifies JSON round-trip through marshaling.
func TestSaveConfig(t *testing.T) {
        tmpDir := t.TempDir()

        cfg := &Config{
                DefaultProvider: "anthropic",
                MaxTurns:        50,
        }

        data, err := json.MarshalIndent(cfg, "", "  ")
        if err != nil {
                t.Fatalf("marshaling: %v", err)
        }

        path := filepath.Join(tmpDir, "config.json")
        os.WriteFile(path, data, 0644)

        loaded, err := loadConfigFile(path)
        if err != nil {
                t.Fatalf("loading: %v", err)
        }
        if loaded.DefaultProvider != "anthropic" {
                t.Errorf("DefaultProvider = %q, want 'anthropic'", loaded.DefaultProvider)
        }
}

// TestLoadConfigIntegration verifies full global+project merge.
func TestLoadConfigIntegration(t *testing.T) {
        // This tests the integration path using actual filesystem.
        // We can't easily mock the global path, but we can test loadConfigFile merging.
        cfg := DefaultConfig()

        global := &Config{DefaultProvider: "ollama", MaxTurns: 25}
        mergeConfig(cfg, global)
        if cfg.DefaultProvider != "ollama" {
                t.Errorf("after global merge: DefaultProvider = %q, want 'ollama'", cfg.DefaultProvider)
        }

        project := &Config{MaxTurns: 10}
        mergeConfig(cfg, project)
        if cfg.MaxTurns != 10 {
                t.Errorf("after project merge: MaxTurns = %d, want 10", cfg.MaxTurns)
        }
        // Global provider should still be there
        if cfg.DefaultProvider != "ollama" {
                t.Errorf("project should not override provider: DefaultProvider = %q", cfg.DefaultProvider)
        }
}
