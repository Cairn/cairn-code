package cost

import "testing"

func TestGetModelPricing(t *testing.T) {
	tests := []struct {
		model          string
		wantInputPerM  float64
		wantOutputPerM float64
	}{
		{"claude-sonnet-4-20250514", 3.00, 15.00},
		{"claude-3-5-sonnet-20241022", 3.00, 15.00},
		{"claude-3-5-haiku-20241022", 0.80, 4.00},
		{"claude-3-opus-20240229", 15.00, 75.00},
		{"gpt-4o", 2.50, 10.00},
		{"gpt-4o-mini", 0.15, 0.60},
		{"gpt-3.5-turbo", 0.50, 1.50},
		{"llama3", 0, 0},     // free
		{"unknown-model", 0, 0}, // unknown = free
	}

	for _, tt := range tests {
		t.Run(tt.model, func(t *testing.T) {
			p := GetModelPricing(tt.model)
			if p.InputCostPerM != tt.wantInputPerM {
				t.Errorf("GetModelPricing(%q).InputCostPerM = %v, want %v", tt.model, p.InputCostPerM, tt.wantInputPerM)
			}
			if p.OutputCostPerM != tt.wantOutputPerM {
				t.Errorf("GetModelPricing(%q).OutputCostPerM = %v, want %v", tt.model, p.OutputCostPerM, tt.wantOutputPerM)
			}
		})
	}
}

func TestGetModelPricing_PrefixMatch(t *testing.T) {
	// Test that partial model names get matched to the correct family
	tests := []struct {
		model         string
		wantInputPerM float64
	}{
		// Claude 4 family
		{"claude-sonnet-4-20250514", 3.00},
		// Claude 3.5 family with different dates
		{"claude-3-5-sonnet-20241022", 3.00},
		{"claude-3-5-haiku-20241022", 0.80},
	}

	for _, tt := range tests {
		p := GetModelPricing(tt.model)
		if p.InputCostPerM != tt.wantInputPerM {
			t.Errorf("GetModelPricing(%q).InputCostPerM = %v, want %v", tt.model, p.InputCostPerM, tt.wantInputPerM)
		}
	}
}

func TestEstimateCost(t *testing.T) {
	tests := []struct {
		name       string
		model      string
		input      int
		output     int
		cacheRead  int
		cacheWrite int
		wantCost   float64
	}{
		{
			name:    "zero usage",
			model:   "claude-sonnet-4-20250514",
			wantCost: 0,
		},
		{
			name:    "simple claude sonnet 4",
			model:   "claude-sonnet-4-20250514",
			input:   1_000_000,
			output:  500_000,
			wantCost: 3.00*1.0 + 15.00*0.5, // $3.00 + $7.50 = $10.50
		},
		{
			name:       "with cache",
			model:      "claude-sonnet-4-20250514",
			input:      500_000,
			output:     250_000,
			cacheRead:  500_000,
			cacheWrite: 100_000,
			wantCost:   3.00*0.5 + 15.00*0.25 + 0.30*0.5 + 3.75*0.1, // $1.50+$3.75+$0.15+$0.375 = $5.775
		},
		{
			name:    "gpt-4o",
			model:   "gpt-4o",
			input:   1_000_000,
			output:  1_000_000,
			wantCost: 2.50 + 10.00, // $12.50
		},
		{
			name:    "free model",
			model:   "llama3",
			input:   1_000_000,
			output:  1_000_000,
			wantCost: 0,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := EstimateCost(tt.model, tt.input, tt.output, tt.cacheRead, tt.cacheWrite)
			// Allow small floating point tolerance
			diff := got - tt.wantCost
			if diff < 0 {
				diff = -diff
			}
			if diff > 0.001 {
				t.Errorf("EstimateCost() = %v, want %v (diff %v)", got, tt.wantCost, diff)
			}
		})
	}
}

func TestFormatCost(t *testing.T) {
	tests := []struct {
		cost float64
		want string
	}{
		{0, "$0.00"},
		{0.001, "$0.0010"},
		{0.01, "$0.010"},
		{0.10, "$0.100"},
		{1.00, "$1.00"},
		{1.23, "$1.23"},
		{10.50, "$10.50"},
		{123.456, "$123.46"},
	}

	for _, tt := range tests {
		got := FormatCost(tt.cost)
		if got != tt.want {
			t.Errorf("FormatCost(%v) = %q, want %q", tt.cost, got, tt.want)
		}
	}
}

func TestFormatCostShort(t *testing.T) {
	tests := []struct {
		cost float64
		want string
	}{
		{0, "$0.00"},
		{0.001, "$0.001"},
		{0.01, "$0.01"},
		{0.10, "$0.10"},
		{1.00, "$1.00"},
		{10.50, "$10.50"},
	}

	for _, tt := range tests {
		got := FormatCostShort(tt.cost)
		if got != tt.want {
			t.Errorf("FormatCostShort(%v) = %q, want %q", tt.cost, got, tt.want)
		}
	}
}

func TestCachePricing(t *testing.T) {
	// Claude models should have cache pricing
	p := GetModelPricing("claude-sonnet-4-20250514")
	if p.CacheReadPerM == 0 {
		t.Error("Claude Sonnet 4 should have non-zero cache read pricing")
	}
	if p.CacheCreatePerM == 0 {
		t.Error("Claude Sonnet 4 should have non-zero cache creation pricing")
	}

	// OpenAI models should NOT have cache pricing
	p = GetModelPricing("gpt-4o")
	if p.CacheReadPerM != 0 {
		t.Error("GPT-4o should have zero cache read pricing")
	}
}
