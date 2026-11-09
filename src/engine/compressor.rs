use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::chunker::{TextChunker, TokenizerKind};
use crate::system::text_util::{find_ascii_case_insensitive, is_stop_word, tokenize_terms};

#[inline]
fn is_word_boundary(text: &str, start: usize, len: usize) -> bool {
    let bytes = text.as_bytes();
    let before_ok = if start == 0 {
        true
    } else {
        !bytes[start - 1].is_ascii_alphanumeric() && bytes[start - 1] != b'_'
    };
    let after_ok = if start + len >= bytes.len() {
        true
    } else {
        !bytes[start + len].is_ascii_alphanumeric() && bytes[start + len] != b'_'
    };
    before_ok && after_ok
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

/// Computes Robertson-Sparck Jones BM25 relevance scores for a slice of text segments
/// against a list of terms given segment token lengths and average document length.
pub fn compute_bm25_scores(
    segments: &[&str],
    lengths: &[usize],
    terms: &[String],
    avgdl: f64,
) -> Vec<f64> {
    let n_docs = segments.len();
    if n_docs == 0 || terms.is_empty() {
        return vec![0.0; n_docs];
    }

    let n_terms = terms.len();
    let mut term_dfs = vec![0usize; n_terms];
    let mut segment_tfs = Vec::with_capacity(n_docs);

    for segment in segments {
        let mut tfs = Vec::with_capacity(n_terms);
        for (term_idx, term) in terms.iter().enumerate() {
            let tf = count_term_occurrences(segment, term);
            if tf > 0 {
                term_dfs[term_idx] += 1;
            }
            tfs.push(tf);
        }
        segment_tfs.push(tfs);
    }

    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    segments
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            let doc_len = lengths[idx] as f64;
            let norm_len = 1.0 - B + B * (doc_len / avgdl);
            let mut score = 0.0;

            for (term_idx, _) in terms.iter().enumerate() {
                let df = term_dfs[term_idx];
                if df == 0 {
                    continue;
                }
                let tf = segment_tfs[idx][term_idx] as f64;
                if tf > 0.0 {
                    let idf = (((n_docs - df) as f64 + 0.5) / (df as f64 + 0.5) + 1.0).ln();
                    let saturation = (tf * (K1 + 1.0)) / (tf + K1 * norm_len);
                    score += idf.max(0.1) * saturation;
                }
            }
            score
        })
        .collect()
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
    for (pos, idx) in selected_indices.into_iter().enumerate() {
        if pos > 0 {
            out.push_str("\n\n...\n\n");
        }
        out.push_str(&chunks_with_counts[idx].0);
    }
    Cow::Owned(out)
}

/// Compresses an oversized document into `max_tokens` using CPU Stratified Salience Compression.
/// Preserves global document structure (headings, beginning and end of sections/chapters)
/// and extracts highest-density informational paragraphs across all strata.
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

    let initial_tokens = tok_kind.count_tokens(content);
    if initial_tokens <= max_tokens {
        return Cow::Borrowed(content);
    }

    // 1. Decompose document into paragraphs
    let mut paragraphs: Vec<(&'a str, usize, bool)> = Vec::new();
    for p in content.split("\n\n") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            let tokens = tok_kind.count_tokens(trimmed);
            let is_heading = trimmed.starts_with('#')
                || trimmed.starts_with("Chapter ")
                || trimmed.starts_with("**Chapter")
                || trimmed.starts_with("Section ")
                || trimmed.starts_with("### Article");
            paragraphs.push((trimmed, tokens, is_heading));
        }
    }

    if paragraphs.is_empty() {
        return Cow::Borrowed(content);
    }

    if paragraphs.len() == 1 {
        return TextChunker::truncate_by_tokens(content, max_tokens);
    }

    // 2. Compute document centroid query (top ~40 non-stopword terms)
    let raw_terms = tokenize_terms(content);
    let mut term_freqs: HashMap<String, usize> = HashMap::new();
    for term in raw_terms {
        if !is_stop_word(&term) && term.len() > 1 {
            *term_freqs.entry(term).or_insert(0) += 1;
        }
    }

    let mut sorted_terms: Vec<(String, usize)> = term_freqs.into_iter().collect();
    sorted_terms.sort_by_key(|a| std::cmp::Reverse(a.1));
    let centroid_terms: Vec<String> = sorted_terms.into_iter().take(40).map(|(t, _)| t).collect();

    // 3. Compute BM25 relevance scores across paragraphs
    let n_paras = paragraphs.len();
    let total_para_tokens: usize = paragraphs.iter().map(|(_, t, _)| *t).sum();
    let avgdl = (total_para_tokens as f64) / (n_paras as f64).max(1.0);

    let para_texts: Vec<&str> = paragraphs.iter().map(|(p, _, _)| *p).collect();
    let para_lens: Vec<usize> = paragraphs.iter().map(|(_, t, _)| *t).collect();
    let bm25_scores = compute_bm25_scores(&para_texts, &para_lens, &centroid_terms, avgdl);

    // Score all paragraphs with BM25 + information density bonus (numbers/metrics)
    let para_scores: Vec<f64> = paragraphs
        .iter()
        .enumerate()
        .map(|(idx, (para, _, is_heading))| {
            if *is_heading {
                return 100.0; // Guaranteed structural priority
            }
            let mut score = bm25_scores[idx];

            // Information density bonus for digits / metrics
            let digit_count = para.chars().filter(|c| c.is_ascii_digit()).count();
            if digit_count > 0 {
                score += (digit_count as f64).min(5.0) * 0.2;
            }

            score
        })
        .collect();

    // 4. Stratified selection
    // Divide the document into K strata (e.g. 10 to 16 equal partitions)
    let num_strata = n_paras.clamp(1, 12);
    let mut selected_indices: HashSet<usize> = HashSet::new();
    let mut current_total_tokens = 0usize;

    // Safety margin for separator tokens (`\n\n...\n\n` is ~4 tokens each)
    let safe_budget = max_tokens
        .saturating_sub(num_strata * 8)
        .max(max_tokens / 2);
    let stratum_budget = (safe_budget / num_strata).max(100);

    for s in 0..num_strata {
        let start = s * n_paras / num_strata;
        let end = ((s + 1) * n_paras / num_strata).max(start + 1).min(n_paras);
        if start >= end {
            continue;
        }

        let mut stratum_tokens = 0usize;

        // Step 4a: Prioritize Headings in this stratum
        for (i, para) in paragraphs.iter().enumerate().take(end).skip(start) {
            if para.2 && current_total_tokens + para.1 <= safe_budget && selected_indices.insert(i)
            {
                current_total_tokens += para.1;
                stratum_tokens += para.1;
            }
        }

        // Step 4b: Pick opening paragraph of this stratum (Lead)
        if current_total_tokens + paragraphs[start].1 <= safe_budget
            && selected_indices.insert(start)
        {
            current_total_tokens += paragraphs[start].1;
            stratum_tokens += paragraphs[start].1;
        }

        // Step 4c: Pick closing paragraph of this stratum (Conclusion)
        let last_idx = end - 1;
        if last_idx > start
            && current_total_tokens + paragraphs[last_idx].1 <= safe_budget
            && selected_indices.insert(last_idx)
        {
            current_total_tokens += paragraphs[last_idx].1;
            stratum_tokens += paragraphs[last_idx].1;
        }

        // Step 4d: Fill remaining stratum budget with top BM25 paragraphs
        let mut candidates: Vec<usize> = (start..end)
            .filter(|i| !selected_indices.contains(i))
            .collect();
        candidates.sort_by(|&a, &b| {
            para_scores[b]
                .partial_cmp(&para_scores[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for cand in candidates {
            if stratum_tokens >= stratum_budget || current_total_tokens >= safe_budget {
                break;
            }
            let p_tokens = paragraphs[cand].1;
            if current_total_tokens + p_tokens <= safe_budget {
                selected_indices.insert(cand);
                current_total_tokens += p_tokens;
                stratum_tokens += p_tokens;
            }
        }
    }

    // 5. Always ensure the absolute first paragraph (document thesis) and absolute last paragraph (document conclusion)
    if !selected_indices.contains(&0) && current_total_tokens + paragraphs[0].1 <= max_tokens {
        selected_indices.insert(0);
    }
    let last = n_paras - 1;
    if !selected_indices.contains(&last) && current_total_tokens + paragraphs[last].1 <= max_tokens
    {
        selected_indices.insert(last);
    }

    // 6. Chronological Reassembly
    let mut sorted_indices: Vec<usize> = selected_indices.into_iter().collect();
    sorted_indices.sort_unstable();

    if sorted_indices.is_empty() {
        return TextChunker::truncate_by_tokens(content, max_tokens);
    }

    let mut out = String::with_capacity(content.len().min(max_tokens * 4));
    let mut prev_idx: Option<usize> = None;

    for idx in sorted_indices {
        if let Some(prev) = prev_idx {
            if idx == prev + 1 {
                out.push_str("\n\n");
            } else {
                out.push_str("\n\n...\n\n");
            }
        }
        out.push_str(paragraphs[idx].0);
        prev_idx = Some(idx);
    }

    // Final guard against token count drift
    if tok_kind.count_tokens(&out) > max_tokens {
        return Cow::Owned(TextChunker::truncate_by_tokens(&out, max_tokens).into_owned());
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
        let selected =
            select_relevant_context_bm25(&combined, Some(question), 600, TokenizerKind::Cl100k);

        assert!(selected.contains("Quantum cryptography utilizes photon entanglement"));
    }
}
