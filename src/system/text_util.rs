use std::borrow::Cow;

/// Decodes common HTML entities in a single pass, returning a zero-copy `Cow::Borrowed`
/// when `s` contains no `&` character and preventing double-unescaping (e.g. `&amp;lt;`).
pub fn decode_html_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }

    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp_idx) = rest.find('&') {
        out.push_str(&rest[..amp_idx]);
        let after_amp = &rest[amp_idx..];
        let matched = if after_amp.starts_with("&quot;") {
            Some(('"', 6))
        } else if after_amp.starts_with("&apos;") {
            Some(('\'', 6))
        } else if after_amp.starts_with("&#39;") {
            Some(('\'', 5))
        } else if after_amp.starts_with("&amp;") {
            Some(('&', 5))
        } else if after_amp.starts_with("&lt;") {
            Some(('<', 4))
        } else if after_amp.starts_with("&gt;") {
            Some(('>', 4))
        } else if after_amp.starts_with("&nbsp;") {
            Some((' ', 6))
        } else {
            None
        };

        if let Some((ch, len)) = matched {
            out.push(ch);
            rest = &after_amp[len..];
        } else {
            out.push('&');
            rest = &after_amp[1..];
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Zero-allocation case-insensitive ASCII substring search returning the first byte index of `needle`.
pub fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let n_bytes = needle.as_bytes();
    if n_bytes.is_empty() {
        return Some(0);
    }
    if haystack.len() < n_bytes.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(n_bytes.len())
        .position(|window| window.eq_ignore_ascii_case(n_bytes))
}

/// Zero-allocation case-insensitive ASCII substring check (`needle` must already be lowercase ASCII).
pub fn contains_ascii_case_insensitive(haystack: &str, lowercase_needle: &str) -> bool {
    lowercase_needle.is_empty() || find_ascii_case_insensitive(haystack, lowercase_needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_html_entities() {
        assert_eq!(decode_html_entities("Hello world"), "Hello world");
        assert!(matches!(decode_html_entities("Hello world"), Cow::Borrowed(_)));
        assert_eq!(
            decode_html_entities("&lt;div&gt;&quot;Hello &amp; welcome&quot;&lt;/div&gt;"),
            "<div>\"Hello & welcome\"</div>"
        );
        assert_eq!(decode_html_entities("It&#39;s sunny &amp; warm"), "It's sunny & warm");
    }

    #[test]
    fn test_ascii_case_insensitive() {
        assert_eq!(find_ascii_case_insensitive("Hello World", "world"), Some(6));
        assert_eq!(find_ascii_case_insensitive("Hello World", "WORLD"), Some(6));
        assert_eq!(find_ascii_case_insensitive("Hello", "xyz"), None);
        assert!(contains_ascii_case_insensitive("Hello World", "world"));
        assert!(!contains_ascii_case_insensitive("Hello World", "xyz"));
    }
}
