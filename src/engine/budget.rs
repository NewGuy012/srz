use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerCapabilities {
    pub endpoint: String,
    pub model_id: String,
    pub n_ctx: usize,
    pub backend_kind: String,
}

const MIN_CONTEXT_WINDOW: usize = 2_048;
const MAX_CONTEXT_WINDOW: usize = 2_097_152;

/// Sizing parameters derived from the active LLM server context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DynamicBudget {
    pub n_ctx: usize,
    pub max_completion: u32,
    pub single_pass_threshold: usize,
    pub ask_doc_budget: usize,
    pub ask_hist_budget: usize,
}

impl DynamicBudget {
    pub(crate) fn new(n_ctx: usize, cli_max_tokens: Option<u32>) -> Self {
        let n_ctx = clamp_context_window(n_ctx);
        let max_completion_cap = (n_ctx / 3) as u32;
        let max_completion = match cli_max_tokens {
            Some(m) if m > 0 => m.min(max_completion_cap),
            _ => ((n_ctx / 4).clamp(1024, 8192) as u32).min(max_completion_cap),
        };
        let system_margin = 500usize;
        let cushion = (n_ctx / 20).clamp(200, 1600);
        let overhead = (max_completion as usize)
            .saturating_add(system_margin)
            .saturating_add(cushion);
        let usable = n_ctx.saturating_sub(overhead);

        Self {
            n_ctx,
            max_completion,
            single_pass_threshold: usable,
            ask_doc_budget: usable / 2,
            ask_hist_budget: usable.saturating_mul(4) / 10,
        }
    }
}

#[inline]
pub(crate) fn clamp_context_window(n: usize) -> usize {
    n.clamp(MIN_CONTEXT_WINDOW, MAX_CONTEXT_WINDOW)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct ModelInfoMeta {
    #[serde(default)]
    n_ctx: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub owned_by: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    meta: Option<ModelInfoMeta>,
    #[serde(default)]
    context_length: Option<usize>,
    #[serde(default)]
    max_model_len: Option<usize>,
}

impl ModelInfo {
    /// Returns the discovered context window (runtime `meta.n_ctx` > `context_length` > `max_model_len`).
    pub(crate) fn context_window(&self) -> Option<usize> {
        self.meta
            .as_ref()
            .and_then(|m| m.n_ctx)
            .filter(|&c| c > 0)
            .or(self.context_length.filter(|&c| c > 0))
            .or(self.max_model_len.filter(|&c| c > 0))
    }

    pub(crate) fn matches_model_id(&self, target: &str) -> bool {
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
        let llamacpp_json = r#"{"id":"ggml-org/gemma-4-E4B-it-GGUF:Q8_0","aliases":["gemma-4-E4B-it-Q8_0"],"context_length":131072,"meta":{"n_ctx":65536}}"#;
        let model: ModelInfo = serde_json::from_str(llamacpp_json).unwrap();
        assert_eq!(model.context_window(), Some(65536));
        assert!(model.matches_model_id("gemma-4-E4B-it-Q8_0"));
        assert!(!model.matches_model_id("llama-3"));

        let vllm_model: ModelInfo =
            serde_json::from_str(r#"{"id":"Llama-3-8B","max_model_len":8192}"#).unwrap();
        assert_eq!(vllm_model.context_window(), Some(8192));

        let minimal: ModelInfo = serde_json::from_str(r#"{"id":"generic"}"#).unwrap();
        assert_eq!(minimal.context_window(), None);
    }

    #[test]
    fn test_dynamic_budget_sizing_and_clamping() {
        for (input_ctx, cli_max, expected_ctx, expected_comp) in [
            (10, None, MIN_CONTEXT_WINDOW, 682),
            (4096, None, 4096, 1024),
            (32_768, None, 32_768, 8192),
            (131_072, None, 131_072, 8192),
            (32_768, Some(2048), 32_768, 2048),
            (
                usize::MAX,
                Some(u32::MAX),
                MAX_CONTEXT_WINDOW,
                (MAX_CONTEXT_WINDOW / 3) as u32,
            ),
        ] {
            let b = DynamicBudget::new(input_ctx, cli_max);
            assert_eq!(b.n_ctx, expected_ctx);
            assert_eq!(b.max_completion, expected_comp);
            assert!(b.single_pass_threshold < expected_ctx);
        }
    }
}
