use crate::llm::Usage;

#[derive(Clone)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_create_per_mtok: f64,
}

fn pricing(model: &str) -> ModelPricing {
    // Match bare ids and effort-qualified forms like `grok-4.5:high`.
    let base = model.split(':').next().unwrap_or(model);
    match base {
        "claude-sonnet-4-20250514" | "claude-sonnet-4" | "claude-4-sonnet" | "claude-sonnet-5" => {
            ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.30,
                cache_create_per_mtok: 3.75,
            }
        }
        "claude-opus-4-8" | "claude-opus-4-20250514" => ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cache_read_per_mtok: 1.50,
            cache_create_per_mtok: 18.75,
        },
        "gpt-4o" | "gpt-4o-2024-11-20" => ModelPricing {
            input_per_mtok: 2.50,
            output_per_mtok: 10.0,
            cache_read_per_mtok: 1.25,
            cache_create_per_mtok: 0.0,
        },
        "grok-4.5" | "grok-4" => ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 6.0,
            cache_read_per_mtok: 0.30,
            cache_create_per_mtok: 0.0,
        },
        _ => ModelPricing {
            input_per_mtok: 0.0,
            output_per_mtok: 0.0,
            cache_read_per_mtok: 0.0,
            cache_create_per_mtok: 0.0,
        },
    }
}

pub fn estimate_cost(model: &str, usage: &Usage) -> f64 {
    let p = pricing(model);
    let input_cost = usage.input_tokens as f64 * p.input_per_mtok / 1_000_000.0;
    let output_cost = usage.output_tokens as f64 * p.output_per_mtok / 1_000_000.0;
    let cache_read_cost = usage.cache_read as f64 * p.cache_read_per_mtok / 1_000_000.0;
    let cache_create_cost = usage.cache_create as f64 * p.cache_create_per_mtok / 1_000_000.0;
    input_cost + output_cost + cache_read_cost + cache_create_cost
}

pub fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cache_read: u64, cache_create: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read,
            cache_create,
        }
    }

    #[test]
    fn sonnet_pricing_known() {
        let u = usage(1_000_000, 1_000_000, 0, 0);
        let c = estimate_cost("claude-sonnet-4-20250514", &u);
        assert!((c - 18.0).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn effort_suffix_uses_base_model() {
        let u = usage(1_000_000, 0, 0, 0);
        let bare = estimate_cost("grok-4.5", &u);
        let effort = estimate_cost("grok-4.5:high", &u);
        assert!((bare - effort).abs() < 1e-12);
        assert!((bare - 2.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_is_zero() {
        let u = usage(999_999, 999_999, 1, 1);
        assert_eq!(estimate_cost("totally-unknown-model", &u), 0.0);
    }

    #[test]
    fn cache_tokens_bill() {
        let u = usage(0, 0, 1_000_000, 1_000_000);
        let c = estimate_cost("claude-sonnet-4", &u);
        // 0.30 + 3.75
        assert!((c - 4.05).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn format_cost_thresholds() {
        assert_eq!(format_cost(0.0012), "$0.0012");
        assert_eq!(format_cost(0.01), "$0.01");
        assert_eq!(format_cost(1.234), "$1.23");
    }
}
