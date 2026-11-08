use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ModelInfoMeta {
    #[serde(default)]
    pub n_ctx: Option<usize>,
    #[serde(default)]
    pub n_ctx_train: Option<usize>,
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

/// Clamps a context window size to safe boundaries `[MIN_CONTEXT_WINDOW, MAX_CONTEXT_WINDOW]`.
#[inline]
pub fn clamp_context_window(n: usize) -> usize {
    n.clamp(
        crate::budget::MIN_CONTEXT_WINDOW,
        crate::budget::MAX_CONTEXT_WINDOW,
    )
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
}
