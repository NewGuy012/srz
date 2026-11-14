use std::borrow::Cow;
use std::collections::HashMap;

use crate::chunker::{TextChunker, TokenizerKind};
use crate::system::text_util::{
    find_ascii_case_insensitive, is_stop_word, is_stop_word_lower, tokenize_terms,
};

#[inline]
fn is_word_boundary(text: &str, start: usize, len: usize) -> bool {
    let b = text.as_bytes();
    let is_word = |i: usize| {
        b.get(i)
            .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_')
    };
    start.checked_sub(1).is_none_or(|i| !is_word(i)) && !is_word(start + len)
}

/// Counts occurrences of a term within `text` on word boundaries, case-insensitively.
pub fn count_term_occurrences(text: &str, term: &str) -> usize {
    if term.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut cursor = 0;
    while let Some(pos) = find_ascii_case_insensitive(&text[cursor..], term) {
        let abs_pos = cursor + pos;
        if is_word_boundary(text, abs_pos, term.len()) {
            count += 1;
        }
        cursor = abs_pos + term.len();
    }
    count
}

/// Evaluates Robertson-Sparck Jones BM25 relevance scores from pre-accumulated term
/// frequencies (`flat_tfs` laid out as `doc_idx * n_terms + term_idx`), per-term
/// document frequencies, and document token lengths.
fn score_bm25_matrix(
    n_docs: usize,
    n_terms: usize,
    lengths: &[usize],
    term_dfs: &[usize],
    flat_tfs: &[usize],
    avgdl: f64,
) -> Vec<f64> {
    if n_docs == 0 || n_terms == 0 {
        return vec![0.0; n_docs];
    }

    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let avgdl = avgdl.max(1.0);

    let idfs: Vec<f64> = term_dfs
        .iter()
        .map(|&df| {
            if df == 0 {
                0.0
            } else {
                (((n_docs.saturating_sub(df)) as f64 + 0.5) / (df as f64 + 0.5) + 1.0)
                    .ln()
                    .max(0.1)
            }
        })
        .collect();

    lengths
        .iter()
        .enumerate()
        .map(|(idx, &len)| {
            let doc_len = len as f64;
            let norm_len = 1.0 - B + B * (doc_len / avgdl);
            let row = &flat_tfs[idx * n_terms..(idx + 1) * n_terms];
            let mut score = 0.0;

            for (term_idx, &tf_usize) in row.iter().enumerate() {
                if term_dfs[term_idx] == 0 || tf_usize == 0 {
                    continue;
                }
                let tf = tf_usize as f64;
                let saturation = (tf * (K1 + 1.0)) / (tf + K1 * norm_len);
                score += idfs[term_idx] * saturation;
            }
            score
        })
        .collect()
}

/// Computes Robertson-Sparck Jones BM25 relevance scores for a slice of text segments
/// against a list of terms given segment token lengths and average document length.
pub fn compute_bm25_scores(
    segments: &[&str],
    lengths: &[usize],
    terms: &[String],
    avgdl: f64,
) -> Vec<f64> {
    let n_docs = segments.len();
    let n_terms = terms.len();
    if n_docs == 0 || n_terms == 0 {
        return vec![0.0; n_docs];
    }

    let mut term_dfs = vec![0usize; n_terms];
    let mut flat_tfs = vec![0usize; n_docs * n_terms];

    for (doc_idx, segment) in segments.iter().enumerate() {
        let row = &mut flat_tfs[doc_idx * n_terms..(doc_idx + 1) * n_terms];
        for (term_idx, term) in terms.iter().enumerate() {
            let tf = count_term_occurrences(segment, term);
            if tf > 0 {
                term_dfs[term_idx] += 1;
            }
            row[term_idx] = tf;
        }
    }

    score_bm25_matrix(n_docs, n_terms, lengths, &term_dfs, &flat_tfs, avgdl)
}

/// Selects the most relevant sections of `content` when it exceeds `max_tokens` using
/// model-aware token counting and Robertson-Sparck Jones BM25 ranking.
pub fn select_relevant_context_bm25<'a>(
    content: &'a str,
    question: Option<&str>,
    max_tokens: usize,
    tokenizer_kind: TokenizerKind,
) -> Cow<'a, str> {
    if content.len() <= (max_tokens.saturating_mul(3) / 4) {
        return Cow::Borrowed(content);
    }

    let Some(q) = question else {
        return Cow::Borrowed(content);
    };

    let raw_terms = tokenize_terms(q);
    if raw_terms.is_empty() {
        return Cow::Borrowed(content);
    }

    // Filter stop words to ensure high discriminatory signal for rare concepts
    let filtered_terms: Vec<String> = raw_terms
        .iter()
        .filter(|t| !is_stop_word(t))
        .cloned()
        .collect();
    let q_terms = if filtered_terms.is_empty() {
        raw_terms
    } else {
        filtered_terms
    };

    // Deduplicate query terms for Robertson-Sparck Jones IDF evaluation
    let mut unique_q_terms = Vec::with_capacity(q_terms.len());
    for term in q_terms {
        if !unique_q_terms.contains(&term) {
            unique_q_terms.push(term);
        }
    }

    let chunk_size = (max_tokens / 8).clamp(500, 2000);
    let chunk_overlap = (chunk_size / 12).clamp(50, 150);
    let chunks_with_counts = TextChunker::chunk_by_tokens_with_counts_and_kind(
        content,
        chunk_size,
        chunk_overlap,
        tokenizer_kind,
    );
    if chunks_with_counts.len() <= 1 {
        return Cow::Borrowed(content);
    }

    let n_docs = chunks_with_counts.len();
    let total_tokens: usize = chunks_with_counts.iter().map(|(_, t)| *t).sum();
    let avgdl = (total_tokens as f64) / (n_docs as f64).max(1.0);

    let seg_texts: Vec<&str> = chunks_with_counts.iter().map(|(c, _)| c.as_ref()).collect();
    let seg_lens: Vec<usize> = chunks_with_counts.iter().map(|(_, t)| *t).collect();
    let bm25_scores = compute_bm25_scores(&seg_texts, &seg_lens, &unique_q_terms, avgdl);

    let mut scored: Vec<(usize, f64, usize)> = chunks_with_counts
        .iter()
        .enumerate()
        .map(|(idx, (_, tokens))| {
            let mut score = bm25_scores[idx];
            if idx == 0 {
                score += 0.5;
            }
            (idx, score, *tokens)
        })
        .collect();

    // Sort descending by score, tie-breaking by original document order
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut selected_indices = Vec::new();
    let mut token_sum = 0;
    for (idx, _, tokens) in scored {
        if token_sum + tokens <= max_tokens {
            selected_indices.push(idx);
            token_sum += tokens;
        }
    }
    selected_indices.sort_unstable();

    let mut out = String::new();
    let selected_text: Vec<&str> = selected_indices
        .into_iter()
        .map(|idx| chunks_with_counts[idx].0.as_ref())
        .collect();
    out.push_str(&selected_text.join("\n\n...\n\n"));
    Cow::Owned(out)
}

#[derive(Clone, Copy)]
struct Segment<'a> {
    text: &'a str,
    tokens: usize,
    heading_level: Option<u8>,
    after_heading: bool,
}

#[derive(Default)]
struct VocabEntry {
    df: usize,
    weight: f64,
    in_heading: bool,
    in_anchor: bool,
    last_seg: Option<usize>,
    postings: Vec<(usize, usize)>,
}

impl VocabEntry {
    fn record(&mut self, seg_idx: usize, is_heading: bool, is_anchor: bool, weight_inc: f64) {
        self.weight += weight_inc;
        if is_heading {
            self.in_heading = true;
        }
        if is_anchor {
            self.in_anchor = true;
        }
        if self.last_seg == Some(seg_idx) {
            if let Some(last) = self.postings.last_mut() {
                last.1 += 1;
            }
        } else {
            self.df += 1;
            self.last_seg = Some(seg_idx);
            self.postings.push((seg_idx, 1));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectionTier {
    AnchorLead,
    AnchorTail,
    Lead,
    Tail,
    Heading(u8),
    SalientBody(usize),
}

impl SelectionTier {
    /// Higher eviction key means lower retention priority (evicted first on budget guard).
    fn eviction_key(self, idx: usize) -> (u8, usize, usize) {
        match self {
            Self::SalientBody(rank) => (5, rank, idx),
            Self::Heading(level) => (4, level as usize, idx),
            Self::Tail => (3, usize::MAX - idx, idx),
            Self::Lead => (2, idx, idx),
            Self::AnchorTail => (1, 0, idx),
            Self::AnchorLead => (0, 0, idx),
        }
    }
}

fn detect_fence_marker(trimmed: &str) -> Option<(u8, usize)> {
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == first).count();
    if count >= 3 && !(first == b'`' && trimmed[count..].contains('`')) {
        Some((first, count))
    } else {
        None
    }
}

fn is_closing_fence(trimmed: &str, fence_char: u8, fence_len: usize) -> bool {
    let count = trimmed.bytes().take_while(|&b| b == fence_char).count();
    count >= fence_len && trimmed[count..].trim().is_empty()
}

fn strip_optional_timestamp_prefix(s: &str) -> &str {
    if !s.starts_with('[') {
        return s;
    }
    if let Some(close) = s.find(']') {
        let inner = &s[1..close];
        let mut has_colon = false;
        let valid_ts = !inner.is_empty()
            && inner.bytes().all(|b| {
                if b == b':' {
                    has_colon = true;
                    true
                } else {
                    b.is_ascii_digit()
                }
            });
        if valid_ts && has_colon {
            return s[close + 1..].trim_start_matches([' ', '\t']);
        }
    }
    s
}

/// Detects ATX Markdown headings at levels 1..=3 (`#`, `##`, `###`), including
/// YouTube chapter headings prefixed with `[MM:SS]` or `[HH:MM:SS]`.
fn parse_heading_level(raw_line: &str) -> Option<u8> {
    if raw_line.starts_with('\t') {
        return None;
    }
    let leading_spaces = raw_line.bytes().take_while(|&b| b == b' ').count();
    if leading_spaces >= 4 {
        return None;
    }
    let trimmed = strip_optional_timestamp_prefix(raw_line[leading_spaces..].trim_end());
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.starts_with([' ', '\t']) || rest.trim().is_empty() {
        return None;
    }
    Some(hashes as u8)
}

fn split_oversized_span_by_words<'a>(
    span: &'a str,
    max_seg_tokens: usize,
    tok_kind: TokenizerKind,
    out: &mut Vec<&'a str>,
) {
    let span = span.trim();
    if span.is_empty() {
        return;
    }
    if tok_kind.count_tokens(span) <= max_seg_tokens {
        out.push(span);
        return;
    }

    // In BPE approximation, ~3 words ≈ 4 tokens; scale down conservatively to stay <= max_seg_tokens.
    let target_words = ((max_seg_tokens * 3) / 5).max(8);
    let mut chunk_start = 0;
    let mut words = 0;
    let mut in_word = false;

    for (idx, ch) in span.char_indices() {
        if ch.is_whitespace() {
            if in_word {
                in_word = false;
                if words >= target_words {
                    let piece = span[chunk_start..idx].trim();
                    if !piece.is_empty() {
                        out.push(piece);
                    }
                    chunk_start = idx + ch.len_utf8();
                    words = 0;
                }
            } else if chunk_start == idx {
                chunk_start = idx + ch.len_utf8();
            }
        } else if !in_word {
            in_word = true;
            words += 1;
        }
    }

    let tail = span[chunk_start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
}

fn split_long_line_by_sentences<'a>(
    line: &'a str,
    max_seg_tokens: usize,
    tok_kind: TokenizerKind,
    out: &mut Vec<&'a str>,
) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if tok_kind.count_tokens(line) <= max_seg_tokens {
        out.push(line);
        return;
    }

    let bytes = line.as_bytes();
    let mut sentences: Vec<&'a str> = Vec::new();
    let mut sent_start = 0;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if matches!(b, b'.' | b'?' | b'!' | b';')
            && (i + 1 == bytes.len() || bytes[i + 1].is_ascii_whitespace())
        {
            let slice = line[sent_start..=i].trim();
            if !slice.is_empty() {
                sentences.push(slice);
            }
            let mut next = i + 1;
            while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                next += 1;
            }
            sent_start = next;
            i = next;
            continue;
        }
        i += 1;
    }
    if sent_start < line.len() {
        let tail = line[sent_start..].trim();
        if !tail.is_empty() {
            sentences.push(tail);
        }
    }

    if sentences.is_empty() {
        split_oversized_span_by_words(line, max_seg_tokens, tok_kind, out);
        return;
    }

    let line_ptr = line.as_ptr() as usize;
    let mut group_start: Option<usize> = None;
    let mut group_end = 0usize;

    for sent in sentences {
        let s_rel = (sent.as_ptr() as usize) - line_ptr;
        let e_rel = s_rel + sent.len();
        let sent_tokens = tok_kind.count_tokens(sent);

        if sent_tokens > max_seg_tokens {
            if let Some(gs) = group_start.take() {
                let piece = line[gs..group_end].trim();
                if !piece.is_empty() {
                    out.push(piece);
                }
            }
            split_oversized_span_by_words(sent, max_seg_tokens, tok_kind, out);
            continue;
        }

        if let Some(gs) = group_start {
            let candidate = line[gs..e_rel].trim();
            if tok_kind.count_tokens(candidate) > max_seg_tokens {
                let piece = line[gs..group_end].trim();
                if !piece.is_empty() {
                    out.push(piece);
                }
                group_start = Some(s_rel);
                group_end = e_rel;
            } else {
                group_end = e_rel;
            }
        } else {
            group_start = Some(s_rel);
            group_end = e_rel;
        }
    }

    if let Some(gs) = group_start {
        let piece = line[gs..group_end].trim();
        if !piece.is_empty() {
            out.push(piece);
        }
    }
}

fn split_oversized_block(block: &str, max_seg_tokens: usize, tok_kind: TokenizerKind) -> Vec<&str> {
    let mut out = Vec::new();
    let mut chunk_start: Option<usize> = None;
    let mut chunk_end = 0usize;
    let mut chunk_tokens = 0usize;

    let mut pos = 0;
    while pos < block.len() {
        let ls = pos;
        let le = match block[pos..].find('\n') {
            Some(rel) => pos + rel,
            None => block.len(),
        };
        pos = if le < block.len() {
            le + 1
        } else {
            block.len()
        };

        let raw = block[ls..le].trim_end_matches('\r');
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        let leading_ws = raw.len() - raw.trim_start().len();
        let cs = ls + leading_ws;
        let ce = ls + raw.trim_end().len();
        let line_tokens = tok_kind.count_tokens(trimmed);

        if line_tokens > max_seg_tokens {
            if let Some(s) = chunk_start.take() {
                let piece = block[s..chunk_end].trim();
                if !piece.is_empty() {
                    out.push(piece);
                }
            }
            chunk_tokens = 0;
            split_long_line_by_sentences(trimmed, max_seg_tokens, tok_kind, &mut out);
            continue;
        }

        if let Some(s) = chunk_start {
            if chunk_tokens + line_tokens > max_seg_tokens {
                let piece = block[s..chunk_end].trim();
                if !piece.is_empty() {
                    out.push(piece);
                }
                chunk_start = Some(cs);
                chunk_end = ce;
                chunk_tokens = line_tokens;
            } else {
                chunk_end = ce;
                chunk_tokens += line_tokens;
            }
        } else {
            chunk_start = Some(cs);
            chunk_end = ce;
            chunk_tokens = line_tokens;
        }
    }

    if let Some(s) = chunk_start {
        let piece = block[s..chunk_end].trim();
        if !piece.is_empty() {
            out.push(piece);
        }
    }

    out
}

fn push_body_block<'a>(
    block: &'a str,
    max_seg_tokens: usize,
    tok_kind: TokenizerKind,
    prev_was_heading: &mut bool,
    segments: &mut Vec<Segment<'a>>,
) {
    let trimmed = block.trim();
    if trimmed.is_empty() {
        return;
    }
    let tokens = tok_kind.count_tokens(trimmed);
    if tokens <= max_seg_tokens {
        segments.push(Segment {
            text: trimmed,
            tokens,
            heading_level: None,
            after_heading: *prev_was_heading,
        });
        *prev_was_heading = false;
        return;
    }

    let chunks = split_oversized_block(trimmed, max_seg_tokens, tok_kind);
    for (idx, chunk) in chunks.into_iter().enumerate() {
        let c_tokens = tok_kind.count_tokens(chunk);
        segments.push(Segment {
            text: chunk,
            tokens: c_tokens,
            heading_level: None,
            after_heading: idx == 0 && *prev_was_heading,
        });
    }
    *prev_was_heading = false;
}

/// Decomposes `content` into zero-copy heading (`#`, `##`, `###`, including `[MM:SS] ## Title`)
/// and body segments while ignoring `# comments` inside fenced code blocks and splitting
/// oversized blocks across line/sentence boundaries.
fn parse_document_segments<'a>(
    content: &'a str,
    max_seg_tokens: usize,
    tok_kind: TokenizerKind,
) -> Vec<Segment<'a>> {
    let mut segments = Vec::new();
    let mut body_start: Option<usize> = None;
    let mut body_end = 0usize;
    let mut fence: Option<(u8, usize)> = None;
    let mut prev_was_heading = false;

    let mut pos = 0;
    while pos < content.len() {
        let line_start = pos;
        let line_end = match content[pos..].find('\n') {
            Some(rel) => pos + rel,
            None => content.len(),
        };
        pos = if line_end < content.len() {
            line_end + 1
        } else {
            content.len()
        };

        let raw_line = content[line_start..line_end].trim_end_matches('\r');
        let trimmed = raw_line.trim();

        if let Some((fc, flen)) = fence {
            if !trimmed.is_empty() {
                let leading_ws = raw_line.len() - raw_line.trim_start().len();
                let cs = line_start + leading_ws;
                let ce = line_start + raw_line.trim_end().len();
                if body_start.is_none() {
                    body_start = Some(cs);
                }
                body_end = ce;
                if is_closing_fence(trimmed, fc, flen) {
                    fence = None;
                }
            }
            continue;
        }

        if trimmed.is_empty() {
            if let Some(s) = body_start.take() {
                push_body_block(
                    &content[s..body_end],
                    max_seg_tokens,
                    tok_kind,
                    &mut prev_was_heading,
                    &mut segments,
                );
            }
            continue;
        }

        if let Some(new_fence) = detect_fence_marker(trimmed) {
            fence = Some(new_fence);
            let leading_ws = raw_line.len() - raw_line.trim_start().len();
            let cs = line_start + leading_ws;
            let ce = line_start + raw_line.trim_end().len();
            if body_start.is_none() {
                body_start = Some(cs);
            }
            body_end = ce;
            continue;
        }

        if let Some(level) = parse_heading_level(raw_line) {
            if let Some(s) = body_start.take() {
                push_body_block(
                    &content[s..body_end],
                    max_seg_tokens,
                    tok_kind,
                    &mut prev_was_heading,
                    &mut segments,
                );
            }
            let tokens = tok_kind.count_tokens(trimmed);
            segments.push(Segment {
                text: trimmed,
                tokens,
                heading_level: Some(level),
                after_heading: false,
            });
            prev_was_heading = true;
            continue;
        }

        let leading_ws = raw_line.len() - raw_line.trim_start().len();
        let cs = line_start + leading_ws;
        let ce = line_start + raw_line.trim_end().len();
        if body_start.is_none() {
            body_start = Some(cs);
        }
        body_end = ce;
    }

    if let Some(s) = body_start {
        push_body_block(
            &content[s..body_end],
            max_seg_tokens,
            tok_kind,
            &mut prev_was_heading,
            &mut segments,
        );
    }

    segments
}

fn scan_segment_terms(
    text: &str,
    seg_idx: usize,
    is_heading: bool,
    is_anchor: bool,
    scratch: &mut String,
    vocab: &mut HashMap<String, VocabEntry>,
) {
    const HEADING_TERM_WEIGHT: f64 = 3.0;
    const ANCHOR_TERM_WEIGHT: f64 = 2.0;
    const MAX_CENTROID_VOCAB: usize = 50_000;

    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if !ch.is_alphanumeric() {
            continue;
        }
        scratch.clear();
        for lc in ch.to_lowercase() {
            scratch.push(lc);
        }
        while let Some(&(_, next_ch)) = chars.peek() {
            if next_ch.is_alphanumeric() || next_ch == '_' {
                for lc in next_ch.to_lowercase() {
                    scratch.push(lc);
                }
                chars.next();
            } else {
                break;
            }
        }
        while scratch.ends_with('_') {
            scratch.pop();
        }
        if scratch.len() < 2
            || !scratch.chars().any(char::is_alphabetic)
            || is_stop_word_lower(scratch.as_str())
        {
            continue;
        }

        let weight_inc = if is_heading {
            HEADING_TERM_WEIGHT
        } else if is_anchor {
            ANCHOR_TERM_WEIGHT
        } else {
            1.0
        };
        if let Some(entry) = vocab.get_mut(scratch.as_str()) {
            entry.record(seg_idx, is_heading, is_anchor, weight_inc);
        } else if vocab.len() < MAX_CENTROID_VOCAB {
            let mut entry = VocabEntry::default();
            entry.record(seg_idx, is_heading, is_anchor, weight_inc);
            vocab.insert(scratch.clone(), entry);
        }
    }
}

/// Scores all document segments against top TF-IDF centroid terms (boosting heading and lead/tail
/// anchor terms) in a single O(bytes) tokenization pass + shared [`score_bm25_matrix`] evaluation.
fn score_segments_centroid_bm25(segments: &[Segment<'_>]) -> Vec<f64> {
    const MAX_CENTROID_TERMS: usize = 48;

    let n_docs = segments.len();
    if n_docs == 0 {
        return Vec::new();
    }

    let anchor_window = (n_docs / 20).clamp(1, 4);
    let mut vocab: HashMap<String, VocabEntry> = HashMap::new();
    let mut scratch = String::with_capacity(64);
    let mut lengths = Vec::with_capacity(n_docs);
    let mut total_tokens = 0usize;

    for (idx, seg) in segments.iter().enumerate() {
        lengths.push(seg.tokens);
        total_tokens += seg.tokens;
        let is_anchor = idx < anchor_window || idx >= n_docs.saturating_sub(anchor_window);
        scan_segment_terms(
            seg.text,
            idx,
            seg.heading_level.is_some(),
            is_anchor,
            &mut scratch,
            &mut vocab,
        );
    }

    if vocab.is_empty() {
        return vec![0.0; n_docs];
    }

    let mut ranked_terms: Vec<(f64, VocabEntry)> = vocab
        .into_values()
        .map(|entry| {
            let df = entry.df;
            let idf = (((n_docs.saturating_sub(df)) as f64 + 0.5) / (df as f64 + 0.5) + 1.0)
                .ln()
                .max(0.1);
            let spread = if n_docs >= 8 && df == 1 && !entry.in_heading && !entry.in_anchor {
                0.35
            } else {
                1.0
            };
            let mut anchor_mult = 1.0;
            if entry.in_heading {
                anchor_mult *= 2.0;
            }
            if entry.in_anchor {
                anchor_mult *= 2.0;
            }
            let centroid_score = entry.weight.ln_1p() * idf * spread * anchor_mult;
            (centroid_score, entry)
        })
        .collect();

    if ranked_terms.len() > MAX_CENTROID_TERMS {
        ranked_terms.select_nth_unstable_by(MAX_CENTROID_TERMS, |a, b| b.0.total_cmp(&a.0));
        ranked_terms.truncate(MAX_CENTROID_TERMS);
    }

    let n_terms = ranked_terms.len();
    let mut term_dfs = Vec::with_capacity(n_terms);
    let mut flat_tfs = vec![0usize; n_docs * n_terms];

    for (term_idx, (_, entry)) in ranked_terms.into_iter().enumerate() {
        term_dfs.push(entry.df);
        for (seg_idx, tf) in entry.postings {
            flat_tfs[seg_idx * n_terms + term_idx] = tf;
        }
    }

    let avgdl = (total_tokens as f64 / n_docs as f64).max(1.0);
    let mut scores = score_bm25_matrix(n_docs, n_terms, &lengths, &term_dfs, &flat_tfs, avgdl);

    for (idx, score) in scores.iter_mut().enumerate() {
        if segments[idx].after_heading {
            *score *= 1.15;
        }
        if segments[idx].tokens < 6 {
            *score *= 0.25;
        }
    }

    scores
}

struct SelectionState {
    selected: Vec<bool>,
    tiers: Vec<Option<SelectionTier>>,
    used_tokens: usize,
    min_selected: Option<usize>,
    max_selected: Option<usize>,
}

impl SelectionState {
    fn new(n: usize) -> Self {
        Self {
            selected: vec![false; n],
            tiers: vec![None; n],
            used_tokens: 0,
            min_selected: None,
            max_selected: None,
        }
    }

    fn gap_delta(&self, idx: usize) -> isize {
        let left_adj = idx > 0 && self.selected[idx - 1];
        let right_adj = idx + 1 < self.selected.len() && self.selected[idx + 1];
        let any_left = self.min_selected.is_some_and(|m| m < idx);
        let any_right = self.max_selected.is_some_and(|m| m > idx);

        match (any_left, any_right) {
            (false, false) => 0,
            (true, false) => isize::from(!left_adj),
            (false, true) => isize::from(!right_adj),
            (true, true) => isize::from(!left_adj) + isize::from(!right_adj) - 1,
        }
    }

    fn add_cost(&self, idx: usize, seg_tokens: usize, ellipsis_tokens: usize) -> usize {
        let gap_tokens = self.gap_delta(idx) * (ellipsis_tokens as isize);
        (seg_tokens as isize + gap_tokens).max(0) as usize
    }

    fn try_select(
        &mut self,
        idx: usize,
        seg_tokens: usize,
        ellipsis_tokens: usize,
        max_tokens: usize,
        tier: SelectionTier,
    ) -> Option<usize> {
        if self.selected[idx] {
            return None;
        }
        let cost = self.add_cost(idx, seg_tokens, ellipsis_tokens);
        if self.used_tokens + cost > max_tokens {
            return None;
        }
        self.selected[idx] = true;
        self.tiers[idx] = Some(tier);
        self.used_tokens += cost;
        self.min_selected = Some(self.min_selected.map_or(idx, |m| m.min(idx)));
        self.max_selected = Some(self.max_selected.map_or(idx, |m| m.max(idx)));
        Some(cost)
    }

    fn selected_count(&self) -> usize {
        self.selected.iter().filter(|&&s| s).count()
    }

    fn deselect_lowest_priority(&mut self) -> bool {
        let target = self
            .tiers
            .iter()
            .enumerate()
            .filter_map(|(idx, tier)| tier.map(|t| (t.eviction_key(idx), idx)))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, idx)| idx);

        let Some(idx) = target else {
            return false;
        };
        self.selected[idx] = false;
        self.tiers[idx] = None;
        self.min_selected = self.selected.iter().position(|&s| s);
        self.max_selected = self.selected.iter().rposition(|&s| s);
        true
    }
}

fn simulate_batch_heading_cost(
    sel: &SelectionState,
    indices: &[usize],
    segments: &[Segment<'_>],
    ellipsis_tokens: usize,
) -> usize {
    let mut temp = sel.selected.clone();
    let mut min_s = sel.min_selected;
    let mut max_s = sel.max_selected;
    let mut total = 0usize;

    for &idx in indices {
        if temp[idx] {
            continue;
        }
        let left_adj = idx > 0 && temp[idx - 1];
        let right_adj = idx + 1 < temp.len() && temp[idx + 1];
        let any_left = min_s.is_some_and(|m| m < idx);
        let any_right = max_s.is_some_and(|m| m > idx);
        let delta = match (any_left, any_right) {
            (false, false) => 0isize,
            (true, false) => isize::from(!left_adj),
            (false, true) => isize::from(!right_adj),
            (true, true) => isize::from(!left_adj) + isize::from(!right_adj) - 1,
        };
        let cost =
            (segments[idx].tokens as isize + delta * ellipsis_tokens as isize).max(0) as usize;
        total += cost;
        temp[idx] = true;
        min_s = Some(min_s.map_or(idx, |m| m.min(idx)));
        max_s = Some(max_s.map_or(idx, |m| m.max(idx)));
    }

    total
}

/// Preserves an outline skeleton of `#`, `##`, and `###` headings across the middle of the
/// document, falling back to tier priority (`#` -> `##` -> `###`) with evenly spaced
/// sampling if the heading count exceeds the heading budget cap.
fn select_heading_skeleton(
    sel: &mut SelectionState,
    segments: &[Segment<'_>],
    max_tokens: usize,
    heading_cap: usize,
    ellipsis_tokens: usize,
) {
    let mut heading_spent = 0usize;

    for level in 1u8..=3u8 {
        let level_indices: Vec<usize> = segments
            .iter()
            .enumerate()
            .filter_map(|(idx, seg)| {
                if !sel.selected[idx] && seg.heading_level == Some(level) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();

        if level_indices.is_empty() {
            continue;
        }

        let heading_budget_left = heading_cap
            .saturating_sub(heading_spent)
            .min(max_tokens.saturating_sub(sel.used_tokens));
        if heading_budget_left == 0 {
            break;
        }

        let full_cost = simulate_batch_heading_cost(sel, &level_indices, segments, ellipsis_tokens);
        if full_cost <= heading_budget_left {
            for idx in level_indices {
                let effective_limit = sel.used_tokens + heading_cap.saturating_sub(heading_spent);
                if let Some(cost) = sel.try_select(
                    idx,
                    segments[idx].tokens,
                    ellipsis_tokens,
                    effective_limit.min(max_tokens),
                    SelectionTier::Heading(level),
                ) {
                    heading_spent += cost;
                }
            }
            continue;
        }

        let target_count = ((level_indices.len() * heading_budget_left) / full_cost.max(1))
            .max(1)
            .min(level_indices.len());
        let denom = (target_count.saturating_sub(1)).max(1);
        let span = level_indices.len().saturating_sub(1);

        for step in 0..target_count {
            let pos = if target_count == 1 {
                0
            } else {
                (step * span) / denom
            };
            let idx = level_indices[pos];
            let effective_limit = sel.used_tokens + heading_cap.saturating_sub(heading_spent);
            if let Some(cost) = sel.try_select(
                idx,
                segments[idx].tokens,
                ellipsis_tokens,
                effective_limit.min(max_tokens),
                SelectionTier::Heading(level),
            ) {
                heading_spent += cost;
            }
        }
    }
}

fn assemble_selected_segments(segments: &[Segment<'_>], selected: &[bool]) -> String {
    let mut out = String::new();
    let mut last_idx: Option<usize> = None;

    for (idx, seg) in segments.iter().enumerate() {
        if !selected[idx] {
            continue;
        }
        if let Some(prev) = last_idx {
            if idx == prev + 1 {
                out.push_str("\n\n");
            } else {
                out.push_str("\n\n...\n\n");
            }
        }
        out.push_str(seg.text);
        last_idx = Some(idx);
    }

    out
}

fn truncate_single_to_budget<'a>(
    text: &'a str,
    max_tokens: usize,
    tok_kind: TokenizerKind,
) -> Cow<'a, str> {
    if tok_kind.count_tokens(text) <= max_tokens {
        return Cow::Borrowed(text);
    }

    // `TextChunker::truncate_by_tokens` estimates tokens with Cl100k; scale conservatively for larger tokenizers.
    let cl_budget = match tok_kind {
        TokenizerKind::Cl100k | TokenizerKind::O200k => max_tokens,
        TokenizerKind::Llama3 => (max_tokens * 89) / 100,
        TokenizerKind::Gemma => (max_tokens * 84) / 100,
    }
    .max(1);

    let truncated = TextChunker::truncate_by_tokens(text, cl_budget);
    if tok_kind.count_tokens(&truncated) <= max_tokens {
        return Cow::Owned(truncated.into_owned());
    }

    // Guarantee exact `<= max_tokens` under any `TokenizerKind` via UTF-8-safe binary search.
    let mut lo = 0usize;
    let mut hi = text.len();
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let mut boundary = mid;
        while boundary > lo && !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        if boundary == lo {
            boundary = mid;
            while boundary < hi && !text.is_char_boundary(boundary) {
                boundary += 1;
            }
        }
        if tok_kind.count_tokens(&text[..boundary]) <= max_tokens {
            lo = boundary;
        } else {
            hi = boundary - 1;
            while hi > lo && !text.is_char_boundary(hi) {
                hi -= 1;
            }
        }
    }

    let mut cut = lo;
    if let Some(ws) = text[..cut].rfind(char::is_whitespace)
        && ws > cut / 2
    {
        cut = ws;
    }
    Cow::Owned(text[..cut].trim_end().to_string())
}

/// Compresses an oversized document into `max_tokens` using CPU Heading-Skeleton + Centroid BM25
/// Salience Compression. Anchors lead and tail, preserves an outline skeleton of `#`, `##`, `###`
/// headings (including timestamped YouTube chapter headings), and fills the remaining budget with
/// highest-salience middle body paragraphs scored against document centroid terms.
pub fn compress_document_stratified<'a>(
    content: &'a str,
    max_tokens: usize,
    tok_kind: TokenizerKind,
) -> Cow<'a, str> {
    if max_tokens == 0 {
        return Cow::Borrowed("");
    }

    // Fast path: In BPE, 1 token is at least 1 byte.
    if content.len() <= max_tokens {
        return Cow::Borrowed(content);
    }

    if tok_kind.count_tokens(content) <= max_tokens {
        return Cow::Borrowed(content);
    }

    let max_seg_tokens = (max_tokens / 5).clamp(16, 512);
    let segments = parse_document_segments(content, max_seg_tokens, tok_kind);

    if segments.is_empty() {
        return Cow::Borrowed("");
    }
    if segments.len() <= 1 {
        return truncate_single_to_budget(content, max_tokens, tok_kind);
    }

    let n = segments.len();
    let ellipsis_tokens = tok_kind.count_tokens("...");
    let lead_cap = (max_tokens * 25) / 100;
    let tail_cap = (max_tokens * 15) / 100;
    let heading_cap = (max_tokens * 30) / 100;

    let mut sel = SelectionState::new(n);

    // 1. Anchor lead (index 0) and tail (index n - 1), then expand contiguous lead/tail windows.
    let mut lead_tokens = sel
        .try_select(
            0,
            segments[0].tokens,
            ellipsis_tokens,
            max_tokens,
            SelectionTier::AnchorLead,
        )
        .unwrap_or(0);
    let mut tail_tokens = sel
        .try_select(
            n - 1,
            segments[n - 1].tokens,
            ellipsis_tokens,
            max_tokens,
            SelectionTier::AnchorTail,
        )
        .unwrap_or(0);

    for (idx, seg) in segments.iter().enumerate().take(n - 1).skip(1) {
        if lead_tokens + seg.tokens > lead_cap {
            break;
        }
        if let Some(cost) = sel.try_select(
            idx,
            seg.tokens,
            ellipsis_tokens,
            max_tokens,
            SelectionTier::Lead,
        ) {
            lead_tokens += cost;
        } else {
            break;
        }
    }

    for idx in (1..n - 1).rev() {
        if sel.selected[idx] || tail_tokens + segments[idx].tokens > tail_cap {
            break;
        }
        if let Some(cost) = sel.try_select(
            idx,
            segments[idx].tokens,
            ellipsis_tokens,
            max_tokens,
            SelectionTier::Tail,
        ) {
            tail_tokens += cost;
        } else {
            break;
        }
    }

    // 2. Preserve middle Markdown heading skeleton (#, ##, ###).
    select_heading_skeleton(
        &mut sel,
        &segments,
        max_tokens,
        heading_cap,
        ellipsis_tokens,
    );

    // 3. Score all segments against document centroid terms and greedily fill remaining budget
    //    with the highest-salience middle body paragraphs.
    let scores = score_segments_centroid_bm25(&segments);
    let mut candidates: Vec<(f64, usize)> = segments
        .iter()
        .enumerate()
        .filter_map(|(idx, seg)| {
            if !sel.selected[idx] && seg.heading_level.is_none() {
                Some((scores[idx], idx))
            } else {
                None
            }
        })
        .collect();

    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    for (rank, (_, idx)) in candidates.into_iter().enumerate() {
        if sel.used_tokens >= max_tokens {
            break;
        }
        let _ = sel.try_select(
            idx,
            segments[idx].tokens,
            ellipsis_tokens,
            max_tokens,
            SelectionTier::SalientBody(rank),
        );
    }

    // 4. Assemble in document order and enforce strict <= max_tokens invariant under `tok_kind`.
    let mut out = assemble_selected_segments(&segments, &sel.selected);
    if out.is_empty() {
        return truncate_single_to_budget(content, max_tokens, tok_kind);
    }

    while tok_kind.count_tokens(&out) > max_tokens {
        if sel.selected_count() <= 1 || !sel.deselect_lowest_priority() {
            return Cow::Owned(truncate_single_to_budget(&out, max_tokens, tok_kind).into_owned());
        }
        out = assemble_selected_segments(&segments, &sel.selected);
    }

    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_term_occurrences() {
        let text = "Quantum cryptography uses quantum mechanics for cryptography.";
        assert_eq!(count_term_occurrences(text, "quantum"), 2);
        assert_eq!(count_term_occurrences(text, "cryptography"), 2);
        assert_eq!(count_term_occurrences(text, "crypto"), 0); // Word boundary check
    }

    #[test]
    fn test_compress_document_stratified_under_budget() {
        let doc = "This is a small document.\n\nIt fits in budget easily.";
        let res = compress_document_stratified(doc, 500, TokenizerKind::Cl100k);
        assert_eq!(res, doc);
        assert!(matches!(res, Cow::Borrowed(_)));
    }

    #[test]
    fn test_compress_document_stratified_oversized() {
        let mut paragraphs = Vec::new();
        paragraphs.push("# Introduction to Systems Architecture\n\nThis is the opening chapter introducing core concepts.".to_string());
        for i in 1..30 {
            paragraphs.push(format!(
                "Section {}: Detailed technical discussion with repeated information and metrics {}.",
                i, i * 10
            ));
        }
        paragraphs.push(
            "Conclusion\n\nIn conclusion, systems architecture is vital for robust performance."
                .to_string(),
        );

        let full_doc = paragraphs.join("\n\n");
        let initial_tokens = TokenizerKind::Cl100k.count_tokens(&full_doc);
        assert!(initial_tokens > 200);

        let budget = 120;
        let compressed = compress_document_stratified(&full_doc, budget, TokenizerKind::Cl100k);
        let compressed_tokens = TokenizerKind::Cl100k.count_tokens(&compressed);

        assert!(compressed_tokens <= budget);
        // Ensure beginning and end are preserved
        assert!(compressed.contains("Introduction to Systems Architecture"));
        assert!(compressed.contains("Conclusion"));
    }

    #[test]
    fn test_parse_heading_level_and_fences() {
        assert_eq!(parse_heading_level("# Title"), Some(1));
        assert_eq!(parse_heading_level("  ## Section Two"), Some(2));
        assert_eq!(parse_heading_level("### Subsection"), Some(3));
        assert_eq!(parse_heading_level("#### Too Deep"), None);
        assert_eq!(parse_heading_level("#hashtag"), None);
        assert_eq!(parse_heading_level("    # indented code"), None);
        assert_eq!(parse_heading_level("###   "), None);
        assert_eq!(parse_heading_level("[05:12] ## Chapter Title"), Some(2));
        assert_eq!(parse_heading_level("[1:05:12] # Hour Chapter"), Some(1));
        assert_eq!(parse_heading_level("[1] ## Footnote Not Heading"), None);

        let doc = "\
# Real H1

```python
# fake python comment
## fake second comment
print('ok')
```

~~~bash
### fake tilde comment
echo ok
~~~

[05:12] ## Real Timestamp H2
Transcript body line.";

        let segs = parse_document_segments(doc, 256, TokenizerKind::Cl100k);
        let headings: Vec<(&str, u8)> = segs
            .iter()
            .filter_map(|s| s.heading_level.map(|lvl| (s.text, lvl)))
            .collect();

        assert_eq!(
            headings,
            vec![("# Real H1", 1), ("[05:12] ## Real Timestamp H2", 2)]
        );
        assert!(
            segs.iter()
                .any(|s| s.after_heading && s.text.contains("Transcript body line."))
        );
    }

    #[test]
    fn test_compress_retains_heading_skeleton_and_salient_middle() {
        let mut parts = Vec::new();
        parts.push("# Distributed Database Engine Annual Report".to_string());
        parts.push(
            "Opening summary introducing distributed database storage latency and benchmark methodology."
                .to_string(),
        );
        parts.push("## Architecture".to_string());
        for i in 0..18 {
            parts.push(format!(
                "Administrative logistics update number {}: catering menu, parking permits, and desk assignments.",
                i
            ));
        }
        parts.push("## Benchmarks".to_string());
        parts.push(
            "Core finding: distributed database storage latency dropped by 47 percent across multi-region replication clusters after zero-copy WAL compaction."
                .to_string(),
        );
        for i in 0..18 {
            parts.push(format!(
                "Quarterly travel reimbursement memo {}: hotel receipts, shuttle schedules, and badge renewals.",
                i
            ));
        }
        parts.push("### Zero-Trust Migration".to_string());
        parts.push(
            "Second finding: multi-region distributed database replication maintained zero-copy storage latency overhead under two percent with mutual TLS."
                .to_string(),
        );
        parts.push("## Roadmap".to_string());
        for i in 0..12 {
            parts.push(format!(
                "Facility maintenance bulletin {}: elevator inspection and hallway repainting window.",
                i
            ));
        }
        parts.push("## Conclusion".to_string());
        parts.push(
            "In conclusion, distributed database replication and zero-copy storage architecture delivered our primary latency benchmark targets."
                .to_string(),
        );

        let doc = parts.join("\n\n");
        let budget = 122;
        assert!(TokenizerKind::Cl100k.count_tokens(&doc) > 700);

        let compressed = compress_document_stratified(&doc, budget, TokenizerKind::Cl100k);
        let tokens = TokenizerKind::Cl100k.count_tokens(&compressed);
        assert!(tokens <= budget, "tokens={tokens} > budget={budget}");

        // Preserves full Markdown heading skeleton in order
        for heading in [
            "# Distributed Database Engine Annual Report",
            "## Architecture",
            "## Benchmarks",
            "### Zero-Trust Migration",
            "## Roadmap",
            "## Conclusion",
        ] {
            assert!(
                compressed.contains(heading),
                "Missing heading `{heading}` in compressed output:\n{compressed}"
            );
        }

        // Retains high-salience middle body findings, drops unrelated administrative filler
        assert!(compressed.contains("dropped by 47 percent"));
        assert!(compressed.contains("maintained zero-copy storage latency overhead"));
        assert!(!compressed.contains("catering menu"));
        assert!(!compressed.contains("elevator inspection"));
    }

    #[test]
    fn test_compress_youtube_timestamped_chapters_single_newlines() {
        let mut lines = Vec::new();
        lines.push("[00:00] ## Introduction".to_string());
        lines.push(
            "Welcome to our deep dive into Rust async executor runtime scheduling.".to_string(),
        );
        lines.push("[05:12] ## Executor Bottleneck".to_string());
        for i in 0..20 {
            lines.push(format!(
                "Casual aside joke {} about coffee mugs, weather, and desk plants.",
                i
            ));
        }
        lines.push(
            "Key takeaway: work-stealing Rust async executor queues eliminated lock contention bottlenecks in tail latency."
                .to_string(),
        );
        lines.push("[18:40] ## Production Rollout".to_string());
        for i in 0..20 {
            lines.push(format!(
                "Brief sponsor shoutout segment {} mentioning headphones and notebooks.",
                i
            ));
        }
        lines.push("[32:10] ## Summary".to_string());
        lines.push(
            "To wrap up, async executor runtime scheduling cut tail latency across production."
                .to_string(),
        );

        // Single `\n` only — no `\n\n` breaks at all
        let doc = lines.join("\n");
        let budget = 220;
        assert!(TokenizerKind::Cl100k.count_tokens(&doc) > 500);

        let compressed = compress_document_stratified(&doc, budget, TokenizerKind::Cl100k);
        assert!(TokenizerKind::Cl100k.count_tokens(&compressed) <= budget);
        assert!(compressed.contains("[00:00] ## Introduction"));
        assert!(compressed.contains("[05:12] ## Executor Bottleneck"));
        assert!(compressed.contains("[18:40] ## Production Rollout"));
        assert!(compressed.contains("[32:10] ## Summary"));
        assert!(compressed.contains("eliminated lock contention bottlenecks"));
    }

    #[test]
    fn test_compress_heading_overflow_prioritizes_levels() {
        let mut parts = Vec::new();
        parts.push("# Book Part One".to_string());
        parts.push(
            "Opening paragraph about compilers and intermediate representations.".to_string(),
        );
        for i in 0..10 {
            parts.push(format!("## Chapter {} Compiler Pass", i + 1));
            for j in 0..4 {
                parts.push(format!("### Minor Subsection {}.{} Detail", i + 1, j + 1));
                parts.push(format!(
                    "Paragraph {} discussing optimization details and constant folding.",
                    i * 4 + j
                ));
            }
        }
        parts.push("# Book Epilogue".to_string());
        parts.push("Final compiler summary and verification notes.".to_string());

        let doc = parts.join("\n\n");
        let budget = 140;
        let compressed = compress_document_stratified(&doc, budget, TokenizerKind::Cl100k);
        assert!(TokenizerKind::Cl100k.count_tokens(&compressed) <= budget);
        assert!(compressed.contains("# Book Part One"));
        assert!(compressed.contains("# Book Epilogue"));
        assert!(compressed.contains("Final compiler summary"));
    }

    #[test]
    fn test_compress_unstructured_prose_retains_salient_middle() {
        let mut paras = Vec::new();
        paras.push(
            "Telemetry pipelines collect distributed tracing spans across Kubernetes microservices."
                .to_string(),
        );
        for i in 0..25 {
            paras.push(format!(
                "Unrelated cafeteria update {}: soup flavors, napkin dispensers, and tray return etiquette.",
                i
            ));
        }
        paras.push(
            "Critical measurement: sampling distributed tracing spans across Kubernetes microservices cut telemetry storage costs by 62 percent."
                .to_string(),
        );
        for i in 0..25 {
            paras.push(format!(
                "Unrelated shuttle bulletin {}: bus departure gate and windshield wiper schedule.",
                i
            ));
        }
        paras.push(
            "Overall conclusion: telemetry pipelines and distributed tracing spans scaled reliably."
                .to_string(),
        );

        let doc = paras.join("\n\n");
        let budget = 65;
        let compressed = compress_document_stratified(&doc, budget, TokenizerKind::Cl100k);
        assert!(TokenizerKind::Cl100k.count_tokens(&compressed) <= budget);
        assert!(compressed.contains("Telemetry pipelines collect"));
        assert!(compressed.contains("cut telemetry storage costs by 62 percent"));
        assert!(compressed.contains("Overall conclusion"));
        assert!(!compressed.contains("cafeteria update"));
    }

    #[test]
    fn test_compress_budget_invariants_across_tokenizers_and_tight_budgets() {
        let mut paras = Vec::new();
        paras.push("# Main Title".to_string());
        for i in 0..30 {
            paras.push(format!("## Section {}", i));
            paras.push(format!(
                "Body content {} covering throughput, memory pooling, and vectorized hashing algorithms.",
                i
            ));
        }
        paras.push("Concluding remarks verifying system stability.".to_string());
        let doc = paras.join("\n\n");

        let single_blob = "word ".repeat(300);

        for tok in [
            TokenizerKind::Cl100k,
            TokenizerKind::O200k,
            TokenizerKind::Llama3,
            TokenizerKind::Gemma,
        ] {
            for budget in [0usize, 1, 5, 25, 80, 250] {
                let out_doc = compress_document_stratified(&doc, budget, tok);
                assert!(
                    tok.count_tokens(&out_doc) <= budget,
                    "doc exceeded budget={budget} for {tok:?}: got {}",
                    tok.count_tokens(&out_doc)
                );

                let out_blob = compress_document_stratified(&single_blob, budget, tok);
                assert!(
                    tok.count_tokens(&out_blob) <= budget,
                    "blob exceeded budget={budget} for {tok:?}: got {}",
                    tok.count_tokens(&out_blob)
                );
            }

            let out_zero = compress_document_stratified(&doc, 0, tok);
            assert_eq!(out_zero, "");
        }
    }

    #[test]
    fn test_compress_large_document_retains_structure_and_findings() {
        let mut parts = Vec::new();
        parts.push("# Comprehensive Systems Engineering Compendium".to_string());
        parts.push(
            "Executive overview detailing kernel bypass networking, lock-free ring buffers, and cache alignment."
                .to_string(),
        );

        for ch in 0..20 {
            parts.push(format!("## Chapter {} Subsystem Analysis", ch + 1));
            for p in 0..95 {
                if ch == 10 && p == 40 {
                    parts.push(
                        "Breakthrough empirical result: lock-free ring buffers with kernel bypass networking achieved 14.2 million packets per second at 99th percentile latency under 18 microseconds."
                            .to_string(),
                    );
                } else {
                    parts.push(format!(
                        "Routine procedural audit log entry ch{} item{} documenting routine checklist verification and timestamp archival.",
                        ch, p
                    ));
                }
            }
        }
        parts.push("## Final Conclusion".to_string());
        parts.push(
            "In summary, kernel bypass networking and lock-free ring buffers govern ultra-low-latency throughput."
                .to_string(),
        );

        let large_doc = parts.join("\n\n");
        let budget = 8_000;
        let t0 = std::time::Instant::now();
        let compressed = compress_document_stratified(&large_doc, budget, TokenizerKind::Cl100k);
        let elapsed = t0.elapsed();

        let out_tokens = TokenizerKind::Cl100k.count_tokens(&compressed);
        eprintln!(
            "large_doc bytes={} seg_budget={} out_tokens={} elapsed_ms={:.2}",
            large_doc.len(),
            budget,
            out_tokens,
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(out_tokens <= budget);
        assert!(compressed.contains("# Comprehensive Systems Engineering Compendium"));
        assert!(compressed.contains("## Chapter 11 Subsystem Analysis"));
        assert!(compressed.contains("14.2 million packets per second"));
        assert!(compressed.contains("## Final Conclusion"));

        let tight_budget = 400;
        let tight = compress_document_stratified(&large_doc, tight_budget, TokenizerKind::Cl100k);
        assert!(TokenizerKind::Cl100k.count_tokens(&tight) <= tight_budget);
        assert!(tight.contains("14.2 million packets per second"));
        assert!(tight.contains("## Final Conclusion"));
    }

    #[test]
    fn test_select_relevant_context_bm25() {
        let chunk1 = format!(
            "{}\n\n",
            "This is the first section. It discusses the general background of the topic in the world."
                .repeat(25)
        );
        let chunk2 = format!(
            "{}\n\n",
            "Quantum cryptography utilizes photon entanglement to guarantee secure key distribution."
                .repeat(25)
        );
        let chunk3 = format!(
            "{}\n\n",
            "Photosynthesis in plants converts solar energy into chemical sugars.".repeat(25)
        );

        let combined = format!("{}{}{}", chunk1, chunk2, chunk3);
        let question = "What is the main advantage of quantum cryptography?";
        for kind in [TokenizerKind::Cl100k, TokenizerKind::Llama3] {
            let selected = select_relevant_context_bm25(&combined, Some(question), 600, kind);
            assert!(selected.contains("Quantum cryptography utilizes photon entanglement"));
        }
    }
}
