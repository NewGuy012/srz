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
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h1), Some(h2)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push(((h1 << 4) | h2) as u8);
            i += 3;
            continue;
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

    let width = b_len + 1;
    let mut d = vec![0usize; (a_len + 1) * width];

    for i in 0..=a_len {
        d[i * width] = i;
    }
    for (j, val) in d.iter_mut().take(width).enumerate() {
        *val = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            let mut min_val = (d[(i - 1) * width + j] + 1)
                .min(d[i * width + (j - 1)] + 1)
                .min(d[(i - 1) * width + (j - 1)] + cost);

            if i > 1
                && j > 1
                && a_chars[i - 1] == b_chars[j - 2]
                && a_chars[i - 2] == b_chars[j - 1]
            {
                min_val = min_val.min(d[(i - 2) * width + (j - 2)] + 1);
            }
            d[i * width + j] = min_val;
        }
    }

    d[a_len * width + b_len]
}

/// Strips all HTML tags (`<...>`) from a string, preserving only text content.
pub fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
            out.push(' ');
        } else if !in_tag {
            out.push(ch);
        }
    }
    out
}

/// Zero-allocation case-insensitive ASCII substring search returning the first byte index of `needle`.
pub fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    find_subslice_ignore_case(haystack.as_bytes(), needle.as_bytes())
}

/// Finds the byte offset of `needle` in `haystack` case-insensitively.
pub fn find_subslice_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
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

#[rustfmt::skip]
const STOP_WORDS: &[&str] = &[
    "a", "about", "above", "after", "again", "against", "all", "am", "an", "and", "any", "are",
    "aren't", "as", "at", "be", "because", "been", "before", "being", "below", "between", "both",
    "but", "by", "can", "can't", "cannot", "could", "couldn't", "did", "didn't", "do", "does",
    "doesn't", "doing", "don't", "down", "during", "each", "few", "for", "from", "further", "had",
    "hadn't", "has", "hasn't", "have", "haven't", "having", "he", "he'd", "he'll", "he's", "her",
    "here", "here's", "hers", "herself", "him", "himself", "his", "how", "how's", "i", "i'd",
    "i'll", "i'm", "i've", "if", "in", "into", "is", "isn't", "it", "it's", "its", "itself",
    "let's", "me", "more", "most", "mustn't", "my", "myself", "no", "nor", "not", "of", "off",
    "on", "once", "only", "or", "other", "ought", "our", "ours", "ourselves", "out", "over", "own",
    "same", "shan't", "she", "she'd", "she'll", "she's", "should", "shouldn't", "so", "some",
    "such", "than", "that", "that's", "the", "their", "theirs", "them", "themselves", "then",
    "there", "there's", "these", "they", "they'd", "they'll", "they're", "they've", "this",
    "those", "through", "to", "too", "under", "until", "up", "very", "was", "wasn't", "we", "we'd",
    "we'll", "we're", "we've", "were", "weren't", "what", "what's", "when", "when's", "where",
    "where's", "which", "while", "who", "who's", "whom", "why", "why's", "with", "won't", "would",
    "wouldn't", "you", "you'd", "you'll", "you're", "you've", "your", "yours", "yourself",
    "yourselves",
];

/// Returns true if `word` is a common English stop word.
pub fn is_stop_word(word: &str) -> bool {
    if word.bytes().any(|b| b.is_ascii_uppercase()) {
        is_stop_word_lower(&word.to_ascii_lowercase())
    } else {
        is_stop_word_lower(word)
    }
}

pub fn is_stop_word_lower(word: &str) -> bool {
    STOP_WORDS.binary_search(&word).is_ok()
}

/// Tokenizes text into normalized lowercased terms for BM25 indexing and querying.
pub fn tokenize_terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Tokenizes text into search/index terms, filtering out stop words and single-character ASCII tokens.
pub fn tokenize_search_terms(text: &str) -> Vec<String> {
    tokenize_terms(text)
        .into_iter()
        .filter(|t| !is_stop_word(t) && !(t.len() <= 1 && t.is_ascii()))
        .collect()
}

/// Returns the local name of a potentially namespaced XML/HTML identifier (`ns:tag` -> `tag`).
pub fn xml_local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Looks up `target_local` (case-insensitive on local name) in a raw tag attribute string.
pub fn xml_attr<'a>(attrs: &'a str, target_local: &str) -> Option<Cow<'a, str>> {
    let bytes = attrs.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        while i < len && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i >= len {
            break;
        }

        let name_start = i;
        while i < len && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'/' {
            i += 1;
        }
        let attr_name = &attrs[name_start..i];
        if attr_name.is_empty() {
            i += 1;
            continue;
        }

        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }

        let val_slice = if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            let val_start = i;
            while i < len && bytes[i] != quote {
                i += 1;
            }
            let slice = &attrs[val_start..i];
            if i < len && bytes[i] == quote {
                i += 1;
            }
            slice
        } else {
            let val_start = i;
            while i < len && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            &attrs[val_start..i]
        };

        if xml_local_name(attr_name).eq_ignore_ascii_case(target_local) {
            return Some(decode_html_entities(val_slice));
        }
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlEvent<'a> {
    Start {
        name: &'a str,
        attrs: &'a str,
        is_empty: bool,
    },
    End {
        name: &'a str,
    },
    Text(&'a str),
    CData(&'a str),
}

/// Lightweight streaming XML/HTML token scanner.
pub struct XmlScanner<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> XmlScanner<'a> {
    pub fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    pub fn next_event(&mut self) -> Option<XmlEvent<'a>> {
        while self.pos < self.src.len() {
            let rest = &self.src[self.pos..];
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Some(XmlEvent::Text(&rest[..end]));
            }

            if let Some(after) = rest.strip_prefix("<!--") {
                self.pos += after.find("-->").map(|p| 4 + p + 3).unwrap_or(rest.len());
                continue;
            }

            if let Some(after) = rest.strip_prefix("<![CDATA[") {
                if let Some(end) = after.find("]]>") {
                    self.pos += 9 + end + 3;
                    return Some(XmlEvent::CData(&after[..end]));
                }
                self.pos = self.src.len();
                return Some(XmlEvent::CData(after));
            }

            if let Some(after) = rest.strip_prefix("<?") {
                self.pos += after.find("?>").map(|p| 2 + p + 2).unwrap_or(rest.len());
                continue;
            }

            if rest.starts_with("<!") {
                let bytes = rest.as_bytes();
                let mut i = 2;
                let mut quote = 0u8;
                let mut bracket_depth = 0usize;
                while i < bytes.len() {
                    let b = bytes[i];
                    if quote != 0 {
                        if b == quote {
                            quote = 0;
                        }
                    } else if b == b'"' || b == b'\'' {
                        quote = b;
                    } else if b == b'[' {
                        bracket_depth += 1;
                    } else if b == b']' && bracket_depth > 0 {
                        bracket_depth -= 1;
                    } else if b == b'>' && bracket_depth == 0 {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                self.pos += i;
                continue;
            }

            if let Some(after) = rest.strip_prefix("</") {
                if let Some(end) = after.find('>') {
                    let name = after[..end].trim();
                    self.pos += 2 + end + 1;
                    return Some(XmlEvent::End { name });
                }
                self.pos += 1;
                return Some(XmlEvent::Text("<"));
            }

            let valid_tag_start = rest
                .as_bytes()
                .get(1)
                .is_some_and(|&b| b.is_ascii_alphabetic() || b == b'_' || b == b':');
            if !valid_tag_start {
                self.pos += 1;
                return Some(XmlEvent::Text("<"));
            }

            let bytes = rest.as_bytes();
            let mut i = 1;
            let mut quote = 0u8;
            while i < bytes.len() {
                let b = bytes[i];
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' {
                    break;
                }
                i += 1;
            }

            if i >= bytes.len() {
                self.pos += 1;
                return Some(XmlEvent::Text("<"));
            }

            self.pos += i + 1;
            let raw_inner = rest[1..i].trim();
            let inner = raw_inner
                .strip_suffix('/')
                .map(str::trim_end)
                .unwrap_or(raw_inner);
            let is_empty = inner.len() != raw_inner.len();
            if inner.is_empty() {
                continue;
            }

            let (name, attrs) = match inner.find(|c: char| c.is_ascii_whitespace()) {
                Some(ws) => (&inner[..ws], inner[ws..].trim_start()),
                None => (inner, ""),
            };
            return Some(XmlEvent::Start {
                name,
                attrs,
                is_empty,
            });
        }
        None
    }

    /// Reads the raw slice inside `tag_name` until its matching closing tag (accounting for nesting).
    pub fn read_raw_until_end(&mut self, tag_name: &str) -> &'a str {
        let start = self.pos;
        let mut depth = 1usize;
        loop {
            let before = self.pos;
            match self.next_event() {
                Some(XmlEvent::Start {
                    name,
                    is_empty: false,
                    ..
                }) if name.eq_ignore_ascii_case(tag_name) => {
                    depth += 1;
                }
                Some(XmlEvent::End { name }) if name.eq_ignore_ascii_case(tag_name) => {
                    depth -= 1;
                    if depth == 0 {
                        return &self.src[start..before];
                    }
                }
                None => return &self.src[start..],
                _ => {}
            }
        }
    }

    /// Reads and entity-decodes text (preserving CDATA sections verbatim) inside `tag_name` until `</tag_name>`.
    pub fn read_text(&mut self, tag_name: &str) -> String {
        let mut out = String::new();
        let mut depth = 1usize;
        while let Some(ev) = self.next_event() {
            match ev {
                XmlEvent::Start {
                    name,
                    is_empty: false,
                    ..
                } if name.eq_ignore_ascii_case(tag_name) => {
                    depth += 1;
                }
                XmlEvent::End { name } if name.eq_ignore_ascii_case(tag_name) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                XmlEvent::Text(t) => out.push_str(&decode_html_entities(t)),
                XmlEvent::CData(c) => out.push_str(c),
                _ => {}
            }
        }
        out
    }
}

/// Extracts the value of a quoted attribute given its `attr=` prefix.
pub fn extract_attr_value<'a>(html: &'a str, attr_prefix: &str) -> Option<&'a str> {
    let start_pos = html.find(attr_prefix)?;
    let remainder = &html[start_pos + attr_prefix.len()..];
    let quote = remainder.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &remainder[1..];
    let end_pos = rest.find(quote)?;
    Some(&rest[..end_pos])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_damerau_levenshtein() {
        assert_eq!(damerau_levenshtein("", ""), 0);
        assert_eq!(damerau_levenshtein("a", ""), 1);
        assert_eq!(damerau_levenshtein("", "b"), 1);
        assert_eq!(damerau_levenshtein("kitten", "sitting"), 3);
        // Transposition test
        assert_eq!(damerau_levenshtein("ca", "ac"), 1);
        assert_eq!(damerau_levenshtein("banana", "banna"), 1);
    }

    #[test]
    fn test_tokenize_terms_and_search_terms() {
        let text = "The Quick Brown Fox jumps over 123 lazy dogs! #Rust_2026-edition";
        let terms = tokenize_terms(text);
        assert!(terms.contains(&"quick".to_string()));
        assert!(terms.contains(&"brown".to_string()));
        assert!(terms.contains(&"the".to_string()));
        assert!(terms.contains(&"rust_2026-edition".to_string()));

        let search_terms = tokenize_search_terms(text);
        // "the" is a stop word and should be filtered out
        assert!(!search_terms.contains(&"the".to_string()));
        assert!(search_terms.contains(&"quick".to_string()));
        assert!(search_terms.contains(&"fox".to_string()));
        assert!(search_terms.contains(&"dogs".to_string()));
    }

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

    #[test]
    fn test_xml_scanner_and_attrs() {
        let xml = r#"<?xml version="1.0"?><!-- comment --><root xmlns:dc="http://purl.org"><dc:title>A &amp; B</dc:title><item id='i1' label="x > y"/><p>a < b</p></root>"#;
        let mut scanner = XmlScanner::new(xml);

        assert_eq!(
            scanner.next_event(),
            Some(XmlEvent::Start {
                name: "root",
                attrs: r#"xmlns:dc="http://purl.org""#,
                is_empty: false,
            })
        );
        assert_eq!(
            scanner.next_event(),
            Some(XmlEvent::Start {
                name: "dc:title",
                attrs: "",
                is_empty: false,
            })
        );
        assert_eq!(scanner.read_text("dc:title"), "A & B");

        let Some(XmlEvent::Start {
            name,
            attrs,
            is_empty,
        }) = scanner.next_event()
        else {
            panic!("expected <item/>");
        };
        assert_eq!(name, "item");
        assert!(is_empty);
        assert_eq!(xml_attr(attrs, "id").as_deref(), Some("i1"));
        assert_eq!(xml_attr(attrs, "label").as_deref(), Some("x > y"));
        assert_eq!(
            extract_attr_value(xml, "xmlns:dc="),
            Some("http://purl.org")
        );
        assert_eq!(extract_attr_value(xml, "id="), Some("i1"));
        assert_eq!(extract_attr_value(xml, "missing="), None);
    }
}
