use std::borrow::Cow;
use std::sync::LazyLock;
use tiktoken_rs::CoreBPE;

static BPE: LazyLock<CoreBPE> = LazyLock::new(|| {
    tiktoken_rs::cl100k_base().expect("Failed to initialize cl100k_base tokenizer")
});

/// Tokenizer profile for model-aware in-process token counting and chunking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenizerKind {
    /// OpenAI GPT-4 / GPT-3.5 standard cl100k_base BPE vocabulary.
    #[default]
    Cl100k,
    /// OpenAI GPT-4o / o1 / o3 o200k_base BPE vocabulary.
    O200k,
    /// Meta Llama 3 / 3.1 / 3.2 128k BPE vocabulary profile.
    Llama3,
    /// Google Gemma 4 / Gemma 2 256k SentencePiece vocabulary profile.
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
        match self {
            Self::Cl100k => BPE.encode_with_special_tokens(text).len(),
            Self::O200k => {
                static O200K: LazyLock<CoreBPE> = LazyLock::new(|| {
                    tiktoken_rs::o200k_base().expect("Failed to initialize o200k_base")
                });
                O200K.encode_with_special_tokens(text).len()
            }
            Self::Llama3 => {
                // Meta Llama 3 uses a 128k BPE vocabulary with specialized splitting regex.
                // In English and code, Llama 3 token count aligns closely with cl100k with ~1.12x inflation for whitespace/syntax.
                let base = BPE.encode_with_special_tokens(text).len();
                base.saturating_mul(112).div_ceil(100)
            }
            Self::Gemma => {
                // Google Gemma uses a 256k SentencePiece vocabulary with byte-level fallback.
                // In English prose, SentencePiece splits on punctuation and prefix underscores, resulting in ~1.18x inflation over cl100k.
                let base = BPE.encode_with_special_tokens(text).len();
                base.saturating_mul(118).div_ceil(100)
            }
        }
    }
}

pub struct TextChunker;

impl TextChunker {
    /// Counts tokens in `text` using the standard OpenAI cl100k_base BPE tokenizer.
    pub fn count_tokens(text: &str) -> usize {
        BPE.encode_with_special_tokens(text).len()
    }

    /// Counts tokens in `text` using a specified model-aware tokenizer profile.
    pub fn count_tokens_with_kind(text: &str, kind: TokenizerKind) -> usize {
        kind.count_tokens(text)
    }

    /// Truncates `text` so its BPE token count does not exceed `max_tokens`, preserving paragraph
    /// boundaries when possible and returning a zero-copy `Cow::Borrowed` slice when under budget.
    pub fn truncate_by_tokens(text: &str, max_tokens: usize) -> Cow<'_, str> {
        if max_tokens == 0 {
            return Cow::Borrowed("[Context trimmed for token budget]");
        }
        // Fast path: 1 token is at least 1 byte in cl100k_base.
        if text.len() <= max_tokens {
            return Cow::Borrowed(text);
        }
        let tokens = BPE.encode_with_special_tokens(text);
        if tokens.len() <= max_tokens {
            return Cow::Borrowed(text);
        }
        if let Ok(decoded) = BPE.decode(&tokens[..max_tokens]) {
            let trimmed = decoded.trim_end();
            return Cow::Owned(format!("{trimmed}\n\n[Context trimmed for token budget]"));
        }
        Cow::Borrowed(text)
    }

    /// Splits text into chunks of roughly `max_tokens` with `overlap_tokens` overlap,
    /// using accurate BPE token counts while preserving paragraph boundaries and zero-copy slices.
    #[doc(hidden)]
    pub fn chunk_by_tokens<'a>(
        text: &'a str,
        max_tokens: usize,
        overlap_tokens: usize,
    ) -> Vec<Cow<'a, str>> {
        Self::chunk_by_tokens_with_counts(text, max_tokens, overlap_tokens)
            .into_iter()
            .map(|(c, _)| c)
            .collect()
    }

    /// Splits text into chunks of roughly `max_tokens` with `overlap_tokens` overlap,
    /// returning each chunk alongside its pre-computed token count to avoid re-tokenization.
    pub fn chunk_by_tokens_with_counts<'a>(
        text: &'a str,
        max_tokens: usize,
        overlap_tokens: usize,
    ) -> Vec<(Cow<'a, str>, usize)> {
        Self::chunk_by_tokens_with_counts_and_kind(
            text,
            max_tokens,
            overlap_tokens,
            TokenizerKind::Cl100k,
        )
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

    /// Splits text into chunks of roughly `max_words` with `overlap_words` overlap.
    /// Preserves paragraph boundaries wherever possible and returns zero-copy `Cow::Borrowed`
    /// slices whenever the input or a chunk does not require concatenation.
    pub fn chunk<'a>(text: &'a str, max_words: usize, overlap_words: usize) -> Vec<Cow<'a, str>> {
        Self::chunk_with_metric_with_counts(text, max_words, overlap_words, |s| {
            s.split_whitespace().count()
        })
        .into_iter()
        .map(|(c, _)| c)
        .collect()
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
        // Allow up to 1.6x of max_limit to preserve intact paragraph boundaries when multiple paragraphs exist
        let para_threshold = if is_single_block {
            max_limit
        } else {
            max_limit.saturating_mul(16) / 10
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

        let chunks_with_counts = TextChunker::chunk_by_tokens_with_counts(&text, 120, 20);
        assert_eq!(chunks_with_counts.len(), chunks.len());
        for (i, (chunk, count)) in chunks_with_counts.into_iter().enumerate() {
            assert_eq!(chunk, chunks[i]);
            assert_eq!(count, TextChunker::count_tokens(&chunk));
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
        assert!(TextChunker::count_tokens(&truncated) <= 45);
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
        let total_tokens = TextChunker::count_tokens(&monolithic);
        assert!(total_tokens > 400);

        let chunks = TextChunker::chunk_by_tokens(&monolithic, 100, 15);
        assert!(
            chunks.len() >= 4,
            "Expected at least 4 chunks, got {}",
            chunks.len()
        );
        for (i, c) in chunks.iter().enumerate() {
            let tokens = TextChunker::count_tokens(c);
            assert!(
                tokens <= 125,
                "Chunk #{} has {} tokens, exceeding limit of 125",
                i,
                tokens
            );
        }
    }

    #[test]
    fn test_chunk_single_newline_separated_text() {
        // Single \n separated lines (e.g. PDF text, code, log files)
        let lines: Vec<String> = (0..50)
            .map(|i| format!("Line #{}: system event telemetry metric logging item.", i))
            .collect();
        let text = lines.join("\n"); // strictly single \n, no \n\n
        let total_tokens = TextChunker::count_tokens(&text);
        assert!(total_tokens > 300);

        let chunks = TextChunker::chunk_by_tokens(&text, 80, 10);
        assert!(
            chunks.len() >= 4,
            "Expected >= 4 chunks, got {}",
            chunks.len()
        );
        for c in &chunks {
            let tokens = TextChunker::count_tokens(c);
            assert!(
                tokens <= 100,
                "Chunk tokens {} exceeded expected budget",
                tokens
            );
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

        let chunks = TextChunker::chunk_by_tokens(&text, 100, 15);
        assert!(
            chunks.len() >= 3,
            "Expected >= 3 chunks for oversized paragraph, got {}",
            chunks.len()
        );
        for c in &chunks {
            let tokens = TextChunker::count_tokens(c);
            assert!(tokens <= 130, "Chunk tokens {} exceeded budget", tokens);
        }
    }

    #[test]
    fn test_chunk_unbroken_giant_token() {
        // Unbroken sequence with zero whitespace
        let giant_token = "A".repeat(1000);
        let chunks = TextChunker::chunk_by_tokens(&giant_token, 50, 5);
        assert!(
            chunks.len() >= 2,
            "Expected unbroken string to be sliced into multiple chunks"
        );
    }
}
