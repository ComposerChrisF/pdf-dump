use lopdf::{Document, Object};
use serde_json::{Value, json};
use std::io::Write;

use std::collections::BTreeMap;

use crate::bookmarks::count_bookmarks;
use crate::embedded::collect_embedded_files;
use crate::forms::collect_form_fields;
use crate::helpers::{json_pretty, object_type_label};
use crate::layers::collect_layers;
use crate::page_labels::collect_page_labels;
use crate::stream::{decode_stream, get_filter_names};
use crate::structure::collect_structure_tree;
use crate::validate::validate_pdf;

pub(crate) fn print_list(writer: &mut impl Write, doc: &Document) {
    wln!(
        writer,
        "PDF {}  |  {} objects\n",
        doc.version,
        doc.objects.len()
    );
    wln!(
        writer,
        "  {:>4}  {:>3}  {:<13} {:<14} Detail",
        "Obj#",
        "Gen",
        "Kind",
        "/Type"
    );

    for (&(obj_num, generation), object) in &doc.objects {
        let kind = object.enum_variant();
        let type_label = object_type_label(object);
        let detail = summary_detail(object);
        wln!(
            writer,
            "  {:>4}  {:>3}  {:<13} {:<14} {}",
            obj_num,
            generation,
            kind,
            type_label,
            detail
        );
    }
}

pub(crate) fn summary_detail(object: &Object) -> String {
    match object {
        Object::Stream(stream) => {
            let filter = stream
                .dict
                .get(b"Filter")
                .ok()
                .and_then(|f| {
                    f.as_name()
                        .ok()
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                })
                .unwrap_or_default();
            if filter.is_empty() {
                format!("{} bytes", stream.content.len())
            } else {
                format!("{} bytes, {}", stream.content.len(), filter)
            }
        }
        Object::Dictionary(dict) => {
            let count = dict.len();
            let notable: Vec<String> = dict
                .iter()
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
                            let items: Vec<String> = arr
                                .iter()
                                .map(|o| match o {
                                    Object::Integer(i) => i.to_string(),
                                    Object::Real(r) => r.to_string(),
                                    _ => "?".to_string(),
                                })
                                .collect();
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

pub(crate) fn list_json_value(doc: &Document) -> Value {
    let objects: Vec<Value> = doc
        .objects
        .iter()
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
    json!({
        "version": doc.version,
        "object_count": doc.objects.len(),
        "objects": objects,
    })
}

#[cfg(test)]
pub(crate) fn print_list_json(writer: &mut impl Write, doc: &Document) {
    let output = list_json_value(doc);
    writeln!(writer, "{}", json_pretty(&output)).unwrap();
}

pub(crate) fn metadata_info(
    doc: &Document,
) -> (
    serde_json::Map<String, Value>,
    serde_json::Map<String, Value>,
) {
    let mut info = serde_json::Map::new();
    let mut catalog = serde_json::Map::new();

    if let Ok(info_ref) = doc.trailer.get(b"Info")
        && let Ok((_, Object::Dictionary(info_dict))) = doc.dereference(info_ref)
    {
        let fields = [
            b"Title".as_slice(),
            b"Author",
            b"Subject",
            b"Keywords",
            b"Creator",
            b"Producer",
            b"CreationDate",
            b"ModDate",
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

    if let Some(root_ref) = doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|o| o.as_reference().ok())
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

// ── Shared helpers for overview ──────────────────────────────────────

fn is_encrypted(doc: &Document) -> bool {
    doc.encryption_state.is_some()
}

fn count_object_types(doc: &Document) -> BTreeMap<&str, usize> {
    let mut type_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for object in doc.objects.values() {
        let kind = match object {
            Object::Boolean(_) => "booleans",
            Object::Integer(_) => "integers",
            Object::Real(_) => "reals",
            Object::Name(_) => "names",
            Object::String(_, _) => "strings",
            Object::Array(_) => "arrays",
            Object::Dictionary(_) => "dictionaries",
            Object::Stream(_) => "streams",
            Object::Reference(_) => "references",
            Object::Null => "nulls",
        };
        *type_counts.entry(kind).or_insert(0) += 1;
    }
    type_counts
}

pub(crate) struct StreamStats {
    pub count: usize,
    pub total_bytes: usize,
    pub total_decoded_bytes: Option<usize>,
    pub filter_counts: BTreeMap<String, usize>,
    pub largest: Vec<(u32, usize)>,
}

fn collect_stream_stats(doc: &Document, decode: bool) -> StreamStats {
    let mut count = 0usize;
    let mut total_bytes = 0usize;
    let mut decoded_total: usize = 0;
    let mut filter_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut largest: Vec<(u32, usize)> = Vec::new();

    for (&(obj_num, _), object) in &doc.objects {
        if let Object::Stream(stream) = object {
            count += 1;
            let raw_bytes = stream.content.len();
            total_bytes += raw_bytes;

            if decode {
                let (decoded, _) = decode_stream(stream);
                decoded_total += decoded.len();
            }

            for filter in get_filter_names(stream) {
                let name = String::from_utf8_lossy(filter).into_owned();
                *filter_counts.entry(name).or_insert(0) += 1;
            }

            largest.push((obj_num, raw_bytes));
        }
    }
    largest.sort_by_key(|b| std::cmp::Reverse(b.1));
    largest.truncate(3);

    StreamStats {
        count,
        total_bytes,
        total_decoded_bytes: decode.then_some(decoded_total),
        filter_counts,
        largest,
    }
}

// ── Overview (default mode) ──────────────────────────────────────────

pub(crate) fn print_overview(writer: &mut impl Write, doc: &Document, decode: bool) {
    let pages = doc.get_pages();

    // Basic counts
    wln!(writer, "PDF Version: {}", doc.version);
    wln!(writer, "Objects:     {}", doc.objects.len());
    wln!(writer, "Pages:       {}", pages.len());

    let encrypted = is_encrypted(doc);
    wln!(
        writer,
        "Encrypted:   {}",
        if encrypted { "yes" } else { "no" }
    );

    // /Info fields and catalog properties
    let (info, catalog) = metadata_info(doc);
    for (key, value) in &info {
        if let Some(s) = value.as_str() {
            wln!(writer, "{:<13}{}", format!("{}:", key), s);
        }
    }
    for (key, value) in &catalog {
        if let Some(s) = value.as_str() {
            wln!(writer, "{:<13}{}", format!("{}:", key), s);
        }
    }

    // Validation summary
    let report = validate_pdf(doc, Some(&pages));
    wln!(writer);
    if report.issues.is_empty() {
        wln!(writer, "Validation:  no issues found");
    } else {
        wln!(
            writer,
            "Validation:  {} errors, {} warnings, {} info",
            report.error_count,
            report.warn_count,
            report.info_count
        );
        for issue in &report.issues {
            wln!(
                writer,
                "  {} {}",
                issue.level.bracket_label(),
                issue.message
            );
        }
    }

    // Object type breakdown
    let type_counts = count_object_types(doc);
    let type_parts: Vec<String> = type_counts
        .iter()
        .map(|(kind, count)| format!("{} {}", count, kind))
        .collect();
    if !type_parts.is_empty() {
        wln!(writer, "Types:       {}", type_parts.join(", "));
    }

    // Stream stats
    let stats = collect_stream_stats(doc, decode);

    wln!(writer);
    if let Some(decoded) = stats.total_decoded_bytes
        && decoded != stats.total_bytes
    {
        wln!(
            writer,
            "Streams:     {} ({} bytes raw, {} decoded)",
            stats.count,
            stats.total_bytes,
            decoded
        );
    } else {
        wln!(
            writer,
            "Streams:     {} ({} bytes)",
            stats.count,
            stats.total_bytes
        );
    }
    if !stats.filter_counts.is_empty() {
        let mut sorted_filters: Vec<_> = stats.filter_counts.iter().collect();
        sorted_filters.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let filter_parts: Vec<String> = sorted_filters
            .iter()
            .map(|(name, count)| format!("{} \u{00d7}{}", name, count))
            .collect();
        wln!(writer, "  Filters:   {}", filter_parts.join(", "));
    }
    if !stats.largest.is_empty() && stats.count > 1 {
        let parts: Vec<String> = stats
            .largest
            .iter()
            .map(|(num, bytes)| format!("#{} ({} B)", num, bytes))
            .collect();
        wln!(writer, "  Largest:   {}", parts.join(", "));
    }

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
        wln!(writer, "Features:    {}", features.join(", "));
    }

    wln!(writer, "\nTip: Use --json for machine-readable output.");
}

pub(crate) fn print_overview_json(writer: &mut impl Write, doc: &Document, decode: bool) {
    let pages = doc.get_pages();
    let (info, catalog) = metadata_info(doc);
    let report = validate_pdf(doc, Some(&pages));

    let issues: Vec<Value> = report
        .issues
        .iter()
        .map(|i| {
            json!({
                "level": i.level.label(),
                "message": i.message,
            })
        })
        .collect();

    // Object type breakdown
    let type_counts = count_object_types(doc);
    let type_counts_json: serde_json::Map<String, Value> = type_counts
        .iter()
        .map(|(kind, count)| (kind.to_string(), json!(count)))
        .collect();

    // Stream stats
    let stats = collect_stream_stats(doc, decode);

    let filter_counts_json: serde_json::Map<String, Value> = stats
        .filter_counts
        .iter()
        .map(|(name, count)| (name.clone(), json!(count)))
        .collect();
    let largest_json: Vec<Value> = stats
        .largest
        .iter()
        .map(|(num, bytes)| json!({"object_number": num, "bytes": bytes}))
        .collect();

    let encrypted = is_encrypted(doc);

    let bookmark_count = count_bookmarks(doc);
    let (_, _, form_fields) = collect_form_fields(doc);
    let layer_count = collect_layers(doc).len();
    let embedded_count = collect_embedded_files(doc).len();
    let has_page_labels = !collect_page_labels(doc).is_empty();
    let (has_tags, _) = collect_structure_tree(doc);

    let output = json!({
        "version": doc.version,
        "object_count": doc.objects.len(),
        "page_count": pages.len(),
        "encrypted": encrypted,
        "info": info,
        "catalog": catalog,
        "object_types": type_counts_json,
        "validation": {
            "error_count": report.error_count,
            "warning_count": report.warn_count,
            "info_count": report.info_count,
            "issues": issues,
        },
        "streams": {
            "count": stats.count,
            "total_bytes": stats.total_bytes,
            "total_decoded_bytes": stats.total_decoded_bytes,
            "filters": filter_counts_json,
            "largest": largest_json,
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
    wln!(writer, "{}", json_pretty(&output));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    #[test]
    fn print_list_shows_version_and_count() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let out = output_of(|w| {
            print_list(w, &doc);
        });
        assert!(out.contains("PDF 1.4"));
        assert!(out.contains("1 objects"));
        assert!(out.contains("Obj#"));
    }

    #[test]
    fn print_list_stream_shows_bytes_and_filter() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 100]);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let out = output_of(|w| {
            print_list(w, &doc);
        });
        assert!(out.contains("100 bytes"));
        assert!(out.contains("FlateDecode"));
        assert!(out.contains("Stream"));
    }

    #[test]
    fn print_list_dict_shows_type_label() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let out = output_of(|w| {
            print_list(w, &doc);
        });
        assert!(out.contains("Page"));
        assert!(out.contains("Dictionary"));
    }

    #[test]
    fn print_list_multiple_objects_sorted() {
        let mut doc = Document::new();
        doc.objects.insert((3, 0), Object::Integer(30));
        doc.objects.insert((1, 0), Object::Integer(10));
        doc.objects.insert((2, 0), Object::Integer(20));
        let out = output_of(|w| {
            print_list(w, &doc);
        });
        assert!(out.contains("3 objects"));
        // All three should appear
        let pos1 = out.find("     1").unwrap();
        let pos2 = out.find("     2").unwrap();
        let pos3 = out.find("     3").unwrap();
        assert!(
            pos1 < pos2 && pos2 < pos3,
            "Objects should be in sorted order"
        );
    }

    #[test]
    fn print_list_json_produces_valid_json() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let out = output_of(|w| print_list_json(w, &doc));
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
        assert!(
            detail.contains("2 keys"),
            "Dict with no notable keys should show key count, got: {}",
            detail
        );
    }

    #[test]
    fn summary_detail_dict_with_mediabox() {
        let mut dict = Dictionary::new();
        dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("MediaBox"), "Should show MediaBox");
        assert!(
            detail.contains("[0 0 612 792]"),
            "Should show array values, got: {}",
            detail
        );
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
        assert!(
            detail.contains("/BaseFont=..."),
            "Non-name/array notable should show '...', got: {}",
            detail
        );
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
        dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(595.28),
                Object::Real(841.89),
            ]),
        );
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(
            detail.contains("595.28"),
            "Should format Real values, got: {}",
            detail
        );
    }

    #[test]
    fn summary_detail_mediabox_with_mixed_types() {
        // Array item that's neither Integer nor Real → "?"
        let mut dict = Dictionary::new();
        dict.set(
            "MediaBox",
            Object::Array(vec![Object::Integer(0), Object::Name(b"Unknown".to_vec())]),
        );
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(
            detail.contains("?"),
            "Non-numeric array items should show '?', got: {}",
            detail
        );
    }

    #[test]
    fn metadata_info_with_info_and_catalog() {
        let mut doc = Document::new();
        let mut info = Dictionary::new();
        info.set(
            "Title",
            Object::String(b"Test".to_vec(), StringFormat::Literal),
        );
        let info_id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(
            "Lang",
            Object::String(b"en-US".to_vec(), StringFormat::Literal),
        );
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
    fn print_list_json_includes_detail() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), vec![0; 50]);
        doc.objects.insert((2, 0), Object::Stream(stream));

        let out = output_of(|w| print_list_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        let objects = parsed["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 2);
        // Check that detail field is populated
        assert!(objects.iter().any(|o| o["type"] == "Font"));
    }

    fn build_minimal_encrypted_doc() -> Document {
        let mut doc = Document::new();
        // Minimal valid structure
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((1, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((2, 0)));

        // Simulate post-decryption state via encryption_state
        doc.encryption_state = Some(lopdf::encryption::EncryptionState::default());

        doc
    }

    #[test]
    fn overview_shows_encrypted_via_encryption_state() {
        let doc = build_minimal_encrypted_doc();
        let out = output_of(|w| print_overview(w, &doc, false));
        assert!(
            out.contains("Encrypted:   yes"),
            "Should detect encryption via encryption_state, got: {}",
            out
        );
    }

    #[test]
    fn overview_json_shows_encrypted_via_encryption_state() {
        let doc = build_minimal_encrypted_doc();
        let out = output_of(|w| print_overview_json(w, &doc, false));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(
            parsed["encrypted"], true,
            "JSON should show encrypted: true via encryption_state"
        );
    }

    #[test]
    fn overview_shows_not_encrypted_when_no_encrypt() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((1, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((2, 0)));

        let out = output_of(|w| print_overview(w, &doc, false));
        assert!(
            out.contains("Encrypted:   no"),
            "Should show not encrypted, got: {}",
            out
        );
    }

    #[test]
    fn overview_shows_object_type_breakdown() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((1, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(catalog));
        doc.objects.insert((3, 0), Object::Integer(42));
        doc.trailer.set(b"Root", Object::Reference((2, 0)));

        let out = output_of(|w| print_overview(w, &doc, false));
        assert!(
            out.contains("Types:"),
            "Should show type breakdown, got: {}",
            out
        );
        assert!(out.contains("dictionaries"), "Should list dictionaries");
        assert!(out.contains("integers"), "Should list integers");
    }

    #[test]
    fn overview_shows_stream_filters() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((1, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(catalog));

        let compressed = crate::test_utils::zlib_compress(b"hello");
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        doc.objects.insert((3, 0), Object::Stream(stream));
        doc.trailer.set(b"Root", Object::Reference((2, 0)));

        let out = output_of(|w| print_overview(w, &doc, false));
        assert!(
            out.contains("Filters:"),
            "Should show filter histogram, got: {}",
            out
        );
        assert!(out.contains("FlateDecode"), "Should list FlateDecode");
    }

    #[test]
    fn overview_skips_decoded_bytes_by_default() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((1, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(catalog));

        let original = b"hello world test data for compression";
        let compressed = crate::test_utils::zlib_compress(original);
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        doc.objects.insert((3, 0), Object::Stream(stream));
        doc.trailer.set(b"Root", Object::Reference((2, 0)));

        let out = output_of(|w| print_overview(w, &doc, false));
        // Overview no longer decodes streams, so should show raw bytes only
        assert!(
            !out.contains("decoded"),
            "Overview should not decode streams by default, got: {}",
            out
        );
        assert!(out.contains("bytes"), "Should still show byte count");
    }

    #[test]
    fn overview_json_includes_object_types_and_stream_stats() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((1, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(catalog));

        let compressed = crate::test_utils::zlib_compress(b"test");
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        doc.objects.insert((3, 0), Object::Stream(stream));
        doc.trailer.set(b"Root", Object::Reference((2, 0)));

        let out = output_of(|w| print_overview_json(w, &doc, false));
        let parsed: Value = serde_json::from_str(&out).expect("Valid JSON");
        assert!(
            parsed.get("object_types").is_some(),
            "Should have object_types"
        );
        assert!(
            parsed["streams"]["total_decoded_bytes"].is_null(),
            "Decoded bytes should be null when not decoding"
        );
        assert!(
            parsed["streams"]["filters"].is_object(),
            "Should have filters"
        );
        assert!(
            parsed["streams"]["largest"].is_array(),
            "Should have largest"
        );
    }
}
