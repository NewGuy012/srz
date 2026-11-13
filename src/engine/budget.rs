use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerCapabilities {
    pub endpoint: String,
    pub model_id: String,
    pub n_ctx: usize,
    pub backend_kind: String,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model_id: String::new(),
            n_ctx: 16_384,
            backend_kind: "default".to_string(),
        }
    }
}

/// Minimum context window size in tokens.
pub const MIN_CONTEXT_WINDOW: usize = 2_048;

/// Maximum context window size in tokens (2M tokens ceiling to prevent arithmetic overflow and OOM).
pub const MAX_CONTEXT_WINDOW: usize = 2_097_152;

/// Sizing parameters dynamically derived from the active LLM server context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicBudget {
    /// Total context window in tokens (n_ctx)
    pub n_ctx: usize,
    /// Maximum tokens the model is instructed to generate in completion
    pub max_completion: u32,
    /// Maximum input tokens allowable in a direct single-pass summarization
    pub single_pass_threshold: usize,
    /// Grounding document context budget for grounded Q&A (srz ask)
    pub ask_doc_budget: usize,
    /// Sliding conversational history budget for multi-turn Q&A (srz ask)
    pub ask_hist_budget: usize,
}

impl DynamicBudget {
    /// Derives dynamic token sizing limits from the detected server context window.
    pub fn new(n_ctx: usize, cli_max_tokens: Option<u32>) -> Self {
        let n_ctx = n_ctx.clamp(MIN_CONTEXT_WINDOW, MAX_CONTEXT_WINDOW);
        let max_completion_cap = (n_ctx / 3).clamp(512, u32::MAX as usize) as u32;
        let max_completion = match cli_max_tokens {
            Some(m) if m > 0 => m.min(max_completion_cap),
            _ => (n_ctx / 4).clamp(1024, 8192) as u32,
        };
        let system_margin = 500usize;
        let cushion = (n_ctx / 20).clamp(200, 1600); // 5% headroom
        let overhead = (max_completion as usize)
            .saturating_add(system_margin)
            .saturating_add(cushion);
        let usable = n_ctx.saturating_sub(overhead);
        let single_pass_threshold = usable;
        let ask_doc_budget = usable / 2;
        let ask_hist_budget = usable.saturating_mul(4) / 10;

        Self {
            n_ctx,
            max_completion,
            single_pass_threshold,
            ask_doc_budget,
            ask_hist_budget,
        }
    }
}

/// Clamps a context window size to safe boundaries `[MIN_CONTEXT_WINDOW, MAX_CONTEXT_WINDOW]`.
#[inline]
pub fn clamp_context_window(n: usize) -> usize {
    n.clamp(MIN_CONTEXT_WINDOW, MAX_CONTEXT_WINDOW)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ModelInfoMeta {
    #[serde(default)]
    pub n_ctx: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub owned_by: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub meta: Option<ModelInfoMeta>,
    #[serde(default)]
    pub context_length: Option<usize>,
    #[serde(default)]
    pub max_model_len: Option<usize>,
}

impl ModelInfo {
    /// Discovers the active context window size in tokens, prioritizing runtime context (`meta.n_ctx`)
    /// followed by context limits (`context_length`, `max_model_len`).
    pub fn context_window(&self) -> Option<usize> {
        self.meta
            .as_ref()
            .and_then(|m| m.n_ctx)
            .filter(|&c| c > 0)
            .or(self.context_length.filter(|&c| c > 0))
            .or(self.max_model_len.filter(|&c| c > 0))
    }

    /// Checks if this model entry matches the target model name by ID or alias.
    pub fn matches_model_id(&self, target: &str) -> bool {
        self.id == target || self.aliases.iter().any(|a| a == target)
    }
}

#[derive(Deserialize)]
pub(crate) struct ModelsListResponse {
    #[serde(default)]
    pub data: Vec<ModelInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_info_deserialization_and_context_discovery() {
        // llama.cpp / Llama.app format with meta and aliases
        let llamacpp_json = r#"{
            "id": "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
            "aliases": ["gemma-4-E4B-it-Q8_0"],
            "owned_by": "llamacpp",
            "context_length": 131072,
            "meta": {
                "n_ctx": 65536,
                "n_ctx_train": 131072
            }
        }"#;
        let model: ModelInfo = serde_json::from_str(llamacpp_json).expect("failed to parse");
        assert_eq!(model.id, "ggml-org/gemma-4-E4B-it-GGUF:Q8_0");
        assert_eq!(model.context_window(), Some(65536));
        assert!(model.matches_model_id("ggml-org/gemma-4-E4B-it-GGUF:Q8_0"));
        assert!(model.matches_model_id("gemma-4-E4B-it-Q8_0"));
        assert!(!model.matches_model_id("llama-3"));

        // vLLM format with max_model_len
        let vllm_json = r#"{
            "id": "meta-llama/Llama-3-8B-Instruct",
            "owned_by": "vllm",
            "max_model_len": 8192
        }"#;
        let vllm_model: ModelInfo = serde_json::from_str(vllm_json).expect("failed to parse");
        assert_eq!(vllm_model.context_window(), Some(8192));
        assert!(vllm_model.matches_model_id("meta-llama/Llama-3-8B-Instruct"));

        // Minimal format (no context info provided)
        let minimal_json = r#"{"id": "generic-model"}"#;
        let minimal_model: ModelInfo = serde_json::from_str(minimal_json).expect("failed to parse");
        assert_eq!(minimal_model.context_window(), None);
    }

    #[test]
    fn test_dynamic_budget_small_context_4k() {
        let budget = DynamicBudget::new(4096, None);
        assert_eq!(budget.n_ctx, 4096);
        assert_eq!(budget.max_completion, 1024);
        assert!(budget.single_pass_threshold <= 2500);
    }

    #[test]
    fn test_dynamic_budget_standard_32k() {
        let budget = DynamicBudget::new(32_768, None);
        assert_eq!(budget.n_ctx, 32_768);
        assert_eq!(budget.max_completion, 8192);
        assert!(budget.single_pass_threshold > 20_000);
    }

    #[test]
    fn test_dynamic_budget_large_128k() {
        let budget = DynamicBudget::new(131_072, None);
        assert_eq!(budget.n_ctx, 131_072);
        assert_eq!(budget.max_completion, 8192);
        assert!(budget.single_pass_threshold > 100_000);
    }

    #[test]
    fn test_dynamic_budget_cli_overrides() {
        let budget = DynamicBudget::new(32_768, Some(2048));
        assert_eq!(budget.max_completion, 2048);
    }

    #[test]
    fn test_dynamic_budget_clamping_and_overflow_safety() {
        // Extreme values from rogue server or misconfigured CLI: usize::MAX should clamp without panic
        let budget = DynamicBudget::new(usize::MAX, Some(u32::MAX));
        assert_eq!(budget.n_ctx, MAX_CONTEXT_WINDOW);
        assert!(budget.max_completion <= (MAX_CONTEXT_WINDOW / 3) as u32);
        assert!(budget.single_pass_threshold < MAX_CONTEXT_WINDOW);

        // Lower bound clamping
        let small_budget = DynamicBudget::new(10, None);
        assert_eq!(small_budget.n_ctx, MIN_CONTEXT_WINDOW);
    }
}
