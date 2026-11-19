use std::borrow::Cow;

/// Decodes numeric and named HTML entities in a single pass, borrowing zero-copy when no entities decode.
pub(crate) fn decode_html_entities(s: &str) -> Cow<'_, str> {
    let Some(mut i) = s.find('&') else {
        return Cow::Borrowed(s);
    };

    let bytes = s.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;

    while i < bytes.len() {
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
            let replacement = if let Some(rest) = entity.strip_prefix('#') {
                let code =
                    if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        rest.parse::<u32>().ok()
                    };
                code.and_then(|n| {
                    if n == 0 {
                        Some('\u{FFFD}')
                    } else {
                        char::from_u32(n)
                    }
                })
                .map(|c| Cow::Owned(c.to_string()))
            } else {
                decode_named_entity(entity).map(Cow::Borrowed)
            };

            if let Some(decoded) = replacement {
                let buf = out.get_or_insert_with(|| String::with_capacity(s.len()));
                buf.push_str(&s[copied..i]);
                buf.push_str(&decoded);
                i = end + 1;
                copied = i;
                if let Some(rel) = s[i..].find('&') {
                    i += rel;
                    continue;
                }
                break;
            }
        }

        match s[i + 1..].find('&') {
            Some(rel) => i += 1 + rel,
            None => break,
        }
    }

    match out {
        None => Cow::Borrowed(s),
        Some(mut buf) => {
            buf.push_str(&s[copied..]);
            Cow::Owned(buf)
        }
    }
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

const fn is_unreserved_uri_byte(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~')
}

/// URL percent-encodes `s` per RFC 3986, borrowing zero-copy if all bytes are unreserved.
pub(crate) fn url_encode(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    if bytes.iter().all(|&b| is_unreserved_uri_byte(b)) {
        return Cow::Borrowed(s);
    }

    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in bytes {
        if is_unreserved_uri_byte(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX_UPPER[(b >> 4) as usize] as char);
            out.push(HEX_UPPER[(b & 0x0F) as usize] as char);
        }
    }
    Cow::Owned(out)
}

/// URL percent-decodes `s` per RFC 3986, borrowing zero-copy when no `%` escapes exist.
pub(crate) fn url_decode(s: &str) -> Result<Cow<'_, str>, std::string::FromUtf8Error> {
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

/// Computes the Damerau-Levenshtein (OSA) edit distance in O(min(m, n)) space.
pub(crate) fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let mut a_chars: Vec<char> = a.chars().collect();
    let mut b_chars: Vec<char> = b.chars().collect();
    if a_chars.len() < b_chars.len() {
        std::mem::swap(&mut a_chars, &mut b_chars);
    }
    let a_len = a_chars.len();
    let b_len = b_chars.len();
    if b_len == 0 {
        return a_len;
    }

    let mut prev2 = vec![0usize; b_len + 1];
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0usize; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = usize::from(a_chars[i - 1] != b_chars[j - 1]);
            let mut min_val = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1
                && j > 1
                && a_chars[i - 1] == b_chars[j - 2]
                && a_chars[i - 2] == b_chars[j - 1]
            {
                min_val = min_val.min(prev2[j - 2] + 1);
            }
            curr[j] = min_val;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

/// Strips `<...>` HTML tags while preserving unclosed `<` literals and quoted `>`.
pub(crate) fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'<' {
            let mut j = i + 1;
            let mut quote = 0u8;
            while j < bytes.len() {
                let b = bytes[j];
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' || b == b'<' {
                    break;
                }
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'>' {
                out.push(' ');
                i = j + 1;
                continue;
            }
            out.push('<');
            i += 1;
            continue;
        }
        let next = bytes[i..]
            .iter()
            .position(|&b| b == b'<')
            .map_or(bytes.len(), |p| i + p);
        out.push_str(&html[i..next]);
        i = next;
    }
    out
}

pub(crate) fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    find_subslice_ignore_case(haystack.as_bytes(), needle.as_bytes())
}

pub(crate) fn find_subslice_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
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

pub(crate) fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.is_ascii() {
        find_subslice_ignore_case(haystack.as_bytes(), needle.as_bytes()).is_some()
    } else {
        haystack.to_lowercase().contains(&needle.to_lowercase())
    }
}

pub(crate) fn ends_with_ascii_case_insensitive(haystack: &str, lowercase_suffix: &str) -> bool {
    let h_bytes = haystack.as_bytes();
    let s_bytes = lowercase_suffix.as_bytes();
    h_bytes.len() >= s_bytes.len()
        && h_bytes[h_bytes.len() - s_bytes.len()..].eq_ignore_ascii_case(s_bytes)
}

pub(crate) fn is_http_url(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"http://"))
        || bytes
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case(b"https://"))
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

pub(crate) fn is_stop_word(word: &str) -> bool {
    if word.bytes().any(|b| b.is_ascii_uppercase()) {
        STOP_WORDS
            .binary_search(&word.to_ascii_lowercase().as_str())
            .is_ok()
    } else {
        STOP_WORDS.binary_search(&word).is_ok()
    }
}

pub(crate) fn tokenize_terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub(crate) fn tokenize_search_terms(text: &str) -> Vec<String> {
    tokenize_terms(text)
        .into_iter()
        .filter(|t| !is_stop_word(t) && !(t.len() <= 1 && t.is_ascii()))
        .collect()
}

pub(crate) fn xml_local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

pub(crate) fn xml_attr<'a>(attrs: &'a str, target_local: &str) -> Option<Cow<'a, str>> {
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
pub(crate) enum XmlEvent<'a> {
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
pub(crate) struct XmlScanner<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> XmlScanner<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    pub(crate) fn next_event(&mut self) -> Option<XmlEvent<'a>> {
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
                self.pos = self.src.len();
                return Some(XmlEvent::Text(rest));
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
            let mut saw_gt = false;
            while i < bytes.len() {
                let b = bytes[i];
                if quote != 0 {
                    if b == b'>' {
                        saw_gt = true;
                    }
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' || b == b'<' {
                    break;
                }
                i += 1;
            }

            if i >= bytes.len() {
                if !saw_gt {
                    self.pos = self.src.len();
                    return Some(XmlEvent::Text(rest));
                }
                self.pos += 1;
                return Some(XmlEvent::Text("<"));
            }
            if bytes[i] != b'>' {
                self.pos += 1;
                return Some(XmlEvent::Text("<"));
            }

            self.pos += i + 1;
            let raw_inner = rest[1..i].trim();
            let is_empty = raw_inner.strip_suffix('/').is_some_and(|before| {
                let last_tok = before
                    .rfind(|c: char| c.is_ascii_whitespace())
                    .map_or(before, |p| &before[p + 1..]);
                before.ends_with(|c: char| c.is_ascii_whitespace() || matches!(c, '"' | '\''))
                    || !last_tok.contains('=')
            });
            let inner = if is_empty {
                raw_inner[..raw_inner.len() - 1].trim_end()
            } else {
                raw_inner
            };
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

    /// Reads and entity-decodes text (preserving CDATA sections verbatim) inside `tag_name` until `</tag_name>`.
    pub(crate) fn read_text(&mut self, tag_name: &str) -> String {
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

pub(crate) fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut prev_dash = false;

    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() && ch != '\'' && ch != '"' && ch != '’' {
            slug.push('-');
            prev_dash = true;
        }
    }

    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "summary".to_string()
    } else if trimmed.len() > 80 {
        trimmed[..80].trim_end_matches('-').to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn slugify_unique(title: &str, unique_key: Option<&str>) -> String {
    let base_slug = slugify(title);
    if let Some(key) = unique_key
        && !key.is_empty()
    {
        let hash = crate::system::sha256::sha256_hex(key.as_bytes());
        let short_hash = &hash[..8];
        if base_slug == "summary" {
            short_hash.to_string()
        } else {
            format!("{}-{}", base_slug, short_hash)
        }
    } else {
        base_slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_damerau_levenshtein() {
        for ((a, b), expected) in [
            (("", ""), 0),
            (("a", ""), 1),
            (("", "b"), 1),
            (("kitten", "sitting"), 3),
            (("ca", "ac"), 1),
            (("banana", "banna"), 1),
        ] {
            assert_eq!(damerau_levenshtein(a, b), expected);
        }
    }

    #[test]
    fn test_tokenize_terms_and_search_terms() {
        assert!(is_stop_word("The"));
        assert!(!is_stop_word("quantum"));

        let text = "The Quick Brown Fox jumps over 123 lazy dogs! #Rust_2026-edition";
        let terms = tokenize_terms(text);
        for expected in ["quick", "brown", "the", "rust_2026-edition"] {
            assert!(terms.contains(&expected.to_string()));
        }

        let search_terms = tokenize_search_terms(text);
        assert!(!search_terms.contains(&"the".to_string()));
        for expected in ["quick", "fox", "dogs"] {
            assert!(search_terms.contains(&expected.to_string()));
        }

        for empty in ["", "   \t\n  ", "!@#$%^&*()+=~`"] {
            assert!(tokenize_terms(empty).is_empty());
        }
        assert_eq!(
            tokenize_terms("Rust-2024_edition & AI 🤖"),
            vec!["rust-2024_edition", "ai"]
        );
        assert_eq!(tokenize_terms("--flag_name--"), vec!["flag_name"]);
    }

    #[test]
    fn test_decode_html_entities_and_strip_tags() {
        assert!(matches!(
            decode_html_entities("AT&T"),
            Cow::Borrowed("AT&T")
        ));
        for (raw, expected) in [
            (
                "&lt;div&gt;&quot;Hello &amp; welcome&quot;&lt;/div&gt;",
                "<div>\"Hello & welcome\"</div>",
            ),
            ("It&#39;s sunny &amp; warm", "It's sunny & warm"),
            ("NUL &#0; &#x00;", "NUL \u{FFFD} \u{FFFD}"),
        ] {
            assert_eq!(decode_html_entities(raw), expected);
        }

        assert_eq!(
            strip_html_tags(r#"x < y and <a title="a > b">hi</a>"#).trim(),
            "x < y and  hi"
        );
    }

    #[test]
    fn test_ascii_case_insensitive_and_http_url() {
        for (haystack, needle, expected) in [
            ("Hello World", "world", Some(6)),
            ("Hello World", "WORLD", Some(6)),
            ("Hello", "xyz", None),
            ("Short", "LongerPattern", None),
        ] {
            assert_eq!(find_ascii_case_insensitive(haystack, needle), expected);
        }
        assert!(ends_with_ascii_case_insensitive("test.RSS", ".rss"));
        assert!(!ends_with_ascii_case_insensitive("rss", ".rss"));
        assert!(is_http_url("HTTPS://Example.Com/path"));
        assert!(!is_http_url("ftp://example.com"));
    }

    #[test]
    fn test_url_encode_and_decode() {
        assert!(matches!(url_encode("simple"), Cow::Borrowed("simple")));
        assert_eq!(url_encode("Dune (novel)!"), "Dune%20%28novel%29%21");
        assert!(matches!(
            url_decode("simple").unwrap(),
            Cow::Borrowed("simple")
        ));
        assert_eq!(
            url_decode("https://en.wikipedia.org/wiki/Dune_%28novel%29").unwrap(),
            "https://en.wikipedia.org/wiki/Dune_(novel)"
        );
    }

    #[test]
    fn test_xml_scanner_and_attrs() {
        let xml = r#"<?xml version="1.0"?><!-- comment --><root xmlns:dc="http://purl.org"><dc:title>A &amp; B</dc:title><item id='i1' label="x > y"/><a href=https://x.com/>link</a></root>"#;
        let mut scanner = XmlScanner::new(xml);

        assert_eq!(
            scanner.next_event(),
            Some(XmlEvent::Start {
                name: "root",
                attrs: r#"xmlns:dc="http://purl.org""#,
                is_empty: false,
            })
        );
        assert!(matches!(
            scanner.next_event(),
            Some(XmlEvent::Start {
                name: "dc:title",
                ..
            })
        ));
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
        assert_eq!(xml_attr(attrs, "missing"), None);

        let Some(XmlEvent::Start {
            name,
            attrs,
            is_empty,
        }) = scanner.next_event()
        else {
            panic!("expected <a>");
        };
        assert_eq!((name, is_empty), ("a", false));
        assert_eq!(xml_attr(attrs, "href").as_deref(), Some("https://x.com/"));

        let mut pathological = "<a ".repeat(20_000);
        pathological.push_str(&"</".repeat(20_000));
        let mut pathological_scanner = XmlScanner::new(&pathological);
        while pathological_scanner.next_event().is_some() {}
    }
}
