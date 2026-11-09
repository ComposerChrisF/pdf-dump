//! Glyph advance widths for `--text --layout` (plan-0002, implemented in v0.26.0).
//!
//! A glyph’s position is only as good as the advances before it, so this module
//! answers one question per font: how far does the pen move for character code
//! `c`, in text-space units per unit of font size?  It never invents an answer.
//! A font whose widths cannot be determined reports `Unknown` with a reason, and
//! the layout engine turns that into a Degraded verdict, which exits 3.
//!
//! Sources, per PDF 32000-1:
//! - Simple fonts (§9.6.2): `/FirstChar` + `/Widths` in glyph space (1/1000 em),
//!   and `/FontDescriptor /MissingWidth` for codes outside the array (default 0).
//! - Type3 fonts (§9.6.5): `/Widths` in the font’s own glyph space, scaled to text
//!   space by `/FontMatrix`.
//! - Type0 fonts (§9.7.4.3): the descendant CIDFont’s `/W` and `/DW` (default 1000),
//!   indexed by CID.  Only the `Identity-H`/`Identity-V` encodings map code → CID
//!   without a CMap table, so any other `/Encoding` leaves the widths unknown.
//!
//! - Standard-14 fonts with no `/Widths` (§9.6.2.2): Adobe’s AFM widths
//!   (`afm.rs`), reached by glyph name through the font’s effective encoding.  A
//!   code whose glyph the AFM does not have is unknown.

use lopdf::{Document, Object};
use std::borrow::Cow;
use std::collections::HashMap;

use crate::afm;
use crate::helpers;

/// The widths source for one font.
pub(crate) struct FontMetrics {
    kind: MetricsKind,
    /// Byte width of one character code when no ToUnicode CMap says otherwise:
    /// 2 for a Type0 font (Identity-H/V are two-byte), else 1.
    pub code_bytes: u8,
    /// Vertical writing mode (`Identity-V`).  Such glyphs never go on the grid.
    pub vertical: bool,
}

enum MetricsKind {
    Simple {
        first_char: i64,
        /// One entry per `/Widths` element; `None` where the element is not a
        /// number (it is then unknown, never 0).
        widths: Vec<Option<f64>>,
        /// `/LastChar`, when stated.  A code inside `FirstChar..=LastChar` but past
        /// the end of a short `/Widths` array is unknown, not `MissingWidth`.
        last_char: Option<i64>,
        /// `/FontDescriptor /MissingWidth`, only when explicitly stated.  The spec
        /// default is 0, but viewers then use the font program’s own width, which
        /// pdf-dump cannot read — so an out-of-range code with no explicit value
        /// is unknown.
        missing: Option<f64>,
        /// Glyph space → text space: 0.001 for ordinary fonts, `/FontMatrix` a
        /// for Type3.
        scale: f64,
    },
    /// A Standard-14 font without `/Widths`: one entry per code, `None` where
    /// the code names no glyph the font’s AFM has.
    Afm(Box<[Option<f64>; 256]>),
    Cid {
        default: f64,
        widths: HashMap<u32, f64>,
        ranges: Vec<(u32, u32, f64)>,
    },
    Unknown(Cow<'static, str>),
}

impl FontMetrics {
    /// Advance for `code` in text-space units per unit font size (so a
    /// 500-unit glyph at 12 pt advances 0.5 × 12 = 6 pt before `Tz`), or why it
    /// cannot be known.  Never an invented number.
    pub(crate) fn advance(&self, code: u32) -> Result<f64, Cow<'_, str>> {
        match &self.kind {
            MetricsKind::Simple {
                first_char,
                widths,
                last_char,
                missing,
                scale,
            } => {
                let code = i64::from(code);
                let idx = code - first_char;
                let in_declared_range =
                    idx >= 0 && last_char.map_or(idx < widths.len() as i64, |last| code <= last);
                let w = if in_declared_range {
                    match usize::try_from(idx).ok().and_then(|i| widths.get(i)) {
                        Some(Some(w)) => *w,
                        Some(None) => return Err("a /Widths entry is not a number".into()),
                        None => {
                            return Err("/Widths is shorter than /FirstChar../LastChar".into());
                        }
                    }
                } else {
                    missing.ok_or(Cow::Borrowed(
                        "a code outside /FirstChar../LastChar with no /MissingWidth",
                    ))?
                };
                Ok(w * scale)
            }
            MetricsKind::Afm(widths) => usize::try_from(code)
                .ok()
                .and_then(|i| widths.get(i).copied().flatten())
                .ok_or(Cow::Borrowed(
                    "a code whose glyph has no width in the Standard-14 font’s AFM metrics",
                )),
            MetricsKind::Cid {
                default,
                widths,
                ranges,
            } => {
                let w = widths.get(&code).copied().or_else(|| {
                    ranges
                        .iter()
                        .find(|(lo, hi, _)| (*lo..=*hi).contains(&code))
                        .map(|(_, _, w)| *w)
                });
                Ok(w.unwrap_or(*default) / 1000.0)
            }
            MetricsKind::Unknown(reason) => Err(Cow::Borrowed(reason)),
        }
    }

    /// Metrics that know nothing, for text shown without a usable font.
    pub(crate) fn unknown(reason: &'static str) -> FontMetrics {
        FontMetrics {
            kind: MetricsKind::Unknown(Cow::Borrowed(reason)),
            code_bytes: 1,
            vertical: false,
        }
    }
}

fn number(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

/// `number`, following one indirect reference.
fn number_deref(doc: &Document, obj: &Object) -> Option<f64> {
    match obj {
        Object::Reference(id) => number(doc.get_object(*id).ok()?),
        other => number(other),
    }
}

fn name_of(doc: &Document, dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
    let obj = dict.get(key).ok()?;
    let obj = match obj {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    helpers::name_to_string(obj)
}

/// Resolve `key` to an array, following one indirect reference.
fn array_of<'a>(
    doc: &'a Document,
    dict: &'a lopdf::Dictionary,
    key: &[u8],
) -> Option<&'a [Object]> {
    helpers::resolve_array(doc, dict.get(key).ok()?)
}

/// Resolve `key` to a number, following one indirect reference.
fn number_of(doc: &Document, dict: &lopdf::Dictionary, key: &[u8]) -> Option<f64> {
    number_deref(doc, dict.get(key).ok()?)
}

/// Build the metrics for one font dictionary.
pub(crate) fn font_metrics(doc: &Document, dict: &lopdf::Dictionary) -> FontMetrics {
    let subtype = name_of(doc, dict, b"Subtype").unwrap_or_default();
    if subtype == "Type0" {
        return type0_metrics(doc, dict);
    }
    let simple = |kind| FontMetrics {
        kind,
        code_bytes: 1,
        vertical: false,
    };
    let unknown = |reason: String| simple(MetricsKind::Unknown(Cow::Owned(reason)));

    let Some(widths) = array_of(doc, dict, b"Widths") else {
        let base_font = name_of(doc, dict, b"BaseFont").unwrap_or_default();
        return match afm::core14(&base_font) {
            Some(font) if subtype != "Type3" => match afm_widths(doc, dict, font) {
                Ok(widths) => simple(MetricsKind::Afm(widths)),
                Err(reason) => unknown(format!(
                    "Standard-14 font {base_font} has no /Widths and {reason}; \
                     positions are estimated"
                )),
            },
            _ => unknown("font has no /Widths; positions are estimated".to_string()),
        };
    };
    // `/FirstChar` is required alongside `/Widths`; guessing it would shift
    // every width onto the wrong code.
    let Some(first_char) = number_of(doc, dict, b"FirstChar") else {
        return unknown("font has /Widths but no /FirstChar; positions are estimated".to_string());
    };
    let widths: Vec<Option<f64>> = widths.iter().map(|w| number_deref(doc, w)).collect();
    let last_char = number_of(doc, dict, b"LastChar").map(|n| n as i64);
    let missing = dict
        .get(b"FontDescriptor")
        .ok()
        .and_then(|d| helpers::resolve_dict(doc, d))
        .and_then(|fd| number_of(doc, fd, b"MissingWidth"));
    let scale = if subtype == "Type3" {
        match array_of(doc, dict, b"FontMatrix").and_then(|m| m.first().and_then(number)) {
            Some(a) => a,
            None => return unknown("Type3 font without a usable /FontMatrix".to_string()),
        }
    } else {
        0.001
    };
    simple(MetricsKind::Simple {
        first_char: first_char as i64,
        widths,
        last_char,
        missing,
        scale,
    })
}

/// Each code’s AFM width, through the font’s effective encoding: its named
/// base encoding (or its builtin one), with `/Differences` on top.  `Err` when
/// the base encoding is one pdf-dump has no glyph names for.
fn afm_widths(
    doc: &Document,
    dict: &lopdf::Dictionary,
    font: &afm::Core14Font,
) -> Result<Box<[Option<f64>; 256]>, String> {
    let encoding = dict.get(b"Encoding").ok().map(|enc| match enc {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(enc),
        other => other,
    });
    let (base, differences) = match encoding {
        None => (None, None),
        Some(Object::Dictionary(d)) => (
            name_of(doc, d, b"BaseEncoding"),
            array_of(doc, d, b"Differences"),
        ),
        Some(other) => match helpers::name_to_string(other) {
            Some(name) => (Some(name), None),
            None => return Err("its /Encoding is neither a name nor a dictionary".to_string()),
        },
    };
    let names = match base.as_deref() {
        None => &font.builtin,
        Some(name) => afm::encoding(name)
            .ok_or_else(|| format!("pdf-dump has no glyph names for its /Encoding {name}"))?,
    };
    let width = |name: &str| font.width(name).map(|w| f64::from(w) / 1000.0);
    let mut widths = Box::new([None; 256]);
    for (slot, name) in widths.iter_mut().zip(names) {
        *slot = name.and_then(width);
    }
    // `/Differences` renames codes from each integer onward; a glyph name the
    // AFM lacks leaves its code unknown.
    let mut code: i64 = 0;
    for item in differences.unwrap_or_default() {
        if let Object::Name(glyph) = item {
            if let Some(slot) = usize::try_from(code).ok().and_then(|i| widths.get_mut(i)) {
                *slot = std::str::from_utf8(glyph).ok().and_then(width);
            }
            code += 1;
        } else if let Some(n) = number(item) {
            code = n as i64;
        }
    }
    Ok(widths)
}

fn type0_metrics(doc: &Document, dict: &lopdf::Dictionary) -> FontMetrics {
    let encoding = name_of(doc, dict, b"Encoding");
    let vertical = encoding.as_deref() == Some("Identity-V");
    let cid = |kind| FontMetrics {
        kind,
        code_bytes: 2,
        vertical,
    };
    let unknown = |reason: String| cid(MetricsKind::Unknown(Cow::Owned(reason)));
    if !matches!(encoding.as_deref(), Some("Identity-H" | "Identity-V")) {
        return unknown(format!(
            "Type0 font with /Encoding {}: code-to-CID mapping not modeled, so widths are unknown",
            encoding.as_deref().unwrap_or("(a CMap stream)")
        ));
    }
    let Some(descendant) = array_of(doc, dict, b"DescendantFonts")
        .and_then(|a| a.first())
        .and_then(|d| helpers::resolve_dict(doc, d))
    else {
        return unknown("Type0 font without a readable /DescendantFonts entry".to_string());
    };
    let default = number_of(doc, descendant, b"DW").unwrap_or(1000.0);
    match descendant.get(b"W") {
        Err(_) => cid(MetricsKind::Cid {
            default,
            widths: HashMap::new(),
            ranges: Vec::new(),
        }),
        Ok(w) => match helpers::resolve_array(doc, w).and_then(|w| parse_cid_widths(doc, w)) {
            Some((widths, ranges)) => cid(MetricsKind::Cid {
                default,
                widths,
                ranges,
            }),
            None => unknown("CIDFont /W array is malformed; positions are estimated".to_string()),
        },
    }
}

type CidWidths = (HashMap<u32, f64>, Vec<(u32, u32, f64)>);

/// Parse a CIDFont `/W` array: `c [w1 w2 …]` and `c_first c_last w`, freely
/// mixed.  Any malformed element makes the whole array `None` (unknown) rather
/// than silently handing the rest of it to `/DW`.
fn parse_cid_widths(doc: &Document, w: &[Object]) -> Option<CidWidths> {
    let cid = |o: &Object| {
        number_deref(doc, o).and_then(|n| (n >= 0.0 && n <= u32::MAX as f64).then_some(n as u32))
    };
    let mut widths = HashMap::new();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < w.len() {
        let first = cid(&w[i])?;
        let next = match w.get(i + 1)? {
            Object::Reference(id) => doc.get_object(*id).ok()?,
            other => other,
        };
        if let Object::Array(list) = next {
            for (k, v) in list.iter().enumerate() {
                let c = first.checked_add(u32::try_from(k).ok()?)?;
                widths.insert(c, number_deref(doc, v)?);
            }
            i += 2;
        } else {
            let last = cid(next)?;
            let v = number_deref(doc, w.get(i + 2)?)?;
            if last < first {
                return None;
            }
            ranges.push((first, last, v));
            i += 3;
        }
    }
    Some((widths, ranges))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Dictionary;

    fn assert_close(actual: Result<f64, Cow<'_, str>>, expected: f64) {
        let a = actual.expect("width should be known");
        assert!((a - expected).abs() < 1e-9, "{a} != {expected}");
    }

    fn nums(v: &[i64]) -> Object {
        Object::Array(v.iter().map(|&n| Object::Integer(n)).collect())
    }

    #[test]
    fn simple_widths_index_from_first_char_and_fall_back_to_missing_width() {
        let mut doc = Document::new();
        let mut fd = Dictionary::new();
        fd.set("MissingWidth", Object::Integer(250));
        let fd_id = doc.add_object(Object::Dictionary(fd));
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        font.set("FirstChar", Object::Integer(65));
        font.set("Widths", nums(&[600, 700]));
        font.set("FontDescriptor", Object::Reference(fd_id));
        let m = font_metrics(&doc, &font);
        assert_close(m.advance(65), 0.6);
        assert_close(m.advance(66), 0.7);
        assert_close(m.advance(67), 0.25);
        assert_close(m.advance(10), 0.25);
    }

    fn simple(first: Option<i64>, last: Option<i64>, widths: Object) -> Dictionary {
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        if let Some(f) = first {
            font.set("FirstChar", Object::Integer(f));
        }
        if let Some(l) = last {
            font.set("LastChar", Object::Integer(l));
        }
        font.set("Widths", widths);
        font
    }

    #[test]
    fn widths_without_first_char_are_unknown_not_shifted() {
        // Defaulting /FirstChar to 0 would move every width onto the wrong code.
        let m = font_metrics(&Document::new(), &simple(None, Some(126), nums(&[600; 95])));
        assert!(m.advance(72).unwrap_err().contains("FirstChar"));
    }

    #[test]
    fn a_non_numeric_widths_entry_is_unknown_for_that_code_only() {
        let widths = Object::Array(vec![Object::Integer(500), Object::Null]);
        let m = font_metrics(&Document::new(), &simple(Some(65), Some(66), widths));
        assert_close(m.advance(65), 0.5);
        assert!(m.advance(66).is_err());
    }

    #[test]
    fn short_widths_leave_in_range_codes_unknown() {
        // LastChar says 26 codes; /Widths has 2.  Code 90 is in range, not "missing".
        let mut font = simple(Some(65), Some(90), nums(&[500, 500]));
        font.set(
            "FontDescriptor",
            Object::Dictionary({
                let mut fd = Dictionary::new();
                fd.set("MissingWidth", Object::Integer(250));
                fd
            }),
        );
        let m = font_metrics(&Document::new(), &font);
        assert!(m.advance(90).unwrap_err().contains("shorter"));
        assert_close(m.advance(91), 0.25);
    }

    #[test]
    fn out_of_range_code_without_explicit_missing_width_is_unknown() {
        let m = font_metrics(&Document::new(), &simple(Some(65), Some(65), nums(&[500])));
        assert!(m.advance(32).unwrap_err().contains("MissingWidth"));
    }

    fn base14(base_font: &str, encoding: Option<Object>) -> Dictionary {
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(base_font.as_bytes().to_vec()));
        if let Some(enc) = encoding {
            font.set("Encoding", enc);
        }
        font
    }

    #[test]
    fn standard_14_without_widths_takes_the_afm_widths() {
        let doc = Document::new();
        let m = font_metrics(&doc, &base14("Helvetica", None));
        assert_close(m.advance(u32::from(b'a')), 0.556);
        assert_close(m.advance(u32::from(b'm')), 0.833);
        assert_close(m.advance(32), 0.278);
        // StandardEncoding is the builtin: 0x27 is quoteright (222), not quotesingle.
        assert_close(m.advance(0x27), 0.222);
        assert!(
            m.advance(0x7F).unwrap_err().contains("AFM"),
            "no glyph at 0x7F"
        );
        assert!(m.advance(256).is_err());
        let symbol = font_metrics(&doc, &base14("Symbol", None));
        assert_close(symbol.advance(u32::from(b'A')), 0.722); // Alpha
    }

    #[test]
    fn standard_14_afm_widths_follow_a_named_encoding() {
        let doc = Document::new();
        let winansi = Object::Name(b"WinAnsiEncoding".to_vec());
        let m = font_metrics(&doc, &base14("Helvetica", Some(winansi)));
        assert_close(m.advance(0x27), 0.191); // quotesingle
        assert_close(m.advance(0x80), 0.556); // Euro
        let expert = Object::Name(b"MacExpertEncoding".to_vec());
        let m = font_metrics(&doc, &base14("Helvetica", Some(expert)));
        assert!(
            m.advance(u32::from(b'a'))
                .unwrap_err()
                .contains("MacExpertEncoding")
        );
    }

    #[test]
    fn differences_rename_codes_and_a_glyph_the_afm_lacks_is_unknown() {
        let doc = Document::new();
        let mut enc = Dictionary::new();
        enc.set("BaseEncoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        enc.set(
            "Differences",
            Object::Array(vec![
                Object::Integer(65),
                Object::Name(b"m".to_vec()),
                Object::Name(b"g0042".to_vec()),
            ]),
        );
        let m = font_metrics(&doc, &base14("Helvetica", Some(Object::Dictionary(enc))));
        assert_close(m.advance(65), 0.833);
        assert!(m.advance(66).unwrap_err().contains("AFM"));
        assert_close(m.advance(67), 0.722); // C, untouched
    }

    #[test]
    fn subset_prefixed_standard_14_resolves_and_other_fonts_stay_unknown() {
        let doc = Document::new();
        let m = font_metrics(&doc, &base14("ABCDEF+Helvetica-Bold", None));
        assert_close(m.advance(u32::from(b'M')), 0.833);
        let m = font_metrics(&doc, &base14("Arial", None));
        assert!(
            m.advance(u32::from(b'a'))
                .unwrap_err()
                .contains("no /Widths")
        );
    }

    #[test]
    fn type3_widths_scale_by_font_matrix() {
        let doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"Type3".to_vec()));
        font.set("FirstChar", Object::Integer(0));
        font.set("Widths", nums(&[50]));
        font.set(
            "FontMatrix",
            Object::Array(vec![
                Object::Real(0.01),
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(0.01),
                Object::Integer(0),
                Object::Integer(0),
            ]),
        );
        let m = font_metrics(&doc, &font);
        assert!((m.advance(0).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn cid_w_array_both_forms_and_dw_default() {
        let mut doc = Document::new();
        let mut desc = Dictionary::new();
        desc.set("DW", Object::Integer(500));
        desc.set(
            "W",
            Object::Array(vec![
                Object::Integer(10),
                nums(&[100, 200]),
                Object::Integer(20),
                Object::Integer(29),
                Object::Integer(300),
            ]),
        );
        let desc_id = doc.add_object(Object::Dictionary(desc));
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"Type0".to_vec()));
        font.set("Encoding", Object::Name(b"Identity-H".to_vec()));
        font.set(
            "DescendantFonts",
            Object::Array(vec![Object::Reference(desc_id)]),
        );
        let m = font_metrics(&doc, &font);
        assert_eq!(m.code_bytes, 2);
        assert!(!m.vertical);
        assert_close(m.advance(10), 0.1);
        assert_close(m.advance(11), 0.2);
        assert_close(m.advance(25), 0.3);
        assert_close(m.advance(99), 0.5);
    }

    fn cid_font_with_w(doc: &mut Document, w: Object) -> Dictionary {
        let mut desc = Dictionary::new();
        desc.set("W", w);
        let desc_id = doc.add_object(Object::Dictionary(desc));
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"Type0".to_vec()));
        font.set("Encoding", Object::Name(b"Identity-H".to_vec()));
        font.set(
            "DescendantFonts",
            Object::Array(vec![Object::Reference(desc_id)]),
        );
        font
    }

    #[test]
    fn malformed_cid_w_is_unknown_not_silently_dw() {
        for w in [
            Object::Array(vec![Object::Name(b"x".to_vec()), nums(&[100])]),
            Object::Array(vec![Object::Integer(10), Object::Array(vec![Object::Null])]),
            Object::Array(vec![
                Object::Integer(10),
                Object::Integer(5),
                Object::Integer(100),
            ]),
            Object::Array(vec![Object::Integer(10)]),
        ] {
            let mut doc = Document::new();
            let font = cid_font_with_w(&mut doc, w.clone());
            let m = font_metrics(&doc, &font);
            assert!(m.advance(10).unwrap_err().contains("/W"), "{w:?}");
        }
    }

    #[test]
    fn cid_w_near_u32_max_does_not_overflow() {
        let mut doc = Document::new();
        let font = cid_font_with_w(
            &mut doc,
            Object::Array(vec![Object::Integer(u32::MAX as i64), nums(&[1, 2])]),
        );
        assert!(font_metrics(&doc, &font).advance(0).is_err());
    }

    #[test]
    fn cid_with_non_identity_encoding_is_unknown() {
        let doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"Type0".to_vec()));
        font.set("Encoding", Object::Name(b"UniJIS-UCS2-H".to_vec()));
        let m = font_metrics(&doc, &font);
        assert!(m.advance(1).is_err());
    }
}
