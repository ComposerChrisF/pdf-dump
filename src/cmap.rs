//! ToUnicode CMap parsing for font-aware text extraction (Tier 1).
//!
//! A `/ToUnicode` CMap maps character codes — as they appear in the bytes of
//! `Tj`/`TJ`/`'`/`"` show-string operands — to Unicode strings. This is a
//! best-effort parser: malformed input never panics, it just yields fewer
//! mappings (degrading to "no mapping for that code").
//!
//! NON-GOALS (deferred to Tier 2/3): predefined CJK CMap resource files, the
//! Adobe Glyph List, and `usecmap` CMap chaining are not handled here.

use std::collections::BTreeMap;

use crate::stream::hex_digit;

/// A `bfrange` span: codes `lo..=hi` map to consecutive or listed destinations.
struct BfRange {
    lo: u32,
    hi: u32,
    dst: BfDst,
}

enum BfDst {
    /// `<lo> <hi> <dstStart>` — code `N` maps to `dstStart` with its last
    /// UTF-16 code unit incremented by `(N - lo)`. Stored as bounds + base
    /// units only; the per-code value is computed lazily in `map_code` so a
    /// huge range (e.g. `<0000><FFFF>`) costs O(1) memory, not 64K strings.
    Incrementing(Vec<u16>),
    /// `<lo> <hi> [ <d0> <d1> ... ]` — one destination string per code.
    Array(Vec<String>),
}

/// Width of a character code in bytes, derived from the codespace ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeWidth {
    /// All codespace ranges agree on a single byte width (1..=4).
    Fixed(u8),
    /// Ranges disagree; caller should fall back to a font-subtype heuristic.
    Variable(u8, u8),
    /// No codespace ranges were parsed.
    Unknown,
}

pub(crate) struct ToUnicodeCMap {
    /// Byte width of each parsed `begincodespacerange` entry.
    codespace_widths: Vec<u8>,
    /// `bfchar` single-code mappings.
    single: BTreeMap<u32, String>,
    /// `bfrange` spans, searched after `single` misses.
    ranges: Vec<BfRange>,
}

impl ToUnicodeCMap {
    /// Parse an already-decoded CMap stream (run `stream::decode_stream` first).
    pub(crate) fn parse(bytes: &[u8]) -> ToUnicodeCMap {
        let tokens = tokenize(bytes);
        let mut cmap = ToUnicodeCMap {
            codespace_widths: Vec::new(),
            single: BTreeMap::new(),
            ranges: Vec::new(),
        };

        let mut i = 0;
        while i < tokens.len() {
            match &tokens[i] {
                Token::Word(w) if w == b"begincodespacerange" => {
                    i = parse_codespace(&tokens, i + 1, &mut cmap);
                }
                Token::Word(w) if w == b"beginbfchar" => {
                    i = parse_bfchar(&tokens, i + 1, &mut cmap);
                }
                Token::Word(w) if w == b"beginbfrange" => {
                    i = parse_bfrange(&tokens, i + 1, &mut cmap);
                }
                _ => i += 1,
            }
        }

        cmap
    }

    /// Map one character code to its Unicode string, or `None` if unmapped.
    pub(crate) fn map_code(&self, code: u32) -> Option<String> {
        if let Some(s) = self.single.get(&code) {
            return Some(s.clone());
        }
        for r in &self.ranges {
            if code >= r.lo && code <= r.hi {
                return match &r.dst {
                    BfDst::Incrementing(units) => {
                        let mut u = units.clone();
                        if let Some(last) = u.last_mut() {
                            *last = last.wrapping_add((code - r.lo) as u16);
                        }
                        Some(units_to_string(&u))
                    }
                    BfDst::Array(v) => v.get((code - r.lo) as usize).cloned(),
                };
            }
        }
        None
    }

    /// Byte width to consume per code, derived from the codespace ranges.
    pub(crate) fn byte_width(&self) -> CodeWidth {
        match (
            self.codespace_widths.iter().min(),
            self.codespace_widths.iter().max(),
        ) {
            (Some(&min), Some(&max)) if min == max => CodeWidth::Fixed(min),
            (Some(&min), Some(&max)) => CodeWidth::Variable(min, max),
            _ => CodeWidth::Unknown,
        }
    }

    /// True if no mappings were parsed (CMap is unusable for decoding).
    pub(crate) fn is_empty(&self) -> bool {
        self.single.is_empty() && self.ranges.is_empty()
    }
}

// --- Section parsers ---------------------------------------------------------

/// Parse `begincodespacerange` pairs until `endcodespacerange`. Returns the
/// index just past the end keyword (or end-of-tokens).
fn parse_codespace(tokens: &[Token], mut i: usize, cmap: &mut ToUnicodeCMap) -> usize {
    while i < tokens.len() {
        match &tokens[i] {
            Token::Word(w) if w == b"endcodespacerange" => return i + 1,
            Token::Hex(lo) => {
                // Pair: <lo> <hi>. Width comes from the low token's byte length.
                cmap.codespace_widths.push(lo.len().clamp(1, 4) as u8);
                i += 2; // skip lo and hi
            }
            _ => i += 1,
        }
    }
    i
}

/// Parse `beginbfchar` `<src> <dst>` pairs until `endbfchar`.
fn parse_bfchar(tokens: &[Token], mut i: usize, cmap: &mut ToUnicodeCMap) -> usize {
    while i < tokens.len() {
        match &tokens[i] {
            Token::Word(w) if w == b"endbfchar" => return i + 1,
            Token::Hex(src) => {
                if let Some(Token::Hex(dst)) = tokens.get(i + 1) {
                    cmap.single.insert(be_to_u32(src), utf16be_to_string(dst));
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    i
}

/// Parse `beginbfrange` entries until `endbfrange`. Each entry is
/// `<lo> <hi> <dst>` or `<lo> <hi> [ <d0> <d1> ... ]`.
fn parse_bfrange(tokens: &[Token], mut i: usize, cmap: &mut ToUnicodeCMap) -> usize {
    while i < tokens.len() {
        match &tokens[i] {
            Token::Word(w) if w == b"endbfrange" => return i + 1,
            Token::Hex(lo) => {
                let lo_v = be_to_u32(lo);
                let hi_v = match tokens.get(i + 1) {
                    Some(Token::Hex(hi)) => be_to_u32(hi),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                match tokens.get(i + 2) {
                    Some(Token::Hex(dst)) => {
                        cmap.ranges.push(BfRange {
                            lo: lo_v,
                            hi: hi_v,
                            dst: BfDst::Incrementing(utf16be_to_units(dst)),
                        });
                        i += 3;
                    }
                    Some(Token::ArrayOpen) => {
                        let mut items = Vec::new();
                        let mut j = i + 3;
                        while j < tokens.len() {
                            match &tokens[j] {
                                Token::ArrayClose => {
                                    j += 1;
                                    break;
                                }
                                Token::Hex(d) => {
                                    items.push(utf16be_to_string(d));
                                    j += 1;
                                }
                                _ => j += 1,
                            }
                        }
                        cmap.ranges.push(BfRange {
                            lo: lo_v,
                            hi: hi_v,
                            dst: BfDst::Array(items),
                        });
                        i = j;
                    }
                    _ => i += 2,
                }
            }
            _ => i += 1,
        }
    }
    i
}

// --- Conversions -------------------------------------------------------------

/// Big-endian byte slice → u32 (codes are at most 4 bytes for Tier 1).
fn be_to_u32(bytes: &[u8]) -> u32 {
    let mut v = 0u32;
    for &b in bytes.iter().take(4) {
        v = (v << 8) | b as u32;
    }
    v
}

/// UTF-16BE bytes → code units. A trailing odd byte is dropped.
fn utf16be_to_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|c| ((c[0] as u16) << 8) | c[1] as u16)
        .collect()
}

/// UTF-16 code units → String, replacing lone surrogates with U+FFFD.
fn units_to_string(units: &[u16]) -> String {
    char::decode_utf16(units.iter().copied())
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

fn utf16be_to_string(bytes: &[u8]) -> String {
    units_to_string(&utf16be_to_units(bytes))
}

// --- Tokenizer ---------------------------------------------------------------

enum Token {
    Hex(Vec<u8>),
    ArrayOpen,
    ArrayClose,
    Word(Vec<u8>),
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0 | 0x0c)
}

fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Tokenize CMap text into the few token kinds the parser cares about.
/// Names, numbers, dict literals (`<< >>`), literal strings, and comments are
/// skipped (numbers surface as `Word` but never match a section keyword).
fn tokenize(data: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if is_ws(b) {
            i += 1;
        } else if b == b'%' {
            // Comment to end of line.
            while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
                i += 1;
            }
        } else if b == b'<' {
            if data.get(i + 1) == Some(&b'<') {
                // Dictionary open `<<` — skip both, contents tokenized normally.
                i += 2;
            } else {
                // Hex string `<...>`.
                i += 1;
                let mut nibbles = Vec::new();
                while i < data.len() && data[i] != b'>' {
                    if let Some(d) = hex_digit(data[i]) {
                        nibbles.push(d);
                    }
                    i += 1;
                }
                i += 1; // skip '>'
                let mut bytes = Vec::with_capacity(nibbles.len().div_ceil(2));
                let mut k = 0;
                while k < nibbles.len() {
                    let hi = nibbles[k];
                    let lo = nibbles.get(k + 1).copied().unwrap_or(0);
                    bytes.push((hi << 4) | lo);
                    k += 2;
                }
                tokens.push(Token::Hex(bytes));
            }
        } else if b == b'>' {
            // Stray `>` (e.g. the second of a `>>` dict close).
            i += 1;
        } else if b == b'[' {
            tokens.push(Token::ArrayOpen);
            i += 1;
        } else if b == b']' {
            tokens.push(Token::ArrayClose);
            i += 1;
        } else if b == b'(' {
            // Literal string — skip with paren balancing and `\` escapes.
            i += 1;
            let mut depth = 1;
            while i < data.len() && depth > 0 {
                match data[i] {
                    b'\\' => {
                        i += 2;
                        continue;
                    }
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
        } else if b == b'/' || b == b'{' || b == b'}' || b == b')' {
            // Name (consume following regular chars), or stray delimiter.
            i += 1;
            if b == b'/' {
                while i < data.len() && !is_ws(data[i]) && !is_delim(data[i]) {
                    i += 1;
                }
            }
        } else {
            // Bare token: keyword or number.
            let start = i;
            while i < data.len() && !is_ws(data[i]) && !is_delim(data[i]) {
                i += 1;
            }
            if i > start {
                tokens.push(Token::Word(data[start..i].to_vec()));
            } else {
                i += 1; // safety against a delimiter we didn't special-case
            }
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bfchar_single_one_byte() {
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <41> <0041> endbfchar");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("A"));
    }

    #[test]
    fn bfchar_single_two_byte() {
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <0041> <0041> endbfchar");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("A"));
    }

    #[test]
    fn bfrange_incrementing() {
        let cmap = ToUnicodeCMap::parse(b"beginbfrange <0041> <0043> <0041> endbfrange");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("A"));
        assert_eq!(cmap.map_code(0x42).as_deref(), Some("B"));
        assert_eq!(cmap.map_code(0x43).as_deref(), Some("C"));
    }

    #[test]
    fn bfrange_array() {
        let cmap =
            ToUnicodeCMap::parse(b"beginbfrange <0041> <0043> [<0058> <0059> <005A>] endbfrange");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("X"));
        assert_eq!(cmap.map_code(0x42).as_deref(), Some("Y"));
        assert_eq!(cmap.map_code(0x43).as_deref(), Some("Z"));
    }

    #[test]
    fn surrogate_pair() {
        // U+1F600 GRINNING FACE = UTF-16BE D83D DE00.
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <0001> <D83DDE00> endbfchar");
        let s = cmap.map_code(1).unwrap();
        assert_eq!(s.chars().count(), 1);
        assert_eq!(s.chars().next().unwrap() as u32, 0x1F600);
    }

    #[test]
    fn multi_unit_ligature() {
        // One code -> two characters "fl".
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <0001> <0066006C> endbfchar");
        let s = cmap.map_code(1).unwrap();
        assert_eq!(s, "fl");
        assert_eq!(s.chars().count(), 2);
    }

    #[test]
    fn byte_width_fixed_two() {
        let cmap = ToUnicodeCMap::parse(b"begincodespacerange <0000> <FFFF> endcodespacerange");
        assert_eq!(cmap.byte_width(), CodeWidth::Fixed(2));
    }

    #[test]
    fn byte_width_fixed_one() {
        let cmap = ToUnicodeCMap::parse(b"begincodespacerange <00> <FF> endcodespacerange");
        assert_eq!(cmap.byte_width(), CodeWidth::Fixed(1));
    }

    #[test]
    fn byte_width_variable() {
        let cmap =
            ToUnicodeCMap::parse(b"begincodespacerange <00> <FF> <0000> <FFFF> endcodespacerange");
        assert_eq!(cmap.byte_width(), CodeWidth::Variable(1, 2));
    }

    #[test]
    fn byte_width_unknown_when_absent() {
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <41> <0041> endbfchar");
        assert_eq!(cmap.byte_width(), CodeWidth::Unknown);
    }

    #[test]
    fn map_code_miss_returns_none() {
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <41> <0041> endbfchar");
        assert_eq!(cmap.map_code(0x42), None);
    }

    #[test]
    fn huge_incrementing_range_is_lazy() {
        // A full 2-byte incrementing range must not expand into 64K strings.
        let cmap = ToUnicodeCMap::parse(b"beginbfrange <0000> <FFFF> <0041> endbfrange");
        assert_eq!(cmap.ranges.len(), 1);
        assert!(cmap.single.is_empty());
        // Lazily computed: code 0x0001 -> 0x0041 + 1 = 'B'.
        assert_eq!(cmap.map_code(0x0001).as_deref(), Some("B"));
    }

    #[test]
    fn unterminated_section_still_maps() {
        // No endbfchar, no trailing tokens.
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <41> <0041>");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("A"));
    }

    #[test]
    fn empty_dst_maps_to_empty_string() {
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <41> <> endbfchar");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some(""));
    }

    #[test]
    fn lone_surrogate_becomes_replacement() {
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <41> <D83D> endbfchar");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("\u{FFFD}"));
    }

    #[test]
    fn garbage_between_sections_is_skipped() {
        let data = b"/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) >> def\n\
                     1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
                     2 beginbfchar <0041> <0041> <0042> <0042> endbfchar";
        let cmap = ToUnicodeCMap::parse(data);
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("A"));
        assert_eq!(cmap.map_code(0x42).as_deref(), Some("B"));
        assert_eq!(cmap.byte_width(), CodeWidth::Fixed(2));
    }

    #[test]
    fn reversed_range_never_matches() {
        let cmap = ToUnicodeCMap::parse(b"beginbfrange <0043> <0041> <0041> endbfrange");
        assert_eq!(cmap.map_code(0x42), None);
    }

    #[test]
    fn array_shorter_than_range_returns_none_for_tail() {
        let cmap = ToUnicodeCMap::parse(b"beginbfrange <0041> <0043> [<0058>] endbfrange");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("X"));
        assert_eq!(cmap.map_code(0x42), None);
    }

    #[test]
    fn odd_hex_digits_do_not_panic() {
        // Three nibbles in the source token; must not panic.
        let cmap = ToUnicodeCMap::parse(b"beginbfchar <041> <0041> endbfchar");
        // Nibbles 0,4,1 -> bytes 0x04, 0x10 -> code 0x0410.
        assert_eq!(cmap.map_code(0x0410).as_deref(), Some("A"));
    }

    #[test]
    fn comments_are_skipped() {
        let cmap =
            ToUnicodeCMap::parse(b"% a comment line\nbeginbfchar <41> <0041> endbfchar % trailing");
        assert_eq!(cmap.map_code(0x41).as_deref(), Some("A"));
    }

    #[test]
    fn empty_input_is_empty() {
        let cmap = ToUnicodeCMap::parse(b"");
        assert!(cmap.is_empty());
        assert_eq!(cmap.byte_width(), CodeWidth::Unknown);
    }
}
