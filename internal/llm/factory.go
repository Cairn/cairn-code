package llm

import (
        "fmt"

        "github.com/cairn/cairn-code/internal/config"
)

// NewProvider creates a new LLM provider based on the configuration.
func NewProvider(cfg *config.Config) (Provider, error) {
        switch cfg.DefaultProvider {
        case "anthropic":
                p := NewAnthropicProvider(cfg)
                if p.apiKey == "" {
                        return nil, fmt.Errorf("anthropic provider requires an API key; set ANTHROPIC_API_KEY or configure in ~/.config/cairn-code/config.json")
                }
                return p, nil
        case "openai":
                p := NewOpenAIProvider(cfg)
                if p.apiKey == "" {
                        return nil, fmt.Errorf("openai provider requires an API key; set OPENAI_API_KEY or configure in ~/.config/cairn-code/config.json")
                }
                return p, nil
        case "opencode":
                return NewOpenCodeProvider(), nil
        case "ollama":
                return NewOllamaProvider(cfg), nil
        default:
                return nil, fmt.Errorf("unknown provider: %q (supported: anthropic, openai, opencode, ollama)", cfg.DefaultProvider)
        }
}
