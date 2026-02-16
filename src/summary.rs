use lopdf::{Document, Object};
use serde_json::{json, Value};
use std::io::Write;

use crate::helpers::object_type_label;
use crate::validate::validate_pdf;
use crate::bookmarks::count_bookmarks;
use crate::forms::collect_form_fields;
use crate::layers::collect_layers;
use crate::embedded::collect_embedded_files;
use crate::page_labels::collect_page_labels;
use crate::structure::collect_structure_tree;

pub(crate) fn print_summary(writer: &mut impl Write, doc: &Document) {
    writeln!(writer, "PDF {}  |  {} objects\n", doc.version, doc.objects.len()).unwrap();
    writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} Detail", "Obj#", "Gen", "Kind", "/Type").unwrap();

    for (&(obj_num, generation), object) in &doc.objects {
        let kind = object.enum_variant();
        let type_label = object_type_label(object);
        let detail = summary_detail(object);
        writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} {}", obj_num, generation, kind, type_label, detail).unwrap();
    }
}

pub(crate) fn summary_detail(object: &Object) -> String {
    match object {
        Object::Stream(stream) => {
            let filter = stream.dict.get(b"Filter").ok()
                .and_then(|f| f.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                .unwrap_or_default();
            if filter.is_empty() {
                format!("{} bytes", stream.content.len())
            } else {
                format!("{} bytes, {}", stream.content.len(), filter)
            }
        }
        Object::Dictionary(dict) => {
            let count = dict.len();
            let notable: Vec<String> = dict.iter()
                .filter(|(k, _)| {
                    let k = &**k;
                    k == b"BaseFont" || k == b"Subtype" || k == b"MediaBox"
                })
                .take(3)
                .map(|(k, v)| {
                    let key = String::from_utf8_lossy(k);
                    match v {
                        Object::Name(n) => format!("/{}={}", key, String::from_utf8_lossy(n)),
                        Object::Array(arr) => {
                            let items: Vec<String> = arr.iter().map(|o| match o {
                                Object::Integer(i) => i.to_string(),
                                Object::Real(r) => r.to_string(),
                                _ => "?".to_string(),
                            }).collect();
                            format!("/{}=[{}]", key, items.join(" "))
                        }
                        _ => format!("/{}=...", key),
                    }
                })
                .collect();
            if notable.is_empty() {
                format!("{} keys", count)
            } else {
                notable.join(", ")
            }
        }
        _ => String::new(),
    }
}

pub(crate) fn print_summary_json(writer: &mut impl Write, doc: &Document) {
    let objects: Vec<Value> = doc.objects.iter()
        .map(|(&(obj_num, generation), object)| {
            json!({
                "object_number": obj_num,
                "generation": generation,
                "kind": object.enum_variant(),
                "type": object_type_label(object),
                "detail": summary_detail(object),
            })
        })
        .collect();
    let output = json!({
        "version": doc.version,
        "object_count": doc.objects.len(),
        "objects": objects,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

pub(crate) fn metadata_info(doc: &Document) -> (serde_json::Map<String, Value>, serde_json::Map<String, Value>) {
    let mut info = serde_json::Map::new();
    let mut catalog = serde_json::Map::new();

    if let Ok(info_ref) = doc.trailer.get(b"Info")
        && let Ok((_, Object::Dictionary(info_dict))) = doc.dereference(info_ref)
    {
        let fields = [
            b"Title".as_slice(), b"Author", b"Subject", b"Keywords",
            b"Creator", b"Producer", b"CreationDate", b"ModDate",
        ];
        for key in fields {
            if let Ok(Object::String(bytes, _)) = info_dict.get(key) {
                info.insert(
                    String::from_utf8_lossy(key).into_owned(),
                    json!(String::from_utf8_lossy(bytes)),
                );
            }
        }
    }

    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok())
        && let Ok(Object::Dictionary(cat)) = doc.get_object(root_ref)
    {
        for key in [b"PageLayout".as_slice(), b"PageMode", b"Lang"] {
            if let Ok(val) = cat.get(key) {
                let text = match val {
                    Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => continue,
                };
                catalog.insert(String::from_utf8_lossy(key).into_owned(), json!(text));
            }
        }
    }

    (info, catalog)
}

// ── Overview (default mode) ──────────────────────────────────────────

pub(crate) fn print_overview(writer: &mut impl Write, doc: &Document) {
    // Basic counts
    writeln!(writer, "PDF Version: {}", doc.version).unwrap();
    writeln!(writer, "Objects:     {}", doc.objects.len()).unwrap();
    writeln!(writer, "Pages:       {}", doc.get_pages().len()).unwrap();

    // Encryption status
    let encrypted = doc.trailer.get(b"Encrypt").is_ok();
    writeln!(writer, "Encrypted:   {}", if encrypted { "yes" } else { "no" }).unwrap();

    // /Info fields
    if let Ok(info_ref) = doc.trailer.get(b"Info")
        && let Ok((_, Object::Dictionary(info))) = doc.dereference(info_ref)
    {
        let fields = [
            b"Producer".as_slice(), b"Creator", b"Title", b"Author",
            b"Subject", b"Keywords", b"CreationDate", b"ModDate",
        ];
        for key in fields {
            if let Ok(Object::String(bytes, _)) = info.get(key) {
                writeln!(writer, "{:<13}{}", format!("{}:", String::from_utf8_lossy(key)), String::from_utf8_lossy(bytes)).unwrap();
            }
        }
    }

    // Catalog properties
    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok())
        && let Ok(Object::Dictionary(catalog)) = doc.get_object(root_ref)
    {
        for key in [b"PageLayout".as_slice(), b"PageMode", b"Lang"] {
            if let Ok(val) = catalog.get(key) {
                let text = match val {
                    Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => continue,
                };
                writeln!(writer, "{:<13}{}", format!("{}:", String::from_utf8_lossy(key)), text).unwrap();
            }
        }
    }

    // Validation summary
    let report = validate_pdf(doc);
    writeln!(writer).unwrap();
    if report.issues.is_empty() {
        writeln!(writer, "Validation:  no issues found").unwrap();
    } else {
        writeln!(writer, "Validation:  {} errors, {} warnings, {} info",
            report.error_count, report.warn_count, report.info_count).unwrap();
        for issue in &report.issues {
            let prefix = match issue.level {
                crate::validate::ValidationLevel::Error => "[ERROR]",
                crate::validate::ValidationLevel::Warn => "[WARN]",
                crate::validate::ValidationLevel::Info => "[INFO]",
            };
            writeln!(writer, "  {} {}", prefix, issue.message).unwrap();
        }
    }

    // Object stats summary
    let mut stream_count = 0usize;
    let mut total_stream_bytes = 0usize;
    for object in doc.objects.values() {
        if let Object::Stream(stream) = object {
            stream_count += 1;
            total_stream_bytes += stream.content.len();
        }
    }
    writeln!(writer).unwrap();
    writeln!(writer, "Streams:     {} ({} bytes)", stream_count, total_stream_bytes).unwrap();

    // Feature indicators
    let mut features = Vec::new();
    let bookmark_count = count_bookmarks(doc);
    if bookmark_count > 0 {
        features.push(format!("bookmarks ({})", bookmark_count));
    }
    let (_, _, form_fields) = collect_form_fields(doc);
    if !form_fields.is_empty() {
        features.push(format!("forms ({} fields)", form_fields.len()));
    }
    let layer_count = collect_layers(doc).len();
    if layer_count > 0 {
        features.push(format!("layers ({})", layer_count));
    }
    let embedded_count = collect_embedded_files(doc).len();
    if embedded_count > 0 {
        features.push(format!("embedded files ({})", embedded_count));
    }
    if !collect_page_labels(doc).is_empty() {
        features.push("page labels".to_string());
    }
    let (has_tags, _) = collect_structure_tree(doc);
    if has_tags {
        features.push("tagged structure".to_string());
    }
    if !features.is_empty() {
        writeln!(writer, "Features:    {}", features.join(", ")).unwrap();
    }
}

pub(crate) fn print_overview_json(writer: &mut impl Write, doc: &Document) {
    let (info, catalog) = metadata_info(doc);
    let report = validate_pdf(doc);

    let issues: Vec<Value> = report.issues.iter().map(|i| {
        json!({
            "level": match i.level {
                crate::validate::ValidationLevel::Error => "error",
                crate::validate::ValidationLevel::Warn => "warning",
                crate::validate::ValidationLevel::Info => "info",
            },
            "message": i.message,
        })
    }).collect();

    let mut stream_count = 0usize;
    let mut total_stream_bytes = 0usize;
    for object in doc.objects.values() {
        if let Object::Stream(stream) = object {
            stream_count += 1;
            total_stream_bytes += stream.content.len();
        }
    }

    let encrypted = doc.trailer.get(b"Encrypt").is_ok();

    let bookmark_count = count_bookmarks(doc);
    let (_, _, form_fields) = collect_form_fields(doc);
    let layer_count = collect_layers(doc).len();
    let embedded_count = collect_embedded_files(doc).len();
    let has_page_labels = !collect_page_labels(doc).is_empty();
    let (has_tags, _) = collect_structure_tree(doc);

    let output = json!({
        "version": doc.version,
        "object_count": doc.objects.len(),
        "page_count": doc.get_pages().len(),
        "encrypted": encrypted,
        "info": info,
        "catalog": catalog,
        "validation": {
            "error_count": report.error_count,
            "warning_count": report.warn_count,
            "info_count": report.info_count,
            "issues": issues,
        },
        "streams": {
            "count": stream_count,
            "total_bytes": total_stream_bytes,
        },
        "features": {
            "bookmark_count": bookmark_count,
            "form_field_count": form_fields.len(),
            "layer_count": layer_count,
            "embedded_file_count": embedded_count,
            "page_labels": has_page_labels,
            "tagged_structure": has_tags,
        },
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

    #[test]
    fn print_summary_shows_version_and_count() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("PDF 1.4"));
        assert!(out.contains("1 objects"));
        assert!(out.contains("Obj#"));
    }

    #[test]
    fn print_summary_stream_shows_bytes_and_filter() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 100]);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("100 bytes"));
        assert!(out.contains("FlateDecode"));
        assert!(out.contains("Stream"));
    }

    #[test]
    fn print_summary_dict_shows_type_label() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("Page"));
        assert!(out.contains("Dictionary"));
    }

    #[test]
    fn print_summary_multiple_objects_sorted() {
        let mut doc = Document::new();
        doc.objects.insert((3, 0), Object::Integer(30));
        doc.objects.insert((1, 0), Object::Integer(10));
        doc.objects.insert((2, 0), Object::Integer(20));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("3 objects"));
        // All three should appear
        let pos1 = out.find("     1").unwrap();
        let pos2 = out.find("     2").unwrap();
        let pos3 = out.find("     3").unwrap();
        assert!(pos1 < pos2 && pos2 < pos3, "Objects should be in sorted order");
    }

    #[test]
    fn print_summary_json_produces_valid_json() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let out = output_of(|w| print_summary_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["object_count"], 1);
        assert!(parsed["objects"].is_array());
    }

    #[test]
    fn summary_detail_integer() {
        assert_eq!(summary_detail(&Object::Integer(42)), "");
    }

    #[test]
    fn summary_detail_stream() {
        let stream = make_stream(None, vec![0; 100]);
        assert_eq!(summary_detail(&Object::Stream(stream)), "100 bytes");
    }

    #[test]
    fn summary_detail_stream_with_filter() {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, vec![0; 50]);
        assert!(summary_detail(&Object::Stream(stream)).contains("FlateDecode"));
    }

    #[test]
    fn summary_detail_dict_with_basefont() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        assert!(summary_detail(&Object::Dictionary(dict)).contains("Helvetica"));
    }

    #[test]
    fn summary_detail_dict_keys_only() {
        // Dict with no notable keys (BaseFont, Subtype, MediaBox) → shows "N keys"
        let mut dict = Dictionary::new();
        dict.set("Foo", Object::Integer(1));
        dict.set("Bar", Object::Integer(2));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("2 keys"), "Dict with no notable keys should show key count, got: {}", detail);
    }

    #[test]
    fn summary_detail_dict_with_mediabox() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("MediaBox"), "Should show MediaBox");
        assert!(detail.contains("[0 0 612 792]"), "Should show array values, got: {}", detail);
    }

    #[test]
    fn summary_detail_dict_with_subtype() {
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("/Subtype=Type1"));
    }

    #[test]
    fn summary_detail_dict_notable_non_name_non_array() {
        // A notable key (BaseFont) with a non-Name, non-Array value → "/BaseFont=..."
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Integer(42));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("/BaseFont=..."), "Non-name/array notable should show '...', got: {}", detail);
    }

    #[test]
    fn summary_detail_stream_no_filter() {
        let stream = make_stream(None, vec![0; 75]);
        let detail = summary_detail(&Object::Stream(stream));
        assert_eq!(detail, "75 bytes");
    }

    #[test]
    fn summary_detail_null() {
        assert_eq!(summary_detail(&Object::Null), "");
    }

    #[test]
    fn summary_detail_boolean() {
        assert_eq!(summary_detail(&Object::Boolean(true)), "");
    }

    #[test]
    fn summary_detail_mediabox_with_reals() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Array(vec![
            Object::Real(0.0), Object::Real(0.0),
            Object::Real(595.28), Object::Real(841.89),
        ]));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("595.28"), "Should format Real values, got: {}", detail);
    }

    #[test]
    fn summary_detail_mediabox_with_mixed_types() {
        // Array item that's neither Integer nor Real → "?"
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0),
            Object::Name(b"Unknown".to_vec()),
        ]));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("?"), "Non-numeric array items should show '?', got: {}", detail);
    }

    #[test]
    fn metadata_info_with_info_and_catalog() {
        let mut doc = Document::new();
        let mut info = Dictionary::new();
        info.set("Title", Object::String(b"Test".to_vec(), StringFormat::Literal));
        let info_id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Lang", Object::String(b"en-US".to_vec(), StringFormat::Literal));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let (info_map, catalog_map) = metadata_info(&doc);
        assert_eq!(info_map["Title"], "Test");
        assert_eq!(catalog_map["Lang"], "en-US");
    }

    #[test]
    fn metadata_info_empty_doc() {
        let doc = Document::new();
        let (info_map, catalog_map) = metadata_info(&doc);
        assert!(info_map.is_empty());
        assert!(catalog_map.is_empty());
    }

    #[test]
    fn metadata_info_catalog_page_layout_name() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("PageLayout", Object::Name(b"TwoColumnLeft".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let (_, catalog_map) = metadata_info(&doc);
        assert_eq!(catalog_map["PageLayout"], "/TwoColumnLeft");
    }

    #[test]
    fn print_summary_json_includes_detail() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            vec![0; 50],
        );
        doc.objects.insert((2, 0), Object::Stream(stream));

        let out = output_of(|w| print_summary_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        let objects = parsed["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 2);
        // Check that detail field is populated
        assert!(objects.iter().any(|o| o["type"] == "Font"));
    }

}
