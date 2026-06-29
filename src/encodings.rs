//! Base-encoding → Unicode tables for Tier 1 text extraction.
//!
//! Simple (single-byte) fonts that declare a recognized `/Encoding` but ship no
//! `/ToUnicode` map are decoded through these tables instead of raw byte
//! passthrough, which mangles every non-ASCII glyph (the bytes are not valid
//! UTF-8). Supported here:
//!
//! - **WinAnsiEncoding** = Windows-1252. The CP1252 high range carries curly
//!   quotes (`0x91`-`0x94`), en/em dashes (`0x96`/`0x97`), accented Latin, etc.
//! - **MacRomanEncoding** = Mac OS Roman. macOS exports use this heavily; its
//!   high range carries the apostrophe/quotes (`0xD5`/`0xD2`-`0xD4`) and dashes
//!   (`0xD0`/`0xD1`) that otherwise extract as U+FFFD.
//! - **StandardEncoding** = Adobe Standard. The historical default for Type1
//!   fonts; its ASCII range differs from US-ASCII at two codes (`0x27` is the
//!   right single quote `’`, `0x60` the left single quote `‘`), and it has a
//!   sparse high range (ligatures, accents, dashes) that passthrough mangles.
//! - **MacExpertEncoding** = Adobe expert set (small caps, old-style figures,
//!   superior/inferior figures, fractions, extra ligatures). Most of these
//!   glyphs are *stylistic variants* with no dedicated Unicode codepoint, so
//!   `macexpert` folds each to its semantic base character (small-cap A → `A`,
//!   old-style 3 → `3`) and uses exact Unicode only where one exists (fractions,
//!   digit super/subscripts). Best-effort: styling is not preserved.
//!
//! Decoders return `&'static str`, so one code can yield more than one
//! character. The f-ligatures (`ﬀﬁﬂﬃﬄ`) are decomposed to their ASCII
//! components (`ff`, `fi`, …) across every table so the extracted text stays
//! searchable (a precomposed `U+FB01` would not match a plain-text search for
//! `fi`), and a few MacExpert glyphs expand likewise (`rupiah` → `Rp`).
//!
//! `/Differences` glyph-name resolution (via the Adobe Glyph List) layers *on
//! top of* these tables: see `crate::glyphlist`, which `text.rs` consults for
//! overridden codes before falling through to the base table here.

/// Printable ASCII `0x20..=0x7E` packed in order. Each byte is single-byte
/// UTF-8, so a one-byte slice is a valid `&'static str` for any code in range —
/// this lets the decoders return `&'static str` for the ASCII identity range
/// with neither per-byte literals nor allocation.
const ASCII_PRINTABLE: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

/// Latin-1 supplement `U+00A0..=U+00FF` packed in order. Every char is two-byte
/// UTF-8, so a two-byte slice on an even offset is a valid `&'static str`.
const LATIN1_HIGH: &str = "\u{00A0}¡¢£¤¥¦§¨©ª«¬\u{00AD}®¯°±²³´µ¶·¸¹º»¼½¾¿ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖ×ØÙÚÛÜÝÞßàáâãäåæçèéêëìíîïðñòóôõö÷øùúûüýþÿ";

/// One printable-ASCII byte (`0x20..=0x7E`) as a `&'static str`.
#[inline]
fn ascii_str(b: u8) -> &'static str {
    let i = (b - 0x20) as usize;
    &ASCII_PRINTABLE[i..i + 1]
}

/// One Latin-1-supplement byte (`0xA0..=0xFF`) as a `&'static str`.
#[inline]
fn latin1_str(b: u8) -> &'static str {
    let i = (b - 0xA0) as usize * 2;
    &LATIN1_HIGH[i..i + 2]
}

/// The CP1252 high range `0x80..=0x9F`. The five unassigned slots
/// (`0x81`, `0x8D`, `0x8F`, `0x90`, `0x9D`) are `None`.
static HIGH_80_9F: [Option<&str>; 32] = [
    Some("\u{20AC}"), // 0x80 €
    None,             // 0x81
    Some("\u{201A}"), // 0x82 ‚
    Some("\u{0192}"), // 0x83 ƒ
    Some("\u{201E}"), // 0x84 „
    Some("\u{2026}"), // 0x85 …
    Some("\u{2020}"), // 0x86 †
    Some("\u{2021}"), // 0x87 ‡
    Some("\u{02C6}"), // 0x88 ˆ
    Some("\u{2030}"), // 0x89 ‰
    Some("\u{0160}"), // 0x8A Š
    Some("\u{2039}"), // 0x8B ‹
    Some("\u{0152}"), // 0x8C Œ
    None,             // 0x8D
    Some("\u{017D}"), // 0x8E Ž
    None,             // 0x8F
    None,             // 0x90
    Some("\u{2018}"), // 0x91 ‘
    Some("\u{2019}"), // 0x92 ’
    Some("\u{201C}"), // 0x93 “
    Some("\u{201D}"), // 0x94 ”
    Some("\u{2022}"), // 0x95 •
    Some("\u{2013}"), // 0x96 –
    Some("\u{2014}"), // 0x97 —
    Some("\u{02DC}"), // 0x98 ˜
    Some("\u{2122}"), // 0x99 ™
    Some("\u{0161}"), // 0x9A š
    Some("\u{203A}"), // 0x9B ›
    Some("\u{0153}"), // 0x9C œ
    None,             // 0x9D
    Some("\u{017E}"), // 0x9E ž
    Some("\u{0178}"), // 0x9F Ÿ
];

/// Map a single byte through WinAnsiEncoding. Returns `None` for codes with no
/// printable glyph (control range and the unassigned CP1252 slots).
pub(crate) fn winansi(b: u8) -> Option<&'static str> {
    match b {
        // Printable ASCII (0x20 SPACE through 0x7E TILDE) is identity.
        0x20..=0x7E => Some(ascii_str(b)),
        // CP1252 high range.
        0x80..=0x9F => HIGH_80_9F[(b - 0x80) as usize],
        // 0xA0..=0xFF is Latin-1: the Unicode scalar equals the byte value.
        0xA0..=0xFF => Some(latin1_str(b)),
        // Control range (0x00..=0x1F) and 0x7F have no text glyph.
        _ => None,
    }
}

/// Mac OS Roman high range `0x80..=0xFF` (every slot is defined). Values follow
/// the Unicode-canonical mapping of Apple's ROMAN.TXT (post-euro): `0xDB` is the
/// euro sign, `0xBD` is GREEK CAPITAL OMEGA, `0xF0` is the Apple-logo PUA char.
/// The two f-ligatures (`0xDE`/`0xDF`) are decomposed to ASCII for searchability.
static MACROMAN_HIGH: [&str; 128] = [
    "\u{00C4}", // 0x80 Ä
    "\u{00C5}", // 0x81 Å
    "\u{00C7}", // 0x82 Ç
    "\u{00C9}", // 0x83 É
    "\u{00D1}", // 0x84 Ñ
    "\u{00D6}", // 0x85 Ö
    "\u{00DC}", // 0x86 Ü
    "\u{00E1}", // 0x87 á
    "\u{00E0}", // 0x88 à
    "\u{00E2}", // 0x89 â
    "\u{00E4}", // 0x8A ä
    "\u{00E3}", // 0x8B ã
    "\u{00E5}", // 0x8C å
    "\u{00E7}", // 0x8D ç
    "\u{00E9}", // 0x8E é
    "\u{00E8}", // 0x8F è
    "\u{00EA}", // 0x90 ê
    "\u{00EB}", // 0x91 ë
    "\u{00ED}", // 0x92 í
    "\u{00EC}", // 0x93 ì
    "\u{00EE}", // 0x94 î
    "\u{00EF}", // 0x95 ï
    "\u{00F1}", // 0x96 ñ
    "\u{00F3}", // 0x97 ó
    "\u{00F2}", // 0x98 ò
    "\u{00F4}", // 0x99 ô
    "\u{00F6}", // 0x9A ö
    "\u{00F5}", // 0x9B õ
    "\u{00FA}", // 0x9C ú
    "\u{00F9}", // 0x9D ù
    "\u{00FB}", // 0x9E û
    "\u{00FC}", // 0x9F ü
    "\u{2020}", // 0xA0 †
    "\u{00B0}", // 0xA1 °
    "\u{00A2}", // 0xA2 ¢
    "\u{00A3}", // 0xA3 £
    "\u{00A7}", // 0xA4 §
    "\u{2022}", // 0xA5 •
    "\u{00B6}", // 0xA6 ¶
    "\u{00DF}", // 0xA7 ß
    "\u{00AE}", // 0xA8 ®
    "\u{00A9}", // 0xA9 ©
    "\u{2122}", // 0xAA ™
    "\u{00B4}", // 0xAB ´
    "\u{00A8}", // 0xAC ¨
    "\u{2260}", // 0xAD ≠
    "\u{00C6}", // 0xAE Æ
    "\u{00D8}", // 0xAF Ø
    "\u{221E}", // 0xB0 ∞
    "\u{00B1}", // 0xB1 ±
    "\u{2264}", // 0xB2 ≤
    "\u{2265}", // 0xB3 ≥
    "\u{00A5}", // 0xB4 ¥
    "\u{00B5}", // 0xB5 µ
    "\u{2202}", // 0xB6 ∂
    "\u{2211}", // 0xB7 ∑
    "\u{220F}", // 0xB8 ∏
    "\u{03C0}", // 0xB9 π
    "\u{222B}", // 0xBA ∫
    "\u{00AA}", // 0xBB ª
    "\u{00BA}", // 0xBC º
    "\u{03A9}", // 0xBD Ω (GREEK CAPITAL OMEGA)
    "\u{00E6}", // 0xBE æ
    "\u{00F8}", // 0xBF ø
    "\u{00BF}", // 0xC0 ¿
    "\u{00A1}", // 0xC1 ¡
    "\u{00AC}", // 0xC2 ¬
    "\u{221A}", // 0xC3 √
    "\u{0192}", // 0xC4 ƒ
    "\u{2248}", // 0xC5 ≈
    "\u{2206}", // 0xC6 ∆
    "\u{00AB}", // 0xC7 «
    "\u{00BB}", // 0xC8 »
    "\u{2026}", // 0xC9 …
    "\u{00A0}", // 0xCA  (no-break space)
    "\u{00C0}", // 0xCB À
    "\u{00C3}", // 0xCC Ã
    "\u{00D5}", // 0xCD Õ
    "\u{0152}", // 0xCE Œ
    "\u{0153}", // 0xCF œ
    "\u{2013}", // 0xD0 – en dash
    "\u{2014}", // 0xD1 — em dash
    "\u{201C}", // 0xD2 “ left double quote
    "\u{201D}", // 0xD3 ” right double quote
    "\u{2018}", // 0xD4 ‘ left single quote
    "\u{2019}", // 0xD5 ’ right single quote (apostrophe)
    "\u{00F7}", // 0xD6 ÷
    "\u{25CA}", // 0xD7 ◊
    "\u{00FF}", // 0xD8 ÿ
    "\u{0178}", // 0xD9 Ÿ
    "\u{2044}", // 0xDA ⁄ fraction slash
    "\u{20AC}", // 0xDB € euro
    "\u{2039}", // 0xDC ‹
    "\u{203A}", // 0xDD ›
    "fi",       // 0xDE ﬁ ligature (decomposed)
    "fl",       // 0xDF ﬂ ligature (decomposed)
    "\u{2021}", // 0xE0 ‡ double dagger
    "\u{00B7}", // 0xE1 · middle dot
    "\u{201A}", // 0xE2 ‚ single low quote
    "\u{201E}", // 0xE3 „ double low quote
    "\u{2030}", // 0xE4 ‰ per mille
    "\u{00C2}", // 0xE5 Â
    "\u{00CA}", // 0xE6 Ê
    "\u{00C1}", // 0xE7 Á
    "\u{00CB}", // 0xE8 Ë
    "\u{00C8}", // 0xE9 È
    "\u{00CD}", // 0xEA Í
    "\u{00CE}", // 0xEB Î
    "\u{00CF}", // 0xEC Ï
    "\u{00CC}", // 0xED Ì
    "\u{00D3}", // 0xEE Ó
    "\u{00D4}", // 0xEF Ô
    "\u{F8FF}", // 0xF0  Apple logo (private use)
    "\u{00D2}", // 0xF1 Ò
    "\u{00DA}", // 0xF2 Ú
    "\u{00DB}", // 0xF3 Û
    "\u{00D9}", // 0xF4 Ù
    "\u{0131}", // 0xF5 ı dotless i
    "\u{02C6}", // 0xF6 ˆ circumflex
    "\u{02DC}", // 0xF7 ˜ small tilde
    "\u{00AF}", // 0xF8 ¯ macron
    "\u{02D8}", // 0xF9 ˘ breve
    "\u{02D9}", // 0xFA ˙ dot above
    "\u{02DA}", // 0xFB ˚ ring above
    "\u{00B8}", // 0xFC ¸ cedilla
    "\u{02DD}", // 0xFD ˝ double acute
    "\u{02DB}", // 0xFE ˛ ogonek
    "\u{02C7}", // 0xFF ˇ caron
];

/// Map a single byte through MacRomanEncoding (Mac OS Roman). Returns `None`
/// only for the control range and `0x7F` (DEL), which have no text glyph; every
/// `0x80..=0xFF` slot is defined.
pub(crate) fn macroman(b: u8) -> Option<&'static str> {
    match b {
        // Printable ASCII (0x20 SPACE through 0x7E TILDE) is identity.
        0x20..=0x7E => Some(ascii_str(b)),
        0x80..=0xFF => Some(MACROMAN_HIGH[(b - 0x80) as usize]),
        // Control range (0x00..=0x1F) and 0x7F have no text glyph.
        _ => None,
    }
}

/// Adobe StandardEncoding high range `0xA0..=0xFF`, indexed by `byte - 0xA0`.
/// The range is sparse — many codes are unassigned (`None`). Values follow the
/// Adobe Glyph List mapping of each glyph name (e.g. `florin` → U+0192,
/// `fraction` → U+2044); the two f-ligatures (`0xAE`/`0xAF`) are decomposed to
/// ASCII for searchability.
static STANDARD_HIGH: [Option<&str>; 96] = [
    None,             // 0xA0
    Some("\u{00A1}"), // 0xA1 ¡ exclamdown
    Some("\u{00A2}"), // 0xA2 ¢ cent
    Some("\u{00A3}"), // 0xA3 £ sterling
    Some("\u{2044}"), // 0xA4 ⁄ fraction
    Some("\u{00A5}"), // 0xA5 ¥ yen
    Some("\u{0192}"), // 0xA6 ƒ florin
    Some("\u{00A7}"), // 0xA7 § section
    Some("\u{00A4}"), // 0xA8 ¤ currency
    Some("\u{0027}"), // 0xA9 ' quotesingle
    Some("\u{201C}"), // 0xAA “ quotedblleft
    Some("\u{00AB}"), // 0xAB « guillemotleft
    Some("\u{2039}"), // 0xAC ‹ guilsinglleft
    Some("\u{203A}"), // 0xAD › guilsinglright
    Some("fi"),       // 0xAE ﬁ fi ligature (decomposed)
    Some("fl"),       // 0xAF ﬂ fl ligature (decomposed)
    None,             // 0xB0
    Some("\u{2013}"), // 0xB1 – endash
    Some("\u{2020}"), // 0xB2 † dagger
    Some("\u{2021}"), // 0xB3 ‡ daggerdbl
    Some("\u{00B7}"), // 0xB4 · periodcentered
    None,             // 0xB5
    Some("\u{00B6}"), // 0xB6 ¶ paragraph
    Some("\u{2022}"), // 0xB7 • bullet
    Some("\u{201A}"), // 0xB8 ‚ quotesinglbase
    Some("\u{201E}"), // 0xB9 „ quotedblbase
    Some("\u{201D}"), // 0xBA ” quotedblright
    Some("\u{00BB}"), // 0xBB » guillemotright
    Some("\u{2026}"), // 0xBC … ellipsis
    Some("\u{2030}"), // 0xBD ‰ perthousand
    None,             // 0xBE
    Some("\u{00BF}"), // 0xBF ¿ questiondown
    None,             // 0xC0
    Some("\u{0060}"), // 0xC1 ` grave
    Some("\u{00B4}"), // 0xC2 ´ acute
    Some("\u{02C6}"), // 0xC3 ˆ circumflex
    Some("\u{02DC}"), // 0xC4 ˜ tilde
    Some("\u{00AF}"), // 0xC5 ¯ macron
    Some("\u{02D8}"), // 0xC6 ˘ breve
    Some("\u{02D9}"), // 0xC7 ˙ dotaccent
    Some("\u{00A8}"), // 0xC8 ¨ dieresis
    None,             // 0xC9
    Some("\u{02DA}"), // 0xCA ˚ ring
    Some("\u{00B8}"), // 0xCB ¸ cedilla
    None,             // 0xCC
    Some("\u{02DD}"), // 0xCD ˝ hungarumlaut
    Some("\u{02DB}"), // 0xCE ˛ ogonek
    Some("\u{02C7}"), // 0xCF ˇ caron
    Some("\u{2014}"), // 0xD0 — emdash
    None,             // 0xD1
    None,             // 0xD2
    None,             // 0xD3
    None,             // 0xD4
    None,             // 0xD5
    None,             // 0xD6
    None,             // 0xD7
    None,             // 0xD8
    None,             // 0xD9
    None,             // 0xDA
    None,             // 0xDB
    None,             // 0xDC
    None,             // 0xDD
    None,             // 0xDE
    None,             // 0xDF
    None,             // 0xE0
    Some("\u{00C6}"), // 0xE1 Æ AE
    None,             // 0xE2
    Some("\u{00AA}"), // 0xE3 ª ordfeminine
    None,             // 0xE4
    None,             // 0xE5
    None,             // 0xE6
    None,             // 0xE7
    Some("\u{0141}"), // 0xE8 Ł Lslash
    Some("\u{00D8}"), // 0xE9 Ø Oslash
    Some("\u{0152}"), // 0xEA Œ OE
    Some("\u{00BA}"), // 0xEB º ordmasculine
    None,             // 0xEC
    None,             // 0xED
    None,             // 0xEE
    None,             // 0xEF
    None,             // 0xF0
    Some("\u{00E6}"), // 0xF1 æ ae
    None,             // 0xF2
    None,             // 0xF3
    None,             // 0xF4
    Some("\u{0131}"), // 0xF5 ı dotlessi
    None,             // 0xF6
    None,             // 0xF7
    Some("\u{0142}"), // 0xF8 ł lslash
    Some("\u{00F8}"), // 0xF9 ø oslash
    Some("\u{0153}"), // 0xFA œ oe
    Some("\u{00DF}"), // 0xFB ß germandbls
    None,             // 0xFC
    None,             // 0xFD
    None,             // 0xFE
    None,             // 0xFF
];

/// Map a single byte through Adobe StandardEncoding. Returns `None` for codes
/// with no glyph (the control range, `0x7F`, `0x80..=0xA0`, and the unassigned
/// high-range slots). Note the two ASCII-range departures from US-ASCII:
/// `0x27` is the right single quote `’` and `0x60` the left single quote `‘`.
pub(crate) fn standard(b: u8) -> Option<&'static str> {
    match b {
        0x27 => Some("\u{2019}"), // quoteright ’ (not the ASCII apostrophe)
        0x60 => Some("\u{2018}"), // quoteleft ‘ (not the ASCII grave)
        // Remaining printable ASCII (0x20 SPACE through 0x7E TILDE) is identity.
        0x20..=0x7E => Some(ascii_str(b)),
        0xA0..=0xFF => STANDARD_HIGH[(b - 0xA0) as usize],
        // Control range (0x00..=0x1F), 0x7F, and 0x80..=0x9F have no text glyph.
        _ => None,
    }
}

/// Map a single byte through MacExpertEncoding. The expert set is dominated by
/// stylistic variants (small caps, old-style figures, superior/inferior forms)
/// with no dedicated Unicode codepoint, so each is folded to its semantic base
/// character; forms that *do* have an exact Unicode codepoint (fractions, digit
/// super/subscripts, parenthesis super/subscripts, leaders) use it, and the
/// f-ligatures (and `rupiah`) expand to their multi-character ASCII equivalents.
/// This recovers readable text but does not preserve the styling. Returns `None`
/// for unassigned codes.
pub(crate) fn macexpert(b: u8) -> Option<&'static str> {
    Some(match b {
        // -- Punctuation, leaders, dashes, fraction bar --
        0x20 => " ",
        0x2A => "\u{2025}", // twodotenleader ‥
        0x2B => "\u{2024}", // onedotenleader ․
        0x2C => ",",        // comma
        0x2D => "-",        // hyphen
        0x2E => ".",        // period
        0x2F => "\u{2044}", // fraction ⁄ (fraction bar)
        0x3A => ":",        // colon
        0x3B => ";",        // semicolon
        0x3D => "\u{2014}", // threequartersemdash (→ em dash)
        0xD0 => "\u{2012}", // figuredash ‒
        0xD1 => "-",        // hyphensuperior (→ hyphen)
        0x61 => "-",        // hypheninferior (→ hyphen)
        // -- Small-cap punctuation (fold to base) --
        0x21 => "!",        // exclamsmall
        0x3F => "?",        // questionsmall
        0x26 => "&",        // ampersandsmall
        0xD6 => "\u{00A1}", // exclamdownsmall ¡
        0xC3 => "\u{00BF}", // questiondownsmall ¿
        // -- Currency (old-style / superior / inferior fold to the sign) --
        0x23 => "\u{00A2}", // centoldstyle ¢
        0x84 => "\u{00A2}", // centsuperior ¢
        0xAC => "\u{00A2}", // centinferior ¢
        0x24 => "$",        // dollaroldstyle
        0x25 => "$",        // dollarsuperior
        0xB9 => "$",        // dollarinferior
        0x7D => "\u{20A1}", // colonmonetary ₡
        0x7F => "Rp",       // rupiah (expanded to its textual form)
        // -- Old-style figures (fold to ASCII digits) --
        0x30..=0x39 => ascii_str(b), // zerooldstyle..nineoldstyle
        0x7E => "1",                 // onefitted (figure-width one)
        // -- Fractions --
        0x49 => "\u{00BC}", // onequarter ¼
        0x4A => "\u{00BD}", // onehalf ½
        0x4B => "\u{00BE}", // threequarters ¾
        0x4C => "\u{215B}", // oneeighth ⅛
        0x4D => "\u{215C}", // threeeighths ⅜
        0x4E => "\u{215D}", // fiveeighths ⅝
        0x4F => "\u{215E}", // seveneighths ⅞
        0x50 => "\u{2153}", // onethird ⅓
        0x51 => "\u{2154}", // twothirds ⅔
        // -- f-ligatures (decomposed to ASCII) --
        0x58 => "ff",  // ff
        0x59 => "fi",  // fi
        0x5A => "fl",  // fl
        0x5B => "ffi", // ffi
        0x5C => "ffl", // ffl
        // -- Parenthesis super/subscripts (exact Unicode) --
        0x28 => "\u{207D}", // parenleftsuperior ⁽
        0x29 => "\u{207E}", // parenrightsuperior ⁾
        0x5D => "\u{208D}", // parenleftinferior ₍
        0x5F => "\u{208E}", // parenrightinferior ₎
        // -- Superior digits (exact Unicode superscripts) --
        0xE2 => "\u{2070}", // zerosuperior ⁰
        0xDA => "\u{00B9}", // onesuperior ¹
        0xDB => "\u{00B2}", // twosuperior ²
        0xDC => "\u{00B3}", // threesuperior ³
        0xDD => "\u{2074}", // foursuperior ⁴
        0xDE => "\u{2075}", // fivesuperior ⁵
        0xDF => "\u{2076}", // sixsuperior ⁶
        0xE0 => "\u{2077}", // sevensuperior ⁷
        0xA4 => "\u{2078}", // eightsuperior ⁸
        0xE1 => "\u{2079}", // ninesuperior ⁹
        // -- Inferior digits (exact Unicode subscripts) --
        0xBF => "\u{2080}", // zeroinferior ₀
        0xC4 => "\u{2081}", // oneinferior ₁
        0xAD => "\u{2082}", // twoinferior ₂
        0xA6 => "\u{2083}", // threeinferior ₃
        0xA5 => "\u{2084}", // fourinferior ₄
        0xB3 => "\u{2085}", // fiveinferior ₅
        0xA7 => "\u{2086}", // sixinferior ₆
        0xA9 => "\u{2087}", // seveninferior ₇
        0xA8 => "\u{2088}", // eightinferior ₈
        0xBE => "\u{2089}", // nineinferior ₉
        // -- Superior letters/punct (fold to base char) --
        0x83 => "a", // asuperior
        0xF3 => "b", // bsuperior
        0xEA => "d", // dsuperior
        0xE4 => "e", // esuperior
        0xE8 => "i", // isuperior
        0xEF => "l", // lsuperior
        0xF5 => "m", // msuperior
        0xF4 => "n", // nsuperior
        0xB2 => "o", // osuperior
        0xE5 => "r", // rsuperior
        0xE9 => "s", // ssuperior
        0xE6 => "t", // tsuperior
        0xF6 => ",", // commasuperior
        0xF7 => ".", // periodsuperior
        // -- Inferior punct (fold to base char) --
        0xB5 => ",", // commainferior
        0xB6 => ".", // periodinferior
        // -- Small accents (spacing diacritics) --
        0x27 => "\u{00B4}", // Acutesmall ´
        0x62 => "`",        // Gravesmall
        0x80 => "\u{02DC}", // Tildesmall ˜
        0x60 => "\u{02C6}", // Circumflexsmall ˆ
        0xB1 => "\u{02C7}", // Caronsmall ˇ
        0xF1 => "\u{02D8}", // Brevesmall ˘
        0xF2 => "\u{00AF}", // Macronsmall ¯
        0xF8 => "\u{02D9}", // Dotaccentsmall ˙
        0xF9 => "\u{02DA}", // Ringsmall ˚
        0xCB => "\u{00B8}", // Cedillasmall ¸
        0xF0 => "\u{02DB}", // Ogoneksmall ˛
        0xAF => "\u{00A8}", // Dieresissmall ¨
        0x22 => "\u{02DD}", // Hungarumlautsmall ˝
        // -- Plain small caps (fold to base uppercase A..Z) --
        0x63..=0x7C => ascii_str(b - 0x63 + b'A'), // Asmall..Zsmall
        // -- Accented / special small caps (fold to base uppercase) --
        0x45 => "\u{00D0}", // Ethsmall Ð
        0x8A => "\u{00C1}", // Aacutesmall Á
        0x8B => "\u{00C0}", // Agravesmall À
        0x8C => "\u{00C2}", // Acircumflexsmall Â
        0x8D => "\u{00C4}", // Adieresissmall Ä
        0x8E => "\u{00C3}", // Atildesmall Ã
        0x8F => "\u{00C5}", // Aringsmall Å
        0x90 => "\u{00C7}", // Ccedillasmall Ç
        0x91 => "\u{00C9}", // Eacutesmall É
        0x92 => "\u{00C8}", // Egravesmall È
        0x93 => "\u{00CA}", // Ecircumflexsmall Ê
        0x94 => "\u{00CB}", // Edieresissmall Ë
        0x95 => "\u{00CD}", // Iacutesmall Í
        0x96 => "\u{00CC}", // Igravesmall Ì
        0x97 => "\u{00CE}", // Icircumflexsmall Î
        0x98 => "\u{00CF}", // Idieresissmall Ï
        0x99 => "\u{00D1}", // Ntildesmall Ñ
        0x9A => "\u{00D3}", // Oacutesmall Ó
        0x9B => "\u{00D2}", // Ogravesmall Ò
        0x9C => "\u{00D4}", // Ocircumflexsmall Ô
        0x9D => "\u{00D6}", // Odieresissmall Ö
        0x9E => "\u{00D5}", // Otildesmall Õ
        0x9F => "\u{00DA}", // Uacutesmall Ú
        0xA0 => "\u{00D9}", // Ugravesmall Ù
        0xA1 => "\u{00DB}", // Ucircumflexsmall Û
        0xA2 => "\u{00DC}", // Udieresissmall Ü
        0xAA => "\u{0160}", // Scaronsmall Š
        0xB7 => "\u{00DD}", // Yacutesmall Ý
        0xD8 => "\u{0178}", // Ydieresissmall Ÿ
        0xC0 => "\u{017D}", // Zcaronsmall Ž
        0xC1 => "\u{00C6}", // AEsmall Æ
        0xC2 => "\u{00D8}", // Oslashsmall Ø
        0xC5 => "\u{0141}", // Lslashsmall Ł
        0xCF => "\u{0152}", // OEsmall Œ
        0xBC => "\u{00DE}", // Thornsmall Þ
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_identity() {
        assert_eq!(winansi(b'A'), Some("A"));
        assert_eq!(winansi(b' '), Some(" "));
        assert_eq!(winansi(b'~'), Some("~"));
    }

    #[test]
    fn curly_quotes() {
        assert_eq!(winansi(0x91), Some("\u{2018}")); // ‘
        assert_eq!(winansi(0x92), Some("\u{2019}")); // ’
        assert_eq!(winansi(0x93), Some("\u{201C}")); // “
        assert_eq!(winansi(0x94), Some("\u{201D}")); // ”
    }

    #[test]
    fn dashes() {
        assert_eq!(winansi(0x96), Some("\u{2013}")); // – en dash
        assert_eq!(winansi(0x97), Some("\u{2014}")); // — em dash
    }

    #[test]
    fn euro_and_trademark() {
        assert_eq!(winansi(0x80), Some("\u{20AC}")); // €
        assert_eq!(winansi(0x99), Some("\u{2122}")); // ™
    }

    #[test]
    fn latin1_high_range() {
        assert_eq!(winansi(0xE9), Some("é"));
        assert_eq!(winansi(0xA0), Some("\u{00A0}")); // nbsp
        assert_eq!(winansi(0xFF), Some("ÿ"));
    }

    #[test]
    fn winansi_latin1_matches_scalar() {
        // The packed-slice Latin-1 path must equal `b as char` for every code.
        for b in 0xA0u8..=0xFF {
            let s = winansi(b).expect("Latin-1 range is fully defined");
            assert_eq!(s.chars().count(), 1, "0x{:02X} should be one char", b);
            assert_eq!(s.chars().next().unwrap(), b as char, "0x{:02X}", b);
        }
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

    #[test]
    fn macroman_ascii_is_identity() {
        assert_eq!(macroman(b'A'), Some("A"));
        assert_eq!(macroman(b' '), Some(" "));
        assert_eq!(macroman(b'~'), Some("~"));
    }

    #[test]
    fn macroman_apostrophe_and_quotes() {
        // The high-value case: macOS apostrophe/quotes that passthrough mangles.
        assert_eq!(macroman(0xD5), Some("\u{2019}")); // ’ right single quote
        assert_eq!(macroman(0xD4), Some("\u{2018}")); // ‘ left single quote
        assert_eq!(macroman(0xD2), Some("\u{201C}")); // “ left double quote
        assert_eq!(macroman(0xD3), Some("\u{201D}")); // ” right double quote
    }

    #[test]
    fn macroman_dashes_and_ellipsis() {
        assert_eq!(macroman(0xD0), Some("\u{2013}")); // – en dash
        assert_eq!(macroman(0xD1), Some("\u{2014}")); // — em dash
        assert_eq!(macroman(0xC9), Some("\u{2026}")); // …
    }

    #[test]
    fn macroman_accented_letters() {
        assert_eq!(macroman(0x8E), Some("é"));
        assert_eq!(macroman(0x80), Some("Ä"));
        assert_eq!(macroman(0x96), Some("ñ"));
    }

    #[test]
    fn macroman_special_symbols() {
        assert_eq!(macroman(0xA5), Some("\u{2022}")); // • bullet
        assert_eq!(macroman(0xCA), Some("\u{00A0}")); // nbsp
        assert_eq!(macroman(0xDB), Some("\u{20AC}")); // € euro
    }

    #[test]
    fn macroman_ligatures_decompose_to_ascii() {
        assert_eq!(macroman(0xDE), Some("fi")); // ﬁ → fi
        assert_eq!(macroman(0xDF), Some("fl")); // ﬂ → fl
    }

    #[test]
    fn macroman_control_range_is_none() {
        assert_eq!(macroman(0x00), None);
        assert_eq!(macroman(0x1F), None);
        assert_eq!(macroman(0x7F), None);
    }

    #[test]
    fn macroman_high_range_all_defined() {
        // Every 0x80..=0xFF slot maps to a glyph in Mac OS Roman.
        for b in 0x80u8..=0xFF {
            assert!(macroman(b).is_some(), "0x{:02X} should be defined", b);
        }
    }

    #[test]
    fn standard_ascii_is_mostly_identity() {
        assert_eq!(standard(b'A'), Some("A"));
        assert_eq!(standard(b' '), Some(" "));
        assert_eq!(standard(b'~'), Some("~"));
        assert_eq!(standard(b'1'), Some("1"));
    }

    #[test]
    fn standard_quote_departures_from_ascii() {
        // The headline difference: 0x27 and 0x60 are curly quotes, not ASCII.
        assert_eq!(standard(0x27), Some("\u{2019}")); // ’ quoteright
        assert_eq!(standard(0x60), Some("\u{2018}")); // ‘ quoteleft
    }

    #[test]
    fn standard_high_range_glyphs() {
        assert_eq!(standard(0xA6), Some("\u{0192}")); // ƒ florin
        assert_eq!(standard(0xA4), Some("\u{2044}")); // ⁄ fraction
        assert_eq!(standard(0xAE), Some("fi")); // ﬁ ligature → fi
        assert_eq!(standard(0xAF), Some("fl")); // ﬂ ligature → fl
        assert_eq!(standard(0xB1), Some("\u{2013}")); // – endash
        assert_eq!(standard(0xD0), Some("\u{2014}")); // — emdash
        assert_eq!(standard(0xB7), Some("\u{2022}")); // • bullet
        assert_eq!(standard(0xBC), Some("\u{2026}")); // … ellipsis
    }

    #[test]
    fn standard_high_range_letters() {
        assert_eq!(standard(0xE1), Some("\u{00C6}")); // Æ AE
        assert_eq!(standard(0xEA), Some("\u{0152}")); // Œ OE
        assert_eq!(standard(0xFA), Some("\u{0153}")); // œ oe
        assert_eq!(standard(0xFB), Some("\u{00DF}")); // ß germandbls
        assert_eq!(standard(0xF8), Some("\u{0142}")); // ł lslash
    }

    #[test]
    fn standard_unassigned_and_control_are_none() {
        // Sparse high-range gaps.
        assert_eq!(standard(0xA0), None);
        assert_eq!(standard(0xB0), None);
        assert_eq!(standard(0xC0), None);
        assert_eq!(standard(0xD1), None);
        assert_eq!(standard(0xFF), None);
        // 0x80..=0x9F is entirely unassigned in StandardEncoding.
        assert_eq!(standard(0x80), None);
        assert_eq!(standard(0x9F), None);
        // Control range and DEL.
        assert_eq!(standard(0x00), None);
        assert_eq!(standard(0x1F), None);
        assert_eq!(standard(0x7F), None);
    }

    #[test]
    fn macexpert_small_caps_fold_to_base_letters() {
        assert_eq!(macexpert(0x63), Some("A")); // Asmall
        assert_eq!(macexpert(0x7C), Some("Z")); // Zsmall
        assert_eq!(macexpert(0x6D), Some("K")); // Ksmall
        // Accented small caps fold to accented uppercase.
        assert_eq!(macexpert(0x8A), Some("Á")); // Aacutesmall
        assert_eq!(macexpert(0x99), Some("Ñ")); // Ntildesmall
        assert_eq!(macexpert(0xC1), Some("Æ")); // AEsmall
    }

    #[test]
    fn macexpert_oldstyle_figures_fold_to_digits() {
        assert_eq!(macexpert(0x30), Some("0")); // zerooldstyle
        assert_eq!(macexpert(0x33), Some("3")); // threeoldstyle
        assert_eq!(macexpert(0x39), Some("9")); // nineoldstyle
    }

    #[test]
    fn macexpert_ligatures_and_rupiah_expand_multi_char() {
        assert_eq!(macexpert(0x58), Some("ff"));
        assert_eq!(macexpert(0x59), Some("fi"));
        assert_eq!(macexpert(0x5B), Some("ffi"));
        assert_eq!(macexpert(0x5C), Some("ffl"));
        assert_eq!(macexpert(0x7F), Some("Rp")); // rupiah
    }

    #[test]
    fn macexpert_exact_unicode_forms() {
        // Fractions.
        assert_eq!(macexpert(0x4A), Some("\u{00BD}")); // onehalf ½
        assert_eq!(macexpert(0x4C), Some("\u{215B}")); // oneeighth ⅛
        // Digit super/subscripts.
        assert_eq!(macexpert(0xDB), Some("\u{00B2}")); // twosuperior ²
        assert_eq!(macexpert(0xE2), Some("\u{2070}")); // zerosuperior ⁰
        assert_eq!(macexpert(0xBF), Some("\u{2080}")); // zeroinferior ₀
        assert_eq!(macexpert(0xBE), Some("\u{2089}")); // nineinferior ₉
        // Fraction bar.
        assert_eq!(macexpert(0x2F), Some("\u{2044}"));
    }

    #[test]
    fn macexpert_superior_inferior_letters_and_punct_fold_to_base() {
        assert_eq!(macexpert(0x83), Some("a")); // asuperior
        assert_eq!(macexpert(0xF4), Some("n")); // nsuperior
        assert_eq!(macexpert(0xF6), Some(",")); // commasuperior
        assert_eq!(macexpert(0xB6), Some(".")); // periodinferior
    }

    #[test]
    fn macexpert_small_punctuation_and_currency_fold() {
        assert_eq!(macexpert(0x21), Some("!")); // exclamsmall
        assert_eq!(macexpert(0xC3), Some("¿")); // questiondownsmall
        assert_eq!(macexpert(0x23), Some("¢")); // centoldstyle
        assert_eq!(macexpert(0x24), Some("$")); // dollaroldstyle
        assert_eq!(macexpert(0x7D), Some("\u{20A1}")); // colonmonetary ₡
    }

    #[test]
    fn macexpert_unassigned_are_none() {
        assert_eq!(macexpert(0x00), None); // control
        assert_eq!(macexpert(0x40), None); // unassigned in the table
        assert_eq!(macexpert(0xFF), None); // unassigned
        assert_eq!(macexpert(0x5E), None); // gap between ffl and parenrightinferior
    }
}
