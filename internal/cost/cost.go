package cost

import (
	"fmt"
	"math"
)

// ModelPricing stores per-million-token pricing for a model.
type ModelPricing struct {
	InputCostPerM   float64 // cost per 1M input tokens (USD)
	OutputCostPerM  float64 // cost per 1M output tokens (USD)
	CacheReadPerM   float64 // cost per 1M cache-read input tokens (USD, 0 if unsupported)
	CacheCreatePerM float64 // cost per 1M cache-creation input tokens (USD, 0 if unsupported)
}

// modelPricingTable maps model name patterns to their pricing.
// Keys are checked with HasPrefix, so "claude-3" matches "claude-3-5-sonnet-20241022".
var modelPricingTable = map[string]ModelPricing{
	// Claude 4 models (2025)
	"claude-sonnet-4-20250514": {
		InputCostPerM:   3.00,
		OutputCostPerM:  15.00,
		CacheReadPerM:   0.30,
		CacheCreatePerM: 3.75,
	},
	"claude-opus-4-20250514": {
		InputCostPerM:   15.00,
		OutputCostPerM:  75.00,
		CacheReadPerM:   1.50,
		CacheCreatePerM: 18.75,
	},
	// Claude 3.5 models
	"claude-3-5-sonnet-20241022": {
		InputCostPerM:   3.00,
		OutputCostPerM:  15.00,
		CacheReadPerM:   0.30,
		CacheCreatePerM: 3.75,
	},
	"claude-3-5-haiku-20241022": {
		InputCostPerM:   0.80,
		OutputCostPerM:  4.00,
		CacheReadPerM:   0.08,
		CacheCreatePerM: 1.00,
	},
	// Claude 3 models
	"claude-3-opus-20240229": {
		InputCostPerM:   15.00,
		OutputCostPerM:  75.00,
		CacheReadPerM:   1.50,
		CacheCreatePerM: 18.75,
	},
	"claude-3-sonnet-20240229": {
		InputCostPerM:   3.00,
		OutputCostPerM:  15.00,
		CacheReadPerM:   0.30,
		CacheCreatePerM: 3.75,
	},
	"claude-3-haiku-20240307": {
		InputCostPerM:   0.25,
		OutputCostPerM:  1.25,
		CacheReadPerM:   0.03,
		CacheCreatePerM: 0.30,
	},
	// OpenAI models
	"gpt-4o": {
		InputCostPerM:  2.50,
		OutputCostPerM: 10.00,
	},
	"gpt-4o-mini": {
		InputCostPerM:  0.15,
		OutputCostPerM: 0.60,
	},
	"gpt-4-turbo": {
		InputCostPerM:  10.00,
		OutputCostPerM: 30.00,
	},
	"gpt-3.5-turbo": {
		InputCostPerM:  0.50,
		OutputCostPerM: 1.50,
	},
	// Ollama and OpenCode are free
	"llama": {
		InputCostPerM:  0,
		OutputCostPerM: 0,
	},
	"big-pickle": {
		InputCostPerM:  0,
		OutputCostPerM: 0,
	},
}

// GetModelPricing returns the pricing for a model, checking exact match first
// then prefix match. Returns zero pricing (free) for unknown models.
func GetModelPricing(model string) ModelPricing {
	// Exact match first
	if p, ok := modelPricingTable[model]; ok {
		return p
	}
	// Prefix match (e.g., "claude-3-5-sonnet-new-date")
	for pattern, p := range modelPricingTable {
		if len(model) > len(pattern) {
			// Strip the date suffix from model and try matching the base name
			continue
		}
		if len(model) < len(pattern) && pattern[:len(model)] == model {
			return p
		}
	}
	// Check by known prefix families
	switch {
	case contains(model, "claude-sonnet-4"):
		return ModelPricing{InputCostPerM: 3.00, OutputCostPerM: 15.00, CacheReadPerM: 0.30, CacheCreatePerM: 3.75}
	case contains(model, "claude-opus-4"):
		return ModelPricing{InputCostPerM: 15.00, OutputCostPerM: 75.00, CacheReadPerM: 1.50, CacheCreatePerM: 18.75}
	case contains(model, "claude-3-5-sonnet"):
		return ModelPricing{InputCostPerM: 3.00, OutputCostPerM: 15.00, CacheReadPerM: 0.30, CacheCreatePerM: 3.75}
	case contains(model, "claude-3-5-haiku"):
		return ModelPricing{InputCostPerM: 0.80, OutputCostPerM: 4.00, CacheReadPerM: 0.08, CacheCreatePerM: 1.00}
	case contains(model, "claude-3-opus"):
		return ModelPricing{InputCostPerM: 15.00, OutputCostPerM: 75.00, CacheReadPerM: 1.50, CacheCreatePerM: 18.75}
	case contains(model, "claude-3-sonnet"):
		return ModelPricing{InputCostPerM: 3.00, OutputCostPerM: 15.00, CacheReadPerM: 0.30, CacheCreatePerM: 3.75}
	case contains(model, "claude-3-haiku"):
		return ModelPricing{InputCostPerM: 0.25, OutputCostPerM: 1.25, CacheReadPerM: 0.03, CacheCreatePerM: 0.30}
	case contains(model, "gpt-4o-mini"):
		return ModelPricing{InputCostPerM: 0.15, OutputCostPerM: 0.60}
	case contains(model, "gpt-4o"):
		return ModelPricing{InputCostPerM: 2.50, OutputCostPerM: 10.00}
	case contains(model, "gpt-4-turbo"):
		return ModelPricing{InputCostPerM: 10.00, OutputCostPerM: 30.00}
	case contains(model, "gpt-3.5"):
		return ModelPricing{InputCostPerM: 0.50, OutputCostPerM: 1.50}
	}
	return ModelPricing{} // free / unknown
}

// EstimateCost calculates the estimated cost in USD for a given model and token usage.
func EstimateCost(model string, inputTokens, outputTokens, cacheRead, cacheCreate int) float64 {
	pricing := GetModelPricing(model)
	cost := 0.0
	cost += float64(inputTokens) / 1_000_000 * pricing.InputCostPerM
	cost += float64(outputTokens) / 1_000_000 * pricing.OutputCostPerM
	cost += float64(cacheRead) / 1_000_000 * pricing.CacheReadPerM
	cost += float64(cacheCreate) / 1_000_000 * pricing.CacheCreatePerM
	return cost
}

// FormatCost formats a cost as a dollar string with appropriate precision.
func FormatCost(cost float64) string {
	if cost == 0 {
		return "$0.00"
	}
	if cost < 0.01 {
		return fmt.Sprintf("$%.4f", cost)
	}
	if cost < 1 {
		return fmt.Sprintf("$%.3f", cost)
	}
	return fmt.Sprintf("$%.2f", cost)
}

// FormatCostShort formats a cost as a compact string, e.g. "$1.23" or "$0.004".
func FormatCostShort(cost float64) string {
	if cost == 0 {
		return "$0.00"
	}
	abs := math.Abs(cost)
	if abs < 0.01 {
		return fmt.Sprintf("$%.3f", cost)
	}
	return fmt.Sprintf("$%.2f", cost)
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && s[:len(substr)] == substr
}
