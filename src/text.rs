use lopdf::{Document, Object, ObjectId, content::Content};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::Write;
use std::rc::Rc;

use crate::cmap::{CodeWidth, ToUnicodeCMap};
use crate::encodings;
use crate::helpers;
use crate::types::PageSpec;

/// Fraction of attempted-decode codes that may go unmapped before a document's
/// text-extraction verdict is downgraded to `Degraded`.
const LOW_COVERAGE_THRESHOLD: f64 = 0.20;

pub(crate) struct TextResult {
    pub text: String,
    pub warnings: Vec<String>,
    /// Character codes we attempted to decode through a font's ToUnicode CMap.
    pub total_codes: u64,
    /// Of `total_codes`, how many had no ToUnicode mapping (emitted U+FFFD).
    pub unmapped_codes: u64,
    /// One record per font in this page's resources (for reliability reporting).
    pub fonts: Vec<FontReliabilityRecord>,
}

/// How trustworthy a font's text extraction is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Reliability {
    Reliable,
    Degraded,
    Unreliable,
}

impl Reliability {
    fn as_str(self) -> &'static str {
        match self {
            Reliability::Reliable => "reliable",
            Reliability::Degraded => "degraded",
            Reliability::Unreliable => "unreliable",
        }
    }
}

/// A per-font reliability classification, surfaced in the banner and JSON.
#[derive(Clone)]
pub(crate) struct FontReliabilityRecord {
    pub name: String,
    pub base_font: String,
    pub subtype: String,
    pub classification: Reliability,
    pub has_to_unicode: bool,
    pub reason: String,
}

/// How a font's show-string bytes are turned into Unicode.
enum FontDecoder {
    /// Decode each code through a parsed ToUnicode CMap.
    ToUnicode { cmap: Rc<ToUnicodeCMap>, width: u8 },
    /// Single-byte WinAnsiEncoding table; `overridden` codes are remapped by
    /// `/Differences` to glyph names we cannot resolve (emit U+FFFD).
    WinAnsi { overridden: HashSet<u8> },
    /// Raw byte passthrough (today's behavior) — used when we cannot decode.
    Passthrough,
}

/// The standard-14 nonsymbolic base fonts (Symbol/ZapfDingbats excluded — they
/// are handled separately as symbolic). ASCII passthrough is accurate for these.
const STANDARD_14_TEXT: &[&str] = &[
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

#[cfg(test)]
fn extract_text_from_page(doc: &Document, page_id: ObjectId) -> String {
    extract_text_from_page_with_warnings(doc, page_id).text
}

pub(crate) fn extract_text_from_page_with_warnings(
    doc: &Document,
    page_id: ObjectId,
) -> TextResult {
    let mut text = String::new();
    let mut warnings = Vec::new();
    let mut total_codes: u64 = 0;
    let mut unmapped_codes: u64 = 0;

    // Check font encodings for this page (legacy per-page warning strings,
    // retained for the JSON `warnings` field and page_info's garbled heuristic).
    if let Ok(Object::Dictionary(page_dict)) = doc.get_object(page_id) {
        let font_warnings = check_page_font_encodings(doc, page_dict);
        warnings.extend(font_warnings);
    }

    // Build the font-aware decoder table + reliability records for this page.
    let (font_table, fonts) = build_page_font_table(doc, page_id);

    let stream_data = match helpers::read_content_streams(doc, page_id) {
        Some(data) => data,
        None => {
            return TextResult {
                text,
                warnings,
                total_codes,
                unmapped_codes,
                fonts,
            };
        }
    };
    warnings.extend(stream_data.warnings);

    if stream_data.bytes.is_empty() {
        return TextResult {
            text,
            warnings,
            total_codes,
            unmapped_codes,
            fonts,
        };
    }

    let operations = match Content::decode(&stream_data.bytes) {
        Ok(content) => content.operations,
        Err(_) => {
            warnings.push("Content stream has syntax errors".to_string());
            return TextResult {
                text,
                warnings,
                total_codes,
                unmapped_codes,
                fonts,
            };
        }
    };

    let mut first_bt = true;
    let mut current_font: Option<&FontDecoder> = None;
    for op in &operations {
        match op.operator.as_str() {
            "BT" => {
                if !first_bt && !text.ends_with('\n') {
                    text.push('\n');
                }
                first_bt = false;
            }
            "Tf" => {
                // Select the active font by resource name (e.g. /F1 12 Tf).
                if let Some(Object::Name(n)) = op.operands.first() {
                    let key = String::from_utf8_lossy(n);
                    current_font = font_table.get(key.as_ref());
                }
            }
            "Td" | "TD" if op.operands.len() >= 2 => {
                // Check ty (second operand) for line break — negative y means downward movement
                if let Object::Integer(ty) = &op.operands[1] {
                    if *ty < 0 {
                        text.push('\n');
                    }
                } else if let Object::Real(ty) = &op.operands[1]
                    && *ty < 0.0
                {
                    text.push('\n');
                }
            }
            "T*" => {
                text.push('\n');
            }
            "Tj" => {
                if let Some(Object::String(bytes, _)) = op.operands.first() {
                    emit_show_string(
                        &mut text,
                        bytes,
                        current_font,
                        &mut total_codes,
                        &mut unmapped_codes,
                    );
                }
            }
            "TJ" => {
                if let Some(Object::Array(arr)) = op.operands.first() {
                    for item in arr {
                        match item {
                            Object::String(bytes, _) => {
                                emit_show_string(
                                    &mut text,
                                    bytes,
                                    current_font,
                                    &mut total_codes,
                                    &mut unmapped_codes,
                                );
                            }
                            Object::Integer(n) if *n < -100 => {
                                text.push(' ');
                            }
                            Object::Real(n) if *n < -100.0 => {
                                text.push(' ');
                            }
                            _ => {}
                        }
                    }
                }
            }
            "'" => {
                text.push('\n');
                if let Some(Object::String(bytes, _)) = op.operands.first() {
                    emit_show_string(
                        &mut text,
                        bytes,
                        current_font,
                        &mut total_codes,
                        &mut unmapped_codes,
                    );
                }
            }
            "\"" => {
                text.push('\n');
                // Third operand is the string
                if let Some(Object::String(bytes, _)) = op.operands.get(2) {
                    emit_show_string(
                        &mut text,
                        bytes,
                        current_font,
                        &mut total_codes,
                        &mut unmapped_codes,
                    );
                }
            }
            _ => {}
        }
    }

    TextResult {
        text,
        warnings,
        total_codes,
        unmapped_codes,
        fonts,
    }
}

/// Append a show-string to `out`, decoding it through the active font.
/// Falls back to today's UTF-8-lossy passthrough when there is no decodable
/// font, so already-working PDFs produce byte-identical output.
fn emit_show_string(
    out: &mut String,
    bytes: &[u8],
    font: Option<&FontDecoder>,
    total: &mut u64,
    unmapped: &mut u64,
) {
    match font {
        Some(FontDecoder::ToUnicode { cmap, width }) => {
            for code in split_codes(bytes, *width) {
                *total += 1;
                match cmap.map_code(code) {
                    Some(s) => out.push_str(&s),
                    None => {
                        out.push('\u{FFFD}');
                        *unmapped += 1;
                    }
                }
            }
        }
        Some(FontDecoder::WinAnsi { overridden }) => {
            for &b in bytes {
                if overridden.contains(&b) {
                    out.push('\u{FFFD}');
                } else {
                    out.push(encodings::winansi(b).unwrap_or('\u{FFFD}'));
                }
            }
        }
        // No active font, unknown font, or undecodable font: passthrough.
        _ => out.push_str(&String::from_utf8_lossy(bytes)),
    }
}

/// Split a show-string into `width`-byte big-endian character codes.
fn split_codes(bytes: &[u8], width: u8) -> Vec<u32> {
    let w = (width.max(1)) as usize;
    bytes
        .chunks(w)
        .map(|c| {
            let mut v = 0u32;
            for &b in c {
                v = (v << 8) | b as u32;
            }
            v
        })
        .collect()
}

/// Build the per-page font decoder table (keyed by resource name, e.g. "F1")
/// plus one reliability record per font in the page's resources.
fn build_page_font_table(
    doc: &Document,
    page_id: ObjectId,
) -> (
    std::collections::HashMap<String, FontDecoder>,
    Vec<FontReliabilityRecord>,
) {
    let mut table = std::collections::HashMap::new();
    let mut records = Vec::new();

    let Some(resources) = crate::resources::resolve_page_resources(doc, page_id) else {
        return (table, records);
    };
    let font_dict = match resources.get(b"Font") {
        Ok(obj) => match helpers::resolve_dict(doc, obj) {
            Some(d) => d,
            None => return (table, records),
        },
        Err(_) => return (table, records),
    };

    for (name, value) in font_dict.iter() {
        let font_name = String::from_utf8_lossy(name).into_owned();
        let dict = match value {
            Object::Reference(r) => match doc.get_object(*r) {
                Ok(Object::Dictionary(d)) => d,
                Ok(Object::Stream(s)) => &s.dict,
                _ => continue,
            },
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };

        let (decoder, record) = build_font_decoder(doc, dict, &font_name);
        table.insert(font_name, decoder);
        records.push(record);
    }

    (table, records)
}

/// Build a decoder and reliability record for a single font dictionary.
fn build_font_decoder(
    doc: &Document,
    dict: &lopdf::Dictionary,
    font_name: &str,
) -> (FontDecoder, FontReliabilityRecord) {
    let subtype = dict
        .get(b"Subtype")
        .ok()
        .and_then(|v| v.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    let base_font = dict
        .get(b"BaseFont")
        .ok()
        .and_then(|v| v.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_else(|| "-".to_string());
    let is_cid = matches!(subtype.as_str(), "Type0" | "CIDFontType0" | "CIDFontType2");
    let has_to_unicode = dict.has(b"ToUnicode");

    let record = |classification, reason: &str| FontReliabilityRecord {
        name: format!("/{}", font_name),
        base_font: base_font.clone(),
        subtype: subtype.clone(),
        classification,
        has_to_unicode,
        reason: reason.to_string(),
    };

    // 1. ToUnicode CMap (the headline Tier 1 path).
    if let Some(tu_ref) = dict
        .get(b"ToUnicode")
        .ok()
        .and_then(|v| v.as_reference().ok())
        && let Ok(Object::Stream(stream)) = doc.get_object(tu_ref)
    {
        let (bytes, _warn) = crate::stream::decode_stream(stream);
        let cmap = ToUnicodeCMap::parse(&bytes);
        if !cmap.is_empty() {
            let width = match cmap.byte_width() {
                CodeWidth::Fixed(w) => w,
                CodeWidth::Variable(..) | CodeWidth::Unknown => {
                    if is_cid {
                        2
                    } else {
                        1
                    }
                }
            };
            return (
                FontDecoder::ToUnicode {
                    cmap: Rc::new(cmap),
                    width,
                },
                record(Reliability::Reliable, ""),
            );
        }
    }

    // 2. Simple font with an explicit WinAnsiEncoding base.
    if !is_cid && font_base_encoding(doc, dict).as_deref() == Some("WinAnsiEncoding") {
        let overridden = encoding_overridden_codes(doc, dict);
        if overridden.is_empty() {
            return (
                FontDecoder::WinAnsi { overridden },
                record(Reliability::Reliable, ""),
            );
        }
        return (
            FontDecoder::WinAnsi { overridden },
            record(
                Reliability::Degraded,
                "custom /Differences glyph names without a ToUnicode map (no Adobe Glyph List)",
            ),
        );
    }

    // 3. Passthrough — classify how trustworthy the raw bytes are.
    let (classification, reason) = classify_passthrough(doc, dict, &subtype, &base_font, is_cid);
    (FontDecoder::Passthrough, record(classification, reason))
}

/// Classify a font we cannot actively decode (raw byte passthrough).
fn classify_passthrough(
    doc: &Document,
    dict: &lopdf::Dictionary,
    subtype: &str,
    base_font: &str,
    is_cid: bool,
) -> (Reliability, &'static str) {
    let _ = subtype;
    if is_cid {
        return (
            Reliability::Unreliable,
            "CID/Type0 font without a ToUnicode map",
        );
    }
    if base_font == "Symbol" || base_font == "ZapfDingbats" {
        return (
            Reliability::Degraded,
            "symbolic font without a ToUnicode map",
        );
    }

    let has_differences = !encoding_overridden_codes(doc, dict).is_empty();
    // A recognized base encoding (besides WinAnsi, handled earlier with a table)
    // decodes the ASCII range — which dominates real content — accurately via
    // passthrough, so it is not flagged. Only the high range (0x80..) can be
    // off, and reserving the banner for genuinely garbled text keeps it loud.
    let recognized_base = matches!(
        font_base_encoding(doc, dict).as_deref(),
        Some("MacRomanEncoding") | Some("StandardEncoding") | Some("MacExpertEncoding")
    );
    if recognized_base {
        if has_differences {
            return (
                Reliability::Degraded,
                "/Differences glyph names without a ToUnicode map (no Adobe Glyph List)",
            );
        }
        return (Reliability::Reliable, "");
    }

    // Standard-14 text fonts with no/unknown encoding decode accurately as ASCII.
    if STANDARD_14_TEXT.contains(&base_font) {
        return (Reliability::Reliable, "");
    }
    if has_differences {
        return (
            Reliability::Degraded,
            "custom /Differences encoding without a ToUnicode map",
        );
    }
    // Unknown simple font: ASCII passes through, anything else may be inaccurate.
    (
        Reliability::Degraded,
        "no ToUnicode map or recognized encoding",
    )
}

/// Resolve a font's base encoding name: a direct `/Encoding` name, or the
/// `/BaseEncoding` of an encoding dictionary.
fn font_base_encoding(doc: &Document, dict: &lopdf::Dictionary) -> Option<String> {
    let enc = dict.get(b"Encoding").ok()?;
    if let Object::Name(n) = enc {
        return Some(String::from_utf8_lossy(n).into_owned());
    }
    let resolved = match enc {
        Object::Reference(r) => doc.get_object(*r).ok()?,
        other => other,
    };
    match resolved {
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        Object::Dictionary(d) => d
            .get(b"BaseEncoding")
            .ok()
            .and_then(|v| v.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

/// Codes remapped by an `/Encoding /Differences` array. We can detect which
/// codes are overridden, but resolving the glyph names to Unicode needs the
/// Adobe Glyph List (Tier 2), so callers emit U+FFFD for these codes.
fn encoding_overridden_codes(doc: &Document, dict: &lopdf::Dictionary) -> HashSet<u8> {
    let mut set = HashSet::new();
    let Some(enc) = dict.get(b"Encoding").ok() else {
        return set;
    };
    let Some(enc_dict) = helpers::resolve_dict(doc, enc) else {
        return set;
    };
    let Some(diffs) = enc_dict
        .get(b"Differences")
        .ok()
        .and_then(|v| helpers::resolve_array(doc, v))
    else {
        return set;
    };
    let mut code: i64 = 0;
    for item in diffs {
        match item {
            Object::Integer(n) => code = *n,
            Object::Real(n) => code = *n as i64,
            Object::Name(_) => {
                if (0..=255).contains(&code) {
                    set.insert(code as u8);
                }
                code += 1;
            }
            _ => {}
        }
    }
    set
}

/// Check whether fonts on a page have known encodings.
/// Returns warnings for fonts that lack ToUnicode maps or recognized encodings.
pub(crate) fn check_page_font_encodings(
    doc: &Document,
    page_dict: &lopdf::Dictionary,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Resolve /Resources (may be a reference)
    let resources = match page_dict.get(b"Resources") {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Reference(r)) => match doc.get_object(*r) {
            Ok(Object::Dictionary(d)) => d,
            _ => return warnings,
        },
        _ => return warnings,
    };

    // Get /Font sub-dictionary
    let font_dict = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Reference(r)) => match doc.get_object(*r) {
            Ok(Object::Dictionary(d)) => d,
            _ => return warnings,
        },
        _ => return warnings,
    };

    for (name, value) in font_dict.iter() {
        let font_name = String::from_utf8_lossy(name);
        let font_obj = match value {
            Object::Reference(r) => match doc.get_object(*r) {
                Ok(obj) => obj,
                _ => continue,
            },
            obj => obj,
        };

        let dict = match font_obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };

        // Check for /ToUnicode — if present, encoding is known
        if dict.has(b"ToUnicode") {
            continue;
        }

        // Check /Encoding
        let has_known_encoding = match dict.get(b"Encoding") {
            Ok(Object::Name(enc)) => {
                let enc_str = String::from_utf8_lossy(enc);
                matches!(
                    enc_str.as_ref(),
                    "WinAnsiEncoding"
                        | "MacRomanEncoding"
                        | "MacExpertEncoding"
                        | "StandardEncoding"
                )
            }
            Ok(Object::Dictionary(_)) => true, // Encoding dict with /Differences
            Ok(Object::Reference(r)) => {
                matches!(
                    doc.get_object(*r),
                    Ok(Object::Dictionary(_)) | Ok(Object::Name(_))
                )
            }
            _ => false,
        };

        if has_known_encoding {
            continue;
        }

        // Check /Subtype — CID fonts without ToUnicode are problematic
        let subtype = dict
            .get(b"Subtype")
            .ok()
            .and_then(|v| v.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();

        let base_font = dict
            .get(b"BaseFont")
            .ok()
            .and_then(|v| v.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_else(|| font_name.to_string());

        if subtype == "Type0" || subtype == "CIDFontType0" || subtype == "CIDFontType2" {
            warnings.push(format!(
                "Font /{} ({}) uses CID encoding without ToUnicode map. Text may be inaccurate.",
                font_name, base_font
            ));
        } else if subtype == "Type1" || subtype == "TrueType" || subtype == "Type3" {
            // Simple fonts without encoding — may use built-in encoding
            // Only warn if it looks custom (not a standard 14 font)
            let standard_14 = [
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
                "Symbol",
                "ZapfDingbats",
            ];
            if !standard_14.iter().any(|s| base_font == *s) {
                warnings.push(format!(
                    "Font /{} ({}) has no explicit encoding or ToUnicode map. Text may be inaccurate.",
                    font_name, base_font
                ));
            }
        }
    }

    warnings
}

/// Marker suffix shared by every `check_page_font_encodings` warning. Used to
/// keep those (now summarized in the reliability banner) out of the
/// deduplicated content-warning stream printed to stderr.
const FONT_WARNING_MARKER: &str = "Text may be inaccurate";

pub(crate) fn print_text(
    writer: &mut impl Write,
    doc: &Document,
    page_filter: Option<&PageSpec>,
) -> bool {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return false;
        }
    };

    let mut content_warnings = std::collections::BTreeSet::new();
    let mut all_fonts = Vec::new();
    let mut total_codes: u64 = 0;
    let mut unmapped_codes: u64 = 0;

    for (pn, page_id) in &page_list {
        wln!(writer, "--- Page {} ---", pn);
        let result = extract_text_from_page_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            if !warn.contains(FONT_WARNING_MARKER) {
                content_warnings.insert(warn.clone());
            }
        }
        total_codes += result.total_codes;
        unmapped_codes += result.unmapped_codes;
        all_fonts.extend(result.fonts);
        wln!(writer, "{}", result.text);
    }

    // Genuine content-stream problems are surfaced once (deduplicated), not
    // repeated per page; font-reliability detail goes in the banner below.
    for warn in &content_warnings {
        eprintln!("Warning: {}", warn);
    }

    let fonts = dedup_font_records(all_fonts);
    print_reliability_banner(&fonts, total_codes, unmapped_codes);
    document_verdict(&fonts, total_codes, unmapped_codes) == Reliability::Unreliable
}

/// Returns `(json_value, had_issues)`; `had_issues` is true when the document's
/// text extraction is classified `Unreliable`.
pub(crate) fn text_json_value(doc: &Document, page_filter: Option<&PageSpec>) -> (Value, bool) {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => {
            return (json!({"error": msg}), false);
        }
    };

    let mut page_results = Vec::new();
    let mut all_fonts = Vec::new();
    let mut total_codes: u64 = 0;
    let mut unmapped_codes: u64 = 0;

    for (pn, page_id) in &page_list {
        let result = extract_text_from_page_with_warnings(doc, *page_id);
        total_codes += result.total_codes;
        unmapped_codes += result.unmapped_codes;
        let mut entry = serde_json::Map::new();
        entry.insert("page_number".to_string(), json!(pn));
        entry.insert("text".to_string(), json!(result.text));
        if !result.warnings.is_empty() {
            entry.insert("warnings".to_string(), json!(result.warnings));
        }
        page_results.push(Value::Object(entry));
        all_fonts.extend(result.fonts);
    }

    let fonts = dedup_font_records(all_fonts);
    // Print the loud banner to stderr even in JSON mode; stdout stays clean.
    print_reliability_banner(&fonts, total_codes, unmapped_codes);
    let reliability = reliability_json_value(&fonts, total_codes, unmapped_codes);
    let had_issues =
        document_verdict(&fonts, total_codes, unmapped_codes) == Reliability::Unreliable;
    (
        json!({"pages": page_results, "reliability": reliability}),
        had_issues,
    )
}

/// Deduplicate per-page font records (a font recurs identically on every page).
fn dedup_font_records(records: Vec<FontReliabilityRecord>) -> Vec<FontReliabilityRecord> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in records {
        let key = format!("{}|{}|{}", r.name, r.base_font, r.subtype);
        if seen.insert(key) {
            out.push(r);
        }
    }
    out
}

/// Worst per-font classification, bumped to `Degraded` if too many codes went
/// unmapped during ToUnicode decoding.
fn document_verdict(fonts: &[FontReliabilityRecord], total: u64, unmapped: u64) -> Reliability {
    let mut verdict = Reliability::Reliable;
    for f in fonts {
        verdict = verdict.max(f.classification);
    }
    if total > 0
        && (unmapped as f64 / total as f64) > LOW_COVERAGE_THRESHOLD
        && verdict < Reliability::Unreliable
    {
        verdict = Reliability::Degraded;
    }
    verdict
}

/// Print a loud, delineated reliability banner to stderr when extraction is not
/// fully reliable. Silent on the happy path.
fn print_reliability_banner(fonts: &[FontReliabilityRecord], total: u64, unmapped: u64) {
    let verdict = document_verdict(fonts, total, unmapped);
    if verdict == Reliability::Reliable {
        return;
    }
    let (label, word) = match verdict {
        Reliability::Unreliable => ("[ERROR]", "UNRELIABLE"),
        _ => ("[WARN]", "DEGRADED"),
    };
    let bar = "=".repeat(56);
    let rule = "-".repeat(56);
    eprintln!("{}", bar);
    eprintln!("{} TEXT EXTRACTION RELIABILITY: {}", label, word);
    eprintln!("{}", rule);

    let problems: Vec<&FontReliabilityRecord> = fonts
        .iter()
        .filter(|f| f.classification != Reliability::Reliable)
        .collect();
    if !problems.is_empty() {
        eprintln!(
            "  {} font(s) could not be decoded reliably; extracted text",
            problems.len()
        );
        eprintln!("  for those fonts may be inaccurate:");
        for f in &problems {
            let subtype = if f.subtype.is_empty() {
                "?"
            } else {
                f.subtype.as_str()
            };
            eprintln!(
                "    - {} ({}, {}): {}",
                f.name, f.base_font, subtype, f.reason
            );
        }
    }
    if total > 0 && unmapped > 0 {
        let pct = (unmapped as f64 / total as f64 * 100.0).round() as u64;
        eprintln!(
            "  {} of {} character codes ({}%) could not be mapped.",
            unmapped, total, pct
        );
    }
    eprintln!("{}", bar);
}

/// Build the JSON `reliability` object summarizing the document verdict.
fn reliability_json_value(fonts: &[FontReliabilityRecord], total: u64, unmapped: u64) -> Value {
    let verdict = document_verdict(fonts, total, unmapped);
    let ratio = if total > 0 {
        (unmapped as f64 / total as f64 * 1000.0).round() / 1000.0
    } else {
        0.0
    };
    let font_values: Vec<Value> = fonts
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "base_font": f.base_font,
                "subtype": f.subtype,
                "classification": f.classification.as_str(),
                "has_to_unicode": f.has_to_unicode,
                "reason": f.reason,
            })
        })
        .collect();
    json!({
        "verdict": verdict.as_str(),
        "total_codes": total,
        "unmapped_codes": unmapped,
        "unmapped_ratio": ratio,
        "fonts": font_values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::Document;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    #[test]
    fn extract_text_tj() {
        let mut doc = Document::new();
        let content = b"BT\n(Hello) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Hello"));
    }

    #[test]
    fn extract_text_tj_array() {
        let mut doc = Document::new();
        let content = b"BT\n[(H) (ello)] TJ\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Hello"));
    }

    #[test]
    fn extract_text_tj_array_spacing() {
        let mut doc = Document::new();
        // -200 should insert a space
        let content = b"BT\n[(Hello) -200 (World)] TJ\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Hello World"));
    }

    #[test]
    fn extract_text_td_newline() {
        let mut doc = Document::new();
        let content = b"BT\n0 -12 Td\n(Line1) Tj\n0 -12 Td\n(Line2) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Line1"));
        assert!(text.contains("Line2"));
        assert!(text.contains('\n'));
    }

    #[test]
    fn extract_text_tstar() {
        let mut doc = Document::new();
        let content = b"BT\n(Line1) Tj\nT*\n(Line2) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Line1\nLine2"));
    }

    #[test]
    fn extract_text_quote_operator() {
        let mut doc = Document::new();
        let content = b"BT\n(Line1) '\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Line1"));
    }

    #[test]
    fn extract_text_empty_stream() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), vec![]);
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.is_empty());
    }

    #[test]
    fn extract_text_no_contents() {
        let mut doc = Document::new();
        let page = Dictionary::new();
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.is_empty());
    }

    #[test]
    fn print_text_all_pages() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_text(w, &doc, None));
        assert!(out.contains("--- Page 1 ---"));
        assert!(out.contains("--- Page 2 ---"));
    }

    #[test]
    fn print_text_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let out = output_of(|w| render_json(w, &text_json_value(&doc, None).0));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert!(parsed["pages"].is_array());
        assert_eq!(parsed["pages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn extract_text_double_quote_operator() {
        // The " operator: third operand is the string
        let mut doc = Document::new();
        let content = b"BT\n1 2 (Quoted) \"\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.contains("Quoted"),
            "Double-quote operator should extract text, got: {:?}",
            text
        );
    }

    #[test]
    fn extract_text_td_uppercase() {
        // TD operator should also produce newline when ty < 0
        let mut doc = Document::new();
        let content = b"BT\n0 -14 TD\n(Line1) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.contains('\n'),
            "TD with negative ty should produce newline"
        );
        assert!(text.contains("Line1"));
    }

    #[test]
    fn extract_text_td_zero_ty_no_newline() {
        // Td with ty=0 should NOT produce a newline
        let mut doc = Document::new();
        let content = b"BT\n100 0 Td\n(NoNewline) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        // The text from BT should not have a newline before "NoNewline"
        // since ty=0. There may be a newline from BT, but not from Td.
        assert!(text.contains("NoNewline"));
    }

    #[test]
    fn extract_text_td_positive_ty_no_newline() {
        // Td with positive ty (e.g. superscript) should NOT produce a newline
        let mut doc = Document::new();
        let content = b"BT\n(Base) Tj\n5 4 Td\n(Super) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Base"));
        assert!(text.contains("Super"));
        // Positive ty should not insert a newline between Base and Super
        assert!(
            !text.contains("Base\nSuper"),
            "Positive ty should not produce newline, got: {:?}",
            text
        );
    }

    #[test]
    fn extract_text_td_positive_real_ty_no_newline() {
        // Td with positive Real ty should NOT produce a newline
        let mut doc = Document::new();
        let content = b"BT\n(Base) Tj\n5 4.5 Td\n(Super) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Base"));
        assert!(text.contains("Super"));
        assert!(
            !text.contains("Base\nSuper"),
            "Positive Real ty should not produce newline, got: {:?}",
            text
        );
    }

    #[test]
    fn extract_text_td_real_operand() {
        // Td with negative Real ty should produce newline
        let mut doc = Document::new();
        let content = b"BT\n0 -14.5 Td\n(RealTd) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.contains('\n'),
            "Td with negative Real ty should produce newline"
        );
        assert!(text.contains("RealTd"));
    }

    #[test]
    fn extract_text_tj_small_negative_no_space() {
        // TJ with small negative (-50 > -100): should NOT insert space
        let mut doc = Document::new();
        let content = b"BT\n[(He) -50 (llo)] TJ\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.contains("Hello"),
            "Small negative should not insert space, got: {:?}",
            text
        );
        assert!(!text.contains("He llo"), "Should not have space");
    }

    #[test]
    fn extract_text_multiple_bt_blocks() {
        // Multiple BT/ET blocks should insert newline between them
        let mut doc = Document::new();
        let content = b"BT\n(Block1) Tj\nET\nBT\n(Block2) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Block1"), "Should contain first block text");
        assert!(text.contains("Block2"), "Should contain second block text");
        // There should be a newline between the blocks (from second BT)
        let block1_pos = text.find("Block1").unwrap();
        let block2_pos = text.find("Block2").unwrap();
        let between = &text[block1_pos + 6..block2_pos];
        assert!(
            between.contains('\n'),
            "Should have newline between BT blocks, between: {:?}",
            between
        );
    }

    #[test]
    fn extract_text_contents_array_of_refs() {
        // /Contents as an array of stream references
        let mut doc = Document::new();
        let s1 = Stream::new(Dictionary::new(), b"BT\n(Part1) Tj\nET".to_vec());
        let s1_id = doc.add_object(Object::Stream(s1));
        let s2 = Stream::new(Dictionary::new(), b"BT\n(Part2) Tj\nET".to_vec());
        let s2_id = doc.add_object(Object::Stream(s2));
        let mut page = Dictionary::new();
        page.set(
            "Contents",
            Object::Array(vec![Object::Reference(s1_id), Object::Reference(s2_id)]),
        );
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.contains("Part1"),
            "Should extract text from first stream"
        );
        assert!(
            text.contains("Part2"),
            "Should extract text from second stream"
        );
    }

    #[test]
    fn extract_text_non_dictionary_page() {
        // Page object is not a dictionary → empty text
        let mut doc = Document::new();
        let p_id = doc.add_object(Object::Integer(42));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.is_empty(),
            "Non-dictionary page should return empty text"
        );
    }

    #[test]
    fn extract_text_contents_ref_to_non_stream() {
        // /Contents references a non-stream object → skipped, no crash
        let mut doc = Document::new();
        let non_stream_id = doc.add_object(Object::Integer(42));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(non_stream_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.is_empty(),
            "Non-stream contents should return empty text"
        );
    }

    #[test]
    fn extract_text_flatedecode_content_stream() {
        // Content stream with FlateDecode should be decoded before text extraction
        let mut doc = Document::new();
        let raw_content = b"BT\n(Compressed) Tj\nET";
        let compressed = zlib_compress(raw_content);
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, compressed);
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(
            text.contains("Compressed"),
            "Should decode FlateDecode stream before extracting text, got: {:?}",
            text
        );
    }

    #[test]
    fn print_text_json_with_page_filter() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let out = output_of(|w| render_json(w, &text_json_value(&doc, Some(&spec)).0));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        let pages = parsed["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1, "Should have exactly one page");
        assert_eq!(pages[0]["page_number"], 1);
    }

    #[test]
    fn text_with_page_range() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Range(1, 2);
        let out = output_of(|w| print_text(w, &doc, Some(&spec)));
        assert!(out.contains("--- Page 1 ---"));
        assert!(out.contains("--- Page 2 ---"));
    }

    // --- Font-aware extraction (Tier 1) ------------------------------------

    /// Build a one-page doc (with a catalog/Pages tree, so it works with both
    /// `extract_text_from_page_with_warnings` and the page-enumerating
    /// `print_text`/`text_json_value`) carrying a single font `/F1` —
    /// optionally with a ToUnicode CMap stream — and the given content stream.
    fn doc_with_font(
        mut font: Dictionary,
        tounicode: Option<&[u8]>,
        content: &[u8],
    ) -> (Document, ObjectId) {
        let mut doc = Document::new();
        if let Some(tu) = tounicode {
            let s = Stream::new(Dictionary::new(), tu.to_vec());
            let tu_id = doc.add_object(Object::Stream(s));
            font.set("ToUnicode", Object::Reference(tu_id));
        }
        let font_id = doc.add_object(Object::Dictionary(font));
        let mut f1 = Dictionary::new();
        f1.set("F1", Object::Reference(font_id));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(f1));
        let c = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(c));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        page.set("Resources", Object::Dictionary(resources));
        let p_id = doc.add_object(Object::Dictionary(page));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference(p_id)]));
        pages.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages));
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(p_id) {
            d.set("Parent", Object::Reference(pages_id));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));
        (doc, p_id)
    }

    fn type0_font() -> Dictionary {
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type0".to_vec()));
        font.set("BaseFont", Object::Name(b"ABCDEF+Custom".to_vec()));
        font
    }

    #[test]
    fn type0_identity_h_with_tounicode_decodes_two_byte() {
        // The headline win: 2-byte codes mapped through ToUnicode.
        let cmap = b"begincodespacerange <0000> <FFFF> endcodespacerange \
                     beginbfchar <0041> <0041> <0042> <0042> endbfchar";
        let (doc, p_id) = doc_with_font(type0_font(), Some(cmap), b"BT /F1 12 Tf <00410042> Tj ET");
        let result = extract_text_from_page_with_warnings(&doc, p_id);
        assert_eq!(result.text, "AB");
        assert_eq!(result.total_codes, 2);
        assert_eq!(result.unmapped_codes, 0);
    }

    #[test]
    fn unmapped_code_emits_replacement_and_counts() {
        let cmap = b"begincodespacerange <0000> <FFFF> endcodespacerange \
                     beginbfchar <0041> <0041> endbfchar";
        let (doc, p_id) = doc_with_font(type0_font(), Some(cmap), b"BT /F1 12 Tf <00410099> Tj ET");
        let result = extract_text_from_page_with_warnings(&doc, p_id);
        assert_eq!(result.text, "A\u{FFFD}");
        assert_eq!(result.total_codes, 2);
        assert_eq!(result.unmapped_codes, 1);
    }

    #[test]
    fn winansi_simple_font_decodes_high_byte() {
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"ABCDEF+Body".to_vec()));
        font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        // 0x96 is an en dash in WinAnsiEncoding; passthrough would mangle it.
        let (doc, p_id) = doc_with_font(font, None, b"BT /F1 12 Tf (\x96) Tj ET");
        let result = extract_text_from_page_with_warnings(&doc, p_id);
        assert!(result.text.contains('\u{2013}'), "got: {:?}", result.text);
    }

    #[test]
    fn simple_helvetica_passthrough_unchanged() {
        // Standard-14 font, no ToUnicode: ASCII passthrough, classified Reliable.
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let (doc, p_id) = doc_with_font(font, None, b"BT /F1 12 Tf (Hello) Tj ET");
        let result = extract_text_from_page_with_warnings(&doc, p_id);
        assert_eq!(result.text, "Hello");
        let fonts = dedup_font_records(result.fonts);
        assert_eq!(
            document_verdict(&fonts, result.total_codes, result.unmapped_codes),
            Reliability::Reliable
        );
    }

    #[test]
    fn cid_without_tounicode_is_unreliable() {
        let (doc, p_id) = doc_with_font(type0_font(), None, b"BT /F1 12 Tf <0041> Tj ET");
        let result = extract_text_from_page_with_warnings(&doc, p_id);
        let rec = result
            .fonts
            .iter()
            .find(|f| f.name == "/F1")
            .expect("F1 record");
        assert_eq!(rec.classification, Reliability::Unreliable);
        assert!(rec.reason.contains("CID"), "reason: {}", rec.reason);
    }

    #[test]
    fn no_tf_falls_back_to_passthrough() {
        // No font resources, no Tf: behaves exactly as before.
        let mut doc = Document::new();
        let c = Stream::new(Dictionary::new(), b"BT (Plain) Tj ET".to_vec());
        let c_id = doc.add_object(Object::Stream(c));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let result = extract_text_from_page_with_warnings(&doc, p_id);
        assert_eq!(result.text, "Plain");
        assert_eq!(result.total_codes, 0);
    }

    #[test]
    fn low_coverage_downgrades_verdict_to_degraded() {
        // ToUnicode present (font is statically Reliable) but most codes used
        // are unmapped -> document verdict drops to Degraded.
        let cmap = b"begincodespacerange <0000> <FFFF> endcodespacerange \
                     beginbfchar <0041> <0041> endbfchar";
        let (doc, p_id) = doc_with_font(
            type0_font(),
            Some(cmap),
            b"BT /F1 12 Tf <00410099009800970096> Tj ET",
        );
        let result = extract_text_from_page_with_warnings(&doc, p_id);
        let fonts = dedup_font_records(result.fonts);
        assert!(result.unmapped_codes * 5 > result.total_codes); // >20%
        assert_eq!(
            document_verdict(&fonts, result.total_codes, result.unmapped_codes),
            Reliability::Degraded
        );
    }

    #[test]
    fn json_includes_reliability_object() {
        let (doc, _p_id) = doc_with_font(type0_font(), None, b"BT /F1 12 Tf <0041> Tj ET");
        let (value, had_issues) = text_json_value(&doc, None);
        assert_eq!(value["reliability"]["verdict"], "unreliable");
        assert!(value["reliability"]["fonts"].is_array());
        assert!(had_issues, "unreliable extraction should flag had_issues");
    }
}
