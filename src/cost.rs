use crate::llm::Usage;

#[derive(Clone)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_create_per_mtok: f64,
}

fn pricing(model: &str) -> ModelPricing {
    match model {
        "claude-sonnet-4-20250514" => ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.30,
            cache_create_per_mtok: 3.75,
        },
        "claude-sonnet-4" => ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.30,
            cache_create_per_mtok: 3.75,
        },
        "claude-4-sonnet" => ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.30,
            cache_create_per_mtok: 3.75,
        },
        "gpt-4o" | "gpt-4o-2024-11-20" => ModelPricing {
            input_per_mtok: 2.50,
            output_per_mtok: 10.0,
            cache_read_per_mtok: 1.25,
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
