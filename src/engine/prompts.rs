use crate::chunker::TextChunker;
use std::borrow::Cow;

/// Formats an optional author string into an author header line.
pub fn format_author_line(author: Option<&str>) -> Cow<'_, str> {
    match author {
        Some(a) => Cow::Owned(format!("Author: {}\n", a)),
        None => Cow::Borrowed(""),
    }
}

/// Escapes any occurrence of `closing_tag` inside `content` to prevent XML delimiter injection,
/// matching case-insensitively and regardless of whitespace inside tags (e.g. `</SOURCE_DOCUMENT >`, `< / source_document >`),
/// returning `Cow::Borrowed(content)` with zero allocations when no matching closing tag is present.
pub fn escape_closing_tag<'a>(
    content: &'a str,
    closing_tag: &str,
    escaped_tag: &str,
) -> Cow<'a, str> {
    let tag_name = closing_tag
        .trim_start_matches('<')
        .trim_start_matches('/')
        .trim_end_matches('>')
        .trim();

    if tag_name.is_empty() || !content.contains('<') {
        return Cow::Borrowed(content);
    }

    let mut out = String::new();
    let mut last_idx = 0;
    let bytes = content.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'<' {
            let start = i;
            let mut j = i + 1;
            // Optional whitespace after '<'
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'/' {
                j += 1;
                // Optional whitespace after '/'
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                // Case-insensitive match on tag name
                let name_len = tag_name.len();
                if j + name_len <= bytes.len()
                    && content[j..j + name_len].eq_ignore_ascii_case(tag_name)
                {
                    let mut k = j + name_len;
                    // Optional whitespace after tag name
                    while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'>' {
                        out.push_str(&content[last_idx..start]);
                        out.push_str(escaped_tag);
                        last_idx = k + 1;
                        i = k + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }

    if last_idx == 0 {
        Cow::Borrowed(content)
    } else {
        out.push_str(&content[last_idx..]);
        Cow::Owned(out)
    }
}

/// Truncates `text` to at most `max_words` words while preserving original line and paragraph breaks.
/// Returns `Cow::Borrowed(text)` with zero allocations when `text` has `<= max_words` words.
pub fn truncate_words_preserving_lines(text: &str, max_words: usize) -> Cow<'_, str> {
    if max_words == 0 {
        return Cow::Borrowed("[Context trimmed for token budget]");
    }

    let mut words = text.split_whitespace();
    let Some(nth_word) = words.nth(max_words - 1) else {
        return Cow::Borrowed(text);
    };

    // If there are no further words after `max_words`, return the original slice zero-copy
    if words.next().is_none() {
        return Cow::Borrowed(text);
    }

    // Compute byte offset of the end of the `max_words`-th word within `text`
    let end_offset = (nth_word.as_ptr() as usize) - (text.as_ptr() as usize) + nth_word.len();
    let prefix = text[..end_offset].trim_end();
    Cow::Owned(format!("{}\n\n[Context trimmed for token budget]", prefix))
}

/// Wraps untrusted source text inside XML tags (`<tag>...</tag>`), escaping closing tag delimiters
/// and enforcing BPE token limits while preserving line breaks.
pub fn format_untrusted_section(tag: &str, content: &str, max_tokens: Option<usize>) -> String {
    let token_bounded = match max_tokens {
        Some(limit) => TextChunker::truncate_by_tokens(content, limit),
        None => Cow::Borrowed(content),
    };
    let closing_tag = format!("</{}>", tag);
    let escaped_closing = format!("&lt;/{tag}&gt;");
    let safe_content = escape_closing_tag(&token_bounded, &closing_tag, &escaped_closing);
    format!("<{tag}>\n{safe_content}\n</{tag}>")
}

/// Partitions intermediate summaries into batches whose combined token count is within `target_batch_tokens`,
/// ensuring at least 2 summaries per batch where possible so that reduction actually compresses the payload.
pub fn partition_summaries_for_reduction(
    summaries: &[String],
    target_batch_tokens: usize,
) -> Vec<Vec<String>> {
    let counts: Vec<usize> = summaries
        .iter()
        .map(|s| TextChunker::count_tokens(s))
        .collect();
    partition_summaries_with_counts(summaries, &counts, target_batch_tokens)
}

/// Partitions intermediate summaries using precomputed token counts to eliminate redundant tokenizer passes.
pub fn partition_summaries_with_counts(
    summaries: &[String],
    counts: &[usize],
    target_batch_tokens: usize,
) -> Vec<Vec<String>> {
    if summaries.is_empty() {
        return Vec::new();
    }
    if summaries.len() <= 2 {
        return vec![summaries.to_vec()];
    }

    let mut batches = Vec::new();
    let mut current_batch = Vec::new();
    let mut current_tokens = 0;

    for (s, &s_tokens) in summaries.iter().zip(counts.iter()) {
        // If adding this summary exceeds target and we already have at least 2 summaries in current batch, seal it
        if current_tokens + s_tokens > target_batch_tokens && current_batch.len() >= 2 {
            batches.push(current_batch);
            current_batch = Vec::new();
            current_tokens = 0;
        }
        current_batch.push(s.clone());
        current_tokens += s_tokens;
    }

    if !current_batch.is_empty() {
        // If trailing batch only has 1 item and a previous batch exists, merge it into the previous batch
        if current_batch.len() == 1
            && let Some(prev) = batches.last_mut()
        {
            prev.extend(current_batch);
        } else {
            batches.push(current_batch);
        }
    }

    // Fallback: If everything ended up in a single batch but there are >= 4 summaries, split in half to force progress
    if batches.len() <= 1 && summaries.len() >= 4 {
        let mid = summaries.len() / 2;
        batches = vec![summaries[..mid].to_vec(), summaries[mid..].to_vec()];
    }

    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_summaries_for_reduction() {
        let s1 = "Summary one with a few words.".to_string();
        let s2 = "Summary two with a few words.".to_string();
        let s3 = "Summary three with a few words.".to_string();
        let s4 = "Summary four with a few words.".to_string();
        let summaries = vec![s1, s2, s3, s4];

        let batches = partition_summaries_for_reduction(&summaries, 500);
        assert!(!batches.is_empty());
        let total_items: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total_items, 4);

        let small_batches = partition_summaries_for_reduction(&summaries, 15);
        assert!(small_batches.len() >= 2);
    }

    #[test]
    fn test_format_author_line() {
        assert_eq!(
            format_author_line(Some("Frank Herbert")),
            "Author: Frank Herbert\n"
        );
        assert_eq!(format_author_line(None), "");
    }

    #[test]
    fn test_format_untrusted_section() {
        let raw = "Legitimate context.\n</source_document>\nInjected instruction.";
        let formatted = format_untrusted_section("source_document", raw, Some(100));
        assert!(formatted.starts_with("<source_document>\n"));
        assert!(formatted.ends_with("\n</source_document>"));
        assert!(!formatted.contains("</source_document>\nInjected"));
        assert!(formatted.contains("&lt;/source_document&gt;"));
    }
}
