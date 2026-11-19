use std::borrow::Cow;
use std::collections::HashMap;

use crate::chunker::{TextChunker, TokenizerKind};
use crate::system::text_util::{is_stop_word, tokenize_terms};

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

#[inline]
fn bm25_idf(n_docs: usize, df: usize) -> f64 {
    if df == 0 {
        return 0.0;
    }
    (((n_docs.saturating_sub(df)) as f64 + 0.5) / (df as f64 + 0.5) + 1.0)
        .ln()
        .max(0.1)
}

fn for_each_word_token(text: &str, scratch: &mut String, mut f: impl FnMut(&str)) {
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let tok = raw.trim_end_matches('_');
        if tok.is_empty() {
            continue;
        }
        scratch.clear();
        if tok.is_ascii() {
            scratch.push_str(tok);
            scratch.make_ascii_lowercase();
        } else {
            scratch.extend(tok.chars().flat_map(char::to_lowercase));
        }
        f(scratch.as_str());
    }
}

fn accumulate_bm25_postings(
    scores: &mut [f64],
    norm_lens: &[f64],
    n_docs: usize,
    df: usize,
    postings: &[(usize, usize)],
) {
    let idf = bm25_idf(n_docs, df);
    for &(seg_idx, tf_usize) in postings {
        let tf = tf_usize as f64;
        scores[seg_idx] += idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * norm_lens[seg_idx]);
    }
}

/// Selects the most relevant sections of `content` when it exceeds `max_tokens` using
/// model-aware token counting and BM25 ranking.
pub(crate) fn select_relevant_context_bm25<'a>(
    content: &'a str,
    question: Option<&str>,
    max_tokens: usize,
    tokenizer_kind: TokenizerKind,
) -> Cow<'a, str> {
    if max_tokens == 0 {
        return Cow::Borrowed("");
    }
    if content.len().saturating_mul(3) <= max_tokens
        || tokenizer_kind.count_tokens(content) <= max_tokens
    {
        return Cow::Borrowed(content);
    }

    let Some(q) = question else {
        return compress_document_stratified(content, max_tokens, tokenizer_kind);
    };

    let raw_terms = tokenize_terms(q);
    if raw_terms.is_empty() {
        return compress_document_stratified(content, max_tokens, tokenizer_kind);
    }

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

    let mut term_index: HashMap<String, usize> = HashMap::new();
    for term in q_terms {
        let next_id = term_index.len();
        term_index.entry(term).or_insert(next_id);
    }

    let max_chunk_cap = (max_tokens / 2).clamp(16, 2000);
    let chunk_size = (max_tokens / 8).clamp(64, 2000).min(max_chunk_cap);
    let chunk_overlap = (chunk_size / 12).clamp(8, 150).min(chunk_size / 3);
    let chunks_with_counts = TextChunker::chunk_by_tokens_with_counts_and_kind(
        content,
        chunk_size,
        chunk_overlap,
        tokenizer_kind,
    );
    let total_tokens: usize = chunks_with_counts.iter().map(|(_, t)| *t).sum();
    if chunks_with_counts.len() <= 1 || total_tokens <= max_tokens {
        return Cow::Borrowed(TextChunker::truncate_to_budget(
            content,
            max_tokens,
            tokenizer_kind,
        ));
    }

    let n_docs = chunks_with_counts.len();
    let avgdl = (total_tokens as f64 / n_docs as f64).max(1.0);
    let mut term_postings: Vec<Vec<(usize, usize)>> = vec![Vec::new(); term_index.len()];
    let mut doc_counts = vec![0usize; term_index.len()];
    let mut scratch = String::with_capacity(32);

    for (doc_idx, (chunk, _)) in chunks_with_counts.iter().enumerate() {
        doc_counts.fill(0);
        for_each_word_token(chunk.as_ref(), &mut scratch, |tok| {
            if let Some(&tid) = term_index.get(tok) {
                doc_counts[tid] += 1;
            }
        });
        for (tid, &tf) in doc_counts.iter().enumerate() {
            if tf > 0 {
                term_postings[tid].push((doc_idx, tf));
            }
        }
    }

    let mut scores = vec![0.0f64; n_docs];
    let norm_lens: Vec<f64> = chunks_with_counts
        .iter()
        .map(|(_, len)| 1.0 - BM25_B + BM25_B * (*len as f64 / avgdl))
        .collect();
    for postings in &term_postings {
        accumulate_bm25_postings(&mut scores, &norm_lens, n_docs, postings.len(), postings);
    }

    let mut scored: Vec<(usize, f64, usize)> = chunks_with_counts
        .iter()
        .enumerate()
        .map(|(idx, (_, tokens))| (idx, scores[idx] + if idx == 0 { 0.5 } else { 0.0 }, *tokens))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let sep_tokens = tokenizer_kind.count_tokens("...");
    let mut selected_indices = Vec::new();
    let mut token_sum = 0usize;
    for &(idx, _, tokens) in &scored {
        let extra = if selected_indices.is_empty() {
            0
        } else {
            sep_tokens
        };
        let cost = tokens.saturating_add(extra);
        if token_sum.saturating_add(cost) <= max_tokens {
            selected_indices.push(idx);
            token_sum = token_sum.saturating_add(cost);
        }
    }
    if selected_indices.is_empty()
        && let Some(&(best_idx, _, _)) = scored.first()
    {
        return Cow::Owned(
            TextChunker::truncate_to_budget(
                chunks_with_counts[best_idx].0.as_ref(),
                max_tokens,
                tokenizer_kind,
            )
            .to_string(),
        );
    }

    let mut rank_order: Vec<usize> = selected_indices.clone();
    loop {
        let mut ordered = selected_indices.clone();
        ordered.sort_unstable();
        let joined: String =
            ordered
                .iter()
                .enumerate()
                .fold(String::new(), |mut acc, (i, &idx)| {
                    if i > 0 {
                        acc.push_str("\n\n...\n\n");
                    }
                    acc.push_str(chunks_with_counts[idx].0.as_ref());
                    acc
                });
        if tokenizer_kind.count_tokens(&joined) <= max_tokens {
            return Cow::Owned(joined);
        }
        if selected_indices.len() <= 1 {
            return Cow::Owned(
                TextChunker::truncate_to_budget(&joined, max_tokens, tokenizer_kind).to_string(),
            );
        }
        if let Some(drop_idx) = rank_order.pop() {
            selected_indices.retain(|&i| i != drop_idx);
        } else {
            break;
        }
    }
    Cow::Borrowed("")
}

struct Segment<'a> {
    text: &'a str,
    tokens: usize,
    heading_level: Option<u8>,
    after_heading: bool,
}

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

    let mut split_chunks = Vec::new();
    TextChunker::decompose_line(text, max_seg_tokens, tok_kind, &mut split_chunks);
    for (i, (piece, piece_tokens)) in split_chunks.into_iter().enumerate() {
        segments.push(Segment {
            text: piece,
            tokens: piece_tokens,
            heading_level: None,
            after_heading: i == 0 && *after_heading,
        });
    }
    *after_heading = false;
}

fn parse_document_segments<'a>(
    content: &'a str,
    max_seg_tokens: usize,
    tok_kind: TokenizerKind,
) -> Vec<Segment<'a>> {
    let mut segments = Vec::new();
    let mut body: Option<(usize, usize, usize)> = None;
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
            let leading_spaces = line.bytes().take_while(|&b| b == b' ').count();
            let first_byte = trimmed.as_bytes()[0];
            if !line.starts_with('\t')
                && leading_spaces < 4
                && (first_byte == b'`' || first_byte == b'~')
            {
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
        if body.is_some_and(|(_, _, bt)| bt.saturating_add(lt) > max_seg_tokens) {
            flush_body(&mut body, &mut after_heading, &mut segments);
        }
        body = Some(match body {
            Some((s, _, bt)) => (s, ce, bt.saturating_add(lt)),
            None => (cs, ce, lt),
        });
    }

    flush_body(&mut body, &mut after_heading, &mut segments);
    segments
}

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
        total_tokens = total_tokens.saturating_add(seg.tokens);
        let is_heading = seg.heading_level.is_some();
        let is_anchor = seg_idx < anchor_window || seg_idx >= n_docs.saturating_sub(anchor_window);
        let weight_inc = if is_heading {
            3.0
        } else if is_anchor {
            2.0
        } else {
            1.0
        };

        for_each_word_token(seg.text, &mut scratch, |tok| {
            if tok.len() < 2 || !tok.chars().any(char::is_alphabetic) || is_stop_word(tok) {
                return;
            }
            if let Some(e) = vocab.get_mut(tok) {
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
                    tok.to_string(),
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
        });
    }

    if vocab.is_empty() {
        return vec![0.0; n_docs];
    }

    let mut ranked: Vec<(f64, TermEntry)> = vocab
        .into_values()
        .map(|e| {
            let idf = bm25_idf(n_docs, e.df);
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

    let avgdl = (total_tokens as f64 / n_docs as f64).max(1.0);
    let norm_lens: Vec<f64> = lengths
        .iter()
        .map(|&len| 1.0 - BM25_B + BM25_B * (len as f64 / avgdl))
        .collect();
    let mut scores = vec![0.0f64; n_docs];
    for (_, entry) in ranked {
        accumulate_bm25_postings(&mut scores, &norm_lens, n_docs, entry.df, &entry.postings);
    }
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

/// Compresses an oversized document to `max_tokens` retaining lead/tail, `#`..`###` headings,
/// and centroid BM25-salient paragraphs.
pub(crate) fn compress_document_stratified<'a>(
    content: &'a str,
    max_tokens: usize,
    tok_kind: TokenizerKind,
) -> Cow<'a, str> {
    if max_tokens == 0 {
        return Cow::Borrowed("");
    }
    if content.len().saturating_mul(3) <= max_tokens || tok_kind.count_tokens(content) <= max_tokens
    {
        return Cow::Borrowed(content);
    }

    let segments = parse_document_segments(content, (max_tokens / 5).clamp(16, 512), tok_kind);
    if segments.is_empty() {
        return Cow::Borrowed("");
    }
    if segments.len() <= 1 {
        return Cow::Borrowed(TextChunker::truncate_to_budget(
            content, max_tokens, tok_kind,
        ));
    }

    let n = segments.len();
    let ellipsis_tokens = tok_kind.count_tokens("...");
    let lead_cap = max_tokens.saturating_mul(25) / 100;
    let tail_cap = max_tokens.saturating_mul(15) / 100;
    let heading_cap = max_tokens.saturating_mul(30) / 100;

    let mut selected = vec![false; n];
    let mut used = 0usize;

    let try_add =
        |idx: usize, limit: usize, selected: &mut [bool], used: &mut usize| -> Option<usize> {
            if selected[idx] {
                return None;
            }
            let adjacent = (idx > 0 && selected[idx - 1]) || (idx + 1 < n && selected[idx + 1]);
            let gap_cost = if *used == 0 || adjacent {
                0
            } else {
                ellipsis_tokens
            };
            let cost = segments[idx].tokens.saturating_add(gap_cost);
            if used.saturating_add(cost) > limit {
                return None;
            }
            selected[idx] = true;
            *used = used.saturating_add(cost);
            Some(cost)
        };

    let mut lead_used = try_add(0, max_tokens, &mut selected, &mut used).unwrap_or(0);
    let mut tail_used = try_add(n - 1, max_tokens, &mut selected, &mut used).unwrap_or(0);
    let mut inner_lead_tail = Vec::new();

    for (idx, seg) in segments.iter().enumerate().take(n - 1).skip(1) {
        if lead_used.saturating_add(seg.tokens) > lead_cap {
            break;
        }
        let Some(cost) = try_add(idx, max_tokens, &mut selected, &mut used) else {
            break;
        };
        lead_used = lead_used.saturating_add(cost);
        inner_lead_tail.push(idx);
    }
    for idx in (1..n - 1).rev() {
        if selected[idx] || tail_used.saturating_add(segments[idx].tokens) > tail_cap {
            break;
        }
        let Some(cost) = try_add(idx, max_tokens, &mut selected, &mut used) else {
            break;
        };
        tail_used = tail_used.saturating_add(cost);
        inner_lead_tail.push(idx);
    }

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
            .map(|&i| segments[i].tokens.saturating_add(ellipsis_tokens))
            .sum();
        let count = if est_cost <= heading_left {
            level_indices.len()
        } else {
            (level_indices.len().saturating_mul(heading_left) / est_cost.max(1))
                .max(1)
                .min(level_indices.len())
        };
        let span = level_indices.len().saturating_sub(1);
        let denom = count.saturating_sub(1).max(1);

        for step in 0..count {
            let pos = if count == 1 {
                0
            } else {
                step.saturating_mul(span) / denom
            };
            let limit = used
                .saturating_add(heading_cap.saturating_sub(heading_used))
                .min(max_tokens);
            if let Some(cost) = try_add(level_indices[pos], limit, &mut selected, &mut used) {
                heading_used = heading_used.saturating_add(cost);
                picked_headings.push(level_indices[pos]);
            }
        }
    }

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
        if try_add(idx, max_tokens, &mut selected, &mut used).is_some() {
            picked_body.push(idx);
        }
    }

    let mut out = assemble_segments(&segments, &selected);
    if out.is_empty() {
        return Cow::Borrowed(TextChunker::truncate_to_budget(
            content, max_tokens, tok_kind,
        ));
    }

    let mut evictable: Vec<usize> = inner_lead_tail
        .into_iter()
        .chain(picked_headings)
        .chain(picked_body)
        .collect();
    let mut selected_count = selected.iter().filter(|&&s| s).count();

    loop {
        let cur_tokens = tok_kind.count_tokens(&out);
        if cur_tokens <= max_tokens {
            break;
        }
        if selected_count <= 1 || evictable.is_empty() {
            return Cow::Owned(
                TextChunker::truncate_to_budget(&out, max_tokens, tok_kind).to_string(),
            );
        }
        let target_shed = cur_tokens.saturating_sub(max_tokens);
        let mut shed = 0usize;
        while selected_count > 1 && shed < target_shed {
            let Some(drop_idx) = evictable.pop() else {
                break;
            };
            if selected[drop_idx] {
                selected[drop_idx] = false;
                selected_count -= 1;
                shed = shed.saturating_add(segments[drop_idx].tokens.max(1));
            }
        }
        out = assemble_segments(&segments, &selected);
    }

    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading_levels_fences_and_indented_code() {
        assert_eq!(parse_heading_level("# Title"), Some(1));
        assert_eq!(parse_heading_level("  ## Two"), Some(2));
        assert_eq!(parse_heading_level("[05:12] ## Chapter"), Some(2));
        assert_eq!(parse_heading_level("    # indented code"), None);
        assert_eq!(parse_heading_level("[1] ## Footnote"), None);

        let doc = "# Real H1\n\n```py\n# fake comment\n```\n\n    ```bash\n\n## Real H2 After Indented Backticks";
        let segs = parse_document_segments(doc, 256, TokenizerKind::Cl100k);
        let headings: Vec<&str> = segs
            .iter()
            .filter(|s| s.heading_level.is_some())
            .map(|s| s.text)
            .collect();
        assert_eq!(
            headings,
            vec!["# Real H1", "## Real H2 After Indented Backticks"]
        );
    }

    #[test]
    fn test_compress_skeleton_salient_findings_and_timestamps() {
        let mut parts = vec![
            "# Systems Report".to_string(),
            "Opening summary introducing distributed database storage latency.".to_string(),
            "## Architecture".to_string(),
        ];
        for i in 0..15 {
            parts.push(format!(
                "Administrative catering update {i}: parking permits and desks."
            ));
        }
        parts.push("## Benchmarks".to_string());
        parts.push("Core finding: distributed database storage latency dropped by 47 percent across replication clusters.".to_string());
        for i in 0..15 {
            parts.push(format!(
                "Quarterly travel reimbursement memo {i}: hotel shuttle schedules."
            ));
        }
        parts.push("## Conclusion".to_string());
        parts.push(
            "In conclusion, distributed database replication met latency targets.".to_string(),
        );

        let doc = parts.join("\n\n");
        let compressed = compress_document_stratified(&doc, 110, TokenizerKind::Cl100k);
        assert!(TokenizerKind::Cl100k.count_tokens(&compressed) <= 110);
        for expected in [
            "# Systems Report",
            "## Architecture",
            "## Benchmarks",
            "## Conclusion",
            "dropped by 47 percent",
        ] {
            assert!(
                compressed.contains(expected),
                "missing {expected} in:\n{compressed}"
            );
        }
        assert!(!compressed.contains("catering update"));

        let yt_doc = "[00:00] ## Intro\nRust async executor scheduling.\n[05:12] ## Bottleneck\n"
            .to_string()
            + &(0..16)
                .map(|i| format!("Joke {i} about coffee mugs."))
                .collect::<Vec<_>>()
                .join("\n")
            + "\nKey takeaway: work-stealing async executor queues cut lock latency.\n[32:10] ## Summary\nWrap up.";
        let yt = compress_document_stratified(&yt_doc, 120, TokenizerKind::Cl100k);
        assert!(TokenizerKind::Cl100k.count_tokens(&yt) <= 120);
        assert!(yt.contains("[05:12] ## Bottleneck"));
        assert!(yt.contains("work-stealing async executor queues"));
    }

    #[test]
    fn test_compress_strict_token_budget_invariants() {
        let mut paras = vec!["# Main".to_string()];
        for i in 0..20 {
            paras.push(format!("## Section {i}"));
            paras.push(format!("Body {i} throughput pooling algorithms."));
        }
        let doc = paras.join("\n\n");
        let giant_unbroken = "A".repeat(2_000);

        for tok in [
            TokenizerKind::Cl100k,
            TokenizerKind::Llama3,
            TokenizerKind::Gemma,
        ] {
            for budget in [0usize, 1, 5, 25, 100] {
                let out_doc = compress_document_stratified(&doc, budget, tok);
                assert!(tok.count_tokens(&out_doc) <= budget);
                let out_giant = compress_document_stratified(&giant_unbroken, budget, tok);
                assert!(tok.count_tokens(&out_giant) <= budget);
            }
            assert!(tok.count_tokens(&compress_document_stratified("a", 1, tok)) <= 1);
            assert!(tok.count_tokens(&compress_document_stratified("a b", 3, tok)) <= 3);
        }
    }

    #[test]
    fn test_select_relevant_context_bm25_unicode_and_none_question() {
        let chunk1 = format!(
            "{}\n\n",
            "General background café history in Europe.".repeat(20)
        );
        let chunk2 = format!(
            "{}\n\n",
            "Quantum cryptography utilizes photon entanglement.".repeat(20)
        );
        let chunk3 = format!(
            "{}\n\n",
            "Photosynthesis converts solar light into sugars.".repeat(20)
        );
        let combined = format!("{chunk1}{chunk2}{chunk3}");

        for kind in [TokenizerKind::Cl100k, TokenizerKind::Llama3] {
            let selected =
                select_relevant_context_bm25(&combined, Some("quantum cryptography"), 300, kind);
            assert!(selected.contains("Quantum cryptography"));
            assert!(kind.count_tokens(&selected) <= 300);

            // Non-ASCII term matching without prefix false-positive (`caf` != `café`)
            let cafe = select_relevant_context_bm25(&combined, Some("Café"), 200, kind);
            assert!(cafe.contains("café history"));
            let caf_mismatch = select_relevant_context_bm25(&combined, Some("!!!"), 120, kind);
            assert!(kind.count_tokens(&caf_mismatch) <= 120);
        }

        let no_q = select_relevant_context_bm25(&combined, None, 100, TokenizerKind::Cl100k);
        assert!(TokenizerKind::Cl100k.count_tokens(&no_q) <= 100);
    }
}
