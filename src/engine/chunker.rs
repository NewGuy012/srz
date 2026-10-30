use std::borrow::Cow;
use std::sync::LazyLock;
use tiktoken_rs::CoreBPE;

static BPE: LazyLock<CoreBPE> = LazyLock::new(|| {
    tiktoken_rs::cl100k_base().expect("Failed to initialize cl100k_base tokenizer")
});

pub struct TextChunker;

impl TextChunker {
    /// Counts tokens in `text` using the standard OpenAI cl100k_base BPE tokenizer.
    pub fn count_tokens(text: &str) -> usize {
        BPE.encode_with_special_tokens(text).len()
    }

    /// Splits text into chunks of roughly `max_tokens` with `overlap_tokens` overlap,
    /// using accurate BPE token counts while preserving paragraph boundaries and zero-copy slices.
    pub fn chunk_by_tokens<'a>(
        text: &'a str,
        max_tokens: usize,
        overlap_tokens: usize,
    ) -> Vec<Cow<'a, str>> {
        // Fast path: In BPE, 1 token is at least 1 byte.
        // If text.len() <= max_tokens, the token count cannot exceed max_tokens.
        if text.len() <= max_tokens {
            return vec![Cow::Borrowed(text)];
        }
        Self::chunk_with_metric(text, max_tokens, overlap_tokens, Self::count_tokens)
    }

    /// Splits text into chunks of roughly `max_words` with `overlap_words` overlap.
    /// Preserves paragraph boundaries wherever possible and returns zero-copy `Cow::Borrowed`
    /// slices whenever the input or a chunk does not require concatenation.
    pub fn chunk<'a>(text: &'a str, max_words: usize, overlap_words: usize) -> Vec<Cow<'a, str>> {
        Self::chunk_with_metric(text, max_words, overlap_words, |s| {
            s.split_whitespace().count()
        })
    }

    fn chunk_with_metric<'a, F>(
        text: &'a str,
        max_limit: usize,
        overlap_limit: usize,
        mut count_fn: F,
    ) -> Vec<Cow<'a, str>>
    where
        F: FnMut(&str) -> usize,
    {
        let mut paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut total_count = 0;

        for p in text.split("\n\n") {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                let count = count_fn(trimmed);
                total_count += count;
                paragraphs.push((trimmed, count));
            }
        }

        if paragraphs.is_empty() || total_count <= max_limit {
            return vec![Cow::Borrowed(text)];
        }

        let mut chunks: Vec<Cow<'a, str>> = Vec::new();
        let mut current_chunk_paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut current_count = 0;

        for &(para, p_count) in &paragraphs {
            if current_count + p_count > max_limit && !current_chunk_paragraphs.is_empty() {
                chunks.push(Self::build_chunk(text, &current_chunk_paragraphs));

                let mut retained = Vec::new();
                let mut retained_count = 0;
                for &(prev_p, c) in current_chunk_paragraphs.iter().rev() {
                    if retained_count + c <= overlap_limit {
                        retained.push((prev_p, c));
                        retained_count += c;
                    } else {
                        break;
                    }
                }
                retained.reverse();

                current_chunk_paragraphs = retained;
                current_count = retained_count;
            }

            current_chunk_paragraphs.push((para, p_count));
            current_count += p_count;
        }

        if !current_chunk_paragraphs.is_empty() {
            chunks.push(Self::build_chunk(text, &current_chunk_paragraphs));
        }

        chunks
    }

    fn build_chunk<'a>(text: &'a str, paras: &[(&'a str, usize)]) -> Cow<'a, str> {
        let (first, _) = match paras.first() {
            Some(p) => p,
            None => return Cow::Borrowed(""),
        };
        let (last, _) = paras.last().unwrap();
        let text_start = text.as_ptr() as usize;
        let first_start = first.as_ptr() as usize;
        let last_start = last.as_ptr() as usize;
        let start = first_start.saturating_sub(text_start);
        let end = last_start.saturating_sub(text_start) + last.len();

        if start <= end && end <= text.len() {
            Cow::Borrowed(&text[start..end])
        } else {
            let total_len: usize = paras.iter().map(|(p, _)| p.len()).sum::<usize>()
                + paras.len().saturating_sub(1) * 2;
            let mut out = String::with_capacity(total_len);
            for (i, (p, _)) in paras.iter().enumerate() {
                if i > 0 {
                    out.push_str("\n\n");
                }
                out.push_str(p);
            }
            Cow::Owned(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens() {
        let text = "Hello, world! This is a test.";
        let tokens = TextChunker::count_tokens(text);
        assert!((6..=10).contains(&tokens));
    }

    #[test]
    fn test_chunk_by_tokens_under_budget() {
        let text = "First paragraph.\n\nSecond paragraph.";
        let chunks = TextChunker::chunk_by_tokens(text, 100, 10);
        assert_eq!(chunks.len(), 1);
        assert!(matches!(chunks[0], Cow::Borrowed(_)));
    }

    #[test]
    fn test_chunk_by_tokens_splits_on_budget() {
        let p1 = "This is a paragraph with several words repeated. ".repeat(20);
        let p2 = "Another different paragraph with several words repeated. ".repeat(20);
        let text = format!("{}\n\n{}", p1.trim(), p2.trim());

        let chunks = TextChunker::chunk_by_tokens(&text, 120, 20);
        assert_eq!(chunks.len(), 2);
        assert!(matches!(chunks[0], Cow::Borrowed(_)));
        assert!(matches!(chunks[1], Cow::Borrowed(_)));
    }
}
