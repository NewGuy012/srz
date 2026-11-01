use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerCapabilities {
    pub endpoint: String,
    pub model_id: String,
    pub n_ctx: usize,
    pub total_slots: usize,
    pub supports_tokenize: bool,
    pub tokenize_endpoint: Option<String>,
    pub backend_kind: String,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model_id: String::new(),
            n_ctx: 16_384,
            total_slots: 1,
            supports_tokenize: false,
            tokenize_endpoint: None,
            backend_kind: "default".to_string(),
        }
    }
}

/// Sizing parameters dynamically derived from the active LLM server context window and slot capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicBudget {
    /// Total context window in tokens (n_ctx)
    pub n_ctx: usize,
    /// Maximum tokens the model is instructed to generate in completion
    pub max_completion: u32,
    /// Parallel map-reduce worker count (autoscaled to total_slots unless CLI overrides)
    pub concurrency: usize,
    /// Maximum input tokens allowable in a direct single-pass summarization
    pub single_pass_threshold: usize,
    /// Maximum tokens per chunk when map-reducing long documents
    pub chunk_tokens: usize,
    /// Token overlap between consecutive chunks
    pub chunk_overlap: usize,
    /// Grounding document context budget for grounded Q&A (srz ask)
    pub ask_doc_budget: usize,
    /// Sliding conversational history budget for multi-turn Q&A (srz ask)
    pub ask_hist_budget: usize,
}

impl DynamicBudget {
    /// Derives dynamic token sizing and concurrency limits from the detected server context and slots.
    pub fn new(
        n_ctx: usize,
        total_slots: usize,
        cli_max_tokens: Option<u32>,
        cli_concurrency: Option<usize>,
    ) -> Self {
        let n_ctx = n_ctx.max(2048);
        let max_completion = match cli_max_tokens {
            Some(m) if m > 0 => m.min((n_ctx / 3).max(512) as u32),
            _ => (n_ctx / 4).clamp(1024, 8192) as u32,
        };
        let concurrency = cli_concurrency.unwrap_or(total_slots).max(1);
        let system_margin = 500;
        let cushion = (n_ctx / 20).clamp(200, 1600); // 5% headroom
        let usable = n_ctx.saturating_sub(max_completion as usize + system_margin + cushion);
        let single_pass_threshold = usable;
        let chunk_tokens = (usable * 7 / 10).max(1500);
        let chunk_overlap = (chunk_tokens / 15).clamp(100, 1000);
        let ask_doc_budget = usable / 2;
        let ask_hist_budget = (usable * 4) / 10;

        Self {
            n_ctx,
            max_completion,
            concurrency,
            single_pass_threshold,
            chunk_tokens,
            chunk_overlap,
            ask_doc_budget,
            ask_hist_budget,
        }
    }

    /// Default baseline budget assuming a standard 32k context window and 1 slot.
    pub fn default_32k() -> Self {
        Self::new(32_768, 1, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dynamic_budget_small_context_4k() {
        let budget = DynamicBudget::new(4096, 1, None, None);
        assert_eq!(budget.n_ctx, 4096);
        assert_eq!(budget.max_completion, 1024);
        assert_eq!(budget.concurrency, 1);
        assert!(budget.single_pass_threshold <= 2500);
        assert!(budget.chunk_tokens <= 2000);
        assert!(budget.chunk_tokens + budget.max_completion as usize <= 4096);
    }

    #[test]
    fn test_dynamic_budget_standard_32k_with_multi_slots() {
        let budget = DynamicBudget::new(32_768, 4, None, None);
        assert_eq!(budget.n_ctx, 32_768);
        assert_eq!(budget.max_completion, 8192);
        assert_eq!(budget.concurrency, 4); // Autoscaled to 4 slots!
        assert!(budget.single_pass_threshold > 20_000);
        assert!(budget.chunk_tokens >= 14_000);
    }

    #[test]
    fn test_dynamic_budget_large_128k() {
        let budget = DynamicBudget::new(131_072, 8, None, None);
        assert_eq!(budget.n_ctx, 131_072);
        assert_eq!(budget.max_completion, 8192);
        assert_eq!(budget.concurrency, 8);
        assert!(budget.single_pass_threshold > 100_000);
    }

    #[test]
    fn test_dynamic_budget_cli_overrides() {
        let budget = DynamicBudget::new(32_768, 4, Some(2048), Some(2));
        assert_eq!(budget.max_completion, 2048);
        assert_eq!(budget.concurrency, 2);
    }
}
