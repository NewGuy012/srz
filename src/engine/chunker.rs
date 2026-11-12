use std::borrow::Cow;

/// Estimates token count using a fast, zero-dependency word-to-token heuristic:
/// 1 word is approximately 1.33 (4/3) tokens in English and code, with a byte fallback for unbroken tokens.
#[inline]
pub fn estimate_tokens(text: &str) -> usize {
    let words = text.split_ascii_whitespace().count();
    if words == 0 {
        return 0;
    }
    let word_estimate = (words * 4).div_ceil(3);
    if text.len() > words * 16 {
        word_estimate.max(text.len().div_ceil(4))
    } else {
        word_estimate
    }
}

/// Tokenizer profile for model-aware in-process token counting and chunking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenizerKind {
    /// OpenAI GPT-4 / GPT-3.5 standard cl100k_base profile.
    #[default]
    Cl100k,
    /// OpenAI GPT-4o / o1 / o3 o200k_base profile.
    O200k,
    /// Meta Llama 3 / 3.1 / 3.2 profile.
    Llama3,
    /// Google Gemma 4 / Gemma 2 profile.
    Gemma,
}

impl TokenizerKind {
    /// Detects the optimal tokenization profile from the target model name.
    pub fn detect_from_model(model: &str) -> Self {
        let lower = model.to_ascii_lowercase();
        if lower.contains("gemma") {
            TokenizerKind::Gemma
        } else if lower.contains("llama") {
            TokenizerKind::Llama3
        } else if lower.contains("4o") || lower.contains("o1") || lower.contains("o3") {
            TokenizerKind::O200k
        } else {
            TokenizerKind::Cl100k
        }
    }

    /// Measures token count of `text` according to this tokenizer profile.
    pub fn count_tokens(&self, text: &str) -> usize {
        let base = estimate_tokens(text);
        match self {
            Self::Cl100k | Self::O200k => base,
            Self::Llama3 => base.saturating_mul(112).div_ceil(100),
            Self::Gemma => base.saturating_mul(118).div_ceil(100),
        }
    }
}

pub struct TextChunker;

impl TextChunker {
    /// Counts tokens in `text` using a specified model-aware tokenizer profile.
    pub fn count_tokens_with_kind(text: &str, kind: TokenizerKind) -> usize {
        kind.count_tokens(text)
    }

    /// Truncates `text` to `max_tokens` (estimated) while preserving paragraph boundaries and
    /// returning `Cow::Borrowed` when under budget.
    pub fn truncate_by_tokens(text: &str, max_tokens: usize) -> Cow<'_, str> {
        if max_tokens == 0 {
            return Cow::Borrowed("[Context trimmed for token budget]");
        }
        if text.len() <= max_tokens || estimate_tokens(text) <= max_tokens {
            return Cow::Borrowed(text);
        }
        let max_words = ((max_tokens * 3) / 4).max(1);
        crate::engine::prompts::truncate_words_preserving_lines(text, max_words)
    }

    /// Splits text into chunks of roughly `max_tokens` with `overlap_tokens` overlap using the specified tokenizer kind,
    /// returning each chunk alongside its pre-computed token count to avoid re-tokenization.
    pub fn chunk_by_tokens_with_counts_and_kind<'a>(
        text: &'a str,
        max_tokens: usize,
        overlap_tokens: usize,
        kind: TokenizerKind,
    ) -> Vec<(Cow<'a, str>, usize)> {
        if text.len() <= max_tokens {
            return vec![(Cow::Borrowed(text), kind.count_tokens(text))];
        }
        Self::chunk_with_metric_with_counts(text, max_tokens, overlap_tokens, |s| {
            kind.count_tokens(s)
        })
    }

    fn chunk_with_metric_with_counts<'a, F>(
        text: &'a str,
        max_limit: usize,
        overlap_limit: usize,
        mut count_fn: F,
    ) -> Vec<(Cow<'a, str>, usize)>
    where
        F: FnMut(&str) -> usize,
    {
        let mut raw_paragraphs: Vec<(&'a str, usize)> = Vec::new();
        let mut total_count = 0;

        for p in text.split("\n\n") {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                let count = count_fn(trimmed);
                total_count += count;
                raw_paragraphs.push((trimmed, count));
            }
        }

        if raw_paragraphs.is_empty() || total_count <= max_limit {
            return vec![(Cow::Borrowed(text), total_count)];
        }

        let segments = Self::decompose_text_segments(raw_paragraphs, max_limit, &mut count_fn);
        if segments.is_empty() {
            return vec![(Cow::Borrowed(text), 0)];
        }

        let mut chunks: Vec<(Cow<'a, str>, usize)> = Vec::new();
        let mut current_chunk_segments: Vec<(&'a str, usize)> = Vec::new();
        let mut current_count = 0;

        for &(seg, s_count) in &segments {
            if current_count + s_count > max_limit && !current_chunk_segments.is_empty() {
                let chunk = Self::build_chunk(text, &current_chunk_segments);
                chunks.push((chunk, current_count));

                let mut retained = Vec::new();
                let mut retained_count = 0;
                for &(prev_s, c) in current_chunk_segments.iter().rev() {
                    if retained_count + c <= overlap_limit {
                        retained.push((prev_s, c));
                        retained_count += c;
                    } else {
                        break;
                    }
                }
                retained.reverse();

                current_chunk_segments = retained;
                current_count = retained_count;
            }

            current_chunk_segments.push((seg, s_count));
            current_count += s_count;
        }

        if !current_chunk_segments.is_empty() {
            let chunk = Self::build_chunk(text, &current_chunk_segments);
            chunks.push((chunk, current_count));
        }

        chunks
    }

    fn decompose_text_segments<'a, F>(
        raw_paragraphs: Vec<(&'a str, usize)>,
        max_limit: usize,
        count_fn: &mut F,
    ) -> Vec<(&'a str, usize)>
    where
        F: FnMut(&str) -> usize,
    {
        let is_single_block = raw_paragraphs.len() <= 1;
        // Allow up to 1.8x of max_limit to preserve intact paragraph boundaries when multiple paragraphs exist
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
                Self::decompose_paragraph(para, max_limit, count_fn, &mut segments);
            }
        }
        segments
    }

    fn decompose_paragraph<'a, F>(
        para: &'a str,
        max_limit: usize,
        count_fn: &mut F,
        out: &mut Vec<(&'a str, usize)>,
    ) where
        F: FnMut(&str) -> usize,
    {
        let mut has_lines = false;
        for line in para.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            has_lines = true;
            let count = count_fn(trimmed);
            if count <= max_limit {
                out.push((trimmed, count));
            } else {
                Self::decompose_line(trimmed, max_limit, count_fn, out);
            }
        }
        if !has_lines {
            Self::decompose_line(para, max_limit, count_fn, out);
        }
    }

    fn split_at_best_boundary<'a, F>(
        remaining: &mut &'a str,
        candidates: &[usize],
        max_limit: usize,
        count_fn: &mut F,
        out: &mut Vec<(&'a str, usize)>,
    ) -> bool
    where
        F: FnMut(&str) -> usize,
    {
        if candidates.is_empty() {
            return false;
        }
        let idx = candidates.partition_point(|&pos| count_fn(&remaining[..pos]) <= max_limit);
        if idx > 0 {
            let pos = candidates[idx - 1];
            let (head, tail) = remaining.split_at(pos);
            let head_trimmed = head.trim();
            if !head_trimmed.is_empty() {
                out.push((head_trimmed, count_fn(head_trimmed)));
            }
            *remaining = tail.trim_start();
            true
        } else {
            false
        }
    }

    fn decompose_line<'a, F>(
        line: &'a str,
        max_limit: usize,
        count_fn: &mut F,
        out: &mut Vec<(&'a str, usize)>,
    ) where
        F: FnMut(&str) -> usize,
    {
        let mut remaining = line;
        while !remaining.is_empty() {
            let r_count = count_fn(remaining);
            if r_count <= max_limit {
                out.push((remaining, r_count));
                break;
            }

            // Collect sentence boundary end-offsets and binary search the largest valid prefix
            let candidates: Vec<usize> = remaining
                .char_indices()
                .filter_map(|(i, c)| {
                    if (c == '.' || c == '?' || c == '!' || c == ';')
                        && remaining[i + c.len_utf8()..].starts_with(|ch: char| ch.is_whitespace())
                    {
                        Some(i + c.len_utf8())
                    } else {
                        None
                    }
                })
                .collect();

            if !Self::split_at_best_boundary(&mut remaining, &candidates, max_limit, count_fn, out)
            {
                Self::decompose_words(remaining, max_limit, count_fn, out);
                break;
            }
        }
    }

    fn decompose_words<'a, F>(
        text: &'a str,
        max_limit: usize,
        count_fn: &mut F,
        out: &mut Vec<(&'a str, usize)>,
    ) where
        F: FnMut(&str) -> usize,
    {
        let mut remaining = text;
        while !remaining.is_empty() {
            let r_count = count_fn(remaining);
            if r_count <= max_limit {
                out.push((remaining, r_count));
                break;
            }

            // Collect whitespace boundary offsets and binary search the largest valid prefix
            let candidates: Vec<usize> = remaining
                .char_indices()
                .filter_map(|(i, c)| if c.is_whitespace() { Some(i) } else { None })
                .collect();

            if !Self::split_at_best_boundary(&mut remaining, &candidates, max_limit, count_fn, out)
            {
                let char_indices: Vec<usize> = remaining.char_indices().map(|(i, _)| i).collect();
                let idx =
                    char_indices.partition_point(|&pos| count_fn(&remaining[..pos]) <= max_limit);
                let mut cut = if idx > 0 { char_indices[idx - 1] } else { 0 };
                if cut == 0 {
                    cut = remaining
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(remaining.len());
                }
                let (head, tail) = remaining.split_at(cut);
                out.push((head, count_fn(head)));
                remaining = tail;
            }
        }
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
    fn test_count_tokens_with_kind() {
        let text = "Hello, world! This is a test.";
        let tokens = TextChunker::count_tokens_with_kind(text, TokenizerKind::Cl100k);
        assert!((6..=10).contains(&tokens));
    }

    #[test]
    fn test_chunk_by_tokens_under_budget() {
        let text = "First paragraph.\n\nSecond paragraph.";
        let chunks =
            TextChunker::chunk_by_tokens_with_counts_and_kind(text, 100, 10, TokenizerKind::Cl100k);
        assert_eq!(chunks.len(), 1);
        assert!(matches!(chunks[0].0, Cow::Borrowed(_)));
    }

    #[test]
    fn test_chunk_by_tokens_splits_on_budget() {
        let p1 = "This is a paragraph with several words repeated. ".repeat(20);
        let p2 = "Another different paragraph with several words repeated. ".repeat(20);
        let text = format!("{}\n\n{}", p1.trim(), p2.trim());

        let chunks = TextChunker::chunk_by_tokens_with_counts_and_kind(
            &text,
            120,
            20,
            TokenizerKind::Cl100k,
        );
        assert_eq!(chunks.len(), 2);
        assert!(matches!(chunks[0].0, Cow::Borrowed(_)));
        assert!(matches!(chunks[1].0, Cow::Borrowed(_)));

        for (chunk, count) in &chunks {
            assert_eq!(
                *count,
                TextChunker::count_tokens_with_kind(chunk, TokenizerKind::Cl100k)
            );
        }
    }

    #[test]
    fn test_truncate_by_tokens() {
        let short = "Short text under budget.";
        let res = TextChunker::truncate_by_tokens(short, 50);
        assert!(matches!(res, Cow::Borrowed(_)));
        assert_eq!(res, short);

        let long = "token ".repeat(200);
        let truncated = TextChunker::truncate_by_tokens(&long, 30);
        assert!(truncated.ends_with("[Context trimmed for token budget]"));
        assert!(TextChunker::count_tokens_with_kind(&truncated, TokenizerKind::Cl100k) <= 45);
    }

    #[test]
    fn test_tokenizer_kind_detection_and_counting() {
        assert_eq!(
            TokenizerKind::detect_from_model("google/gemma-4-27b-it"),
            TokenizerKind::Gemma
        );
        assert_eq!(
            TokenizerKind::detect_from_model("meta-llama/Llama-3.1-8B-Instruct"),
            TokenizerKind::Llama3
        );
        assert_eq!(
            TokenizerKind::detect_from_model("gpt-4o-mini"),
            TokenizerKind::O200k
        );
        assert_eq!(
            TokenizerKind::detect_from_model("gpt-4-turbo"),
            TokenizerKind::Cl100k
        );

        let sample =
            "This is a sample document for token estimation across distinct architectures.";
        let cl100k_count = TokenizerKind::Cl100k.count_tokens(sample);
        let gemma_count = TokenizerKind::Gemma.count_tokens(sample);
        let llama_count = TokenizerKind::Llama3.count_tokens(sample);
        assert!(gemma_count >= cl100k_count);
        assert!(llama_count >= cl100k_count);
    }

    #[test]
    fn test_chunk_monolithic_block_without_double_newlines() {
        // Monolithic text with NO \n\n (reproduces the unchunked fallthrough bug)
        let sentence =
            "This is a detailed analysis of the system architecture without any double newlines. ";
        let monolithic = sentence.repeat(40); // ~560 tokens
        let total_tokens = TextChunker::count_tokens_with_kind(&monolithic, TokenizerKind::Cl100k);
        assert!(total_tokens > 400);

        let chunks = TextChunker::chunk_by_tokens_with_counts_and_kind(
            &monolithic,
            100,
            15,
            TokenizerKind::Cl100k,
        );
        assert!(
            chunks.len() >= 4,
            "Expected at least 4 chunks, got {}",
            chunks.len()
        );
        for (i, (c, count)) in chunks.iter().enumerate() {
            assert!(
                *count <= 125,
                "Chunk #{} has {} tokens, exceeding limit of 125",
                i,
                count
            );
            let measured = TextChunker::count_tokens_with_kind(c, TokenizerKind::Cl100k);
            assert!(measured <= 125);
        }
    }

    #[test]
    fn test_chunk_single_newline_separated_text() {
        // Single \n separated lines (e.g. PDF text, code, log files)
        let lines: Vec<String> = (0..50)
            .map(|i| format!("Line #{}: system event telemetry metric logging item.", i))
            .collect();
        let text = lines.join("\n"); // strictly single \n, no \n\n
        let total_tokens = TextChunker::count_tokens_with_kind(&text, TokenizerKind::Cl100k);
        assert!(total_tokens > 300);

        let chunks =
            TextChunker::chunk_by_tokens_with_counts_and_kind(&text, 80, 10, TokenizerKind::Cl100k);
        assert!(
            chunks.len() >= 4,
            "Expected >= 4 chunks, got {}",
            chunks.len()
        );
        for (c, count) in &chunks {
            assert!(
                *count <= 100,
                "Chunk tokens {} exceeded expected budget",
                count
            );
            let measured = TextChunker::count_tokens_with_kind(c, TokenizerKind::Cl100k);
            assert!(measured <= 100);
        }
    }

    #[test]
    fn test_chunk_oversized_paragraph_in_multiparagraph_document() {
        let p_intro = "Introductory context paragraph.";
        let p_huge =
            "Massive continuous detailed sentence repeating for testing paragraph decomposition. "
                .repeat(30);
        let p_outro = "Concluding summary paragraph.";
        let text = format!("{}\n\n{}\n\n{}", p_intro, p_huge, p_outro);

        let chunks = TextChunker::chunk_by_tokens_with_counts_and_kind(
            &text,
            100,
            15,
            TokenizerKind::Cl100k,
        );
        assert!(
            chunks.len() >= 3,
            "Expected >= 3 chunks for oversized paragraph, got {}",
            chunks.len()
        );
        for (c, count) in &chunks {
            assert!(*count <= 130, "Chunk tokens {} exceeded budget", count);
            let measured = TextChunker::count_tokens_with_kind(c, TokenizerKind::Cl100k);
            assert!(measured <= 130);
        }
    }

    #[test]
    fn test_chunk_unbroken_giant_token() {
        // Unbroken sequence with zero whitespace
        let giant_token = "A".repeat(1000);
        let chunks = TextChunker::chunk_by_tokens_with_counts_and_kind(
            &giant_token,
            50,
            5,
            TokenizerKind::Cl100k,
        );
        assert!(
            chunks.len() >= 2,
            "Expected unbroken string to be sliced into multiple chunks"
        );
    }
}
