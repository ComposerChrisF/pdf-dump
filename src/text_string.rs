//! PDF text strings (ISO 32000-1 § 7.9.2.2, ISO 32000-2 § 7.9.2.2) and the XML-based rich text and
//! XFA data that travel in them.  Every mode that prints a text string goes through here, so a
//! string is never passed through lossily: what cannot be decoded comes back as hex, and flagged.

use crate::encodings::pdfdoc;
use crate::helpers::format_dict_value;
use crate::stream::decode_stream;
use lopdf::{Document, Object};
use std::fmt::Write;

/// A decoded text string.  When `decoded` is false the bytes could not be read under the encoding
/// they claim, and `text` is their lossless hex form, `<FEFF0041…>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextString {
    pub text: String,
    pub decoded: bool,
}

impl TextString {
    pub(crate) fn undecoded(bytes: &[u8]) -> Self {
        let mut text = String::with_capacity(bytes.len() * 2 + 2);
        text.push('<');
        for b in bytes {
            let _ = write!(text, "{b:02X}");
        }
        text.push('>');
        TextString {
            text,
            decoded: false,
        }
    }
}

/// The text of a value that should be a text string or text stream, directly or by reference.
/// `None` only when it is absent by the standard's own reading: null, or a reference to nothing
/// (ISO 32000-1 § 7.3.10).  Anything else that cannot be read comes back flagged: a stream that will
/// not decode, a reference lopdf could not resolve (a cycle), or a value of another type.  None of
/// those is evidence of nothing being there.
pub(crate) fn read_text_value(doc: &Document, obj: &Object) -> Option<TextString> {
    let target = match obj {
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(target) => target,
            Err(lopdf::Error::ObjectNotFound(_)) => return None,
            Err(_) => {
                return Some(TextString {
                    text: "(unresolvable reference)".to_string(),
                    decoded: false,
                });
            }
        },
        other => other,
    };
    match target {
        Object::Null => None,
        Object::String(bytes, _) => Some(decode_text_string(bytes)),
        Object::Stream(stream) => Some(read_text_stream(doc, stream)),
        other => Some(TextString {
            text: format_dict_value(other),
            decoded: false,
        }),
    }
}

/// A text stream: its decoded bytes, read as a text string.
pub(crate) fn read_text_stream(doc: &Document, stream: &lopdf::Stream) -> TextString {
    let (bytes, warning) = decode_stream(doc, stream);
    match warning {
        Some(_) => TextString::undecoded(&bytes),
        None => decode_text_string(&bytes),
    }
}

/// The words of a rich-text value (`/RV`, `/RC`), markup removed.  Flagged when the bytes will not
/// decode or the markup is malformed; the text is then as much as could be kept.
pub(crate) fn read_rich_text(doc: &Document, obj: &Object) -> Option<TextString> {
    let raw = read_text_value(doc, obj)?;
    if !raw.decoded {
        return Some(raw);
    }
    let (text, well_formed) = strip_markup(&raw.text);
    Some(TextString {
        text,
        decoded: well_formed,
    })
}

/// Decodes a text string by its byte-order mark: UTF-16BE (`FE FF`), UTF-8 (`EF BB BF`, PDF 2.0), and
/// otherwise PDFDocEncoding.  A language escape (§ 7.9.2.2: U+001B, a language code, optionally a
/// country code, U+001B) is metadata about the text, not text, so it is removed.
///
/// Not decodable, so flagged: an odd byte count or an unpaired surrogate under the UTF-16 mark,
/// invalid UTF-8 under the UTF-8 mark, a byte PDFDocEncoding leaves undefined, and an escape that is
/// never closed or holds anything but a code.  An escape's contents must be checked: taking whatever
/// lies between two escapes would let them hide any amount of text.
pub(crate) fn decode_text_string(bytes: &[u8]) -> TextString {
    let decoded = if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        decode_utf16be(rest)
    } else if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        decode_utf8(rest)
    } else {
        bytes.iter().map(|&b| pdfdoc(b)).collect()
    };
    match decoded {
        Some(text) => TextString {
            text,
            decoded: true,
        },
        None => TextString::undecoded(bytes),
    }
}

const ESCAPE: u16 = 0x1B;

/// Under UTF-16 a language code is its raw ASCII bytes, so 2 or 4 letters are 1 or 2 code units.
fn decode_utf16be(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    let mut text = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        if units[i] != ESCAPE {
            text.push(units[i]);
            i += 1;
            continue;
        }
        let len = units[i + 1..].iter().position(|u| *u == ESCAPE)?;
        let is_code = matches!(len, 1 | 2)
            && units[i + 1..i + 1 + len]
                .iter()
                .flat_map(|u| u.to_be_bytes())
                .all(|b| b.is_ascii_alphabetic());
        if !is_code {
            return None;
        }
        i += len + 2;
    }
    char::decode_utf16(text).collect::<Result<String, _>>().ok()
}

/// Under UTF-8 a language code is its letters, so 2 or 4 bytes.
fn decode_utf8(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes).ok()?;
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('\u{1B}') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let len = after.find('\u{1B}')?;
        if !matches!(len, 2 | 4) || !after[..len].bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        rest = &after[len + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Elements that sit inside a run of text; every other element's boundary separates text, so that
/// `<a>Jane</a><b>Doe</b>`-style data does not run together.
const INLINE_ELEMENTS: &[&str] = &[
    "a", "b", "big", "code", "em", "font", "i", "s", "small", "span", "strike", "strong", "sub",
    "sup", "u",
];

/// The text of XML markup: rich text (`/RV`, `/RC`) or XFA data.  Tags, comments and processing
/// instructions go; character data, CDATA and entities stay, and a block boundary starts a new line.
/// Attribute values are not text.
///
/// The bool is false when the markup is malformed: a tag or section with no end, a `<` inside a tag,
/// an attribute with no value, or elements that do not balance.  Then nothing is stripped and the
/// input comes back whole, since what looks like a tag there may be someone's words.
pub(crate) fn strip_markup(xml: &str) -> (String, bool) {
    match try_strip_markup(xml) {
        Some(text) => (text, true),
        None => (xml.to_string(), false),
    }
}

fn try_strip_markup(xml: &str) -> Option<String> {
    let mut out = String::with_capacity(xml.len());
    let mut open: Vec<&str> = Vec::new();
    let mut rest = xml;
    while let Some(lt) = rest.find('<') {
        push_character_data(&mut out, &rest[..lt]);
        let markup = &rest[lt..];
        if is_literal_lt(markup) {
            // A bare `<`, as in `a < b`: text, not markup.
            out.push('<');
            rest = &markup[1..];
            continue;
        }
        let (end, kind) = markup_end(markup)?;
        match kind {
            // `<![CDATA[` and `]]>` are 9 and 3 bytes.
            Markup::Cdata => out.push_str(&markup[9..end - 3]),
            Markup::Skipped => {}
            Markup::Tag(tag) => {
                match tag {
                    Tag::Open(name) => open.push(name),
                    Tag::Close(name) => {
                        if open.pop() != Some(name) {
                            return None;
                        }
                    }
                    Tag::SelfClosed => {}
                }
                if !is_inline_tag(&markup[..end]) {
                    start_new_line(&mut out);
                }
            }
        }
        rest = &markup[end..];
    }
    push_character_data(&mut out, rest);
    open.is_empty().then(|| tidy_lines(&out))
}

enum Tag<'a> {
    Open(&'a str),
    Close(&'a str),
    SelfClosed,
}

enum Markup<'a> {
    Tag(Tag<'a>),
    Cdata,
    Skipped,
}

/// The byte length of the markup opening `s` (which starts with `<`), and its kind; `None` when it
/// is malformed.
fn markup_end(s: &str) -> Option<(usize, Markup<'_>)> {
    if s.starts_with("<![CDATA[") {
        return s.find("]]>").map(|i| (i + 3, Markup::Cdata));
    }
    if s.starts_with("<!--") {
        return s.find("-->").map(|i| (i + 3, Markup::Skipped));
    }
    if s.starts_with("<?") {
        return s.find("?>").map(|i| (i + 2, Markup::Skipped));
    }
    if s.starts_with("<!") {
        return s.find('>').map(|i| (i + 1, Markup::Skipped));
    }
    parse_tag(s)
}

/// One element tag: a name, then attributes of the form `name="value"`.
fn parse_tag(s: &str) -> Option<(usize, Markup<'_>)> {
    let closing = s.starts_with("</");
    let name_start = if closing { 2 } else { 1 };
    let name_len =
        s[name_start..].find(|c: char| c.is_whitespace() || c == '/' || c == '>' || c == '<')?;
    if name_len == 0 {
        return None;
    }
    let name = &s[name_start..name_start + name_len];
    let mut at = name_start + name_len;
    loop {
        at += s[at..].len() - s[at..].trim_start().len();
        match s[at..].chars().next()? {
            '>' => {
                let tag = if closing {
                    Tag::Close(name)
                } else {
                    Tag::Open(name)
                };
                return Some((at + 1, Markup::Tag(tag)));
            }
            '/' if !closing => {
                return s[at + 1..]
                    .starts_with('>')
                    .then_some((at + 2, Markup::Tag(Tag::SelfClosed)));
            }
            _ if closing => return None,
            _ => {
                let eq = s[at..].find('=')?;
                let attribute = s[at..at + eq].trim_end();
                if attribute.is_empty()
                    || attribute
                        .contains(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '/'))
                {
                    return None;
                }
                at += eq + 1;
                at += s[at..].len() - s[at..].trim_start().len();
                let quote = s[at..].chars().next().filter(|c| matches!(c, '"' | '\''))?;
                let close = s[at + 1..].find(quote)?;
                if s[at + 1..at + 1 + close].contains('<') {
                    return None;
                }
                at += close + 2;
            }
        }
    }
}

/// Every element named `local`, under any namespace prefix: from its `<` through its closing tag, or
/// to the end of the input if it has none, which `strip_markup` then reports.  The walk goes
/// by markup, so an element inside a comment or CDATA does not count.
pub(crate) fn find_elements<'a>(xml: &'a str, local: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut opened_at = None;
    let mut depth = 0usize;
    let mut at = 0;
    while let Some(lt) = xml[at..].find('<') {
        let start = at + lt;
        let markup = &xml[start..];
        if is_literal_lt(markup) {
            at = start + 1;
            continue;
        }
        let Some((end, kind)) = markup_end(markup) else {
            break;
        };
        if let Markup::Tag(tag) = kind {
            let is_named = |name: &str| name.rsplit(':').next() == Some(local);
            match tag {
                Tag::Open(name) if is_named(name) => {
                    if depth == 0 {
                        opened_at = Some(start);
                    }
                    depth += 1;
                }
                Tag::Close(name) if is_named(name) && depth > 0 => {
                    depth -= 1;
                    if depth == 0
                        && let Some(open) = opened_at.take()
                    {
                        found.push(&xml[open..start + end]);
                    }
                }
                _ => {}
            }
        }
        at = start + end;
    }
    if let Some(open) = opened_at {
        found.push(&xml[open..]);
    }
    found
}

/// A `<` that cannot open markup: nothing that could start a name follows it.
fn is_literal_lt(s: &str) -> bool {
    match s[1..].chars().next() {
        Some('/' | '?' | '!' | '_' | ':') => false,
        Some(c) => !c.is_alphabetic(),
        None => true,
    }
}

fn is_inline_tag(tag: &str) -> bool {
    let name = tag
        .trim_start_matches('<')
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .next()
        .unwrap_or("");
    let local = name.rsplit(':').next().unwrap_or(name);
    INLINE_ELEMENTS.contains(&local.to_ascii_lowercase().as_str())
}

fn start_new_line(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn push_character_data(out: &mut String, data: &str) {
    let mut rest = data;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let entity = &rest[amp..];
        match entity
            .find(';')
            .and_then(|semi| Some((semi, resolve_entity(&entity[1..semi])?)))
        {
            Some((semi, c)) => {
                out.push(c);
                rest = &entity[semi + 1..];
            }
            // An entity this reader cannot resolve stays visible rather than vanishing.
            None => {
                out.push('&');
                rest = &entity[1..];
            }
        }
    }
    out.push_str(rest);
}

fn resolve_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let code = name.strip_prefix('#')?;
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value)
        }
    }
}

/// Drops the blank lines and the indentation that pretty-printed XML leaves between elements.
fn tidy_lines(s: &str) -> String {
    let lines: Vec<&str> = s
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn utf16(s: &str) -> Vec<u8> {
        let mut out = vec![0xFE, 0xFF];
        out.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        out
    }

    fn text(bytes: &[u8]) -> String {
        let d = decode_text_string(bytes);
        assert!(d.decoded, "expected to decode: {bytes:?}");
        d.text
    }

    #[test]
    fn utf16be_with_a_bom_decodes() {
        assert_eq!(text(&utf16("Aia i ka La‘i")), "Aia i ka La‘i");
    }

    #[test]
    fn utf16be_surrogate_pair_decodes() {
        assert_eq!(text(&utf16("x😀y")), "x😀y");
    }

    #[test]
    fn utf8_with_a_bom_decodes() {
        assert_eq!(text("\u{FEFF}Mākaha".as_bytes()), "Mākaha");
    }

    #[test]
    fn pdfdoc_high_bytes_are_their_table_values_not_latin1_guesses() {
        // 0x80 bullet, 0x8D left quote, 0xA0 euro, 0xE9 e-acute.
        assert_eq!(text(&[0x80, 0x8D, 0xA0, 0xE9]), "•“€é");
    }

    #[test]
    fn raw_utf8_without_a_bom_reads_as_pdfdoc_as_the_standard_says() {
        // Invalid per § 7.9.2.2, and showing it as mojibake is what makes the defect visible:
        // pdf-orchestrator's bug-0039 stayed hidden for as long as this printed as valid.
        assert_eq!(text("é".as_bytes()), "Ã©");
    }

    #[test]
    fn language_escape_is_removed() {
        // A UTF-16 string carries the code as raw bytes: 00 1B, 65 6E, 00 1B.
        let mut bytes = vec![0xFE, 0xFF, 0x00, 0x1B, b'e', b'n', 0x00, 0x1B];
        bytes.extend("Hello".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(text(&bytes), "Hello");
        // With a country code, between other text.
        let mut bytes = vec![0xFE, 0xFF, 0x00, b'a', 0x00, 0x1B];
        bytes.extend(b"enUS");
        bytes.extend([0x00, 0x1B, 0x00, b'b']);
        assert_eq!(text(&bytes), "ab");
        assert_eq!(text("\u{FEFF}a\u{1B}en\u{1B}b".as_bytes()), "ab");
        assert_eq!(text("\u{FEFF}a\u{1B}enUS\u{1B}b".as_bytes()), "ab");
    }

    #[test]
    fn undecodable_strings_are_flagged_and_lossless() {
        for (bytes, hex) in [
            // Odd length under the UTF-16 mark.
            (&[0xFE, 0xFF, 0x00][..], "<FEFF00>"),
            // An unpaired high surrogate.
            (&[0xFE, 0xFF, 0xD8, 0x00][..], "<FEFFD800>"),
            // Invalid UTF-8 under the UTF-8 mark.
            (&[0xEF, 0xBB, 0xBF, 0xC3][..], "<EFBBBFC3>"),
            // Bytes PDFDocEncoding leaves undefined.
            (&[b'a', 0x7F][..], "<617F>"),
            (&[0x9F][..], "<9F>"),
            (&[0xAD][..], "<AD>"),
            (&[0x00][..], "<00>"),
        ] {
            let d = decode_text_string(bytes);
            assert_eq!((d.text.as_str(), d.decoded), (hex, false));
        }
    }

    #[test]
    fn an_unclosed_language_escape_is_undecodable() {
        assert!(!decode_text_string(&utf16("a\u{1B}en")).decoded);
        assert!(!decode_text_string("\u{FEFF}a\u{1B}en".as_bytes()).decoded);
    }

    #[test]
    fn an_escape_holding_anything_but_a_code_is_undecodable_not_dropped() {
        for hidden in [
            // Words between escapes.
            utf16("A\u{1B}Lockbox code 7301\u{1B}B"),
            // The code spelled as UTF-16 characters, which is not what the standard writes.
            utf16("\u{1B}en\u{1B}Hello"),
            "\u{FEFF}A\u{1B}Lockbox code 7301\u{1B}B"
                .as_bytes()
                .to_vec(),
            "\u{FEFF}A\u{1B}123\u{1B}B".as_bytes().to_vec(),
        ] {
            let d = decode_text_string(&hidden);
            assert!(!d.decoded, "{hidden:?}");
            assert!(d.text.starts_with('<'), "lossless hex: {}", d.text);
        }
    }

    #[test]
    fn empty_string_decodes_to_empty() {
        assert_eq!(text(b""), "");
        assert_eq!(text(&[0xFE, 0xFF]), "");
    }

    fn stripped(xml: &str) -> String {
        let (s, ok) = strip_markup(xml);
        assert!(ok, "expected well-formed: {xml}");
        s
    }

    #[test]
    fn rich_text_keeps_the_words_and_drops_the_markup() {
        assert_eq!(
            stripped(
                r#"<?xml version="1.0"?><body xmlns="http://www.w3.org/1999/xhtml"><p>Lockbox <b>code</b>: 7301</p><p style="a>b">Next</p></body>"#
            ),
            "Lockbox code: 7301\nNext"
        );
    }

    #[test]
    fn entities_and_cdata_are_resolved() {
        assert_eq!(
            stripped("<p>a &amp; b &#65;&#x42; <![CDATA[<raw> &amp;]]></p>"),
            "a & b AB <raw> &amp;"
        );
    }

    #[test]
    fn data_elements_do_not_run_together() {
        assert_eq!(
            stripped("<data>\n  <name>Jane</name>\n  <ssn>123</ssn>\n</data>"),
            "Jane\n123"
        );
    }

    #[test]
    fn comments_and_attributes_are_not_text() {
        assert_eq!(
            stripped("<p title='hidden'>seen<!-- not seen --></p>"),
            "seen"
        );
    }

    #[test]
    fn a_bare_less_than_is_text() {
        assert_eq!(stripped("<p>a < b</p>"), "a < b");
    }

    #[test]
    fn an_unknown_entity_stays_visible() {
        assert_eq!(stripped("<p>a&nbsp;b &</p>"), "a&nbsp;b &");
    }

    #[test]
    fn malformed_markup_is_flagged_and_comes_back_whole() {
        for input in [
            // An unterminated tag.
            "<p>seen</p><p class='x",
            // A `<` inside a tag.
            "<p>Code<abc 7301</p>tail",
            // Words in angle brackets: an attribute with no value.
            "Call <Jane Doe> at 555",
            // Elements that do not balance.
            "<p><b>unclosed</p>",
            "<p>unclosed",
            "stray</p>",
            // An unterminated comment and section.
            "seen<!-- never closed",
            "seen<![CDATA[ never closed",
        ] {
            let (s, ok) = strip_markup(input);
            assert!(!ok, "{input}");
            assert_eq!(s, input);
        }
    }

    #[test]
    fn self_closing_and_attribute_forms_are_well_formed() {
        assert_eq!(stripped("<p a=\"1\" b = 'x'>one<br/>two</p>"), "one\ntwo");
    }

    #[test]
    fn find_elements_goes_by_markup_not_substrings() {
        let xml = "<!-- <x:datasets>example</x:datasets> --><mydatasets>no</mydatasets>\
                   <x:datasets a='1'>one<datasets>two</datasets></x:datasets><datasets>three</datasets>";
        assert_eq!(
            find_elements(xml, "datasets"),
            vec![
                "<x:datasets a='1'>one<datasets>two</datasets></x:datasets>",
                "<datasets>three</datasets>"
            ]
        );
        assert_eq!(
            find_elements("<datasets>cut", "datasets"),
            vec!["<datasets>cut"]
        );
        assert!(find_elements("<p>datasets</p>", "datasets").is_empty());
    }

    #[test]
    fn a_value_of_the_wrong_type_is_flagged_not_absent() {
        let doc = Document::new();
        let d = read_text_value(&doc, &Object::Integer(7)).unwrap();
        assert_eq!((d.text.as_str(), d.decoded), ("7", false));
        assert_eq!(read_text_value(&doc, &Object::Null), None);
        assert_eq!(read_text_value(&doc, &Object::Reference((99, 0))), None);
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(stripped("just words"), "just words");
    }
}
