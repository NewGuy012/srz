use std::borrow::Cow;

/// Decodes numeric and named HTML entities in a single pass, returning a zero-copy
/// `Cow::Borrowed` when `s` contains no entities.
pub fn decode_html_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }

    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'&' {
            // Check for entity termination ';' within reasonable distance (<= 12 bytes)
            let mut end = i + 1;
            while end < bytes.len()
                && end - i <= 12
                && bytes[end] != b';'
                && bytes[end] != b'&'
                && bytes[end] != b' '
            {
                end += 1;
            }

            if end < bytes.len() && bytes[end] == b';' {
                let entity = &s[i + 1..end];
                if let Some(rest) = entity.strip_prefix('#') {
                    let code = if let Some(hex) =
                        rest.strip_prefix('x').or_else(|| rest.strip_prefix('X'))
                    {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        rest.parse::<u32>().ok()
                    };
                    if let Some(c) = code.and_then(char::from_u32) {
                        out.push(c);
                        i = end + 1;
                        continue;
                    }
                } else if let Some(decoded) = decode_named_entity(entity) {
                    out.push_str(decoded);
                    i = end + 1;
                    continue;
                }
            }
        }

        // Push character as utf-8 safely
        let rest = &s[i..];
        let ch = rest.chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }

    Cow::Owned(out)
}

fn decode_named_entity(entity: &str) -> Option<&'static str> {
    match entity {
        "amp" => Some("&"),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "quot" => Some("\""),
        "apos" => Some("'"),
        "nbsp" => Some("\u{00A0}"),
        "mdash" => Some("—"),
        "ndash" => Some("–"),
        "hellip" => Some("…"),
        "lsquo" => Some("‘"),
        "rsquo" => Some("’"),
        "ldquo" => Some("“"),
        "rdquo" => Some("”"),
        "laquo" => Some("«"),
        "raquo" => Some("»"),
        "bull" => Some("•"),
        "copy" => Some("©"),
        "reg" => Some("®"),
        "trade" => Some("™"),
        "cent" => Some("¢"),
        "pound" => Some("£"),
        "yen" => Some("¥"),
        "euro" => Some("€"),
        "sect" => Some("§"),
        "deg" => Some("°"),
        "plusmn" => Some("±"),
        "times" => Some("×"),
        "divide" => Some("÷"),
        _ => None,
    }
}

/// URL percent-encodes a string according to RFC 3986 (unreserved: a-z, A-Z, 0-9, -, _, ., ~).
/// Returns a borrowed Cow if no characters need encoding.
pub fn url_encode(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    if bytes
        .iter()
        .all(|&b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'))
    {
        return Cow::Borrowed(s);
    }

    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() * 3);
    for &b in bytes {
        if matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX_UPPER[(b >> 4) as usize] as char);
            out.push(HEX_UPPER[(b & 0x0F) as usize] as char);
        }
    }
    Cow::Owned(out)
}

/// URL percent-decodes a string according to RFC 3986.
/// Returns a borrowed Cow if no percent-encoded octets exist.
pub fn url_decode(s: &str) -> Result<Cow<'_, str>, std::string::FromUtf8Error> {
    if !s.as_bytes().contains(&b'%') {
        return Ok(Cow::Borrowed(s));
    }

    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = match bytes[i + 1] {
                b'0'..=b'9' => Some(bytes[i + 1] - b'0'),
                b'a'..=b'f' => Some(bytes[i + 1] - b'a' + 10),
                b'A'..=b'F' => Some(bytes[i + 1] - b'A' + 10),
                _ => None,
            };
            let h2 = match bytes[i + 2] {
                b'0'..=b'9' => Some(bytes[i + 2] - b'0'),
                b'a'..=b'f' => Some(bytes[i + 2] - b'a' + 10),
                b'A'..=b'F' => Some(bytes[i + 2] - b'A' + 10),
                _ => None,
            };
            if let (Some(h1), Some(h2)) = (h1, h2) {
                out.push((h1 << 4) | h2);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).map(Cow::Owned)
}

/// Computes the Damerau-Levenshtein (OSA) edit distance between two character sequences.
pub fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut d = vec![vec![0usize; b_len + 1]; a_len + 1];

    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, val) in d[0].iter_mut().enumerate() {
        *val = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            let mut min_val = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);

            if i > 1
                && j > 1
                && a_chars[i - 1] == b_chars[j - 2]
                && a_chars[i - 2] == b_chars[j - 1]
            {
                min_val = min_val.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = min_val;
        }
    }

    d[a_len][b_len]
}

/// Computes the Jaro similarity between two character sequences, returning a value between 0.0 and 1.0.
pub fn jaro(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 && b_len == 0 {
        return 1.0;
    }
    if a_len == 0 || b_len == 0 {
        return 0.0;
    }

    let match_distance = (a_len.max(b_len) / 2).saturating_sub(1);

    let mut a_matches = vec![false; a_len];
    let mut b_matches = vec![false; b_len];

    let mut matches = 0;
    for i in 0..a_len {
        let start = i.saturating_sub(match_distance);
        let end = (i + match_distance + 1).min(b_len);

        for j in start..end {
            if b_matches[j] || a_chars[i] != b_chars[j] {
                continue;
            }
            a_matches[i] = true;
            b_matches[j] = true;
            matches += 1;
            break;
        }
    }

    if matches == 0 {
        return 0.0;
    }

    let mut transpositions = 0;
    let mut k = 0;
    for i in 0..a_len {
        if !a_matches[i] {
            continue;
        }
        while !b_matches[k] {
            k += 1;
        }
        if a_chars[i] != b_chars[k] {
            transpositions += 1;
        }
        k += 1;
    }

    let m = matches as f64;
    let t = (transpositions / 2) as f64;
    (m / (a_len as f64) + m / (b_len as f64) + (m - t) / m) / 3.0
}

/// Computes the Jaro-Winkler similarity between two character sequences.
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let jaro_sim = jaro(a, b);
    if jaro_sim < 0.7 {
        return jaro_sim;
    }

    let prefix = a
        .chars()
        .zip(b.chars())
        .take(4)
        .take_while(|(ca, cb)| ca == cb)
        .count();

    jaro_sim + 0.1 * (prefix as f64) * (1.0 - jaro_sim)
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

    #[test]
    fn test_url_encode_and_decode() {
        assert_eq!(url_encode("hello_world-123.~"), "hello_world-123.~");
        assert!(matches!(url_encode("simple"), Cow::Borrowed(_)));
        assert_eq!(url_encode("hello world!"), "hello%20world%21");
        assert_eq!(url_encode("Dune (novel)"), "Dune%20%28novel%29");

        assert_eq!(url_decode("simple").unwrap(), "simple");
        assert!(matches!(url_decode("simple").unwrap(), Cow::Borrowed(_)));
        assert_eq!(url_decode("hello%20world%21").unwrap(), "hello world!");
        assert_eq!(
            url_decode("https://en.wikipedia.org/wiki/Dune_%28novel%29").unwrap(),
            "https://en.wikipedia.org/wiki/Dune_(novel)"
        );
    }
}
