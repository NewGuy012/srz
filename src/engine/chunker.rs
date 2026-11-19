use std::borrow::Cow;

#[inline]
fn estimate_tokens(text: &str) -> usize {
    let words = text.split_ascii_whitespace().count();
    if words == 0 {
        return 0;
    }
    let word_est = words.saturating_mul(4).div_ceil(3);
    if !text.is_ascii() {
        word_est.max(text.len().div_ceil(3))
    } else if text.len() > words.saturating_mul(16) {
        word_est.max(text.len().div_ceil(4))
    } else {
        word_est
    }
}

/// Tokenizer profile for model-aware in-process token counting and chunking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenizerKind {
    #[default]
    Cl100k,
    Llama3,
    Gemma,
}

impl TokenizerKind {
    pub fn detect_from_model(model: &str) -> Self {
        let lower = model.to_ascii_lowercase();
        if lower.contains("gemma") {
            Self::Gemma
        } else if lower.contains("llama") {
            Self::Llama3
        } else {
            Self::Cl100k
        }
    }

    const fn scale(self) -> usize {
        match self {
            Self::Cl100k => 100,
            Self::Llama3 => 112,
            Self::Gemma => 118,
        }
    }

    pub fn count_tokens(&self, text: &str) -> usize {
        let base = estimate_tokens(text);
        let s = self.scale();
        if s == 100 {
            base
        } else {
            base.saturating_mul(s).div_ceil(100)
        }
    }
}

pub(crate) struct TextChunker;

impl TextChunker {
    /// Truncates `text` to at most `max_tokens` under `kind`, returning `""` if even 1 word exceeds it.
    pub(crate) fn truncate_to_budget(text: &str, max_tokens: usize, kind: TokenizerKind) -> &str {
        if max_tokens == 0 {
            return "";
        }
        if text.len().saturating_mul(3) <= max_tokens || kind.count_tokens(text) <= max_tokens {
            return text;
        }
        let base_budget = max_tokens.saturating_mul(100) / kind.scale();
        let max_words = base_budget.saturating_mul(3) / 4;
        if max_words == 0 {
            return "";
        }

        let base_ptr = text.as_ptr() as usize;
        let max_bytes = base_budget.saturating_mul(4);
        let mut end = 0usize;
        for (i, word) in text.split_whitespace().take(max_words).enumerate() {
            let word_end = (word.as_ptr() as usize) - base_ptr + word.len();
            if word_end > (i + 1).saturating_mul(16) && word_end > max_bytes {
                break;
            }
            end = word_end;
        }

        if end == 0 {
            end = text.floor_char_boundary(max_bytes);
        }
        end = text[..end].trim_end().len();
        while end > 0 && kind.count_tokens(&text[..end]) > max_tokens {
            end = match text[..end].rfind(char::is_whitespace) {
                Some(w) => text[..w].trim_end().len(),
                None => text.floor_char_boundary(end - 1),
            };
        }
        text[..end].trim_end()
    }

    /// Splits text into chunks of roughly `max_tokens` with `overlap_tokens` overlap using `kind`.
    pub(crate) fn chunk_by_tokens_with_counts_and_kind<'a>(
        text: &'a str,
        max_tokens: usize,
        overlap_tokens: usize,
        kind: TokenizerKind,
    ) -> Vec<(Cow<'a, str>, usize)> {
        if text.len().saturating_mul(3) <= max_tokens {
            return vec![(Cow::Borrowed(text), kind.count_tokens(text))];
        }
        let mut raw_paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut total_count = 0usize;

        for p in text.split("\n\n") {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                let count = kind.count_tokens(trimmed);
                total_count = total_count.saturating_add(count);
                raw_paragraphs.push((trimmed, count));
            }
        }

        if raw_paragraphs.is_empty() || total_count <= max_tokens {
            return vec![(Cow::Borrowed(text), total_count)];
        }

        let segments = Self::decompose_text_segments(raw_paragraphs, max_tokens, kind);
        if segments.is_empty() {
            return vec![(Cow::Borrowed(text), 0)];
        }

        let mut chunks: Vec<(Cow<'a, str>, usize)> = Vec::new();
        let mut current_chunk_segments: Vec<(&'a str, usize)> = Vec::new();
        let mut current_count = 0usize;

        for &(seg, s_count) in &segments {
            if current_count.saturating_add(s_count) > max_tokens
                && !current_chunk_segments.is_empty()
            {
                let chunk = Self::build_chunk(text, &current_chunk_segments);
                chunks.push((chunk, current_count));

                let mut retained = Vec::new();
                let mut retained_count = 0usize;
                for &(prev_s, c) in current_chunk_segments.iter().rev() {
                    if retained_count.saturating_add(c) <= overlap_tokens {
                        retained.push((prev_s, c));
                        retained_count = retained_count.saturating_add(c);
                    } else {
                        break;
                    }
                }
                retained.reverse();

                current_chunk_segments = retained;
                current_count = retained_count;
            }

            current_chunk_segments.push((seg, s_count));
            current_count = current_count.saturating_add(s_count);
        }

        if !current_chunk_segments.is_empty() {
            let chunk = Self::build_chunk(text, &current_chunk_segments);
            chunks.push((chunk, current_count));
        }

        chunks
    }

    fn decompose_text_segments(
        raw_paragraphs: Vec<(&str, usize)>,
        max_limit: usize,
        kind: TokenizerKind,
    ) -> Vec<(&str, usize)> {
        let is_single_block = raw_paragraphs.len() <= 1;
        let para_threshold = if is_single_block {
            max_limit
        } else {
            max_limit.saturating_mul(18) / 10
        };

        let mut segments = Vec::new();
        for (para, count) in raw_paragraphs {
            if count <= para_threshold && !is_single_block {
                segments.push((para, count));
            } else {
                Self::decompose_paragraph(para, max_limit, kind, &mut segments);
            }
        }
        segments
    }

    fn decompose_paragraph<'a>(
        para: &'a str,
        max_limit: usize,
        kind: TokenizerKind,
        out: &mut Vec<(&'a str, usize)>,
    ) {
        for line in para.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let count = kind.count_tokens(trimmed);
            if count <= max_limit {
                out.push((trimmed, count));
            } else {
                Self::decompose_line(trimmed, max_limit, kind, out);
            }
        }
    }

    pub(crate) fn decompose_line<'a>(
        mut remaining: &'a str,
        max_limit: usize,
        kind: TokenizerKind,
        out: &mut Vec<(&'a str, usize)>,
    ) {
        while !remaining.is_empty() {
            let r_count = kind.count_tokens(remaining);
            if r_count <= max_limit {
                out.push((remaining, r_count));
                break;
            }

            let window = Self::truncate_to_budget(remaining, max_limit, kind);
            let split_at = if window.is_empty() {
                remaining
                    .chars()
                    .next()
                    .map_or(remaining.len(), char::len_utf8)
            } else {
                window
                    .char_indices()
                    .rev()
                    .find_map(|(i, c)| {
                        let end = i + c.len_utf8();
                        (matches!(c, '.' | '?' | '!' | ';')
                            && remaining[end..].starts_with(char::is_whitespace)
                            && end >= window.len() / 2)
                            .then_some(end)
                    })
                    .unwrap_or(window.len())
            };

            let (head, tail) = remaining.split_at(split_at);
            let head_trimmed = head.trim();
            if !head_trimmed.is_empty() {
                out.push((head_trimmed, kind.count_tokens(head_trimmed)));
            }
            remaining = tail.trim_start();
        }
    }

    fn build_chunk<'a>(text: &'a str, paras: &[(&'a str, usize)]) -> Cow<'a, str> {
        let Some((first, _)) = paras.first() else {
            return Cow::Borrowed("");
        };
        let (last, _) = paras.last().unwrap();
        let text_start = text.as_ptr() as usize;
        let start = (first.as_ptr() as usize).saturating_sub(text_start);
        let end = (last.as_ptr() as usize).saturating_sub(text_start) + last.len();
        Cow::Borrowed(&text[start..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_kind_detection_cjk_and_truncation() {
        assert_eq!(
            TokenizerKind::detect_from_model("google/gemma-4-27b-it"),
            TokenizerKind::Gemma
        );
        assert_eq!(
            TokenizerKind::detect_from_model("meta-llama/Llama-3.1-8B"),
            TokenizerKind::Llama3
        );
        assert_eq!(
            TokenizerKind::detect_from_model("demo1"),
            TokenizerKind::Cl100k
        );

        let sample =
            "This is a sample document for token estimation across distinct architectures.";
        let cl = TokenizerKind::Cl100k.count_tokens(sample);
        assert!((6..=20).contains(&cl));
        assert!(TokenizerKind::Gemma.count_tokens(sample) >= cl);
        assert!(TokenizerKind::Llama3.count_tokens(sample) >= cl);

        // CJK spaced text must reflect byte density instead of just word count
        let cjk = "你好世界测试 ".repeat(20);
        assert!(TokenizerKind::Cl100k.count_tokens(&cjk) >= 100);

        let unbroken = "A".repeat(1000);
        assert!(
            TokenizerKind::Gemma.count_tokens(TextChunker::truncate_to_budget(
                &unbroken,
                20,
                TokenizerKind::Gemma
            )) <= 20
        );

        let ws_gap = format!("word1{}word2", " ".repeat(5_000));
        assert!(
            TokenizerKind::Cl100k.count_tokens(TextChunker::truncate_to_budget(
                &ws_gap,
                2,
                TokenizerKind::Cl100k
            )) <= 2
        );
    }

    #[test]
    fn test_chunk_splitting_modes_and_unbroken_text() {
        let short = "First paragraph.\n\nSecond paragraph.";
        let under = TextChunker::chunk_by_tokens_with_counts_and_kind(
            short,
            100,
            10,
            TokenizerKind::Cl100k,
        );
        assert_eq!(under.len(), 1);
        assert!(matches!(under[0].0, Cow::Borrowed(_)));

        let p1 = "Paragraph one repeated words here. ".repeat(20);
        let p2 = "Paragraph two repeated words here. ".repeat(20);
        let two_para = format!("{}\n\n{}", p1.trim(), p2.trim());
        let split = TextChunker::chunk_by_tokens_with_counts_and_kind(
            &two_para,
            120,
            20,
            TokenizerKind::Cl100k,
        );
        assert_eq!(split.len(), 2);
        for (c, cnt) in &split {
            assert_eq!(*cnt, TokenizerKind::Cl100k.count_tokens(c));
        }

        let monolithic =
            "Detailed system architecture analysis sentence without blank lines. ".repeat(40);
        let mono_chunks = TextChunker::chunk_by_tokens_with_counts_and_kind(
            &monolithic,
            100,
            15,
            TokenizerKind::Cl100k,
        );
        assert!(mono_chunks.len() >= 4);
        assert!(mono_chunks.iter().all(|(_, c)| *c <= 125));

        let giant = "A".repeat(1000);
        let giant_chunks =
            TextChunker::chunk_by_tokens_with_counts_and_kind(&giant, 50, 5, TokenizerKind::Cl100k);
        assert!(giant_chunks.len() >= 2);
    }
}
