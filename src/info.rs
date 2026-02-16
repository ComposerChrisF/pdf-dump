use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::types::DumpConfig;
use crate::helpers::{format_dict_value, format_filter, format_color_space};
use crate::object::{object_to_json, print_object, deref_summary};
use crate::refs::{collect_reverse_refs, reverse_refs_to_json, collect_forward_refs_json, collect_refs_with_paths};

pub(crate) fn classify_object(doc: &Document, obj_num: u32, object: &Object, pages: &BTreeMap<u32, ObjectId>) -> (String, String, Vec<(String, String)>) {
    let dict = match object {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    };

    if let Some(dict) = dict {
        let type_name = dict.get_type().ok().map(|t| String::from_utf8_lossy(t).into_owned());
        let subtype = dict.get(b"Subtype").ok()
            .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));

        match type_name.as_deref() {
            Some("Catalog") => {
                let page_count = dict.get(b"Pages").ok()
                    .and_then(|p| p.as_reference().ok())
                    .and_then(|r| doc.get_object(r).ok())
                    .and_then(|o| match o {
                        Object::Dictionary(d) => d.get(b"Count").ok().and_then(|c| c.as_i64().ok()),
                        _ => None,
                    });
                let mut details = Vec::new();
                if let Some(count) = page_count {
                    details.push(("Pages".to_string(), count.to_string()));
                }
                return ("Catalog".to_string(), format!("Object {} is the document catalog (root object).", obj_num), details);
            }
            Some("Pages") => {
                let count = dict.get(b"Count").ok().and_then(|c| c.as_i64().ok());
                let mut details = Vec::new();
                if let Some(c) = count {
                    details.push(("Count".to_string(), c.to_string()));
                }
                return ("Page Tree".to_string(), format!("Object {} is a page tree node.", obj_num), details);
            }
            Some("Page") => {
                let page_num = pages.iter().find(|(_, id)| **id == (obj_num, 0)).map(|(n, _)| *n);
                let mut details = Vec::new();
                if let Some(n) = page_num {
                    details.push(("Page Number".to_string(), n.to_string()));
                }
                if let Ok(mb) = dict.get(b"MediaBox") {
                    details.push(("MediaBox".to_string(), format_dict_value(mb)));
                }
                let desc = if let Some(n) = page_num {
                    format!("Object {} is page {}.", obj_num, n)
                } else {
                    format!("Object {} is a page.", obj_num)
                };
                return ("Page".to_string(), desc, details);
            }
            Some("Font") => {
                return classify_font(doc, obj_num, dict);
            }
            Some("Annot") => {
                let sub = subtype.as_deref().unwrap_or("unknown");
                let mut details = vec![("Subtype".to_string(), sub.to_string())];
                if let Ok(rect) = dict.get(b"Rect") {
                    details.push(("Rect".to_string(), format_dict_value(rect)));
                }
                return ("Annotation".to_string(), format!("Object {} is a {} annotation.", obj_num, sub), details);
            }
            Some("Action") => {
                let action_type = dict.get(b"S").ok()
                    .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                    .unwrap_or_else(|| "unknown".to_string());
                let details = vec![("Action Type".to_string(), action_type.clone())];
                return ("Action".to_string(), format!("Object {} is a {} action.", obj_num, action_type), details);
            }
            Some("FontDescriptor") => {
                let font_name = dict.get(b"FontName").ok()
                    .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                    .unwrap_or_else(|| "-".to_string());
                let mut details = vec![("FontName".to_string(), font_name.clone())];
                for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                    if let Ok(v) = dict.get(key) {
                        details.push(("Embedded".to_string(), format!("{} ({})", format_dict_value(v), String::from_utf8_lossy(key))));
                    }
                }
                return ("Font Descriptor".to_string(), format!("Object {} is a font descriptor for {}.", obj_num, font_name), details);
            }
            Some("Encoding") => {
                let base = dict.get(b"BaseEncoding").ok()
                    .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                    .unwrap_or_else(|| "-".to_string());
                let has_diffs = dict.get(b"Differences").is_ok();
                let mut details = vec![("BaseEncoding".to_string(), base.clone())];
                if has_diffs { details.push(("Differences".to_string(), "yes".to_string())); }
                return ("Encoding".to_string(), format!("Object {} is a font encoding ({}).", obj_num, base), details);
            }
            Some("ExtGState") => {
                let keys: Vec<String> = dict.iter()
                    .map(|(k, _)| format!("/{}", String::from_utf8_lossy(k)))
                    .collect();
                let details = vec![("Keys".to_string(), keys.join(", "))];
                return ("Graphics State".to_string(), format!("Object {} is an extended graphics state.", obj_num), details);
            }
            Some("XRef") => {
                return ("XRef Stream".to_string(), format!("Object {} is a cross-reference stream.", obj_num), vec![]);
            }
            Some("ObjStm") => {
                return ("Object Stream".to_string(), format!("Object {} is an object stream.", obj_num), vec![]);
            }
            _ => {}
        }

        // Check subtype for things without /Type
        if let Some(ref sub) = subtype {
            match sub.as_str() {
                "Image" => {
                    return classify_image(obj_num, dict, doc, object);
                }
                "Form" => {
                    let mut details = Vec::new();
                    if let Ok(bbox) = dict.get(b"BBox") {
                        details.push(("BBox".to_string(), format_dict_value(bbox)));
                    }
                    return ("Form XObject".to_string(), format!("Object {} is a form XObject.", obj_num), details);
                }
                "Type1" | "TrueType" | "Type0" | "CIDFontType0" | "CIDFontType2" | "MMType1" | "Type3" => {
                    return classify_font(doc, obj_num, dict);
                }
                _ => {}
            }
        }

        // Generic dictionary/stream
        let kind = if matches!(object, Object::Stream(_)) { "stream" } else { "dictionary" };
        let key_count = dict.len();
        let mut details = vec![("Keys".to_string(), key_count.to_string())];
        if let Some(ref t) = type_name {
            details.insert(0, ("Type".to_string(), t.clone()));
        }
        if let Some(ref s) = subtype {
            details.insert(if type_name.is_some() { 1 } else { 0 }, ("Subtype".to_string(), s.clone()));
        }
        if let Object::Stream(stream) = object {
            details.push(("Stream Size".to_string(), format!("{} bytes", stream.content.len())));
        }
        return ("Generic".to_string(), format!("Object {} is a {} with {} keys.", obj_num, kind, key_count), details);
    }

    // Primitive types
    let (role, desc) = match object {
        Object::Integer(i) => ("Integer".to_string(), format!("Object {} is an integer: {}.", obj_num, i)),
        Object::Real(r) => ("Real".to_string(), format!("Object {} is a real number: {}.", obj_num, r)),
        Object::Boolean(b) => ("Boolean".to_string(), format!("Object {} is a boolean: {}.", obj_num, b)),
        Object::String(bytes, _) => ("String".to_string(), format!("Object {} is a string: ({}).", obj_num, String::from_utf8_lossy(bytes))),
        Object::Name(n) => ("Name".to_string(), format!("Object {} is a name: /{}.", obj_num, String::from_utf8_lossy(n))),
        Object::Array(arr) => ("Array".to_string(), format!("Object {} is an array with {} items.", obj_num, arr.len())),
        Object::Null => ("Null".to_string(), format!("Object {} is null.", obj_num)),
        Object::Reference(id) => ("Reference".to_string(), format!("Object {} is a reference to {} {} R.", obj_num, id.0, id.1)),
        _ => ("Unknown".to_string(), format!("Object {} has an unknown type.", obj_num)),
    };
    (role, desc, vec![])
}

fn classify_font(doc: &Document, obj_num: u32, dict: &lopdf::Dictionary) -> (String, String, Vec<(String, String)>) {
    let base_font = dict.get(b"BaseFont").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
        .unwrap_or_else(|| "-".to_string());
    let subtype = dict.get(b"Subtype").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
        .unwrap_or_else(|| "-".to_string());
    let encoding = dict.get(b"Encoding").ok()
        .map(|v| match v {
            Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
            Object::Reference(id) => format!("{} {} R", id.0, id.1),
            _ => format_dict_value(v),
        })
        .unwrap_or_else(|| "-".to_string());

    let embedded = if let Ok(desc_val) = dict.get(b"FontDescriptor") {
        match desc_val {
            Object::Reference(desc_ref) => {
                if let Ok(desc_obj) = doc.get_object(*desc_ref) {
                    let desc_dict = match desc_obj {
                        Object::Dictionary(d) => Some(d),
                        Object::Stream(s) => Some(&s.dict),
                        _ => None,
                    };
                    if let Some(dd) = desc_dict {
                        if let Some(key) = [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"]
                            .iter()
                            .find(|k| dd.get(k).is_ok())
                        {
                            format!("embedded ({})", String::from_utf8_lossy(key))
                        } else {
                            "not embedded".to_string()
                        }
                    } else {
                        format!("FontDescriptor at {} {} R", desc_ref.0, desc_ref.1)
                    }
                } else {
                    format!("FontDescriptor at {} {} R (unresolvable)", desc_ref.0, desc_ref.1)
                }
            }
            Object::Dictionary(_) => "has FontDescriptor (inline)".to_string(),
            _ => "has FontDescriptor".to_string(),
        }
    } else {
        "no FontDescriptor".to_string()
    };

    let has_tounicode = dict.get(b"ToUnicode").is_ok();

    let mut details = vec![
        ("BaseFont".to_string(), base_font.clone()),
        ("Subtype".to_string(), subtype.clone()),
        ("Encoding".to_string(), encoding),
        ("FontDescriptor".to_string(), embedded),
        ("ToUnicode".to_string(), if has_tounicode { "yes" } else { "no" }.to_string()),
    ];

    if let Ok(fc) = dict.get(b"FirstChar").and_then(|v| v.as_i64()) {
        details.push(("FirstChar".to_string(), fc.to_string()));
    }
    if let Ok(lc) = dict.get(b"LastChar").and_then(|v| v.as_i64()) {
        details.push(("LastChar".to_string(), lc.to_string()));
    }

    let desc = format!("Object {} is a {} font ({}).", obj_num, subtype, base_font);
    ("Font".to_string(), desc, details)
}

fn classify_image(obj_num: u32, dict: &lopdf::Dictionary, doc: &Document, object: &Object) -> (String, String, Vec<(String, String)>) {
    let width = dict.get(b"Width").ok().and_then(|v| v.as_i64().ok());
    let height = dict.get(b"Height").ok().and_then(|v| v.as_i64().ok());
    let bpc = dict.get(b"BitsPerComponent").ok().and_then(|v| v.as_i64().ok());
    let cs = dict.get(b"ColorSpace").ok().map(|v| format_color_space(v, doc));
    let filter = dict.get(b"Filter").ok().map(format_filter);
    let stream_size = if let Object::Stream(s) = object { Some(s.content.len()) } else { None };

    let mut details = Vec::new();
    let mut dim_parts = Vec::new();
    if let Some(w) = width {
        details.push(("Width".to_string(), w.to_string()));
        dim_parts.push(w.to_string());
    }
    if let Some(h) = height {
        details.push(("Height".to_string(), h.to_string()));
        dim_parts.push(h.to_string());
    }
    if let Some(ref c) = cs {
        details.push(("ColorSpace".to_string(), c.clone()));
    }
    if let Some(b) = bpc {
        details.push(("BitsPerComponent".to_string(), b.to_string()));
    }
    if let Some(ref f) = filter {
        details.push(("Filter".to_string(), f.clone()));
    }
    if let Some(size) = stream_size {
        details.push(("Stream Size".to_string(), format!("{} bytes", size)));
    }

    let dims = if dim_parts.len() == 2 { format!(" ({}x{})", dim_parts[0], dim_parts[1]) } else { String::new() };
    let cs_str = cs.as_deref().unwrap_or("");
    let desc = format!("Object {} is an image{}{}.", obj_num, dims,
        if !cs_str.is_empty() { format!(", {}", cs_str) } else { String::new() });
    ("Image".to_string(), desc, details)
}

fn collect_references_in_object(obj: &Object, target_id: ObjectId, path: &str) -> Vec<String> {
    let mut found = Vec::new();
    collect_references_in_object_into(obj, target_id, path, &mut found);
    found
}

fn collect_references_in_object_into(obj: &Object, target_id: ObjectId, path: &str, found: &mut Vec<String>) {
    match obj {
        Object::Reference(id) if *id == target_id => {
            found.push(path.to_string());
        }
        Object::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                collect_references_in_object_into(item, target_id, &child_path, found);
            }
        }
        Object::Dictionary(dict) => {
            for (key, value) in dict.iter() {
                let child_path = format!("{}/{}", path, String::from_utf8_lossy(key));
                collect_references_in_object_into(value, target_id, &child_path, found);
            }
        }
        Object::Stream(stream) => {
            for (key, value) in stream.dict.iter() {
                let child_path = format!("{}/{}", path, String::from_utf8_lossy(key));
                collect_references_in_object_into(value, target_id, &child_path, found);
            }
        }
        _ => {}
    }
}

pub(crate) fn find_page_associations(doc: &Document, obj_num: u32, pages: &BTreeMap<u32, ObjectId>) -> Vec<u32> {
    let target_id: ObjectId = (obj_num, 0);
    let mut result = Vec::new();

    for (&page_num, &page_id) in pages {
        if let Ok(page_obj) = doc.get_object(page_id) {
            // Check direct references in page dict
            let paths = collect_references_in_object(page_obj, target_id, "");
            if !paths.is_empty() {
                result.push(page_num);
                continue;
            }
            // Check one level into /Resources
            let dict = match page_obj {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                _ => continue,
            };
            if let Ok(Object::Reference(res_ref)) = dict.get(b"Resources")
                && let Ok(res_obj) = doc.get_object(*res_ref) {
                let res_paths = collect_references_in_object(res_obj, target_id, "");
                if !res_paths.is_empty() {
                    result.push(page_num);
                }
            }
        }
    }

    result.sort();
    result
}

pub(crate) fn print_info(writer: &mut impl Write, doc: &Document, obj_num: u32) {
    let obj_id = (obj_num, 0);
    let object = match doc.get_object(obj_id) {
        Ok(obj) => obj,
        Err(_) => {
            eprintln!("Error: Object {} not found in the document.", obj_num);
            std::process::exit(1);
        }
    };

    let pages = doc.get_pages();
    let (role, description, details) = classify_object(doc, obj_num, object, &pages);

    writeln!(writer, "{}", description).unwrap();
    writeln!(writer, "\nRole: {}", role).unwrap();
    writeln!(writer, "Kind: {}", object.enum_variant()).unwrap();

    if !details.is_empty() {
        writeln!(writer, "\nDetails:").unwrap();
        for (key, value) in &details {
            writeln!(writer, "  {}: {}", key, value).unwrap();
        }
    }

    // Page associations
    let page_assoc = find_page_associations(doc, obj_num, &pages);
    if !page_assoc.is_empty() {
        let pages_str: Vec<String> = page_assoc.iter().map(|p| p.to_string()).collect();
        writeln!(writer, "\nReferenced by pages: {}", pages_str.join(", ")).unwrap();
    }

    // Full object content
    let config = DumpConfig {
        decode_streams: false, truncate: None, json: false,
        hex: false, depth: None, deref: false, raw: false,
    };
    writeln!(writer, "\nObject {} 0:", obj_num).unwrap();
    let visited = BTreeSet::new();
    let mut child_refs = BTreeSet::new();
    print_object(writer, object, doc, &visited, 1, &config, false, &mut child_refs);
    writeln!(writer).unwrap();

    // Forward references
    let forward_refs = collect_refs_with_paths(object);
    writeln!(writer, "\nReferences from this object:").unwrap();
    if forward_refs.is_empty() {
        writeln!(writer, "  (none)").unwrap();
    } else {
        for (path, ref_id) in &forward_refs {
            let summary = if let Ok(resolved) = doc.get_object(*ref_id) {
                deref_summary(resolved, doc)
            } else {
                "(not found)".to_string()
            };
            writeln!(writer, "  {} -> {} {} R  {}", path, ref_id.0, ref_id.1, summary).unwrap();
        }
    }

    // Reverse references
    let rev_refs = collect_reverse_refs(doc, (obj_num, 0));
    writeln!(writer, "\nReferenced by:").unwrap();
    if rev_refs.is_empty() {
        writeln!(writer, "  (none)").unwrap();
    } else {
        for r in &rev_refs {
            writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} via {}", r.obj_num, r.generation, r.kind, r.type_label, r.paths.join(", ")).unwrap();
        }
    }
}

pub(crate) fn print_info_json(writer: &mut impl Write, doc: &Document, obj_num: u32) {
    let obj_id = (obj_num, 0);
    let object = match doc.get_object(obj_id) {
        Ok(obj) => obj,
        Err(_) => {
            let output = json!({
                "object_number": obj_num,
                "error": "not found",
            });
            writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
            return;
        }
    };

    let pages = doc.get_pages();
    let (role, description, details) = classify_object(doc, obj_num, object, &pages);

    let config = DumpConfig {
        decode_streams: false, truncate: None, json: true,
        hex: false, depth: None, deref: false, raw: false,
    };
    let refs_to = collect_forward_refs_json(doc, object);
    let referenced_by = reverse_refs_to_json(&collect_reverse_refs(doc, (obj_num, 0)));
    let page_assoc = find_page_associations(doc, obj_num, &pages);

    let details_map: serde_json::Map<String, Value> = details.into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect();

    let output = json!({
        "object_number": obj_num,
        "generation": 0,
        "role": role,
        "description": description,
        "kind": format!("{}", object.enum_variant()),
        "details": details_map,
        "object": object_to_json(object, doc, &config),
        "page_associations": page_assoc,
        "references": refs_to,
        "referenced_by": referenced_by,
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
    fn classify_object_catalog() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(3));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog.clone()));

        let obj = Object::Dictionary(catalog);
        let (role, desc, details) = classify_object(&doc, 1, &obj, &doc.get_pages());
        assert_eq!(role, "Catalog");
        assert!(desc.contains("document catalog"));
        assert!(details.iter().any(|(k, v)| k == "Pages" && v == "3"));
    }

    #[test]
    fn classify_object_font() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        dict.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 10, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("Type1"));
        assert!(desc.contains("Helvetica"));
        assert!(details.iter().any(|(k, v)| k == "BaseFont" && v == "Helvetica"));
        assert!(details.iter().any(|(k, v)| k == "Encoding" && v == "WinAnsiEncoding"));
    }

    #[test]
    fn classify_object_page() {
        let mut doc = Document::new();
        let mut page_tree = Dictionary::new();
        page_tree.set("Type", Object::Name(b"Pages".to_vec()));
        page_tree.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        page_tree.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(page_tree));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(page.clone()));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let obj = Object::Dictionary(page);
        let (role, desc, details) = classify_object(&doc, 3, &obj, &doc.get_pages());
        assert_eq!(role, "Page");
        assert!(desc.contains("page 1") || desc.contains("page"));
        assert!(details.iter().any(|(k, _)| k == "MediaBox"));
    }

    #[test]
    fn classify_object_image() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        dict.set("Width", Object::Integer(100));
        dict.set("Height", Object::Integer(200));
        dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        dict.set("BitsPerComponent", Object::Integer(8));
        let stream = Stream::new(dict, vec![0u8; 50]);
        let obj = Object::Stream(stream);

        let (role, desc, details) = classify_object(&doc, 7, &obj, &doc.get_pages());
        assert_eq!(role, "Image");
        assert!(desc.contains("100x200"));
        assert!(details.iter().any(|(k, v)| k == "Width" && v == "100"));
        assert!(details.iter().any(|(k, v)| k == "Height" && v == "200"));
    }

    #[test]
    fn classify_object_annotation() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Annot".to_vec()));
        dict.set("Subtype", Object::Name(b"Link".to_vec()));
        dict.set("Rect", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(100), Object::Integer(50),
        ]));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 8, &obj, &doc.get_pages());
        assert_eq!(role, "Annotation");
        assert!(desc.contains("Link"));
    }

    #[test]
    fn classify_object_action() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Action".to_vec()));
        dict.set("S", Object::Name(b"URI".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 9, &obj, &doc.get_pages());
        assert_eq!(role, "Action");
        assert!(desc.contains("URI"));
        assert!(details.iter().any(|(k, v)| k == "Action Type" && v == "URI"));
    }

    #[test]
    fn classify_object_integer() {
        let doc = Document::new();
        let obj = Object::Integer(42);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Integer");
        assert!(desc.contains("42"));
    }

    #[test]
    fn classify_object_string() {
        let doc = Document::new();
        let obj = Object::String(b"hello".to_vec(), StringFormat::Literal);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "String");
        assert!(desc.contains("hello"));
    }

    #[test]
    fn classify_object_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Array");
        assert!(desc.contains("2 items"));
    }

    #[test]
    fn classify_object_generic_dict() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Foo", Object::Integer(1));
        dict.set("Bar", Object::Integer(2));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Generic");
        assert!(desc.contains("dictionary"));
        assert!(details.iter().any(|(k, _)| k == "Keys"));
    }

    #[test]
    fn classify_object_font_descriptor() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"FontDescriptor".to_vec()));
        dict.set("FontName", Object::Name(b"Helvetica".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Font Descriptor");
        assert!(desc.contains("Helvetica"));
    }

    #[test]
    fn classify_object_encoding() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Encoding".to_vec()));
        dict.set("BaseEncoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Encoding");
        assert!(desc.contains("WinAnsiEncoding"));
    }

    #[test]
    fn classify_object_ext_gstate() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"ExtGState".to_vec()));
        dict.set("CA", Object::Real(0.5));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Graphics State");
        assert!(desc.contains("extended graphics state"));
    }

    #[test]
    fn classify_object_form_xobject() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        dict.set("BBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(100), Object::Integer(100),
        ]));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Form XObject");
        assert!(desc.contains("form XObject"));
        assert!(details.iter().any(|(k, _)| k == "BBox"));
    }

    #[test]
    fn classify_object_pages_tree() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Pages".to_vec()));
        dict.set("Count", Object::Integer(5));
        let obj = Object::Dictionary(dict);

        let (role, _, details) = classify_object(&doc, 2, &obj, &doc.get_pages());
        assert_eq!(role, "Page Tree");
        assert!(details.iter().any(|(k, v)| k == "Count" && v == "5"));
    }

    #[test]
    fn find_page_associations_finds_direct_ref() {
        let mut doc = Document::new();

        // Font object
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        // Page that references the font
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        page.set("Resources", Object::Dictionary(res));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        // Page tree
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let assoc = find_page_associations(&doc, 5, &doc.get_pages());
        assert_eq!(assoc, vec![1]);
    }

    #[test]
    fn find_page_associations_no_association() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let assoc = find_page_associations(&doc, 5, &doc.get_pages());
        assert!(assoc.is_empty());
    }

    #[test]
    fn print_info_shows_role_and_details() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("Type1 font (Courier)"));
        assert!(out.contains("Role: Font"));
        assert!(out.contains("Kind: Dictionary"));
        assert!(out.contains("BaseFont: Courier"));
        // Now includes full object content
        assert!(out.contains("Object 5 0:"));
    }

    #[test]
    fn print_info_shows_reverse_refs() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));
        let mut dict = Dictionary::new();
        dict.set("Value", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("Referenced by:"));
        assert!(out.contains("via /Value"));
    }

    #[test]
    fn print_info_json_valid() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        font.set("BaseFont", Object::Name(b"Arial".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font));

        let out = output_of(|w| print_info_json(w, &doc, 10));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["object_number"], 10);
        assert_eq!(val["role"], "Font");
        assert!(val["description"].as_str().unwrap().contains("Arial"));
        assert!(val["details"].is_object());
        assert!(val["object"].is_object()); // now includes full object content
        assert!(val["page_associations"].is_array());
        assert!(val["references"].is_array());
        assert!(val["referenced_by"].is_array());
    }

    #[test]
    fn print_info_json_with_page_associations() {
        let mut doc = Document::new();

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        page.set("Resources", Object::Dictionary(res));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_info_json(w, &doc, 5));
        let val: Value = serde_json::from_str(&out).unwrap();
        let pages = val["page_associations"].as_array().unwrap();
        assert_eq!(pages, &[1]);
    }

    #[test]
    fn print_info_integer_object() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("integer: 42"));
        assert!(out.contains("Role: Integer"));
    }

    #[test]
    fn print_info_json_integer_object() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));

        let out = output_of(|w| print_info_json(w, &doc, 5));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["role"], "Integer");
        assert_eq!(val["object_number"], 5);
    }

    #[test]
    fn classify_object_xref_stream() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XRef".to_vec()));
        let obj = Object::Dictionary(dict);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "XRef Stream");
        assert!(desc.contains("cross-reference stream"));
    }

    #[test]
    fn classify_object_obj_stream() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"ObjStm".to_vec()));
        let obj = Object::Dictionary(dict);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Object Stream");
        assert!(desc.contains("object stream"));
    }

    #[test]
    fn classify_object_null() {
        let doc = Document::new();
        let obj = Object::Null;
        let (role, _, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Null");
    }

    #[test]
    fn classify_object_boolean() {
        let doc = Document::new();
        let obj = Object::Boolean(true);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Boolean");
        assert!(desc.contains("true"));
    }

    #[test]
    fn classify_object_real() {
        let doc = Document::new();
        let obj = Object::Real(2.72);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Real");
        assert!(desc.contains("2.72"));
    }

    #[test]
    fn classify_object_name() {
        let doc = Document::new();
        let obj = Object::Name(b"Test".to_vec());
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Name");
        assert!(desc.contains("Test"));
    }

    #[test]
    fn classify_font_by_subtype_only() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"TrueType".to_vec()));
        dict.set("BaseFont", Object::Name(b"TimesNewRoman".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("TrueType"));
        assert!(desc.contains("TimesNewRoman"));
    }

    #[test]
    fn print_info_shows_forward_refs() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(dict));
        doc.objects.insert((10, 0), Object::Integer(99));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("References from this object:"));
        assert!(out.contains("/Font -> 10 0 R"));
    }

    #[test]
    fn print_info_page_associations_text() {
        let mut doc = Document::new();

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        page.set("Resources", Object::Dictionary(res));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("Referenced by pages: 1"));
    }

    #[test]
    fn find_page_associations_via_resource_ref() {
        let mut doc = Document::new();

        // Font object
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        // Resources dict as separate object with ref to font
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        doc.objects.insert((4, 0), Object::Dictionary(res));

        // Page referencing resources by ref
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("Resources", Object::Reference((4, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let assoc = find_page_associations(&doc, 5, &doc.get_pages());
        assert_eq!(assoc, vec![1]);
    }

}
