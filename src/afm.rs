//! Built-in glyph widths for the Standard-14 fonts (plan-0004, implemented in v0.35.0).
//!
//! A Standard-14 font (Courier, Helvetica and Times in their four styles, Symbol,
//! ZapfDingbats) may legally omit `/Widths`: the reader is expected to know its
//! metrics (PDF 32000-1 §9.6.2.2).  These tables are those metrics, from Adobe’s
//! Core 14 AFM files, reduced to each glyph’s code, width and name
//! (`core14.txt`, which carries Adobe’s notice).
//!
//! An AFM keys widths by glyph name, so a character code reaches its width
//! through the font’s effective encoding: a named base encoding (WinAnsi or
//! MacRoman, from `encoding_names.txt`; StandardEncoding, which is the builtin
//! encoding of the twelve text fonts) or the font’s builtin one, with
//! `/Differences` on top.  `metrics.rs` does that walk; this module only holds
//! the tables.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The reduced Core 14 AFM data, embedded at build time.
const CORE14_TXT: &str = include_str!("core14.txt");

/// WinAnsiEncoding and MacRomanEncoding as glyph names, embedded at build time.
const ENCODING_NAMES_TXT: &str = include_str!("encoding_names.txt");

/// A code → glyph-name table for the 256 single-byte codes.
pub(crate) type NameTable = [Option<&'static str>; 256];

/// One Standard-14 font’s metrics.
pub(crate) struct Core14Font {
    /// The font’s builtin encoding, from the AFM’s character codes.
    pub builtin: NameTable,
    /// Glyph name → advance width in glyph space (1/1000 em).
    widths: HashMap<&'static str, u16>,
}

impl Core14Font {
    /// The advance of the glyph named `name`, in 1/1000 em, or `None` when the
    /// font has no such glyph.
    pub(crate) fn width(&self, name: &str) -> Option<u16> {
        self.widths.get(name).copied()
    }
}

fn fonts() -> &'static HashMap<&'static str, Core14Font> {
    static FONTS: OnceLock<HashMap<&'static str, Core14Font>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = HashMap::new();
        let mut current: Option<(&'static str, Core14Font)> = None;
        for line in CORE14_TXT.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix("FontName ") {
                fonts.extend(current.take());
                current = Some((
                    name,
                    Core14Font {
                        builtin: [None; 256],
                        widths: HashMap::new(),
                    },
                ));
                continue;
            }
            let mut fields = line.split(' ');
            let (Some(code), Some(width), Some(glyph), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                panic!("core14.txt: malformed line {line:?}");
            };
            let code: i32 = code.parse().expect("core14.txt: code");
            let width: u16 = width.parse().expect("core14.txt: width");
            let (_, font) = current.as_mut().expect("core14.txt: glyph before FontName");
            if let Ok(code) = usize::try_from(code) {
                font.builtin[code] = Some(glyph);
            }
            font.widths.insert(glyph, width);
        }
        fonts.extend(current);
        fonts
    })
}

fn named_encodings() -> &'static HashMap<&'static str, NameTable> {
    static TABLES: OnceLock<HashMap<&'static str, NameTable>> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut tables = HashMap::new();
        let mut current: Option<(&'static str, NameTable)> = None;
        for line in ENCODING_NAMES_TXT.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix("Encoding ") {
                tables.extend(current.take());
                current = Some((name, [None; 256]));
                continue;
            }
            let (code, glyph) = line
                .split_once(' ')
                .expect("encoding_names.txt: malformed line");
            let code: usize = code.parse().expect("encoding_names.txt: code");
            let (_, table) = current
                .as_mut()
                .expect("encoding_names.txt: code before Encoding");
            table[code] = Some(glyph);
        }
        tables.extend(current);
        // StandardEncoding is the text fonts’ builtin (a test pins that all
        // twelve agree).
        tables.insert(
            "StandardEncoding",
            fonts()
                .get("Helvetica")
                .expect("core14.txt has Helvetica")
                .builtin,
        );
        tables
    })
}

/// The metrics of the Standard-14 font `base_font` names, after stripping a
/// subset prefix (`ABCDEF+`), or `None` for any other font.
pub(crate) fn core14(base_font: &str) -> Option<&'static Core14Font> {
    fonts().get(strip_subset_prefix(base_font))
}

/// A named base encoding as glyph names: `WinAnsiEncoding`,
/// `MacRomanEncoding` or `StandardEncoding`.
pub(crate) fn encoding(name: &str) -> Option<&'static NameTable> {
    named_encodings().get(name)
}

/// `ABCDEF+Helvetica` → `Helvetica`: a subset tag is six uppercase letters and
/// a plus sign (§9.6.4).
fn strip_subset_prefix(base_font: &str) -> &str {
    match base_font.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => base_font,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT_FONTS: [&str; 12] = [
        "Courier",
        "Courier-Bold",
        "Courier-BoldOblique",
        "Courier-Oblique",
        "Helvetica",
        "Helvetica-Bold",
        "Helvetica-BoldOblique",
        "Helvetica-Oblique",
        "Times-Roman",
        "Times-Bold",
        "Times-BoldItalic",
        "Times-Italic",
    ];

    #[test]
    fn all_fourteen_fonts_load_with_their_known_widths() {
        assert_eq!(fonts().len(), 14);
        let helvetica = core14("Helvetica").unwrap();
        assert_eq!(helvetica.width("space"), Some(278));
        assert_eq!(helvetica.width("a"), Some(556));
        assert_eq!(helvetica.width("m"), Some(833));
        assert_eq!(helvetica.width("W"), Some(944));
        assert_eq!(core14("Helvetica-Bold").unwrap().width("space"), Some(278));
        assert_eq!(core14("Courier").unwrap().width("W"), Some(600));
        assert_eq!(core14("Times-Roman").unwrap().width("space"), Some(250));
        assert_eq!(core14("Symbol").unwrap().width("Alpha"), Some(722));
        assert_eq!(core14("ZapfDingbats").unwrap().width("a1"), Some(974));
        assert_eq!(helvetica.width("Alpha"), None, "not a Helvetica glyph");
    }

    #[test]
    fn the_text_fonts_share_standard_encoding_as_their_builtin() {
        let standard = encoding("StandardEncoding").unwrap();
        for font in TEXT_FONTS {
            assert_eq!(&core14(font).unwrap().builtin, standard, "{font}");
        }
        assert_eq!(standard[0x27], Some("quoteright"));
        assert_eq!(standard[0x60], Some("quoteleft"));
        assert_eq!(standard[0xAE], Some("fi"));
        assert_eq!(standard[0x7F], None);
    }

    #[test]
    fn symbolic_fonts_have_builtins_of_their_own() {
        assert_eq!(core14("Symbol").unwrap().builtin[0x41], Some("Alpha"));
        assert_eq!(core14("ZapfDingbats").unwrap().builtin[0x21], Some("a1"));
    }

    #[test]
    fn subset_prefix_is_stripped_and_nothing_else_is() {
        assert!(core14("ABCDEF+Helvetica").is_some());
        assert!(core14("abcdef+Helvetica").is_none());
        assert!(core14("ABCDE+Helvetica").is_none());
        assert!(core14("Arial").is_none());
        assert!(core14("Helvetica-Narrow").is_none());
    }

    /// The name tables agree with the Unicode tables `--text` decodes through,
    /// except where Annex D’s notes give a code a glyph CP1252 leaves undefined
    /// or Unicode spells differently.
    #[test]
    fn name_tables_agree_with_the_unicode_tables() {
        let decompose = |s: String| s.replace('\u{FB01}', "fi").replace('\u{FB02}', "fl");
        type Decode = fn(u8) -> Option<&'static str>;
        let cases: [(&str, Decode, &[u8]); 3] = [
            (
                "WinAnsiEncoding",
                crate::encodings::winansi,
                // Unused codes are the bullet; 0xA0 is the space, 0xAD the hyphen.
                &[0x7F, 0x81, 0x8D, 0x8F, 0x90, 0x9D, 0xA0, 0xAD],
            ),
            (
                "MacRomanEncoding",
                crate::encodings::macroman,
                // 0xCA is the space (Unicode no-break space); 0xBD is Omega, which
                // the AGL reads as the Ohm sign; 0xF0 the Apple logo, which the AGL
                // does not name; 0xDB is Annex D’s currency, which `--text` reads as
                // Apple’s later euro.
                &[0xBD, 0xCA, 0xDB, 0xF0],
            ),
            ("StandardEncoding", crate::encodings::standard, &[]),
        ];
        for (name, unicode, exceptions) in cases {
            let names = encoding(name).unwrap();
            for code in 0..=255u8 {
                if exceptions.contains(&code) {
                    continue;
                }
                let via_name = names[code as usize]
                    .and_then(crate::glyphlist::glyph_name_to_string)
                    .map(decompose);
                let direct = unicode(code).map(str::to_string);
                assert_eq!(via_name, direct, "{name} code {code:#04X}");
            }
        }
    }
}
