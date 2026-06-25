//! WinAnsiEncoding → Unicode table for Tier 1 text extraction.
//!
//! WinAnsiEncoding is, for text-extraction purposes, Windows-1252. Simple
//! (single-byte) fonts that declare `/Encoding /WinAnsiEncoding` but ship no
//! `/ToUnicode` map are decoded through this table. Plain byte-passthrough
//! mangles the CP1252 high range — curly quotes (`0x91`-`0x94`), en/em dashes
//! (`0x96`/`0x97`), and accented Latin (`é` = `0xE9`) are all invalid UTF-8 —
//! so the table is a strict improvement for that case.
//!
//! NON-GOAL (Tier 2): MacRomanEncoding/StandardEncoding tables and Adobe Glyph
//! List glyph-name resolution for `/Differences` are not handled here.

/// The CP1252 high range `0x80..=0x9F`. The five unassigned slots
/// (`0x81`, `0x8D`, `0x8F`, `0x90`, `0x9D`) are `None`.
static HIGH_80_9F: [Option<char>; 32] = [
    Some('\u{20AC}'), // 0x80 €
    None,             // 0x81
    Some('\u{201A}'), // 0x82 ‚
    Some('\u{0192}'), // 0x83 ƒ
    Some('\u{201E}'), // 0x84 „
    Some('\u{2026}'), // 0x85 …
    Some('\u{2020}'), // 0x86 †
    Some('\u{2021}'), // 0x87 ‡
    Some('\u{02C6}'), // 0x88 ˆ
    Some('\u{2030}'), // 0x89 ‰
    Some('\u{0160}'), // 0x8A Š
    Some('\u{2039}'), // 0x8B ‹
    Some('\u{0152}'), // 0x8C Œ
    None,             // 0x8D
    Some('\u{017D}'), // 0x8E Ž
    None,             // 0x8F
    None,             // 0x90
    Some('\u{2018}'), // 0x91 ‘
    Some('\u{2019}'), // 0x92 ’
    Some('\u{201C}'), // 0x93 “
    Some('\u{201D}'), // 0x94 ”
    Some('\u{2022}'), // 0x95 •
    Some('\u{2013}'), // 0x96 –
    Some('\u{2014}'), // 0x97 —
    Some('\u{02DC}'), // 0x98 ˜
    Some('\u{2122}'), // 0x99 ™
    Some('\u{0161}'), // 0x9A š
    Some('\u{203A}'), // 0x9B ›
    Some('\u{0153}'), // 0x9C œ
    None,             // 0x9D
    Some('\u{017E}'), // 0x9E ž
    Some('\u{0178}'), // 0x9F Ÿ
];

/// Map a single byte through WinAnsiEncoding. Returns `None` for codes with no
/// printable glyph (control range and the unassigned CP1252 slots).
pub(crate) fn winansi(b: u8) -> Option<char> {
    match b {
        // Printable ASCII (0x20 SPACE through 0x7E TILDE) is identity.
        0x20..=0x7E => Some(b as char),
        // CP1252 high range.
        0x80..=0x9F => HIGH_80_9F[(b - 0x80) as usize],
        // 0xA0..=0xFF is Latin-1: the Unicode scalar equals the byte value.
        0xA0..=0xFF => Some(b as char),
        // Control range (0x00..=0x1F) and 0x7F have no text glyph.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_identity() {
        assert_eq!(winansi(b'A'), Some('A'));
        assert_eq!(winansi(b' '), Some(' '));
        assert_eq!(winansi(b'~'), Some('~'));
    }

    #[test]
    fn curly_quotes() {
        assert_eq!(winansi(0x91), Some('\u{2018}')); // ‘
        assert_eq!(winansi(0x92), Some('\u{2019}')); // ’
        assert_eq!(winansi(0x93), Some('\u{201C}')); // “
        assert_eq!(winansi(0x94), Some('\u{201D}')); // ”
    }

    #[test]
    fn dashes() {
        assert_eq!(winansi(0x96), Some('\u{2013}')); // – en dash
        assert_eq!(winansi(0x97), Some('\u{2014}')); // — em dash
    }

    #[test]
    fn euro_and_trademark() {
        assert_eq!(winansi(0x80), Some('\u{20AC}')); // €
        assert_eq!(winansi(0x99), Some('\u{2122}')); // ™
    }

    #[test]
    fn latin1_high_range() {
        assert_eq!(winansi(0xE9), Some('é'));
        assert_eq!(winansi(0xA0), Some('\u{00A0}')); // nbsp
        assert_eq!(winansi(0xFF), Some('ÿ'));
    }

    #[test]
    fn unassigned_and_control_are_none() {
        assert_eq!(winansi(0x81), None);
        assert_eq!(winansi(0x8D), None);
        assert_eq!(winansi(0x9D), None);
        assert_eq!(winansi(0x00), None);
        assert_eq!(winansi(0x1F), None);
        assert_eq!(winansi(0x7F), None);
    }
}
