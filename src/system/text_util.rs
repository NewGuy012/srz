use std::borrow::Cow;

/// Decodes HTML entities in a single pass using `html_escape`, returning a zero-copy
/// `Cow::Borrowed` when `s` contains no entities.
pub fn decode_html_entities(s: &str) -> Cow<'_, str> {
    html_escape::decode_html_entities(s)
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
        assert!(matches!(
            decode_html_entities("Hello world"),
            Cow::Borrowed(_)
        ));
        assert_eq!(
            decode_html_entities("&lt;div&gt;&quot;Hello &amp; welcome&quot;&lt;/div&gt;"),
            "<div>\"Hello & welcome\"</div>"
        );
        assert_eq!(
            decode_html_entities("It&#39;s sunny &amp; warm"),
            "It's sunny & warm"
        );
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
