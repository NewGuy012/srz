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

struct Segment<'a> {
    text: &'a str,
    tokens: usize,
    heading_level: Option<u8>,
    after_heading: bool,
}

/// Detects ATX Markdown headings (`#`, `##`, `###`), including YouTube chapter
/// headings prefixed with `[MM:SS]` or `[HH:MM:SS]`.
fn parse_heading_level(raw_line: &str) -> Option<u8> {
    if raw_line.starts_with('\t') {
        return None;
    }
    let spaces = raw_line.bytes().take_while(|&b| b == b' ').count();
    if spaces >= 4 {
        return None;
    }

    let mut line = raw_line[spaces..].trim_end();
    if let Some(rest) = line.strip_prefix('[')
        && let Some((ts, after)) = rest.split_once(']')
        && !ts.is_empty()
        && ts.contains(':')
        && ts.bytes().all(|b| b.is_ascii_digit() || b == b':')
    {
        line = after.trim_start_matches([' ', '\t']);
    }

    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let title = &line[hashes..];
    (title.starts_with([' ', '\t']) && !title.trim().is_empty()).then_some(hashes as u8)
}

fn push_body_segment<'a>(
    block: &'a str,
    max_seg_tokens: usize,
    tok_kind: TokenizerKind,
    after_heading: &mut bool,
    segments: &mut Vec<Segment<'a>>,
) {
    let text = block.trim();
    if text.is_empty() {
        return;
    }
    let tokens = tok_kind.count_tokens(text);
    if tokens <= max_seg_tokens {
        segments.push(Segment {
            text,
            tokens,
            heading_level: None,
            after_heading: *after_heading,
        });
        *after_heading = false;
        return;
    }

    // Fallback for giant unbroken lines without newlines: chunk at whitespace boundaries.
    let target_words = ((max_seg_tokens * 3) / 5).max(8);
    let base_ptr = text.as_ptr() as usize;
    let mut words = text.split_whitespace();
    let mut start = 0usize;
    let mut first = true;

    while let Some(last_word) = words.by_ref().take(target_words).last() {
        let end = (last_word.as_ptr() as usize) - base_ptr + last_word.len();
        let piece = text[start..end].trim();
        if !piece.is_empty() {
            segments.push(Segment {
                text: piece,
                tokens: tok_kind.count_tokens(piece),
                heading_level: None,
                after_heading: first && *after_heading,
            });
            first = false;
        }
        start = end;
    }
    *after_heading = false;
}

/// Splits `content` into zero-copy heading (`#`, `##`, `###`) and body segments while ignoring
/// `#` lines inside fenced code blocks and bounding huge blocks to `max_seg_tokens`.
fn parse_document_segments<'a>(
    content: &'a str,
    max_seg_tokens: usize,
    tok_kind: TokenizerKind,
) -> Vec<Segment<'a>> {
    let mut segments = Vec::new();
    let mut body: Option<(usize, usize, usize)> = None; // (start_byte, end_byte, approx_tokens)
    let mut fence: Option<(u8, usize)> = None;
    let mut after_heading = false;
    let mut offset = 0;

    let flush_body = |body: &mut Option<(usize, usize, usize)>,
                      after_h: &mut bool,
                      segs: &mut Vec<Segment<'a>>| {
        if let Some((s, e, _)) = body.take() {
            push_body_segment(&content[s..e], max_seg_tokens, tok_kind, after_h, segs);
        }
    };

    for raw in content.split_inclusive('\n') {
        let line_start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();

        if let Some((fc, flen)) = fence {
            if trimmed.is_empty() {
                continue;
            }
            let close_len = trimmed.bytes().take_while(|&b| b == fc).count();
            if close_len >= flen && trimmed[close_len..].trim().is_empty() {
                fence = None;
            }
        } else if trimmed.is_empty() {
            flush_body(&mut body, &mut after_heading, &mut segments);
            continue;
        } else {
            let first_byte = trimmed.as_bytes()[0];
            if first_byte == b'`' || first_byte == b'~' {
                let flen = trimmed.bytes().take_while(|&b| b == first_byte).count();
                if flen >= 3 && !(first_byte == b'`' && trimmed[flen..].contains('`')) {
                    fence = Some((first_byte, flen));
                }
            }
            if fence.is_none()
                && let Some(level) = parse_heading_level(line)
            {
                flush_body(&mut body, &mut after_heading, &mut segments);
                segments.push(Segment {
                    text: trimmed,
                    tokens: tok_kind.count_tokens(trimmed),
                    heading_level: Some(level),
                    after_heading: false,
                });
                after_heading = true;
                continue;
            }
        }

        let cs = line_start + (line.len() - line.trim_start().len());
        let ce = line_start + line.trim_end().len();
        let lt = tok_kind.count_tokens(trimmed);
        if body.is_some_and(|(_, _, bt)| bt + lt > max_seg_tokens) {
            flush_body(&mut body, &mut after_heading, &mut segments);
        }
        body = Some(match body {
            Some((s, _, bt)) => (s, ce, bt + lt),
            None => (cs, ce, lt),
        });
    }

    flush_body(&mut body, &mut after_heading, &mut segments);
    segments
}

/// Scores document segments in a single O(bytes) pass against top TF-IDF centroid terms
/// (boosting heading and lead/tail anchor terms) via [`score_bm25_matrix`].
fn score_segments_centroid_bm25(segments: &[Segment<'_>]) -> Vec<f64> {
    const MAX_CENTROID_TERMS: usize = 48;
    const MAX_VOCAB: usize = 50_000;

    struct TermEntry {
        df: usize,
        weight: f64,
        in_heading: bool,
        in_anchor: bool,
        last_seg: usize,
        postings: Vec<(usize, usize)>,
    }

    let n_docs = segments.len();
    if n_docs == 0 {
        return Vec::new();
    }

    let anchor_window = (n_docs / 20).clamp(1, 4);
    let mut vocab: HashMap<String, TermEntry> = HashMap::new();
    let mut scratch = String::with_capacity(48);
    let mut lengths = Vec::with_capacity(n_docs);
    let mut total_tokens = 0usize;

    for (seg_idx, seg) in segments.iter().enumerate() {
        lengths.push(seg.tokens);
        total_tokens += seg.tokens;
        let is_heading = seg.heading_level.is_some();
        let is_anchor = seg_idx < anchor_window || seg_idx >= n_docs.saturating_sub(anchor_window);
        let weight_inc = if is_heading {
            3.0
        } else if is_anchor {
            2.0
        } else {
            1.0
        };

        for raw_tok in seg.text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            let tok = raw_tok.trim_end_matches('_');
            if tok.len() < 2 || !tok.chars().any(char::is_alphabetic) {
                continue;
            }
            scratch.clear();
            if tok.is_ascii() {
                scratch.push_str(tok);
                scratch.make_ascii_lowercase();
            } else {
                scratch.extend(tok.chars().flat_map(char::to_lowercase));
            }
            if is_stop_word_lower(&scratch) {
                continue;
            }

            if let Some(e) = vocab.get_mut(scratch.as_str()) {
                e.weight += weight_inc;
                e.in_heading |= is_heading;
                e.in_anchor |= is_anchor;
                if e.last_seg == seg_idx {
                    if let Some(last) = e.postings.last_mut() {
                        last.1 += 1;
                    }
                } else {
                    e.df += 1;
                    e.last_seg = seg_idx;
                    e.postings.push((seg_idx, 1));
                }
            } else if vocab.len() < MAX_VOCAB {
                vocab.insert(
                    scratch.clone(),
                    TermEntry {
                        df: 1,
                        weight: weight_inc,
                        in_heading: is_heading,
                        in_anchor: is_anchor,
                        last_seg: seg_idx,
                        postings: vec![(seg_idx, 1)],
                    },
                );
            }
        }
    }

    if vocab.is_empty() {
        return vec![0.0; n_docs];
    }

    let mut ranked: Vec<(f64, TermEntry)> = vocab
        .into_values()
        .map(|e| {
            let idf = (((n_docs.saturating_sub(e.df)) as f64 + 0.5) / (e.df as f64 + 0.5) + 1.0)
                .ln()
                .max(0.1);
            let spread = if n_docs >= 8 && e.df == 1 && !e.in_heading && !e.in_anchor {
                0.35
            } else {
                1.0
            };
            let mult = if e.in_heading { 2.0 } else { 1.0 } * if e.in_anchor { 2.0 } else { 1.0 };
            (e.weight.ln_1p() * idf * spread * mult, e)
        })
        .collect();

    if ranked.len() > MAX_CENTROID_TERMS {
        ranked.select_nth_unstable_by(MAX_CENTROID_TERMS, |a, b| b.0.total_cmp(&a.0));
        ranked.truncate(MAX_CENTROID_TERMS);
    }

    let n_terms = ranked.len();
    let mut term_dfs = Vec::with_capacity(n_terms);
    let mut flat_tfs = vec![0usize; n_docs * n_terms];
    for (term_idx, (_, entry)) in ranked.into_iter().enumerate() {
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

fn gap_ellipsis_delta(idx: usize, selected: &[bool], bounds: Option<(usize, usize)>) -> isize {
    let Some((min_s, max_s)) = bounds else {
        return 0;
    };
    let left_adj = idx > 0 && selected[idx - 1];
    let right_adj = idx + 1 < selected.len() && selected[idx + 1];
    match (min_s < idx, max_s > idx) {
        (false, false) => 0,
        (true, false) => isize::from(!left_adj),
        (false, true) => isize::from(!right_adj),
        (true, true) => isize::from(!left_adj) + isize::from(!right_adj) - 1,
    }
}

fn assemble_segments(segments: &[Segment<'_>], selected: &[bool]) -> String {
    let mut out = String::new();
    let mut prev_idx: Option<usize> = None;
    for (idx, seg) in segments.iter().enumerate() {
        if !selected[idx] {
            continue;
        }
        if let Some(prev) = prev_idx {
            out.push_str(if idx == prev + 1 {
                "\n\n"
            } else {
                "\n\n...\n\n"
            });
        }
        out.push_str(seg.text);
        prev_idx = Some(idx);
    }
    out
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
    if content.len() <= max_tokens || tok_kind.count_tokens(content) <= max_tokens {
        return Cow::Borrowed(content);
    }

    let segments = parse_document_segments(content, (max_tokens / 5).clamp(16, 512), tok_kind);
    if segments.is_empty() {
        return Cow::Borrowed("");
    }
    if segments.len() <= 1 {
        return TextChunker::truncate_to_budget(content, max_tokens, tok_kind);
    }

    let n = segments.len();
    let ellipsis_tokens = tok_kind.count_tokens("...");
    let lead_cap = (max_tokens * 25) / 100;
    let tail_cap = (max_tokens * 15) / 100;
    let heading_cap = (max_tokens * 30) / 100;

    let mut selected = vec![false; n];
    let mut bounds: Option<(usize, usize)> = None;
    let mut used = 0usize;

    let try_add = |idx: usize,
                   limit: usize,
                   selected: &mut [bool],
                   bounds: &mut Option<(usize, usize)>,
                   used: &mut usize|
     -> Option<usize> {
        if selected[idx] {
            return None;
        }
        let delta = gap_ellipsis_delta(idx, selected, *bounds) * (ellipsis_tokens as isize);
        let cost = (segments[idx].tokens as isize + delta).max(0) as usize;
        if *used + cost > limit {
            return None;
        }
        selected[idx] = true;
        *used += cost;
        *bounds = Some(match *bounds {
            Some((lo, hi)) => (lo.min(idx), hi.max(idx)),
            None => (idx, idx),
        });
        Some(cost)
    };

    // 1. Anchor lead (0) and tail (n - 1), then expand contiguous lead/tail windows.
    let mut lead_used = try_add(0, max_tokens, &mut selected, &mut bounds, &mut used).unwrap_or(0);
    let mut tail_used =
        try_add(n - 1, max_tokens, &mut selected, &mut bounds, &mut used).unwrap_or(0);
    let mut inner_lead_tail = Vec::new();

    for (idx, seg) in segments.iter().enumerate().take(n - 1).skip(1) {
        if lead_used + seg.tokens > lead_cap {
            break;
        }
        let Some(cost) = try_add(idx, max_tokens, &mut selected, &mut bounds, &mut used) else {
            break;
        };
        lead_used += cost;
        inner_lead_tail.push(idx);
    }
    for idx in (1..n - 1).rev() {
        if selected[idx] || tail_used + segments[idx].tokens > tail_cap {
            break;
        }
        let Some(cost) = try_add(idx, max_tokens, &mut selected, &mut bounds, &mut used) else {
            break;
        };
        tail_used += cost;
        inner_lead_tail.push(idx);
    }

    // 2. Preserve outline skeleton of `#`, `##`, `###` headings across middle segments.
    let mut heading_used = 0usize;
    let mut picked_headings = Vec::new();
    for level in 1u8..=3 {
        let level_indices: Vec<usize> = segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (!selected[i] && s.heading_level == Some(level)).then_some(i))
            .collect();
        let heading_left = heading_cap
            .saturating_sub(heading_used)
            .min(max_tokens.saturating_sub(used));
        if level_indices.is_empty() || heading_left == 0 {
            continue;
        }

        let est_cost: usize = level_indices
            .iter()
            .map(|&i| segments[i].tokens + ellipsis_tokens)
            .sum();
        let count = if est_cost <= heading_left {
            level_indices.len()
        } else {
            ((level_indices.len() * heading_left) / est_cost.max(1))
                .max(1)
                .min(level_indices.len())
        };
        let span = level_indices.len().saturating_sub(1);
        let denom = count.saturating_sub(1).max(1);

        for step in 0..count {
            let pos = if count == 1 { 0 } else { (step * span) / denom };
            let limit = (used + heading_cap.saturating_sub(heading_used)).min(max_tokens);
            if let Some(cost) = try_add(
                level_indices[pos],
                limit,
                &mut selected,
                &mut bounds,
                &mut used,
            ) {
                heading_used += cost;
                picked_headings.push(level_indices[pos]);
            }
        }
    }

    // 3. Fill remaining budget with top centroid BM25-salient middle body paragraphs.
    let scores = score_segments_centroid_bm25(&segments);
    let mut candidates: Vec<(f64, usize)> = segments
        .iter()
        .enumerate()
        .filter_map(|(i, s)| (!selected[i] && s.heading_level.is_none()).then_some((scores[i], i)))
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut picked_body = Vec::new();
    for (_, idx) in candidates {
        if used >= max_tokens {
            break;
        }
        if try_add(idx, max_tokens, &mut selected, &mut bounds, &mut used).is_some() {
            picked_body.push(idx);
        }
    }

    // 4. Assemble and enforce strict <= max_tokens under `tok_kind`.
    let mut out = assemble_segments(&segments, &selected);
    if out.is_empty() {
        return TextChunker::truncate_to_budget(content, max_tokens, tok_kind);
    }

    // `pop()` removes from the back: drop lowest-salience body first, then headings, then inner lead/tail.
    let mut evictable: Vec<usize> = inner_lead_tail
        .into_iter()
        .chain(picked_headings)
        .chain(picked_body)
        .collect();
    let mut selected_count = selected.iter().filter(|&&s| s).count();

    while tok_kind.count_tokens(&out) > max_tokens {
        if selected_count <= 1 {
            return Cow::Owned(
                TextChunker::truncate_to_budget(&out, max_tokens, tok_kind).into_owned(),
            );
        }
        let Some(drop_idx) = evictable.pop() else {
            return Cow::Owned(
                TextChunker::truncate_to_budget(&out, max_tokens, tok_kind).into_owned(),
            );
        };
        if selected[drop_idx] {
            selected[drop_idx] = false;
            selected_count -= 1;
            out = assemble_segments(&segments, &selected);
        }
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
