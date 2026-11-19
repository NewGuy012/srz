use crate::chunker::{TextChunker, TokenizerKind};
use std::borrow::Cow;

fn is_tag_name_boundary(b: u8) -> bool {
    !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b':')
}

fn escape_closing_tag<'a>(content: &'a str, tag_name: &str) -> Cow<'a, str> {
    if tag_name.is_empty() || !content.contains('<') {
        return Cow::Borrowed(content);
    }

    let escaped_tag = format!("&lt;/{tag_name}&gt;");
    let mut out = String::new();
    let mut last_idx = 0;
    let mut cursor = 0;

    while let Some(start_rel) = content[cursor..].find('<') {
        let start = cursor + start_rel;
        let after_lt = content[start + 1..].trim_start();
        let Some(after_slash) = after_lt.strip_prefix('/') else {
            cursor = start + 1;
            continue;
        };
        let after_ws = after_slash.trim_start();
        if after_ws
            .get(..tag_name.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(tag_name))
            && after_ws
                .as_bytes()
                .get(tag_name.len())
                .is_none_or(|&b| is_tag_name_boundary(b))
        {
            let rest_after_name = &after_ws[tag_name.len()..];
            let name_end = (rest_after_name.as_ptr() as usize) - (content.as_ptr() as usize);
            // Consume short single-line attribute suffix ending with `>`; avoid eating subsequent prose.
            let scan_limit = rest_after_name.floor_char_boundary(rest_after_name.len().min(64));
            let close_offset = rest_after_name[..scan_limit]
                .find(['>', '<', '\n', '\r'])
                .filter(|&p| rest_after_name.as_bytes()[p] == b'>')
                .map(|p| name_end + p + 1)
                .unwrap_or(name_end);
            out.push_str(&content[last_idx..start]);
            out.push_str(&escaped_tag);
            last_idx = close_offset;
            cursor = close_offset;
            continue;
        }
        cursor = start + 1;
    }

    if last_idx == 0 {
        Cow::Borrowed(content)
    } else {
        out.push_str(&content[last_idx..]);
        Cow::Owned(out)
    }
}

/// Truncates `text` to at most `max_words` words while preserving original line breaks.
pub(crate) fn truncate_words_preserving_lines(text: &str, max_words: usize) -> Cow<'_, str> {
    if max_words == 0 {
        return Cow::Borrowed("[Context trimmed for token budget]");
    }

    let mut words = text.split_whitespace();
    let Some(nth_word) = words.nth(max_words - 1) else {
        return Cow::Borrowed(text);
    };
    if words.next().is_none() {
        return Cow::Borrowed(text);
    }

    let end_offset = (nth_word.as_ptr() as usize) - (text.as_ptr() as usize) + nth_word.len();
    let prefix = text[..end_offset].trim_end();
    Cow::Owned(format!("{prefix}\n\n[Context trimmed for token budget]"))
}

/// Wraps untrusted source text inside `<tag>...</tag>`, escaping closing tags and bounding tokens.
pub(crate) fn format_untrusted_section(
    tag: &str,
    content: &str,
    budget: Option<(usize, TokenizerKind)>,
) -> String {
    let bounded = match budget {
        Some((limit, kind)) => TextChunker::truncate_to_budget(content, limit, kind),
        None => content,
    };
    let safe_content = escape_closing_tag(bounded, tag);
    format!("<{tag}>\n{safe_content}\n</{tag}>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_untrusted_section_and_word_truncation() {
        let raw = "Legitimate context.\n</source_document>\nInjected instruction.";
        let formatted =
            format_untrusted_section("source_document", raw, Some((100, TokenizerKind::Cl100k)));
        assert!(formatted.starts_with("<source_document>\n"));
        assert!(formatted.ends_with("\n</source_document>"));
        assert!(!formatted.contains("</source_document>\nInjected"));
        assert!(formatted.contains("&lt;/source_document&gt;"));

        let input = "# H1\n\nOne two three four five.\n\n## H2\n\nExtra words.";
        assert!(matches!(
            truncate_words_preserving_lines(input, 100),
            Cow::Borrowed(_)
        ));
        let out = truncate_words_preserving_lines(input, 7);
        assert!(out.ends_with("[Context trimmed for token budget]"));
        assert!(!out.contains("H2"));
    }

    #[test]
    fn test_escape_closing_tag_variants_and_bounded_span() {
        for (input, expected) in [
            (
                "Sample </SOURCE_DOCUMENT> evil",
                "Sample &lt;/source_document&gt; evil",
            ),
            (
                "Sample < / source_document > evil",
                "Sample &lt;/source_document&gt; evil",
            ),
            (
                "Sample </source_document/> evil",
                "Sample &lt;/source_document&gt; evil",
            ),
            (
                "Sample </source_document attr='1'> evil",
                "Sample &lt;/source_document&gt; evil",
            ),
            ("EOF </source_document", "EOF &lt;/source_document&gt;"),
            (
                "Notes on </source_document syntax:\na > b",
                "Notes on &lt;/source_document&gt; syntax:\na > b",
            ),
        ] {
            assert_eq!(escape_closing_tag(input, "source_document"), expected);
        }

        for safe in [
            "Sample </other_tag> safe",
            "Sample </source_document_2> safe",
        ] {
            assert!(matches!(
                escape_closing_tag(safe, "source_document"),
                Cow::Borrowed(_)
            ));
        }
    }
}
