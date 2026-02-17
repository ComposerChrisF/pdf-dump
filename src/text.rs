use lopdf::{content::Content, Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::types::PageSpec;
use crate::stream::decode_stream;
use crate::helpers;

pub(crate) struct TextResult {
    pub text: String,
    pub warnings: Vec<String>,
}

#[cfg(test)]
fn extract_text_from_page(doc: &Document, page_id: ObjectId) -> String {
    extract_text_from_page_with_warnings(doc, page_id).text
}

pub(crate) fn extract_text_from_page_with_warnings(doc: &Document, page_id: ObjectId) -> TextResult {
    let mut text = String::new();
    let mut warnings = Vec::new();

    // Get content stream(s) for the page
    let page_dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return TextResult { text, warnings },
    };

    // Check font encodings for this page
    let font_warnings = check_page_font_encodings(doc, page_dict);
    warnings.extend(font_warnings);

    let content_ids: Vec<ObjectId> = match page_dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return TextResult { text, warnings },
    };

    let mut all_bytes = Vec::new();
    let mut decode_failed = false;
    for cid in &content_ids {
        match doc.get_object(*cid) {
            Ok(Object::Stream(stream)) => {
                let (decoded, warning) = decode_stream(stream);
                if let Some(warn) = warning {
                    warnings.push(format!("Content stream {} {}: {}", cid.0, cid.1, warn));
                    decode_failed = true;
                }
                all_bytes.extend_from_slice(&decoded);
            }
            Ok(_) => {
                warnings.push(format!("Content stream {} {} is not a stream object", cid.0, cid.1));
                decode_failed = true;
            }
            Err(_) => {
                warnings.push(format!("Content stream {} {} not found", cid.0, cid.1));
            }
        }
    }

    if all_bytes.is_empty() && !content_ids.is_empty() && decode_failed {
        warnings.push("Content stream could not be decoded".to_string());
        return TextResult { text, warnings };
    }

    let operations = match Content::decode(&all_bytes) {
        Ok(content) => content.operations,
        Err(_) => {
            warnings.push("Content stream has syntax errors".to_string());
            return TextResult { text, warnings };
        }
    };

    let mut first_bt = true;
    for op in &operations {
        match op.operator.as_str() {
            "BT" => {
                if !first_bt && !text.ends_with('\n') {
                    text.push('\n');
                }
                first_bt = false;
            }
            "Td" | "TD" => {
                // Check ty (second operand) for line break — negative y means downward movement
                if op.operands.len() >= 2 {
                    if let Object::Integer(ty) = &op.operands[1] {
                        if *ty < 0 { text.push('\n'); }
                    } else if let Object::Real(ty) = &op.operands[1]
                        && *ty < 0.0 { text.push('\n');
                    }
                }
            }
            "T*" => { text.push('\n'); }
            "Tj" => {
                if let Some(Object::String(bytes, _)) = op.operands.first() {
                    text.push_str(&String::from_utf8_lossy(bytes));
                }
            }
            "TJ" => {
                if let Some(Object::Array(arr)) = op.operands.first() {
                    for item in arr {
                        match item {
                            Object::String(bytes, _) => {
                                text.push_str(&String::from_utf8_lossy(bytes));
                            }
                            Object::Integer(n) if *n < -100 => { text.push(' '); }
                            Object::Real(n) if *n < -100.0 => { text.push(' '); }
                            _ => {}
                        }
                    }
                }
            }
            "'" => {
                text.push('\n');
                if let Some(Object::String(bytes, _)) = op.operands.first() {
                    text.push_str(&String::from_utf8_lossy(bytes));
                }
            }
            "\"" => {
                text.push('\n');
                // Third operand is the string
                if let Some(Object::String(bytes, _)) = op.operands.get(2) {
                    text.push_str(&String::from_utf8_lossy(bytes));
                }
            }
            _ => {}
        }
    }

    TextResult { text, warnings }
}

/// Check whether fonts on a page have known encodings.
/// Returns warnings for fonts that lack ToUnicode maps or recognized encodings.
pub(crate) fn check_page_font_encodings(doc: &Document, page_dict: &lopdf::Dictionary) -> Vec<String> {
    let mut warnings = Vec::new();

    // Resolve /Resources (may be a reference)
    let resources = match page_dict.get(b"Resources") {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Reference(r)) => {
            match doc.get_object(*r) {
                Ok(Object::Dictionary(d)) => d,
                _ => return warnings,
            }
        }
        _ => return warnings,
    };

    // Get /Font sub-dictionary
    let font_dict = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Reference(r)) => {
            match doc.get_object(*r) {
                Ok(Object::Dictionary(d)) => d,
                _ => return warnings,
            }
        }
        _ => return warnings,
    };

    for (name, value) in font_dict.iter() {
        let font_name = String::from_utf8_lossy(name);
        let font_obj = match value {
            Object::Reference(r) => {
                match doc.get_object(*r) {
                    Ok(obj) => obj,
                    _ => continue,
                }
            }
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
                matches!(enc_str.as_ref(), "WinAnsiEncoding" | "MacRomanEncoding" | "MacExpertEncoding" | "StandardEncoding")
            }
            Ok(Object::Dictionary(_)) => true, // Encoding dict with /Differences
            Ok(Object::Reference(r)) => {
                matches!(doc.get_object(*r), Ok(Object::Dictionary(_)) | Ok(Object::Name(_)))
            }
            _ => false,
        };

        if has_known_encoding {
            continue;
        }

        // Check /Subtype — CID fonts without ToUnicode are problematic
        let subtype = dict.get(b"Subtype").ok()
            .and_then(|v| v.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();

        let base_font = dict.get(b"BaseFont").ok()
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
                "Courier", "Courier-Bold", "Courier-BoldOblique", "Courier-Oblique",
                "Helvetica", "Helvetica-Bold", "Helvetica-BoldOblique", "Helvetica-Oblique",
                "Times-Roman", "Times-Bold", "Times-BoldItalic", "Times-Italic",
                "Symbol", "ZapfDingbats",
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

pub(crate) fn print_text(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => { eprintln!("Error: {}", msg); return; }
    };

    for (pn, page_id) in &page_list {
        wln!(writer, "--- Page {} ---", pn);
        let result = extract_text_from_page_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        wln!(writer, "{}", result.text);
    }
}

pub(crate) fn text_json_value(doc: &Document, page_filter: Option<&PageSpec>) -> Value {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => { return json!({"error": msg}); }
    };

    let mut page_results = Vec::new();
    for (pn, page_id) in &page_list {
        let result = extract_text_from_page_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        let mut entry = serde_json::Map::new();
        entry.insert("page_number".to_string(), json!(pn));
        entry.insert("text".to_string(), json!(result.text));
        if !result.warnings.is_empty() {
            entry.insert("warnings".to_string(), json!(result.warnings));
        }
        page_results.push(Value::Object(entry));
    }

    json!({"pages": page_results})
}

#[cfg(test)]
pub(crate) fn print_text_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    use crate::helpers::json_pretty;
    let output = text_json_value(doc, page_filter);
    writeln!(writer, "{}", json_pretty(&output)).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::{Dictionary, Stream};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;
    use lopdf::Document;

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
        let out = output_of(|w| print_text_json(w, &doc, None));
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
        assert!(text.contains("Quoted"), "Double-quote operator should extract text, got: {:?}", text);
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
        assert!(text.contains('\n'), "TD with negative ty should produce newline");
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
        assert!(!text.contains("Base\nSuper"), "Positive ty should not produce newline, got: {:?}", text);
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
        assert!(!text.contains("Base\nSuper"), "Positive Real ty should not produce newline, got: {:?}", text);
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
        assert!(text.contains('\n'), "Td with negative Real ty should produce newline");
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
        assert!(text.contains("Hello"), "Small negative should not insert space, got: {:?}", text);
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
        assert!(between.contains('\n'), "Should have newline between BT blocks, between: {:?}", between);
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
        page.set("Contents", Object::Array(vec![
            Object::Reference(s1_id),
            Object::Reference(s2_id),
        ]));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Part1"), "Should extract text from first stream");
        assert!(text.contains("Part2"), "Should extract text from second stream");
    }

    #[test]
    fn extract_text_non_dictionary_page() {
        // Page object is not a dictionary → empty text
        let mut doc = Document::new();
        let p_id = doc.add_object(Object::Integer(42));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.is_empty(), "Non-dictionary page should return empty text");
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
        assert!(text.is_empty(), "Non-stream contents should return empty text");
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
        assert!(text.contains("Compressed"), "Should decode FlateDecode stream before extracting text, got: {:?}", text);
    }

    #[test]
    fn print_text_json_with_page_filter() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let out = output_of(|w| print_text_json(w, &doc, Some(&spec)));
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

}
