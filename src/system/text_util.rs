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

/// Zero-allocation case-insensitive ASCII suffix check (`lowercase_suffix` must already be lowercase ASCII).
pub fn ends_with_ascii_case_insensitive(haystack: &str, lowercase_suffix: &str) -> bool {
    let h_bytes = haystack.as_bytes();
    let s_bytes = lowercase_suffix.as_bytes();
    if h_bytes.len() < s_bytes.len() {
        return false;
    }
    h_bytes[h_bytes.len() - s_bytes.len()..].eq_ignore_ascii_case(s_bytes)
}

/// Returns `true` if the string starts with `"http://"` or `"https://"`.
pub fn is_http_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Formats a byte slice into a lowercase hexadecimal string in a single allocation.
pub fn format_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(hex, "{:02x}", b);
    }
    hex
}

/// Returns true if `word` is a common English stop word.
pub fn is_stop_word(word: &str) -> bool {
    if word.bytes().any(|b| b.is_ascii_uppercase()) {
        is_stop_word_lower(&word.to_ascii_lowercase())
    } else {
        is_stop_word_lower(word)
    }
}

pub fn is_stop_word_lower(word: &str) -> bool {
    matches!(
        word,
        "a" | "about"
            | "above"
            | "after"
            | "again"
            | "against"
            | "all"
            | "am"
            | "an"
            | "and"
            | "any"
            | "are"
            | "aren't"
            | "as"
            | "at"
            | "be"
            | "because"
            | "been"
            | "before"
            | "being"
            | "below"
            | "between"
            | "both"
            | "but"
            | "by"
            | "can"
            | "can't"
            | "cannot"
            | "could"
            | "couldn't"
            | "did"
            | "didn't"
            | "do"
            | "does"
            | "doesn't"
            | "doing"
            | "don't"
            | "down"
            | "during"
            | "each"
            | "few"
            | "for"
            | "from"
            | "further"
            | "had"
            | "hadn't"
            | "has"
            | "hasn't"
            | "have"
            | "haven't"
            | "having"
            | "he"
            | "he'd"
            | "he'll"
            | "he's"
            | "her"
            | "here"
            | "here's"
            | "hers"
            | "herself"
            | "him"
            | "himself"
            | "his"
            | "how"
            | "how's"
            | "i"
            | "i'd"
            | "i'll"
            | "i'm"
            | "i've"
            | "if"
            | "in"
            | "into"
            | "is"
            | "isn't"
            | "it"
            | "it's"
            | "its"
            | "itself"
            | "let's"
            | "me"
            | "more"
            | "most"
            | "mustn't"
            | "my"
            | "myself"
            | "no"
            | "nor"
            | "not"
            | "of"
            | "off"
            | "on"
            | "once"
            | "only"
            | "or"
            | "other"
            | "ought"
            | "our"
            | "ours"
            | "ourselves"
            | "out"
            | "over"
            | "own"
            | "same"
            | "shan't"
            | "she"
            | "she'd"
            | "she'll"
            | "she's"
            | "should"
            | "shouldn't"
            | "so"
            | "some"
            | "such"
            | "than"
            | "that"
            | "that's"
            | "the"
            | "their"
            | "theirs"
            | "them"
            | "themselves"
            | "then"
            | "there"
            | "there's"
            | "these"
            | "they"
            | "they'd"
            | "they'll"
            | "they're"
            | "they've"
            | "this"
            | "those"
            | "through"
            | "to"
            | "too"
            | "under"
            | "until"
            | "up"
            | "very"
            | "was"
            | "wasn't"
            | "we"
            | "we'd"
            | "we'll"
            | "we're"
            | "we've"
            | "were"
            | "weren't"
            | "what"
            | "what's"
            | "when"
            | "when's"
            | "where"
            | "where's"
            | "which"
            | "while"
            | "who"
            | "who's"
            | "whom"
            | "why"
            | "why's"
            | "with"
            | "won't"
            | "would"
            | "wouldn't"
            | "you"
            | "you'd"
            | "you'll"
            | "you're"
            | "you've"
            | "your"
            | "yours"
            | "yourself"
            | "yourselves"
    )
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
        assert!(ends_with_ascii_case_insensitive("test.RSS", ".rss"));
        assert!(ends_with_ascii_case_insensitive("test.rss", ".RSS"));
        assert!(!ends_with_ascii_case_insensitive("test.xml", ".rss"));
        assert!(!ends_with_ascii_case_insensitive("rss", ".rss"));
    }

    #[test]
    fn test_is_http_url() {
        assert!(is_http_url("http://example.com"));
        assert!(is_http_url("https://example.com/feed"));
        assert!(!is_http_url("ftp://example.com"));
        assert!(!is_http_url("example.com"));
        assert!(!is_http_url("/path/to/file"));
    }

    #[test]
    fn test_format_hex() {
        assert_eq!(format_hex(&[]), "");
        assert_eq!(format_hex(&[0x00, 0x0f, 0x10, 0xff]), "000f10ff");
    }
}
