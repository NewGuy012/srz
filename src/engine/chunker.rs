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
        let mut paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut total_tokens = 0;

        for p in text.split("\n\n") {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                let tokens = Self::count_tokens(trimmed);
                total_tokens += tokens;
                paragraphs.push((trimmed, tokens));
            }
        }

        if paragraphs.is_empty() || total_tokens <= max_tokens {
            return vec![Cow::Borrowed(text)];
        }

        let mut chunks: Vec<Cow<'a, str>> = Vec::new();
        let mut current_chunk_paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut current_token_count = 0;

        for &(para, p_tokens) in &paragraphs {
            if current_token_count + p_tokens > max_tokens && !current_chunk_paragraphs.is_empty() {
                chunks.push(Self::build_chunk(text, &current_chunk_paragraphs));

                let mut retained = Vec::new();
                let mut retained_tokens = 0;
                for &(prev_p, t) in current_chunk_paragraphs.iter().rev() {
                    if retained_tokens + t <= overlap_tokens {
                        retained.push((prev_p, t));
                        retained_tokens += t;
                    } else {
                        break;
                    }
                }
                retained.reverse();

                current_chunk_paragraphs = retained;
                current_token_count = retained_tokens;
            }

            current_chunk_paragraphs.push((para, p_tokens));
            current_token_count += p_tokens;
        }

        if !current_chunk_paragraphs.is_empty() {
            chunks.push(Self::build_chunk(text, &current_chunk_paragraphs));
        }

        chunks
    }

    /// Splits text into chunks of roughly `max_words` with `overlap_words` overlap.
    /// Preserves paragraph boundaries wherever possible and returns zero-copy `Cow::Borrowed`
    /// slices whenever the input or a chunk does not require concatenation.
    pub fn chunk<'a>(text: &'a str, max_words: usize, overlap_words: usize) -> Vec<Cow<'a, str>> {
        let mut paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut total_words = 0;

        for p in text.split("\n\n") {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                let words = trimmed.split_whitespace().count();
                total_words += words;
                paragraphs.push((trimmed, words));
            }
        }

        if paragraphs.is_empty() || total_words <= max_words {
            return vec![Cow::Borrowed(text)];
        }

        let mut chunks: Vec<Cow<'a, str>> = Vec::new();
        let mut current_chunk_paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut current_word_count = 0;

        for &(para, p_words) in &paragraphs {
            if current_word_count + p_words > max_words && !current_chunk_paragraphs.is_empty() {
                chunks.push(Self::build_chunk(text, &current_chunk_paragraphs));

                // Calculate overlap paragraphs to retain using pre-computed word counts
                let mut retained = Vec::new();
                let mut retained_words = 0;
                for &(prev_p, w) in current_chunk_paragraphs.iter().rev() {
                    if retained_words + w <= overlap_words {
                        retained.push((prev_p, w));
                        retained_words += w;
                    } else {
                        break;
                    }
                }
                retained.reverse();

                current_chunk_paragraphs = retained;
                current_word_count = retained_words;
            }

            current_chunk_paragraphs.push((para, p_words));
            current_word_count += p_words;
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
        assert!(tokens >= 6 && tokens <= 10);
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
