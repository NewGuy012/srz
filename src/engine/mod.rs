pub mod budget;
pub mod chunker;
pub mod compressor;
pub mod llm;
pub mod prompts;
pub mod sse;

pub use budget::{DynamicBudget, ModelInfo, ServerCapabilities, clamp_context_window};
pub use chunker::{TextChunker, TokenizerKind};
pub use compressor::{
    compress_document_stratified, compute_bm25_scores, select_relevant_context_bm25,
};
pub use llm::LlmClient;
pub use prompts::{escape_closing_tag, format_untrusted_section};
pub use sse::{LlmOutput, RequestMessage, StreamMode};

#[cfg(test)]
mod tests {
    use super::LlmClient;

    #[test]
    fn test_parse_error_body_multibyte_utf8_and_ansi_safety() {
        // Multi-byte UTF-8 character straddling byte index 300 must not panic
        let raw = format!("{}🦀{}", "a".repeat(299), "b".repeat(20));
        let parsed = LlmClient::parse_error_body(&raw).expect("non-empty body");
        assert!(parsed.ends_with("..."));
        assert!(parsed.is_ascii());
        assert_eq!(parsed.len(), 302); // 299 'a's + "..."

        // Exact boundary where 2-byte UTF-8 finishes at byte 300
        let raw_exact = format!("{}é{}", "a".repeat(298), "b".repeat(20));
        let parsed_exact = LlmClient::parse_error_body(&raw_exact).expect("non-empty body");
        assert_eq!(parsed_exact, format!("{}é...", "a".repeat(298)));

        // Raw terminal escape sequences in upstream error response must be stripped
        let escaped = "\x1b]2;pwned\x07\x1b[31mGateway timeout\x1b[0m";
        let sanitized = LlmClient::parse_error_body(escaped).expect("non-empty body");
        assert!(!sanitized.contains('\x1b'));
        assert!(!sanitized.contains('\x07'));
        assert_eq!(sanitized, "Gateway timeout");
    }
}
