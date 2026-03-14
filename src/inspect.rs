use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::helpers::{format_color_space, format_dict_value, format_filter, json_pretty};
use crate::object::{deref_summary, object_header_label, object_to_json, print_object};
use crate::refs::{
    collect_forward_refs_json, collect_references_in_object, collect_refs_with_paths,
    collect_reverse_refs, reverse_refs_to_json,
};
use crate::types::DumpConfig;

pub(crate) struct ObjectClassification {
    pub role: String,
    pub description: String,
    pub details: Vec<(String, String)>,
}

pub(crate) fn classify_object(
    doc: &Document,
    obj_num: u32,
    object: &Object,
    pages: &BTreeMap<u32, ObjectId>,
) -> ObjectClassification {
    let dict = match object {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    };

    if let Some(dict) = dict {
        let type_name = dict
            .get_type()
            .ok()
            .map(|t| String::from_utf8_lossy(t).into_owned());
        let subtype = dict.get(b"Subtype").ok().and_then(|v| {
            v.as_name()
                .ok()
                .map(|n| String::from_utf8_lossy(n).into_owned())
        });

        match type_name.as_deref() {
            Some("Catalog") => {
                let page_count = dict
                    .get(b"Pages")
                    .ok()
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
                return ObjectClassification {
                    role: "Catalog".to_string(),
                    description: format!(
                        "Object {} is the document catalog (root object).",
                        obj_num
                    ),
                    details,
                };
            }
            Some("Pages") => {
                let count = dict.get(b"Count").ok().and_then(|c| c.as_i64().ok());
                let mut details = Vec::new();
                if let Some(c) = count {
                    details.push(("Count".to_string(), c.to_string()));
                }
                return ObjectClassification {
                    role: "Page Tree".to_string(),
                    description: format!("Object {} is a page tree node.", obj_num),
                    details,
                };
            }
            Some("Page") => {
                let page_num = pages
                    .iter()
                    .find(|(_, id)| **id == (obj_num, 0))
                    .map(|(n, _)| *n);
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
                return ObjectClassification {
                    role: "Page".to_string(),
                    description: desc,
                    details,
                };
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
                return ObjectClassification {
                    role: "Annotation".to_string(),
                    description: format!("Object {} is a {} annotation.", obj_num, sub),
                    details,
                };
            }
            Some("Action") => {
                let action_type = dict
                    .get(b"S")
                    .ok()
                    .and_then(|v| {
                        v.as_name()
                            .ok()
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                let details = vec![("Action Type".to_string(), action_type.clone())];
                return ObjectClassification {
                    role: "Action".to_string(),
                    description: format!("Object {} is a {} action.", obj_num, action_type),
                    details,
                };
            }
            Some("FontDescriptor") => {
                let font_name = dict
                    .get(b"FontName")
                    .ok()
                    .and_then(|v| {
                        v.as_name()
                            .ok()
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                    })
                    .unwrap_or_else(|| "-".to_string());
                let mut details = vec![("FontName".to_string(), font_name.clone())];
                for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                    if let Ok(v) = dict.get(key) {
                        details.push((
                            "Embedded".to_string(),
                            format!(
                                "{} ({})",
                                format_dict_value(v),
                                String::from_utf8_lossy(key)
                            ),
                        ));
                    }
                }
                return ObjectClassification {
                    role: "Font Descriptor".to_string(),
                    description: format!(
                        "Object {} is a font descriptor for {}.",
                        obj_num, font_name
                    ),
                    details,
                };
            }
            Some("Encoding") => {
                let base = dict
                    .get(b"BaseEncoding")
                    .ok()
                    .and_then(|v| {
                        v.as_name()
                            .ok()
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                    })
                    .unwrap_or_else(|| "-".to_string());
                let has_diffs = dict.get(b"Differences").is_ok();
                let mut details = vec![("BaseEncoding".to_string(), base.clone())];
                if has_diffs {
                    details.push(("Differences".to_string(), "yes".to_string()));
                }
                return ObjectClassification {
                    role: "Encoding".to_string(),
                    description: format!("Object {} is a font encoding ({}).", obj_num, base),
                    details,
                };
            }
            Some("ExtGState") => {
                let keys: Vec<String> = dict
                    .iter()
                    .map(|(k, _)| format!("/{}", String::from_utf8_lossy(k)))
                    .collect();
                let details = vec![("Keys".to_string(), keys.join(", "))];
                return ObjectClassification {
                    role: "Graphics State".to_string(),
                    description: format!("Object {} is an extended graphics state.", obj_num),
                    details,
                };
            }
            Some("XRef") => {
                return ObjectClassification {
                    role: "XRef Stream".to_string(),
                    description: format!("Object {} is a cross-reference stream.", obj_num),
                    details: vec![],
                };
            }
            Some("ObjStm") => {
                return ObjectClassification {
                    role: "Object Stream".to_string(),
                    description: format!("Object {} is an object stream.", obj_num),
                    details: vec![],
                };
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
                    return ObjectClassification {
                        role: "Form XObject".to_string(),
                        description: format!("Object {} is a form XObject.", obj_num),
                        details,
                    };
                }
                "Type1" | "TrueType" | "Type0" | "CIDFontType0" | "CIDFontType2" | "MMType1"
                | "Type3" => {
                    return classify_font(doc, obj_num, dict);
                }
                _ => {}
            }
        }

        // Generic dictionary/stream
        let kind = if matches!(object, Object::Stream(_)) {
            "stream"
        } else {
            "dictionary"
        };
        let key_count = dict.len();
        let mut details = vec![("Keys".to_string(), key_count.to_string())];
        if let Some(ref t) = type_name {
            details.insert(0, ("Type".to_string(), t.clone()));
        }
        if let Some(ref s) = subtype {
            details.insert(
                if type_name.is_some() { 1 } else { 0 },
                ("Subtype".to_string(), s.clone()),
            );
        }
        if let Object::Stream(stream) = object {
            details.push((
                "Stream Size".to_string(),
                format!("{} bytes", stream.content.len()),
            ));
        }
        return ObjectClassification {
            role: "Generic".to_string(),
            description: format!("Object {} is a {} with {} keys.", obj_num, kind, key_count),
            details,
        };
    }

    // Primitive types
    let (role, description) = match object {
        Object::Integer(i) => (
            "Integer".to_string(),
            format!("Object {} is an integer: {}.", obj_num, i),
        ),
        Object::Real(r) => (
            "Real".to_string(),
            format!("Object {} is a real number: {}.", obj_num, r),
        ),
        Object::Boolean(b) => (
            "Boolean".to_string(),
            format!("Object {} is a boolean: {}.", obj_num, b),
        ),
        Object::String(bytes, _) => (
            "String".to_string(),
            format!(
                "Object {} is a string: ({}).",
                obj_num,
                String::from_utf8_lossy(bytes)
            ),
        ),
        Object::Name(n) => (
            "Name".to_string(),
            format!(
                "Object {} is a name: /{}.",
                obj_num,
                String::from_utf8_lossy(n)
            ),
        ),
        Object::Array(arr) => (
            "Array".to_string(),
            format!("Object {} is an array with {} items.", obj_num, arr.len()),
        ),
        Object::Null => ("Null".to_string(), format!("Object {} is null.", obj_num)),
        Object::Reference(id) => (
            "Reference".to_string(),
            format!("Object {} is a reference to {} {} R.", obj_num, id.0, id.1),
        ),
        _ => (
            "Unknown".to_string(),
            format!("Object {} has an unknown type.", obj_num),
        ),
    };
    ObjectClassification {
        role,
        description,
        details: vec![],
    }
}

fn classify_font(doc: &Document, obj_num: u32, dict: &lopdf::Dictionary) -> ObjectClassification {
    let base_font = dict
        .get(b"BaseFont")
        .ok()
        .and_then(|v| {
            v.as_name()
                .ok()
                .map(|n| String::from_utf8_lossy(n).into_owned())
        })
        .unwrap_or_else(|| "-".to_string());
    let subtype = dict
        .get(b"Subtype")
        .ok()
        .and_then(|v| {
            v.as_name()
                .ok()
                .map(|n| String::from_utf8_lossy(n).into_owned())
        })
        .unwrap_or_else(|| "-".to_string());
    let encoding = dict
        .get(b"Encoding")
        .ok()
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
                    format!(
                        "FontDescriptor at {} {} R (unresolvable)",
                        desc_ref.0, desc_ref.1
                    )
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
        (
            "ToUnicode".to_string(),
            if has_tounicode { "yes" } else { "no" }.to_string(),
        ),
    ];

    if let Ok(fc) = dict.get(b"FirstChar").and_then(|v| v.as_i64()) {
        details.push(("FirstChar".to_string(), fc.to_string()));
    }
    if let Ok(lc) = dict.get(b"LastChar").and_then(|v| v.as_i64()) {
        details.push(("LastChar".to_string(), lc.to_string()));
    }

    let description = format!("Object {} is a {} font ({}).", obj_num, subtype, base_font);
    ObjectClassification {
        role: "Font".to_string(),
        description,
        details,
    }
}

fn classify_image(
    obj_num: u32,
    dict: &lopdf::Dictionary,
    doc: &Document,
    object: &Object,
) -> ObjectClassification {
    let width = dict.get(b"Width").ok().and_then(|v| v.as_i64().ok());
    let height = dict.get(b"Height").ok().and_then(|v| v.as_i64().ok());
    let bpc = dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|v| v.as_i64().ok());
    let cs = dict
        .get(b"ColorSpace")
        .ok()
        .map(|v| format_color_space(v, doc));
    let filter = dict.get(b"Filter").ok().map(format_filter);
    let stream_size = if let Object::Stream(s) = object {
        Some(s.content.len())
    } else {
        None
    };

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

    let dims = if dim_parts.len() == 2 {
        format!(" ({}x{})", dim_parts[0], dim_parts[1])
    } else {
        String::new()
    };
    let cs_str = cs.as_deref().unwrap_or("");
    let desc = format!(
        "Object {} is an image{}{}.",
        obj_num,
        dims,
        if !cs_str.is_empty() {
            format!(", {}", cs_str)
        } else {
            String::new()
        }
    );
    ObjectClassification {
        role: "Image".to_string(),
        description: desc,
        details,
    }
}

pub(crate) fn find_page_associations(
    doc: &Document,
    obj_num: u32,
    pages: &BTreeMap<u32, ObjectId>,
) -> Vec<u32> {
    let target_id: ObjectId = (obj_num, 0); // Generation 0 assumed — tool convention
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
                && let Ok(res_obj) = doc.get_object(*res_ref)
            {
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
    let obj_id = (obj_num, 0); // Generation 0 assumed — tool convention
    let object = match doc.get_object(obj_id) {
        Ok(obj) => obj,
        Err(_) => {
            wln!(writer, "Error: Object {} not found.", obj_num);
            return;
        }
    };

    let pages = doc.get_pages();
    let cls = classify_object(doc, obj_num, object, &pages);

    wln!(writer, "{}", cls.description);
    wln!(writer, "\nRole: {}", cls.role);
    wln!(writer, "Kind: {}", object.enum_variant());

    if !cls.details.is_empty() {
        wln!(writer, "\nDetails:");
        for (key, value) in &cls.details {
            wln!(writer, "  {}: {}", key, value);
        }
    }

    // Page associations
    let page_assoc = find_page_associations(doc, obj_num, &pages);
    if !page_assoc.is_empty() {
        let pages_str: Vec<String> = page_assoc.iter().map(|p| p.to_string()).collect();
        wln!(writer, "\nReferenced by pages: {}", pages_str.join(", "));
    }

    // Full object content
    let config = DumpConfig {
        decode: false,
        truncate: None,
        json: false,
        hex: false,
        depth: None,
        deref: false,
        raw: false,
    };
    wln!(
        writer,
        "\nObject {} 0 ({}):",
        obj_num,
        object_header_label(object)
    );
    let visited = BTreeSet::new();
    let mut child_refs = BTreeSet::new();
    print_object(
        writer,
        object,
        doc,
        &visited,
        1,
        &config,
        false,
        &mut child_refs,
    );
    wln!(writer);

    // Forward references
    let forward_refs = collect_refs_with_paths(object);
    wln!(writer, "\nReferences from this object:");
    if forward_refs.is_empty() {
        wln!(writer, "  (none)");
    } else {
        for (path, ref_id) in &forward_refs {
            let summary = if let Ok(resolved) = doc.get_object(*ref_id) {
                deref_summary(resolved)
            } else {
                "(not found)".to_string()
            };
            wln!(
                writer,
                "  {} -> {} {} R  {}",
                path,
                ref_id.0,
                ref_id.1,
                summary
            );
        }
    }

    // Reverse references
    let rev_refs = collect_reverse_refs(doc, (obj_num, 0));
    wln!(writer, "\nReferenced by:");
    if rev_refs.is_empty() {
        wln!(writer, "  (none)");
    } else {
        for r in &rev_refs {
            wln!(
                writer,
                "  {:>4}  {:>3}  {:<13} {:<14} via {}",
                r.obj_num,
                r.generation,
                r.kind,
                r.type_label,
                r.paths.join(", ")
            );
        }
    }
}

pub(crate) fn print_info_json(
    writer: &mut impl Write,
    doc: &Document,
    obj_num: u32,
    config: &DumpConfig,
) {
    let obj_id = (obj_num, 0); // Generation 0 assumed — tool convention
    let object = match doc.get_object(obj_id) {
        Ok(obj) => obj,
        Err(_) => {
            let output = json!({
                "object_number": obj_num,
                "error": "not found",
            });
            wln!(writer, "{}", json_pretty(&output));
            return;
        }
    };

    let pages = doc.get_pages();
    let cls = classify_object(doc, obj_num, object, &pages);

    let json_config = DumpConfig {
        decode: false,
        truncate: None,
        json: true,
        hex: false,
        depth: config.depth,
        deref: config.deref,
        raw: false,
    };
    let refs_to = collect_forward_refs_json(doc, object);
    let referenced_by = reverse_refs_to_json(&collect_reverse_refs(doc, (obj_num, 0)));
    let page_assoc = find_page_associations(doc, obj_num, &pages);

    let details_map: serde_json::Map<String, Value> = cls
        .details
        .into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect();

    let output = json!({
        "object_number": obj_num,
        "generation": 0,
        "role": cls.role,
        "description": cls.description,
        "kind": format!("{}", object.enum_variant()),
        "details": details_map,
        "object": object_to_json(object, doc, &json_config),
        "page_associations": page_assoc,
        "references": refs_to,
        "referenced_by": referenced_by,
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
    fn classify_object_catalog() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(3));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects
            .insert((1, 0), Object::Dictionary(catalog.clone()));

        let obj = Object::Dictionary(catalog);
        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 1, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 10, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("Type1"));
        assert!(desc.contains("Helvetica"));
        assert!(
            details
                .iter()
                .any(|(k, v)| k == "BaseFont" && v == "Helvetica")
        );
        assert!(
            details
                .iter()
                .any(|(k, v)| k == "Encoding" && v == "WinAnsiEncoding")
        );
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
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        doc.objects.insert((3, 0), Object::Dictionary(page.clone()));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let obj = Object::Dictionary(page);
        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 3, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 7, &obj, &doc.get_pages());
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
        dict.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(50),
            ]),
        );
        let obj = Object::Dictionary(dict);

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 8, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 9, &obj, &doc.get_pages());
        assert_eq!(role, "Action");
        assert!(desc.contains("URI"));
        assert!(
            details
                .iter()
                .any(|(k, v)| k == "Action Type" && v == "URI")
        );
    }

    #[test]
    fn classify_object_integer() {
        let doc = Document::new();
        let obj = Object::Integer(42);
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Integer");
        assert!(desc.contains("42"));
    }

    #[test]
    fn classify_object_string() {
        let doc = Document::new();
        let obj = Object::String(b"hello".to_vec(), StringFormat::Literal);
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "String");
        assert!(desc.contains("hello"));
    }

    #[test]
    fn classify_object_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Graphics State");
        assert!(desc.contains("extended graphics state"));
    }

    #[test]
    fn classify_object_form_xobject() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        dict.set(
            "BBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(100),
            ]),
        );
        let obj = Object::Dictionary(dict);

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
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

        let ObjectClassification { role, details, .. } =
            classify_object(&doc, 2, &obj, &doc.get_pages());
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
        assert!(
            out.contains("Object 5 0 (Dictionary, /Font):"),
            "got: {}",
            out
        );
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

        let config = default_config();
        let out = output_of(|w| print_info_json(w, &doc, 10, &config));
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

        let config = default_config();
        let out = output_of(|w| print_info_json(w, &doc, 5, &config));
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

        let config = default_config();
        let out = output_of(|w| print_info_json(w, &doc, 5, &config));
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
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "XRef Stream");
        assert!(desc.contains("cross-reference stream"));
    }

    #[test]
    fn classify_object_obj_stream() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"ObjStm".to_vec()));
        let obj = Object::Dictionary(dict);
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Object Stream");
        assert!(desc.contains("object stream"));
    }

    #[test]
    fn classify_object_null() {
        let doc = Document::new();
        let obj = Object::Null;
        let ObjectClassification { role, .. } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Null");
    }

    #[test]
    fn classify_object_boolean() {
        let doc = Document::new();
        let obj = Object::Boolean(true);
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Boolean");
        assert!(desc.contains("true"));
    }

    #[test]
    fn classify_object_real() {
        let doc = Document::new();
        let obj = Object::Real(2.72);
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Real");
        assert!(desc.contains("2.72"));
    }

    #[test]
    fn classify_object_name() {
        let doc = Document::new();
        let obj = Object::Name(b"Test".to_vec());
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
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

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
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

    // --- New tests below ---

    #[test]
    fn classify_object_metadata_stream() {
        // Type=Metadata falls through to Generic since there's no dedicated branch
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Metadata".to_vec()));
        dict.set("Subtype", Object::Name(b"XML".to_vec()));
        let stream = Stream::new(dict, b"<?xml version='1.0'?>".to_vec());
        let obj = Object::Stream(stream);

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 20, &obj, &doc.get_pages());
        assert_eq!(role, "Generic");
        assert!(
            desc.contains("stream"),
            "expected 'stream' in desc, got: {}",
            desc
        );
        assert!(details.iter().any(|(k, v)| k == "Type" && v == "Metadata"));
        assert!(details.iter().any(|(k, v)| k == "Subtype" && v == "XML"));
        assert!(details.iter().any(|(k, _)| k == "Stream Size"));
    }

    #[test]
    fn classify_object_embedded_font_program_cidfonttype0c() {
        // Subtype=CIDFontType0C is not a font subtype match, falls to Generic
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"CIDFontType0C".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 100]);
        let obj = Object::Stream(stream);

        let ObjectClassification { role, details, .. } =
            classify_object(&doc, 15, &obj, &doc.get_pages());
        assert_eq!(role, "Generic");
        assert!(
            details
                .iter()
                .any(|(k, v)| k == "Subtype" && v == "CIDFontType0C")
        );
    }

    #[test]
    fn classify_object_embedded_font_program_opentype() {
        // Subtype=OpenType is not a font subtype match, falls to Generic
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"OpenType".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 200]);
        let obj = Object::Stream(stream);

        let ObjectClassification { role, details, .. } =
            classify_object(&doc, 16, &obj, &doc.get_pages());
        assert_eq!(role, "Generic");
        assert!(
            details
                .iter()
                .any(|(k, v)| k == "Subtype" && v == "OpenType")
        );
    }

    #[test]
    fn classify_font_cid_font_type0() {
        // CIDFontType0 is recognized via the subtype match arm, calls classify_font
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"CIDFontType0".to_vec()));
        dict.set("BaseFont", Object::Name(b"AdobeSongStd-Light".to_vec()));
        let obj = Object::Dictionary(dict);

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 30, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("CIDFontType0"));
        assert!(desc.contains("AdobeSongStd-Light"));
        assert!(
            details
                .iter()
                .any(|(k, v)| k == "Subtype" && v == "CIDFontType0")
        );
        assert!(
            details
                .iter()
                .any(|(k, v)| k == "BaseFont" && v == "AdobeSongStd-Light")
        );
    }

    #[test]
    fn classify_font_cid_font_type2() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
        dict.set("BaseFont", Object::Name(b"MSGothic".to_vec()));
        let obj = Object::Dictionary(dict);

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 31, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("CIDFontType2"));
        assert!(desc.contains("MSGothic"));
    }

    #[test]
    fn classify_font_with_embedded_fontfile2() {
        // FontDescriptor as a reference to an object containing FontFile2
        let mut doc = Document::new();

        let mut fd = Dictionary::new();
        fd.set("Type", Object::Name(b"FontDescriptor".to_vec()));
        fd.set("FontName", Object::Name(b"ArialMT".to_vec()));
        fd.set("FontFile2", Object::Reference((50, 0)));
        doc.objects.insert((40, 0), Object::Dictionary(fd));

        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"TrueType".to_vec()));
        dict.set("BaseFont", Object::Name(b"ArialMT".to_vec()));
        dict.set("FontDescriptor", Object::Reference((40, 0)));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { role, details, .. } =
            classify_object(&doc, 35, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        let fd_detail = details.iter().find(|(k, _)| k == "FontDescriptor").unwrap();
        assert!(
            fd_detail.1.contains("embedded"),
            "expected 'embedded' in FontDescriptor detail, got: {}",
            fd_detail.1
        );
        assert!(
            fd_detail.1.contains("FontFile2"),
            "expected 'FontFile2' in FontDescriptor detail, got: {}",
            fd_detail.1
        );
    }

    #[test]
    fn classify_font_with_embedded_fontfile() {
        // FontDescriptor containing /FontFile (Type1 embedding)
        let mut doc = Document::new();

        let mut fd = Dictionary::new();
        fd.set("Type", Object::Name(b"FontDescriptor".to_vec()));
        fd.set("FontName", Object::Name(b"CourierNew".to_vec()));
        fd.set("FontFile", Object::Reference((51, 0)));
        doc.objects.insert((41, 0), Object::Dictionary(fd));

        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        dict.set("BaseFont", Object::Name(b"CourierNew".to_vec()));
        dict.set("FontDescriptor", Object::Reference((41, 0)));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 36, &obj, &doc.get_pages());
        let fd_detail = details.iter().find(|(k, _)| k == "FontDescriptor").unwrap();
        assert!(fd_detail.1.contains("embedded"), "got: {}", fd_detail.1);
        assert!(fd_detail.1.contains("FontFile"), "got: {}", fd_detail.1);
    }

    #[test]
    fn classify_font_not_embedded() {
        // FontDescriptor exists but has no FontFile/FontFile2/FontFile3
        let mut doc = Document::new();

        let mut fd = Dictionary::new();
        fd.set("Type", Object::Name(b"FontDescriptor".to_vec()));
        fd.set("FontName", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((42, 0), Object::Dictionary(fd));

        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        dict.set("FontDescriptor", Object::Reference((42, 0)));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 37, &obj, &doc.get_pages());
        let fd_detail = details.iter().find(|(k, _)| k == "FontDescriptor").unwrap();
        assert!(fd_detail.1.contains("not embedded"), "got: {}", fd_detail.1);
    }

    #[test]
    fn classify_font_with_tounicode() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type0".to_vec()));
        dict.set("BaseFont", Object::Name(b"KozMinPro-Regular".to_vec()));
        dict.set("ToUnicode", Object::Reference((60, 0)));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 38, &obj, &doc.get_pages());
        let tu = details.iter().find(|(k, _)| k == "ToUnicode").unwrap();
        assert_eq!(tu.1, "yes");
    }

    #[test]
    fn classify_font_without_tounicode() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        dict.set("BaseFont", Object::Name(b"Symbol".to_vec()));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 39, &obj, &doc.get_pages());
        let tu = details.iter().find(|(k, _)| k == "ToUnicode").unwrap();
        assert_eq!(tu.1, "no");
    }

    #[test]
    fn classify_font_with_firstchar_lastchar() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"TrueType".to_vec()));
        dict.set("BaseFont", Object::Name(b"TimesNewRoman".to_vec()));
        dict.set("FirstChar", Object::Integer(32));
        dict.set("LastChar", Object::Integer(255));
        dict.set("Widths", Object::Array(vec![Object::Integer(250); 224]));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 40, &obj, &doc.get_pages());
        assert!(
            details.iter().any(|(k, v)| k == "FirstChar" && v == "32"),
            "details: {:?}",
            details
        );
        assert!(
            details.iter().any(|(k, v)| k == "LastChar" && v == "255"),
            "details: {:?}",
            details
        );
    }

    #[test]
    fn classify_font_inline_font_descriptor() {
        // FontDescriptor as an inline dictionary (not a reference)
        let doc = Document::new();
        let mut fd_dict = Dictionary::new();
        fd_dict.set("FontName", Object::Name(b"ArialMT".to_vec()));

        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"TrueType".to_vec()));
        dict.set("BaseFont", Object::Name(b"ArialMT".to_vec()));
        dict.set("FontDescriptor", Object::Dictionary(fd_dict));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 41, &obj, &doc.get_pages());
        let fd_detail = details.iter().find(|(k, _)| k == "FontDescriptor").unwrap();
        assert!(
            fd_detail.1.contains("inline"),
            "expected 'inline' in FontDescriptor detail, got: {}",
            fd_detail.1
        );
    }

    #[test]
    fn classify_image_without_colorspace() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        dict.set("Width", Object::Integer(640));
        dict.set("Height", Object::Integer(480));
        dict.set("BitsPerComponent", Object::Integer(8));
        // No ColorSpace
        let stream = Stream::new(dict, vec![0u8; 10]);
        let obj = Object::Stream(stream);

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 50, &obj, &doc.get_pages());
        assert_eq!(role, "Image");
        assert!(desc.contains("640x480"));
        assert!(details.iter().any(|(k, v)| k == "Width" && v == "640"));
        assert!(details.iter().any(|(k, v)| k == "Height" && v == "480"));
        assert!(
            !details.iter().any(|(k, _)| k == "ColorSpace"),
            "should not have ColorSpace detail"
        );
    }

    #[test]
    fn classify_image_without_dimensions() {
        // Image with only Subtype and ColorSpace, no Width/Height
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 5]);
        let obj = Object::Stream(stream);

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 51, &obj, &doc.get_pages());
        assert_eq!(role, "Image");
        // Without both dimensions, no "WxH" in description
        assert!(
            !desc.contains('x') || !desc.contains("("),
            "unexpected dimensions in: {}",
            desc
        );
        assert!(!details.iter().any(|(k, _)| k == "Width"));
        assert!(!details.iter().any(|(k, _)| k == "Height"));
        assert!(details.iter().any(|(k, _)| k == "ColorSpace"));
    }

    #[test]
    fn classify_image_with_only_width() {
        // Image with Width but no Height — partial dimensions
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        dict.set("Width", Object::Integer(320));
        let stream = Stream::new(dict, vec![0u8; 5]);
        let obj = Object::Stream(stream);

        let ObjectClassification { role, details, .. } =
            classify_object(&doc, 52, &obj, &doc.get_pages());
        assert_eq!(role, "Image");
        assert!(details.iter().any(|(k, v)| k == "Width" && v == "320"));
        assert!(!details.iter().any(|(k, _)| k == "Height"));
    }

    #[test]
    fn classify_object_reference() {
        let doc = Document::new();
        let obj = Object::Reference((42, 0));
        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Reference");
        assert!(
            desc.contains("42 0 R"),
            "expected '42 0 R' in desc, got: {}",
            desc
        );
    }

    #[test]
    fn classify_object_font_heuristic_widths_firstchar() {
        // Dict with /Widths and /FirstChar but no /Type and no font /Subtype
        // This falls through to Generic since there's no heuristic branch in the code
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Widths", Object::Array(vec![Object::Integer(250); 10]));
        dict.set("FirstChar", Object::Integer(32));
        dict.set("LastChar", Object::Integer(41));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { role, .. } = classify_object(&doc, 55, &obj, &doc.get_pages());
        assert_eq!(role, "Generic");
    }

    #[test]
    fn classify_object_generic_stream() {
        // A stream with unknown Type/Subtype falls to Generic with "stream" in description
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Length", Object::Integer(100));
        let stream = Stream::new(dict, vec![0u8; 100]);
        let obj = Object::Stream(stream);

        let ObjectClassification {
            role,
            description: desc,
            details,
        } = classify_object(&doc, 60, &obj, &doc.get_pages());
        assert_eq!(role, "Generic");
        assert!(
            desc.contains("stream"),
            "expected 'stream' in desc, got: {}",
            desc
        );
        assert!(details.iter().any(|(k, _)| k == "Stream Size"));
    }

    #[test]
    fn classify_object_mmtype1_font() {
        // MMType1 is in the subtype match
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"MMType1".to_vec()));
        dict.set("BaseFont", Object::Name(b"Minion-Regular".to_vec()));
        let obj = Object::Dictionary(dict);

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 70, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("MMType1"));
        assert!(desc.contains("Minion-Regular"));
    }

    #[test]
    fn classify_object_type3_font() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Type3".to_vec()));
        dict.set("BaseFont", Object::Name(b"CustomFont".to_vec()));
        let obj = Object::Dictionary(dict);

        let ObjectClassification {
            role,
            description: desc,
            ..
        } = classify_object(&doc, 71, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("Type3"));
    }

    #[test]
    fn print_info_object_not_found() {
        let doc = Document::new();
        let out = output_of(|w| print_info(w, &doc, 999));
        assert!(
            out.contains("not found"),
            "expected 'not found' in output, got: {}",
            out
        );
    }

    #[test]
    fn print_info_json_object_not_found() {
        let doc = Document::new();
        let config = default_config();
        let out = output_of(|w| print_info_json(w, &doc, 999, &config));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["object_number"], 999);
        assert_eq!(val["error"], "not found");
    }

    #[test]
    fn find_page_associations_multiple_pages() {
        let mut doc = Document::new();

        // Shared font object
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font));

        // Page 1 references the font
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Parent", Object::Reference((2, 0)));
        let mut res1 = Dictionary::new();
        let mut fd1 = Dictionary::new();
        fd1.set("F1", Object::Reference((10, 0)));
        res1.set("Font", Object::Dictionary(fd1));
        page1.set("Resources", Object::Dictionary(res1));
        doc.objects.insert((3, 0), Object::Dictionary(page1));

        // Page 2 also references the font
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Parent", Object::Reference((2, 0)));
        let mut res2 = Dictionary::new();
        let mut fd2 = Dictionary::new();
        fd2.set("F1", Object::Reference((10, 0)));
        res2.set("Font", Object::Dictionary(fd2));
        page2.set("Resources", Object::Dictionary(res2));
        doc.objects.insert((4, 0), Object::Dictionary(page2));

        // Page tree
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set(
            "Kids",
            Object::Array(vec![Object::Reference((3, 0)), Object::Reference((4, 0))]),
        );
        pages_dict.set("Count", Object::Integer(2));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let assoc = find_page_associations(&doc, 10, &doc.get_pages());
        assert_eq!(assoc, vec![1, 2]);
    }

    #[test]
    fn classify_font_encoding_as_reference() {
        // Encoding specified as a reference rather than a name
        let mut doc = Document::new();

        let mut enc = Dictionary::new();
        enc.set("Type", Object::Name(b"Encoding".to_vec()));
        enc.set("BaseEncoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        doc.objects.insert((20, 0), Object::Dictionary(enc));

        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        dict.set("Encoding", Object::Reference((20, 0)));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 25, &obj, &doc.get_pages());
        let enc_detail = details.iter().find(|(k, _)| k == "Encoding").unwrap();
        assert!(
            enc_detail.1.contains("20 0 R"),
            "expected '20 0 R' in Encoding detail, got: {}",
            enc_detail.1
        );
    }

    #[test]
    fn classify_font_no_basefont() {
        // Font without BaseFont should use default "-"
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type3".to_vec()));
        let obj = Object::Dictionary(dict);

        let ObjectClassification {
            details,
            description: desc,
            ..
        } = classify_object(&doc, 26, &obj, &doc.get_pages());
        assert!(details.iter().any(|(k, v)| k == "BaseFont" && v == "-"));
        assert!(desc.contains("-"));
    }

    #[test]
    fn classify_font_unresolvable_font_descriptor() {
        // FontDescriptor ref that points to a nonexistent object
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"TrueType".to_vec()));
        dict.set("BaseFont", Object::Name(b"Arial".to_vec()));
        dict.set("FontDescriptor", Object::Reference((999, 0)));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 27, &obj, &doc.get_pages());
        let fd_detail = details.iter().find(|(k, _)| k == "FontDescriptor").unwrap();
        assert!(
            fd_detail.1.contains("unresolvable"),
            "expected 'unresolvable', got: {}",
            fd_detail.1
        );
    }

    #[test]
    fn classify_font_no_font_descriptor() {
        // Font without any FontDescriptor at all
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        dict.set("BaseFont", Object::Name(b"Symbol".to_vec()));
        let obj = Object::Dictionary(dict);

        let ObjectClassification { details, .. } =
            classify_object(&doc, 28, &obj, &doc.get_pages());
        let fd_detail = details.iter().find(|(k, _)| k == "FontDescriptor").unwrap();
        assert_eq!(fd_detail.1, "no FontDescriptor");
    }

    #[test]
    fn print_info_json_metadata_stream() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Metadata".to_vec()));
        dict.set("Subtype", Object::Name(b"XML".to_vec()));
        let stream = Stream::new(dict, b"<rdf:RDF/>".to_vec());
        doc.objects.insert((20, 0), Object::Stream(stream));

        let config = default_config();
        let out = output_of(|w| print_info_json(w, &doc, 20, &config));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["role"], "Generic");
        assert_eq!(val["object_number"], 20);
        assert!(val["details"]["Type"].as_str().unwrap() == "Metadata");
    }

    #[test]
    fn print_info_json_multiple_page_associations() {
        let mut doc = Document::new();

        // Shared image
        let mut img_dict = Dictionary::new();
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", Object::Integer(100));
        img_dict.set("Height", Object::Integer(100));
        let stream = Stream::new(img_dict, vec![0u8; 10]);
        doc.objects.insert((10, 0), Object::Stream(stream));

        // Page 1
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Parent", Object::Reference((2, 0)));
        let mut res1 = Dictionary::new();
        let mut xobj1 = Dictionary::new();
        xobj1.set("Im1", Object::Reference((10, 0)));
        res1.set("XObject", Object::Dictionary(xobj1));
        page1.set("Resources", Object::Dictionary(res1));
        doc.objects.insert((3, 0), Object::Dictionary(page1));

        // Page 2
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Parent", Object::Reference((2, 0)));
        let mut res2 = Dictionary::new();
        let mut xobj2 = Dictionary::new();
        xobj2.set("Im1", Object::Reference((10, 0)));
        res2.set("XObject", Object::Dictionary(xobj2));
        page2.set("Resources", Object::Dictionary(res2));
        doc.objects.insert((4, 0), Object::Dictionary(page2));

        // Page tree
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set(
            "Kids",
            Object::Array(vec![Object::Reference((3, 0)), Object::Reference((4, 0))]),
        );
        pages_dict.set("Count", Object::Integer(2));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = default_config();
        let out = output_of(|w| print_info_json(w, &doc, 10, &config));
        let val: Value = serde_json::from_str(&out).unwrap();
        let pages = val["page_associations"].as_array().unwrap();
        assert_eq!(pages, &[1, 2]);
    }
}
