use lopdf::{content::Content, Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::types::{DumpConfig, PageSpec};
use crate::stream::{decode_stream, get_filter_names, is_binary_stream, format_hex_dump};
use crate::helpers::{format_dict_value, format_operation, object_type_label};

pub(crate) fn dump_object_and_children(writer: &mut impl Write, obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, config: &DumpConfig, is_contents: bool, current_depth: usize) {
    if visited.contains(&obj_id) {
        return;
    }
    visited.insert(obj_id);

    writeln!(writer, "Object {} {}:", obj_id.0, obj_id.1).unwrap();

    match doc.get_object(obj_id) {
        Ok(object) => {
            let visited_for_print = BTreeSet::new();
            let mut child_refs = BTreeSet::new();
            print_object(writer, object, doc, &visited_for_print, 1, config, is_contents, &mut child_refs);
            writeln!(writer, "\n").unwrap();

            if let Some(max_depth) = config.depth
                && current_depth >= max_depth
            {
                let unvisited: Vec<_> = child_refs.iter()
                    .filter(|(_, id)| !visited.contains(id))
                    .collect();
                if !unvisited.is_empty() {
                    writeln!(writer, "  (depth limit reached, {} references not followed)", unvisited.len()).unwrap();
                }
                return;
            }

            for (is_contents, child_id) in child_refs {
                if !visited.contains(&child_id) {
                    writeln!(writer, "--------------------------------\n").unwrap();
                    dump_object_and_children(writer, child_id, doc, visited, config, is_contents, current_depth + 1);
                }
            }
        }
        Err(e) => {
            writeln!(writer, "  Error getting object: {}", e).unwrap();
        }
    }
}

pub(crate) fn print_stream_content(writer: &mut impl Write, stream: &lopdf::Stream, indent_str: &str, config: &DumpConfig, is_contents: bool) {
    let (decoded_content, warning) = decode_stream(stream);
    let filters = get_filter_names(stream);
    let description = if warning.is_none() && !filters.is_empty() {
        "decoded"
    } else {
        "raw"
    };

    print_content_data(writer, &decoded_content, description, indent_str, config, is_contents, warning.as_deref());
}

pub(crate) fn print_content_data(writer: &mut impl Write, content: &[u8], description: &str, indent_str: &str, config: &DumpConfig, is_contents: bool, warning: Option<&str>) {
    if let Some(warn) = warning {
        writeln!(writer, "\n{}[WARNING: {}]", indent_str, warn).unwrap();
    }

    if is_contents {
        match Content::decode(content) {
            Ok(content) => {
                writeln!(
                    writer,
                    "\n{}Parsed Content Stream ({} operations):",
                    indent_str,
                    content.operations.len()
                ).unwrap();
                for op in &content.operations {
                    writeln!(writer, "{}  {}", indent_str, format_operation(op)).unwrap();
                }
                return;
            }
            Err(e) => {
                writeln!(writer, "\n{}[Could not parse content stream: {}. Falling back to raw view.]", indent_str, e).unwrap();
            }
        }
    }

    let full_len = content.len();
    let is_binary = is_binary_stream(content);
    let content_to_display = if let Some(limit) = config.truncate {
        if is_binary { &content[..full_len.min(limit)] } else { content }
    } else {
        content
    };

    let len_str = if let Some(limit) = config.truncate {
        if full_len > limit && is_binary {
            format!("{} (truncated to {})", full_len, limit)
        } else {
            full_len.to_string()
        }
    } else {
        full_len.to_string()
    };

    if config.hex && is_binary {
        writeln!(
            writer,
            "\n{}Stream content ({}, {} bytes):\n{}",
            indent_str,
            description,
            len_str,
            format_hex_dump(content_to_display)
        ).unwrap();
    } else {
        writeln!(
            writer,
            "\n{}Stream content ({}, {} bytes):\n---\n{}\n---",
            indent_str,
            description,
            len_str,
            String::from_utf8_lossy(content_to_display)
        ).unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::only_used_in_recursion)]
pub(crate) fn print_object(writer: &mut impl Write, obj: &Object, doc: &Document, visited: &BTreeSet<ObjectId>, indent: usize, config: &DumpConfig, is_contents: bool, child_refs: &mut BTreeSet<(bool, ObjectId)>) {
    let indent_str = "  ".repeat(indent);
    let child_indent = "  ".repeat(indent + 1);

    match obj {
        Object::Null => write!(writer, "null").unwrap(),
        Object::Boolean(b) => write!(writer, "{}", b).unwrap(),
        Object::Integer(i) => write!(writer, "{}", i).unwrap(),
        Object::Real(r) => write!(writer, "{}", r).unwrap(),
        Object::Name(name) => write!(writer, "/{}", String::from_utf8_lossy(name)).unwrap(),
        Object::String(bytes, _) => write!(writer, "({})", String::from_utf8_lossy(bytes)).unwrap(),
        Object::Array(array) => {
            writeln!(writer, "[").unwrap();
            for item in array {
                write!(writer, "{}", child_indent).unwrap();
                print_object(writer, item, doc, visited, indent + 1, config, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}]", indent_str).unwrap();
        }
        Object::Stream(stream) => {
            writeln!(writer, "<<").unwrap();
            for (key, value) in stream.dict.iter() {
                write!(writer, "{}/{} ", child_indent, String::from_utf8_lossy(key)).unwrap();
                print_object(writer, value, doc, visited, indent + 1, config, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}>> stream", indent_str).unwrap();

            if config.raw {
                print_content_data(writer, &stream.content, "raw, undecoded", &indent_str, config, false, None);
            } else if config.decode_streams {
                print_stream_content(writer, stream, &indent_str, config, is_contents);
            }
        }
        Object::Dictionary(dict) => {
            writeln!(writer, "<<").unwrap();
            for (key, value) in dict.iter() {
                write!(writer, "{}/{} ", child_indent, String::from_utf8_lossy(key)).unwrap();
                let is_contents = key == b"Contents";
                print_object(writer, value, doc, visited, indent + 1, config, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}>>", indent_str).unwrap();
        }
        Object::Reference(id) => {
            child_refs.insert((is_contents, *id));
            write!(writer, "{} {} R", id.0, id.1).unwrap();
            if visited.contains(id) {
                write!(writer, " (visited)").unwrap();
            } else if config.deref
                && let Ok(resolved) = doc.get_object(*id) {
                write!(writer, " => {}", deref_summary(resolved, doc)).unwrap();
            }
        }
    }
}

pub(crate) fn deref_summary(obj: &Object, _doc: &Document) -> String {
    match obj {
        Object::Null => "null".to_string(),
        Object::Boolean(b) => b.to_string(),
        Object::Integer(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::String(bytes, _) => format!("({})", String::from_utf8_lossy(bytes)),
        Object::Array(arr) => format!("[{} items]", arr.len()),
        Object::Reference(id) => format!("{} {} R", id.0, id.1),
        Object::Stream(stream) => {
            let type_label = object_type_label(obj);
            let filter = stream.dict.get(b"Filter").ok()
                .and_then(|f| f.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));
            let mut parts = vec![format!("stream, {} bytes", stream.content.len())];
            if type_label != "-" { parts.insert(0, format!("/Type /{}", type_label)); }
            if let Some(f) = filter { parts.push(f); }
            format!("<< {} >>", parts.join(", "))
        }
        Object::Dictionary(dict) => {
            let type_label = object_type_label(obj);
            let count = dict.len();
            let mut parts = Vec::new();
            if type_label != "-" { parts.push(format!("/Type /{}", type_label)); }
            // Show a few notable keys
            for key in [b"BaseFont".as_slice(), b"Subtype", b"Count", b"MediaBox"] {
                if let Ok(val) = dict.get(key) {
                    parts.push(format!("/{}={}", String::from_utf8_lossy(key), format_dict_value(val)));
                }
            }
            if parts.is_empty() {
                format!("<< {} keys >>", count)
            } else {
                format!("<< {}, {} keys >>", parts.join(", "), count)
            }
        }
    }
}

pub(crate) fn print_single_object(writer: &mut impl Write, doc: &Document, obj_num: u32, config: &DumpConfig) {
    let obj_id = (obj_num, 0);
    match doc.get_object(obj_id) {
        Ok(object) => {
            writeln!(writer, "Object {} 0:", obj_num).unwrap();
            let visited = BTreeSet::new();
            let mut child_refs = BTreeSet::new();
            print_object(writer, object, doc, &visited, 1, config, false, &mut child_refs);
            writeln!(writer).unwrap();
        }
        Err(_) => {
            eprintln!("Error: Object {} not found in the document.", obj_num);
            std::process::exit(1);
        }
    }
}

pub(crate) fn print_objects(writer: &mut impl Write, doc: &Document, nums: &[u32], config: &DumpConfig) {
    for (i, &obj_num) in nums.iter().enumerate() {
        if i > 0 { writeln!(writer).unwrap(); }
        print_single_object(writer, doc, obj_num, config);
    }
}

pub(crate) fn print_objects_json(writer: &mut impl Write, doc: &Document, nums: &[u32], config: &DumpConfig) {
    if nums.len() == 1 {
        print_single_object_json(writer, doc, nums[0], config);
    } else {
        let mut items = Vec::new();
        for &obj_num in nums {
            let obj_id = (obj_num, 0);
            match doc.get_object(obj_id) {
                Ok(object) => {
                    items.push(json!({
                        "object_number": obj_num,
                        "generation": 0,
                        "object": object_to_json(object, doc, config),
                    }));
                }
                Err(_) => {
                    items.push(json!({
                        "object_number": obj_num,
                        "generation": 0,
                        "error": "not found",
                    }));
                }
            }
        }
        let output = json!({"objects": items});
        writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
    }
}

pub(crate) fn dump_page(writer: &mut impl Write, doc: &Document, spec: &PageSpec, config: &DumpConfig) {
    let pages = doc.get_pages();
    let total = pages.len();

    for page_num in spec.pages() {
        let page_id = match pages.get(&page_num) {
            Some(&id) => id,
            None => {
                eprintln!("Error: Page {} not found. Document has {} pages.", page_num, total);
                std::process::exit(1);
            }
        };

        let mut visited = BTreeSet::new();

        // Pre-seed visited with /Parent to confine traversal to this page's subtree
        if let Ok(Object::Dictionary(dict)) = doc.get_object(page_id)
            && let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference())
        {
            visited.insert(parent_ref);
        }

        writeln!(writer, "Page {} (Object {} {}):", page_num, page_id.0, page_id.1).unwrap();
        dump_object_and_children(writer, page_id, doc, &mut visited, config, false, 0);
    }
}

// ── JSON output (Phase 1) ────────────────────────────────────────────

#[allow(clippy::only_used_in_recursion)]
pub(crate) fn object_to_json(obj: &Object, doc: &Document, config: &DumpConfig) -> Value {
    match obj {
        Object::Null => json!({"type": "null"}),
        Object::Boolean(b) => json!({"type": "boolean", "value": b}),
        Object::Integer(i) => json!({"type": "integer", "value": i}),
        Object::Real(r) => json!({"type": "real", "value": r}),
        Object::Name(n) => json!({"type": "name", "value": String::from_utf8_lossy(n)}),
        Object::String(bytes, _) => json!({"type": "string", "value": String::from_utf8_lossy(bytes)}),
        Object::Array(arr) => {
            let items: Vec<Value> = arr.iter().map(|o| object_to_json(o, doc, config)).collect();
            json!({"type": "array", "items": items})
        }
        Object::Dictionary(dict) => {
            let entries: serde_json::Map<String, Value> = dict.iter()
                .map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), object_to_json(v, doc, config)))
                .collect();
            json!({"type": "dictionary", "entries": entries})
        }
        Object::Stream(stream) => {
            let entries: serde_json::Map<String, Value> = stream.dict.iter()
                .map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), object_to_json(v, doc, config)))
                .collect();
            let mut val = json!({"type": "stream", "dict": entries});
            if config.raw {
                let content = &stream.content;
                if !is_binary_stream(content) {
                    val["raw_content"] = json!(String::from_utf8_lossy(content));
                } else if config.hex {
                    let display_data = if let Some(limit) = config.truncate {
                        &content[..content.len().min(limit)]
                    } else {
                        content.as_slice()
                    };
                    val["raw_content_hex"] = json!(format_hex_dump(display_data));
                } else {
                    val["raw_content_binary"] = json!(format!("<binary, {} bytes>", content.len()));
                }
            } else if config.decode_streams {
                let (decoded, warning) = decode_stream(stream);
                if let Some(warn) = &warning {
                    val["decode_warning"] = json!(warn);
                }
                if !is_binary_stream(&decoded) {
                    val["content"] = json!(String::from_utf8_lossy(&decoded));
                } else if config.hex {
                    let display_data = if let Some(limit) = config.truncate {
                        &decoded[..decoded.len().min(limit)]
                    } else {
                        &decoded
                    };
                    val["content_hex"] = json!(format_hex_dump(display_data));
                } else if config.truncate.is_some() {
                    val["content_truncated"] = json!(format!("<binary, {} bytes>", decoded.len()));
                } else {
                    val["content_binary"] = json!(format!("<binary, {} bytes>", decoded.len()));
                }
            }
            val
        }
        Object::Reference(id) => {
            let mut val = json!({"type": "reference", "object_number": id.0, "generation": id.1});
            if config.deref
                && let Ok(resolved) = doc.get_object(*id) {
                let no_deref = DumpConfig { deref: false, ..*config };
                val["resolved"] = object_to_json(resolved, doc, &no_deref);
            }
            val
        }
    }
}

pub(crate) fn collect_reachable_objects(doc: &Document, max_depth: Option<usize>) -> BTreeMap<String, Value> {
    let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
    let mut result = BTreeMap::new();
    let mut visited = BTreeSet::new();

    fn walk(doc: &Document, obj_id: ObjectId, visited: &mut BTreeSet<ObjectId>, result: &mut BTreeMap<String, Value>, config: &DumpConfig, current_depth: usize, max_depth: Option<usize>) {
        if visited.contains(&obj_id) { return; }
        if let Some(max) = max_depth
            && current_depth > max { return; }
        visited.insert(obj_id);
        if let Ok(obj) = doc.get_object(obj_id) {
            let key = format!("{}:{}", obj_id.0, obj_id.1);
            result.insert(key, object_to_json(obj, doc, config));
            collect_refs(obj, doc, visited, result, config, current_depth, max_depth);
        }
    }

    fn collect_refs(obj: &Object, doc: &Document, visited: &mut BTreeSet<ObjectId>, result: &mut BTreeMap<String, Value>, config: &DumpConfig, current_depth: usize, max_depth: Option<usize>) {
        match obj {
            Object::Reference(id) => walk(doc, *id, visited, result, config, current_depth + 1, max_depth),
            Object::Array(arr) => {
                for item in arr { collect_refs(item, doc, visited, result, config, current_depth, max_depth); }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() { collect_refs(v, doc, visited, result, config, current_depth, max_depth); }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter() { collect_refs(v, doc, visited, result, config, current_depth, max_depth); }
            }
            _ => {}
        }
    }

    // Start from trailer refs
    for (_, v) in doc.trailer.iter() {
        if let Ok(id) = v.as_reference() {
            walk(doc, id, &mut visited, &mut result, &config, 0, max_depth);
        }
    }

    result
}

pub(crate) fn dump_json(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let trailer_json = object_to_json(&Object::Dictionary(doc.trailer.clone()), doc, config);
    let objects = collect_reachable_objects(doc, config.depth);
    let output = json!({
        "trailer": trailer_json,
        "objects": objects,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

pub(crate) fn print_single_object_json(writer: &mut impl Write, doc: &Document, obj_num: u32, config: &DumpConfig) {
    let obj_id = (obj_num, 0);
    match doc.get_object(obj_id) {
        Ok(object) => {
            let output = json!({
                "object_number": obj_num,
                "generation": 0,
                "object": object_to_json(object, doc, config),
            });
            writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
        }
        Err(_) => {
            eprintln!("Error: Object {} not found in the document.", obj_num);
            std::process::exit(1);
        }
    }
}

pub(crate) fn dump_page_json(writer: &mut impl Write, doc: &Document, spec: &PageSpec, config: &DumpConfig) {
    let pages = doc.get_pages();
    let total = pages.len();

    fn walk_page(doc: &Document, obj_id: ObjectId, visited: &mut BTreeSet<ObjectId>, objects: &mut BTreeMap<String, Value>, config: &DumpConfig) {
        if visited.contains(&obj_id) { return; }
        visited.insert(obj_id);
        if let Ok(obj) = doc.get_object(obj_id) {
            let key = format!("{}:{}", obj_id.0, obj_id.1);
            objects.insert(key, object_to_json(obj, doc, config));
            collect_refs_page(obj, doc, visited, objects, config);
        }
    }

    fn collect_refs_page(obj: &Object, doc: &Document, visited: &mut BTreeSet<ObjectId>, objects: &mut BTreeMap<String, Value>, config: &DumpConfig) {
        match obj {
            Object::Reference(id) => walk_page(doc, *id, visited, objects, config),
            Object::Array(arr) => {
                for item in arr { collect_refs_page(item, doc, visited, objects, config); }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() { collect_refs_page(v, doc, visited, objects, config); }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter() { collect_refs_page(v, doc, visited, objects, config); }
            }
            _ => {}
        }
    }

    let mut page_outputs = Vec::new();

    for page_num in spec.pages() {
        let page_id = match pages.get(&page_num) {
            Some(&id) => id,
            None => {
                eprintln!("Error: Page {} not found. Document has {} pages.", page_num, total);
                std::process::exit(1);
            }
        };

        let mut visited = BTreeSet::new();
        let mut objects = BTreeMap::new();

        if let Ok(Object::Dictionary(dict)) = doc.get_object(page_id)
            && let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference())
        {
            visited.insert(parent_ref);
        }

        walk_page(doc, page_id, &mut visited, &mut objects, config);

        page_outputs.push(json!({
            "page_number": page_num,
            "objects": objects,
        }));
    }

    // For single page, output as before; for range, output array
    if page_outputs.len() == 1 {
        writeln!(writer, "{}", serde_json::to_string_pretty(&page_outputs[0]).unwrap()).unwrap();
    } else {
        writeln!(writer, "{}", serde_json::to_string_pretty(&json!({"pages": page_outputs})).unwrap()).unwrap();
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::DumpConfig;
    use crate::types::PageSpec;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use std::collections::BTreeSet;
    use lopdf::Object;
    use lopdf::Document;
    use crate::validate::collect_reachable_ids;
    use flate2::write::ZlibEncoder;
    

    #[test]
    fn print_content_data_with_warning() {
        let content = b"raw data";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, false, Some("test warning message"));
        });
        assert!(out.contains("[WARNING: test warning message]"));
    }

    #[test]
    fn print_content_data_without_warning() {
        let content = b"raw data";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, false, None);
        });
        assert!(!out.contains("WARNING"));
    }

    #[test]
    fn object_to_json_stream_decode_warning() {
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"corrupt data".to_vec(),
        );
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert!(val.get("decode_warning").is_some(), "Corrupt stream should have decode_warning in JSON");
    }

    #[test]
    fn object_to_json_stream_no_decode_warning() {
        let stream = make_stream(None, b"text content".to_vec());
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert!(val.get("decode_warning").is_none(), "Valid stream should not have decode_warning");
    }

    fn print_obj(obj: &Object) -> (String, BTreeSet<(bool, ObjectId)>) {
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            print_object(w, obj, &doc, &visited, 1, &config, false, &mut child_refs);
        });
        (out, child_refs)
    }

    #[test]
    fn print_object_null() {
        let (out, _) = print_obj(&Object::Null);
        assert_eq!(out, "null");
    }

    #[test]
    fn print_object_boolean() {
        let (out, _) = print_obj(&Object::Boolean(true));
        assert_eq!(out, "true");
        let (out, _) = print_obj(&Object::Boolean(false));
        assert_eq!(out, "false");
    }

    #[test]
    fn print_object_integer() {
        let (out, _) = print_obj(&Object::Integer(42));
        assert_eq!(out, "42");
    }

    #[test]
    fn print_object_real() {
        let (out, _) = print_obj(&Object::Real(2.72));
        assert_eq!(out, "2.72");
    }

    #[test]
    fn print_object_name() {
        let (out, _) = print_obj(&Object::Name(b"Type".to_vec()));
        assert_eq!(out, "/Type");
    }

    #[test]
    fn print_object_string() {
        let (out, _) = print_obj(&Object::String(b"hello".to_vec(), StringFormat::Literal));
        assert_eq!(out, "(hello)");
    }

    #[test]
    fn print_object_array() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let (out, _) = print_obj(&arr);
        assert!(out.contains("["));
        assert!(out.contains("1"));
        assert!(out.contains("2"));
        assert!(out.contains("]"));
    }

    #[test]
    fn print_object_dictionary() {
        let mut dict = Dictionary::new();
        dict.set("Key", Object::Integer(99));
        let (out, _) = print_obj(&Object::Dictionary(dict));
        assert!(out.contains("<<"));
        assert!(out.contains("/Key"));
        assert!(out.contains("99"));
        assert!(out.contains(">>"));
    }

    #[test]
    fn print_object_stream_no_decode() {
        let stream = make_stream(None, b"stream data".to_vec());
        let (out, _) = print_obj(&Object::Stream(stream));
        assert!(out.contains("<<"));
        assert!(out.contains(">> stream"));
        // decode_streams=false, so no stream content printed
        assert!(!out.contains("Stream content"));
    }

    #[test]
    fn print_object_stream_with_decode() {
        let stream = make_stream(None, b"visible data".to_vec());
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_object(w, &Object::Stream(stream), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(out.contains(">> stream"));
        assert!(out.contains("Stream content"));
        assert!(out.contains("visible data"));
    }

    #[test]
    fn print_object_reference_populates_child_refs() {
        let obj = Object::Reference((5, 0));
        let (out, refs) = print_obj(&obj);
        assert_eq!(out, "5 0 R");
        assert!(refs.contains(&(false, (5, 0))));
    }

    #[test]
    fn print_object_reference_visited() {
        let doc = empty_doc();
        let mut visited = BTreeSet::new();
        visited.insert((5, 0));
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            print_object(w, &Object::Reference((5, 0)), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(out.contains("5 0 R (visited)"));
    }

    #[test]
    fn print_object_contents_key_propagates_is_contents() {
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Reference((10, 0)));
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &Object::Dictionary(dict), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        // The reference under /Contents should have is_contents=true
        assert!(child_refs.contains(&(true, (10, 0))));
    }

    #[test]
    fn print_content_data_ascii_no_truncation() {
        let content = b"Hello PDF stream";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, false, None);
        });
        assert!(out.contains("Stream content (raw, 16 bytes)"));
        assert!(out.contains("Hello PDF stream"));
    }

    #[test]
    fn print_content_data_binary_truncated() {
        // 200 bytes of binary data (contains 0x80 so is_binary_stream = true)
        let content: Vec<u8> = (0..200).map(|i| (i as u8).wrapping_add(0x80)).collect();
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 (truncated to 100)"));
    }

    #[test]
    fn print_content_data_is_contents_parses_operations() {
        // A simple PDF content stream: "BT /F1 12 Tf ET"
        let content = b"BT\n/F1 12 Tf\nET";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "decoded", "  ", &config, true, None);
        });
        assert!(out.contains("Parsed Content Stream"));
        assert!(out.contains("operations"));
    }

    #[test]
    fn dump_single_object_no_refs() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("42"));
        assert!(visited.contains(&(1, 0)));
    }

    #[test]
    fn dump_object_follows_references() {
        let mut doc = Document::new();
        // Object 1 is a dict with a reference to object 2
        let mut dict = Dictionary::new();
        dict.set("Child", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(99));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("99"));
        assert!(visited.contains(&(1, 0)));
        assert!(visited.contains(&(2, 0)));
    }

    #[test]
    fn dump_object_circular_reference() {
        let mut doc = Document::new();
        // Object 1 references object 2, object 2 references object 1
        let mut dict1 = Dictionary::new();
        dict1.set("Next", Object::Reference((2, 0)));
        let mut dict2 = Dictionary::new();
        dict2.set("Prev", Object::Reference((1, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        let mut visited = BTreeSet::new();
        let config = default_config();
        // This should terminate (not infinite-loop) thanks to the visited set
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(visited.contains(&(1, 0)));
        assert!(visited.contains(&(2, 0)));
    }

    #[test]
    fn print_object_empty_array() {
        let arr = Object::Array(vec![]);
        let (out, refs) = print_obj(&arr);
        assert!(out.contains("["));
        assert!(out.contains("]"));
        assert!(refs.is_empty());
    }

    #[test]
    fn print_object_empty_dictionary() {
        let dict = Dictionary::new();
        let (out, refs) = print_obj(&Object::Dictionary(dict));
        assert!(out.contains("<<"));
        assert!(out.contains(">>"));
        assert!(refs.is_empty());
    }

    #[test]
    fn print_object_nested_dictionary() {
        let mut inner = Dictionary::new();
        inner.set("InnerKey", Object::Integer(7));
        let mut outer = Dictionary::new();
        outer.set("Outer", Object::Dictionary(inner));
        let (out, _) = print_obj(&Object::Dictionary(outer));
        assert!(out.contains("/Outer"));
        assert!(out.contains("/InnerKey"));
        assert!(out.contains("7"));
    }

    #[test]
    fn print_object_array_with_references_collects_child_refs() {
        let arr = Object::Array(vec![
            Object::Reference((3, 0)),
            Object::Reference((4, 0)),
        ]);
        let (out, refs) = print_obj(&arr);
        assert!(out.contains("3 0 R"));
        assert!(out.contains("4 0 R"));
        assert!(refs.contains(&(false, (3, 0))));
        assert!(refs.contains(&(false, (4, 0))));
    }

    #[test]
    fn print_object_negative_integer() {
        let (out, _) = print_obj(&Object::Integer(-99));
        assert_eq!(out, "-99");
    }

    #[test]
    fn print_object_zero_real() {
        let (out, _) = print_obj(&Object::Real(0.0));
        assert_eq!(out, "0");
    }

    #[test]
    fn print_object_name_with_special_chars() {
        let (out, _) = print_obj(&Object::Name(b"Font+Name".to_vec()));
        assert_eq!(out, "/Font+Name");
    }

    #[test]
    fn print_object_string_hex_format() {
        let (out, _) = print_obj(&Object::String(b"hex".to_vec(), StringFormat::Hexadecimal));
        assert_eq!(out, "(hex)");
    }

    #[test]
    fn print_object_stream_with_flatedecode_and_decode_flag() {
        let compressed = zlib_compress(b"decompressed text");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_object(w, &Object::Stream(stream), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(out.contains(">> stream"));
        assert!(out.contains("decoded"));
        assert!(out.contains("decompressed text"));
    }

    #[test]
    fn print_object_multiple_refs_in_dict() {
        let mut dict = Dictionary::new();
        dict.set("A", Object::Reference((10, 0)));
        dict.set("B", Object::Reference((20, 0)));
        let (_, refs) = print_obj(&Object::Dictionary(dict));
        assert!(refs.contains(&(false, (10, 0))));
        assert!(refs.contains(&(false, (20, 0))));
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn print_object_is_contents_propagated_to_array_ref() {
        // When print_object is called with is_contents=true, refs in arrays get is_contents=true
        let arr = Object::Array(vec![Object::Reference((7, 0))]);
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &arr, &doc, &visited, 1, &config, true, &mut child_refs);
        });
        assert!(child_refs.contains(&(true, (7, 0))));
    }

    #[test]
    fn print_object_stream_dict_entries_printed() {
        let mut dict = Dictionary::new();
        dict.set("Length", Object::Integer(11));
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, b"stream data".to_vec());
        let (out, _) = print_obj(&Object::Stream(stream));
        assert!(out.contains("/Length"));
        assert!(out.contains("11"));
        assert!(out.contains("/Filter"));
        assert!(out.contains("/FlateDecode"));
    }

    #[test]
    fn print_content_data_empty_content() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"", "raw", "  ", &config, false, None);
        });
        assert!(out.contains("Stream content (raw, 0 bytes)"));
    }

    #[test]
    fn print_content_data_binary_no_truncation() {
        // Binary content but truncate=None → full output
        let content: Vec<u8> = (0..200).map(|i| (i as u8).wrapping_add(0x80)).collect();
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_short_with_truncation_enabled() {
        // Binary content < 100 bytes with truncation enabled → no truncation applied
        let content: Vec<u8> = vec![0x80; 50];
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("50 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_exactly_100_bytes_with_truncation() {
        // Exactly 100 bytes of binary → no truncation (only truncates > 100)
        let content: Vec<u8> = vec![0x80; 100];
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("100 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_101_bytes_with_truncation() {
        // 101 bytes of binary → should truncate
        let content: Vec<u8> = vec![0x80; 101];
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("101 (truncated to 100)"));
    }

    #[test]
    fn print_content_data_truncate_none_no_truncation() {
        let content: Vec<u8> = vec![0x80; 200];
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_truncate_custom_50() {
        let content: Vec<u8> = vec![0x80; 200];
        let config = DumpConfig { decode_streams: false, truncate: Some(50), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 (truncated to 50)"));
    }

    #[test]
    fn print_content_data_truncate_larger_than_stream() {
        let content: Vec<u8> = vec![0x80; 50];
        let config = DumpConfig { decode_streams: false, truncate: Some(500), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("50 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_is_contents_invalid_stream_falls_back() {
        // Content::decode is lenient, so we verify the fallback path by checking
        // that badly formed streams either parse (with 0 ops) or show the fallback.
        // Use content that Content::decode will reject: unbalanced parens cause a parse error.
        let content = b"( unclosed string";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, true, None);
        });
        // lopdf's Content::decode may or may not fail on this.
        // If it parses: we see "Parsed Content Stream"; if it fails: we see the fallback.
        let parsed = out.contains("Parsed Content Stream");
        let fallback = out.contains("Could not parse content stream") && out.contains("Stream content");
        assert!(parsed || fallback, "Expected either parsed or fallback output, got: {}", out);
    }

    #[test]
    fn print_content_data_ascii_not_truncated_even_when_flag_set() {
        // ASCII content >100 bytes with truncation flag → no truncation (not binary)
        let content = b"abcdefghij".repeat(20); // 200 bytes of ASCII
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_stream_content_no_filter() {
        let stream = make_stream(None, b"raw stream bytes".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, false);
        });
        assert!(out.contains("raw"));
        assert!(out.contains("raw stream bytes"));
    }

    #[test]
    fn print_stream_content_flatedecode() {
        let compressed = zlib_compress(b"decoded content");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, false);
        });
        assert!(out.contains("decoded"));
        assert!(out.contains("decoded content"));
    }

    #[test]
    fn print_stream_content_with_truncation() {
        // Large binary stream with truncation enabled
        let content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, content);
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("truncated to 100"));
    }

    #[test]
    fn print_stream_content_is_contents_parses() {
        let content = b"BT\n/F1 12 Tf\nET";
        let stream = make_stream(None, content.to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, true);
        });
        assert!(out.contains("Parsed Content Stream"));
    }

    #[test]
    fn dump_object_not_found() {
        let doc = Document::new();
        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (99, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 99 0:"));
        assert!(out.contains("Error getting object"));
        assert!(visited.contains(&(99, 0)));
    }

    #[test]
    fn dump_object_already_visited_produces_no_output() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let mut visited = BTreeSet::new();
        visited.insert((1, 0));
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert_eq!(out, "");
    }

    #[test]
    fn dump_object_deep_chain_three_levels() {
        let mut doc = Document::new();
        let mut dict1 = Dictionary::new();
        dict1.set("Next", Object::Reference((2, 0)));
        let mut dict2 = Dictionary::new();
        dict2.set("Next", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));
        doc.objects.insert((3, 0), Object::Integer(777));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("Object 3 0:"));
        assert!(out.contains("777"));
        assert_eq!(visited.len(), 3);
    }

    #[test]
    fn dump_object_multiple_children_from_parent() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Child1", Object::Reference((2, 0)));
        dict.set("Child2", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(22));
        doc.objects.insert((3, 0), Object::Integer(33));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("Object 3 0:"));
        assert!(out.contains("22"));
        assert!(out.contains("33"));
    }

    #[test]
    fn dump_object_with_stream_and_decode() {
        let mut doc = Document::new();
        let compressed = zlib_compress(b"stream content here");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("stream content here"));
    }

    #[test]
    fn dump_object_is_contents_propagates() {
        let mut doc = Document::new();
        // Object 1 has /Contents referencing object 2
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        // Object 2 is a valid content stream
        let content = b"BT\n/F1 12 Tf\nET";
        let stream = make_stream(None, content.to_vec());
        doc.objects.insert((2, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("Parsed Content Stream"));
    }

    #[test]
    fn dump_object_separator_between_siblings() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("A", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(1));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("--------------------------------"));
    }

    #[test]
    fn print_object_integer_zero() {
        let (out, _) = print_obj(&Object::Integer(0));
        assert_eq!(out, "0");
    }

    #[test]
    fn print_object_real_negative() {
        let (out, _) = print_obj(&Object::Real(-2.75));
        assert_eq!(out, "-2.75");
    }

    #[test]
    fn print_object_real_large() {
        let (out, _) = print_obj(&Object::Real(99999.5));
        assert_eq!(out, "99999.5");
    }

    #[test]
    fn print_object_empty_string() {
        let (out, _) = print_obj(&Object::String(b"".to_vec(), StringFormat::Literal));
        assert_eq!(out, "()");
    }

    #[test]
    fn print_object_empty_name() {
        let (out, _) = print_obj(&Object::Name(b"".to_vec()));
        assert_eq!(out, "/");
    }

    #[test]
    fn print_object_string_non_utf8() {
        // Non-UTF8 bytes should be handled by from_utf8_lossy with replacement char
        let (out, _) = print_obj(&Object::String(vec![0xFF, 0xFE], StringFormat::Literal));
        assert!(out.starts_with('('));
        assert!(out.ends_with(')'));
        assert!(out.contains('\u{FFFD}'), "Non-UTF8 bytes should produce replacement chars");
    }

    #[test]
    fn print_object_name_non_utf8() {
        let (out, _) = print_obj(&Object::Name(vec![0x80, 0x81]));
        assert!(out.starts_with('/'));
        assert!(out.contains('\u{FFFD}'), "Non-UTF8 name bytes should produce replacement chars");
    }

    #[test]
    fn print_object_reference_nonzero_generation() {
        let obj = Object::Reference((5, 2));
        let (out, refs) = print_obj(&obj);
        assert_eq!(out, "5 2 R");
        assert!(refs.contains(&(false, (5, 2))));
    }

    #[test]
    fn print_object_array_mixed_types() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Name(b"Foo".to_vec()),
            Object::Boolean(true),
            Object::Null,
            Object::Real(1.5),
        ]);
        let (out, _) = print_obj(&arr);
        assert!(out.contains("1"));
        assert!(out.contains("/Foo"));
        assert!(out.contains("true"));
        assert!(out.contains("null"));
        assert!(out.contains("1.5"));
    }

    #[test]
    fn print_object_array_of_arrays() {
        let inner = Object::Array(vec![Object::Integer(10)]);
        let outer = Object::Array(vec![inner]);
        let (out, _) = print_obj(&outer);
        // Should have nested brackets
        let open_count = out.matches('[').count();
        let close_count = out.matches(']').count();
        assert_eq!(open_count, 2, "Expected 2 opening brackets for nested arrays");
        assert_eq!(close_count, 2, "Expected 2 closing brackets for nested arrays");
        assert!(out.contains("10"));
    }

    #[test]
    fn print_object_dict_in_array() {
        let mut dict = Dictionary::new();
        dict.set("K", Object::Integer(5));
        let arr = Object::Array(vec![Object::Dictionary(dict)]);
        let (out, _) = print_obj(&arr);
        assert!(out.contains("<<"));
        assert!(out.contains("/K"));
        assert!(out.contains("5"));
        assert!(out.contains(">>"));
    }

    #[test]
    fn print_object_stream_dict_with_reference_collects_child_ref() {
        // Stream dict entries that are references should be collected
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((20, 0)));
        let stream = Stream::new(dict, b"data".to_vec());
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &Object::Stream(stream), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(child_refs.contains(&(false, (20, 0))), "Reference in stream dict should be collected");
    }

    #[test]
    fn print_object_contents_key_with_non_reference_value() {
        // /Contents with a non-reference value (e.g., an integer) should not crash
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Integer(42));
        let (out, refs) = print_obj(&Object::Dictionary(dict));
        assert!(out.contains("/Contents"));
        assert!(out.contains("42"));
        assert!(refs.is_empty(), "Non-reference Contents value should not add child refs");
    }

    #[test]
    fn print_object_contents_key_with_array_of_refs() {
        // /Contents pointing to an array of references: each ref should get is_contents=true
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &Object::Dictionary(dict), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(child_refs.contains(&(true, (10, 0))), "Array ref under /Contents should have is_contents=true");
        assert!(child_refs.contains(&(true, (11, 0))), "Array ref under /Contents should have is_contents=true");
    }

    #[test]
    fn print_content_data_description_propagated() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"x", "custom-desc", "  ", &config, false, None);
        });
        assert!(out.contains("custom-desc"), "Description should appear in output");
    }

    #[test]
    fn print_content_data_indent_str_used() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"data", "raw", "    ", &config, false, None);
        });
        assert!(out.contains("    Stream content"), "Indent string should prefix stream content line");
    }

    #[test]
    fn print_content_data_is_contents_indent_str_used() {
        let content = b"BT\n/F1 12 Tf\nET";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", ">>> ", &config, true, None);
        });
        assert!(out.contains(">>> Parsed Content Stream"), "Indent string should prefix parsed content header");
    }

    #[test]
    fn print_stream_content_flatedecode_is_contents() {
        // Combined path: FlateDecode decompression + content stream parsing
        let content = b"BT\n/F1 12 Tf\n(Hello) Tj\nET";
        let compressed = zlib_compress(content);
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, true);
        });
        assert!(out.contains("Parsed Content Stream"), "Decoded content stream should be parsed");
    }

    #[test]
    fn print_stream_content_corrupt_flatedecode_not_contents() {
        // Corrupt FlateDecode with is_contents=false → falls back to raw borrowed content
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"not valid zlib data at all".to_vec(),
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("raw"), "Corrupt FlateDecode should fall back to 'raw'");
        assert!(out.contains("not valid zlib data"));
    }

    #[test]
    fn print_stream_content_description_shows_decoded_for_flatedecode() {
        let compressed = zlib_compress(b"text");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("decoded"), "Successfully decompressed should show 'decoded'");
    }

    #[test]
    fn print_stream_content_description_shows_raw_for_no_filter() {
        let stream = make_stream(None, b"plain".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("raw"), "No filter should show 'raw'");
    }

    #[test]
    fn dump_object_stream_dict_refs_traversed() {
        // Stream dict contains references → those children should be traversed
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((2, 0)));
        let stream = Stream::new(dict, b"data".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        doc.objects.insert((2, 0), Object::Integer(42));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Parent stream should be printed");
        assert!(out.contains("Object 2 0:"), "Referenced object in stream dict should be traversed");
        assert!(out.contains("42"));
    }

    #[test]
    fn dump_object_is_contents_direct_param() {
        // Passing is_contents=true directly to dump_object_and_children
        let mut doc = Document::new();
        let content = b"BT\n/F1 12 Tf\nET";
        let stream = make_stream(None, content.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, true, 0);
        });
        assert!(out.contains("Parsed Content Stream"), "Direct is_contents=true should trigger content parsing");
    }

    #[test]
    fn dump_object_with_decode_and_truncate() {
        // Both decode_streams=true and truncate=Some(100) with binary stream
        let mut doc = Document::new();
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("truncated to 100"), "Binary stream should be truncated");
    }

    #[test]
    fn dump_object_diamond_dependency() {
        // A → B, A → C, B → D, C → D  (diamond: D visited once)
        let mut doc = Document::new();
        let mut dict_a = Dictionary::new();
        dict_a.set("B", Object::Reference((2, 0)));
        dict_a.set("C", Object::Reference((3, 0)));
        let mut dict_b = Dictionary::new();
        dict_b.set("D", Object::Reference((4, 0)));
        let mut dict_c = Dictionary::new();
        dict_c.set("D", Object::Reference((4, 0)));

        doc.objects.insert((1, 0), Object::Dictionary(dict_a));
        doc.objects.insert((2, 0), Object::Dictionary(dict_b));
        doc.objects.insert((3, 0), Object::Dictionary(dict_c));
        doc.objects.insert((4, 0), Object::Integer(999));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert_eq!(visited.len(), 4, "All 4 objects should be visited exactly once");
        // Object 4 should appear exactly once (not duplicated)
        let count = out.matches("Object 4 0:").count();
        assert_eq!(count, 1, "Diamond dependency: object 4 should be dumped only once");
    }

    #[test]
    fn dump_object_self_referencing() {
        // An object that references itself
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Self", Object::Reference((1, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        // Should terminate and print the object once
        let count = out.matches("Object 1 0:").count();
        assert_eq!(count, 1, "Self-referencing object should be printed once");
    }

    #[test]
    fn depth_zero_prints_root_only() {
        // depth=0 means print root but don't follow any refs
        let mut doc = Document::new();
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root object");
        assert!(!out.contains("Object 2 0:"), "Should NOT follow child ref");
        assert!(out.contains("depth limit reached"));
        assert!(out.contains("1 references not followed"));
    }

    #[test]
    fn depth_one_follows_immediate_refs_only() {
        // Root -> Child -> Grandchild; depth=1 should show Root + Child but not Grandchild
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root");
        assert!(out.contains("Object 2 0:"), "Should follow immediate child");
        assert!(!out.contains("Object 3 0:"), "Should NOT follow grandchild");
        assert!(out.contains("depth limit reached"));
    }

    #[test]
    fn depth_none_traverses_everything() {
        // depth=None means unlimited (current behavior)
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root");
        assert!(out.contains("Object 2 0:"), "Should print child");
        assert!(out.contains("Object 3 0:"), "Should print grandchild");
        assert!(!out.contains("depth limit reached"));
    }

    #[test]
    fn depth_limit_shows_correct_ref_count() {
        // Root has 3 child refs, depth=0 should say "3 references not followed"
        let mut doc = Document::new();
        doc.objects.insert((2, 0), Object::Dictionary(Dictionary::new()));
        doc.objects.insert((3, 0), Object::Dictionary(Dictionary::new()));
        doc.objects.insert((4, 0), Object::Dictionary(Dictionary::new()));
        let root_dict = Dictionary::from_iter(vec![
            ("A", Object::Reference((2, 0))),
            ("B", Object::Reference((3, 0))),
            ("C", Object::Reference((4, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("3 references not followed"));
    }

    #[test]
    fn collect_reachable_with_depth_limit() {
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=0: only the root (immediate trailer ref)
        let objects = collect_reachable_objects(&doc, Some(0));
        assert!(objects.contains_key("1:0"), "Root should be included");
        assert!(!objects.contains_key("2:0"), "Child should NOT be included at depth 0");

        // depth=1: root + child
        let objects = collect_reachable_objects(&doc, Some(1));
        assert!(objects.contains_key("1:0"));
        assert!(objects.contains_key("2:0"));
        assert!(!objects.contains_key("3:0"), "Grandchild should NOT be included at depth 1");

        // depth=None: everything
        let objects = collect_reachable_objects(&doc, None);
        assert!(objects.contains_key("1:0"));
        assert!(objects.contains_key("2:0"));
        assert!(objects.contains_key("3:0"));
    }

    #[test]
    fn print_single_object_integer() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = default_config();
        let out = output_of(|w| {
            print_single_object(w, &doc, 1, &config);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("42"));
    }

    #[test]
    fn print_single_object_dict_does_not_follow_refs() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Child", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(99));
        let config = default_config();
        let out = output_of(|w| {
            print_single_object(w, &doc, 1, &config);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("2 0 R"));
        // Should NOT follow into object 2
        assert!(!out.contains("Object 2 0:"));
        assert!(!out.contains("99"));
    }

    #[test]
    fn print_single_object_stream_with_decode() {
        let mut doc = Document::new();
        let stream = make_stream(None, b"visible data".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_single_object(w, &doc, 1, &config);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("visible data"));
    }

    #[test]
    fn dump_page_shows_page_header() {
        let doc = build_two_page_doc();
        let config = default_config();
        let out = output_of(|w| {
            dump_page(w, &doc, &PageSpec::Single(1), &config);
        });
        assert!(out.contains("Page 1 (Object"));
    }

    #[test]
    fn dump_page_confines_to_target_page() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_page(w, &doc, &PageSpec::Single(1), &config);
        });
        // Should contain page 1's content but not page 2's
        assert!(out.contains("Page1"), "Should contain page 1 content");
        assert!(!out.contains("Page2"), "Should NOT contain page 2 content");
    }

    #[test]
    fn dump_page_two_shows_only_page_two() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_page(w, &doc, &PageSpec::Single(2), &config);
        });
        assert!(out.contains("Page 2 (Object"));
        assert!(out.contains("Page2"), "Should contain page 2 content");
        assert!(!out.contains("Page1"), "Should NOT contain page 1 content");
    }

    #[test]
    fn object_to_json_null() {
        let val = object_to_json(&Object::Null, &empty_doc(), &json_config());
        assert_eq!(val["type"], "null");
    }

    #[test]
    fn object_to_json_boolean() {
        let val = object_to_json(&Object::Boolean(true), &empty_doc(), &json_config());
        assert_eq!(val["type"], "boolean");
        assert_eq!(val["value"], true);
    }

    #[test]
    fn object_to_json_integer() {
        let val = object_to_json(&Object::Integer(42), &empty_doc(), &json_config());
        assert_eq!(val["type"], "integer");
        assert_eq!(val["value"], 42);
    }

    #[test]
    fn object_to_json_real() {
        let val = object_to_json(&Object::Real(2.72), &empty_doc(), &json_config());
        assert_eq!(val["type"], "real");
    }

    #[test]
    fn object_to_json_name() {
        let val = object_to_json(&Object::Name(b"Page".to_vec()), &empty_doc(), &json_config());
        assert_eq!(val["type"], "name");
        assert_eq!(val["value"], "Page");
    }

    #[test]
    fn object_to_json_string() {
        let val = object_to_json(&Object::String(b"Hello".to_vec(), StringFormat::Literal), &empty_doc(), &json_config());
        assert_eq!(val["type"], "string");
        assert_eq!(val["value"], "Hello");
    }

    #[test]
    fn object_to_json_array() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let val = object_to_json(&arr, &empty_doc(), &json_config());
        assert_eq!(val["type"], "array");
        assert_eq!(val["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn object_to_json_dictionary() {
        let mut dict = Dictionary::new();
        dict.set("Key", Object::Integer(99));
        let val = object_to_json(&Object::Dictionary(dict), &empty_doc(), &json_config());
        assert_eq!(val["type"], "dictionary");
        assert_eq!(val["entries"]["Key"]["value"], 99);
    }

    #[test]
    fn object_to_json_stream() {
        let stream = make_stream(None, b"data".to_vec());
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &json_config());
        assert_eq!(val["type"], "stream");
        assert!(val.get("dict").is_some());
    }

    #[test]
    fn object_to_json_reference() {
        let val = object_to_json(&Object::Reference((5, 0)), &empty_doc(), &json_config());
        assert_eq!(val["type"], "reference");
        assert_eq!(val["object_number"], 5);
        assert_eq!(val["generation"], 0);
    }

    #[test]
    fn object_to_json_stream_with_decode() {
        let stream = make_stream(None, b"text content".to_vec());
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["content"], "text content");
    }

    #[test]
    fn dump_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let config = json_config();
        let out = output_of(|w| dump_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert!(parsed.get("trailer").is_some());
        assert!(parsed.get("objects").is_some());
    }

    #[test]
    fn print_single_object_json_produces_valid_json() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = json_config();
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["object_number"], 1);
        assert_eq!(parsed["generation"], 0);
        assert_eq!(parsed["object"]["type"], "integer");
    }

    #[test]
    fn dump_page_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let config = json_config();
        let out = output_of(|w| dump_page_json(w, &doc, &PageSpec::Single(1), &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["page_number"], 1);
        assert!(parsed.get("objects").is_some());
    }

    #[test]
    fn object_to_json_stream_with_decode_binary() {
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content_binary").is_some(), "Binary stream should have content_binary field");
    }

    #[test]
    fn object_to_json_stream_with_decode_binary_truncated() {
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        let config = DumpConfig { decode_streams: true, truncate: Some(100), json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content_truncated").is_some(), "Truncated binary should have content_truncated field");
    }

    #[test]
    fn object_to_json_stream_no_decode() {
        let stream = make_stream(None, b"text data".to_vec());
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content").is_none(), "No content when decode_streams=false");
        assert!(val.get("content_binary").is_none());
    }

    #[test]
    fn collect_reachable_objects_basic() {
        let doc = build_two_page_doc();
        let objects = collect_reachable_objects(&doc, None);
        assert!(!objects.is_empty(), "Should collect reachable objects");
        // Every reachable object should have a valid JSON value
        for (key, val) in &objects {
            assert!(key.contains(':'), "Key should be obj:gen format, got: {}", key);
            assert!(val.get("type").is_some(), "Each object should have a type field");
        }
    }

    #[test]
    fn collect_reachable_objects_empty_doc() {
        let doc = Document::new();
        let objects = collect_reachable_objects(&doc, None);
        assert!(objects.is_empty(), "Empty doc should have no reachable objects");
    }

    #[test]
    fn dump_page_json_confines_to_page() {
        let doc = build_two_page_doc();
        let config = json_config();
        let spec1 = PageSpec::Single(1);
        let spec2 = PageSpec::Single(2);
        let out1 = output_of(|w| dump_page_json(w, &doc, &spec1, &config));
        let out2 = output_of(|w| dump_page_json(w, &doc, &spec2, &config));
        let parsed1: Value = serde_json::from_str(&out1).unwrap();
        let parsed2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(parsed1["page_number"], 1);
        assert_eq!(parsed2["page_number"], 2);
        // Both should have objects but potentially different sets
        assert!(parsed1.get("objects").is_some());
        assert!(parsed2.get("objects").is_some());
    }

    #[test]
    fn hex_mode_binary_stream() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("00000000  "));
        assert!(!out.contains("---"));
    }

    #[test]
    fn hex_mode_text_stream_unaffected() {
        let mut doc = Document::new();
        let text_content = b"Hello world".to_vec();
        let stream = Stream::new(Dictionary::new(), text_content);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        // Text streams still use --- delimiters
        assert!(out.contains("---"));
    }

    #[test]
    fn hex_mode_json_shows_content_hex() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["content_hex"].is_string());
    }

    #[test]
    fn collect_reachable_ids_from_trailer() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Integer(99)); // unreachable
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(1, 0)));
        assert!(!reachable.contains(&(2, 0)));
    }

    #[test]
    fn collect_reachable_ids_multi_hop() {
        let mut doc = Document::new();
        // Chain: trailer → 1 (dict with ref to 2) → 2 (dict with ref to 3) → 3
        let mut dict1 = Dictionary::new();
        dict1.set(b"Next", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));

        let mut dict2 = Dictionary::new();
        dict2.set(b"Next", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        doc.objects.insert((3, 0), Object::Integer(99));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(1, 0)));
        assert!(reachable.contains(&(2, 0)));
        assert!(reachable.contains(&(3, 0)));
    }

    #[test]
    fn collect_reachable_ids_cycle_safe() {
        let mut doc = Document::new();
        // 1 → 2 → 1 (cycle)
        let mut dict1 = Dictionary::new();
        dict1.set(b"Next", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));

        let mut dict2 = Dictionary::new();
        dict2.set(b"Prev", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        // Should not infinite-loop
        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(1, 0)));
        assert!(reachable.contains(&(2, 0)));
    }

    #[test]
    fn collect_reachable_ids_via_array() {
        let mut doc = Document::new();
        let arr = Object::Array(vec![
            Object::Reference((2, 0)),
            Object::Reference((3, 0)),
        ]);
        doc.objects.insert((1, 0), arr);
        doc.objects.insert((2, 0), Object::Integer(1));
        doc.objects.insert((3, 0), Object::Integer(2));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(2, 0)));
        assert!(reachable.contains(&(3, 0)));
    }

    #[test]
    fn collect_reachable_ids_via_stream_dict() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference((2, 0)));
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((1, 0), Object::Stream(stream));
        doc.objects.insert((2, 0), Object::Integer(42));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(2, 0)));
    }

    #[test]
    fn hex_mode_with_truncate() {
        let mut doc = Document::new();
        // 200 bytes of binary content
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: Some(100),
            json: false,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        // Should show hex dump but truncated to 100 bytes
        assert!(out.contains("00000000  "));
        assert!(out.contains("truncated to 100"));
        // 100 bytes = 6 full lines + 4 bytes = 7 lines
        let hex_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("0000")).collect();
        assert_eq!(hex_lines.len(), 7);
    }

    #[test]
    fn hex_mode_without_decode_streams_no_hex() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        // hex=true but decode_streams=false → no stream content shown at all
        let config = DumpConfig {
            decode_streams: false,
            truncate: None,
            json: false,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(!out.contains("00000000  "));
    }

    #[test]
    fn hex_mode_json_with_truncate() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: Some(100),
            json: true,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Should have content_hex (truncated)
        assert!(parsed["object"]["content_hex"].is_string());
        let hex_str = parsed["object"]["content_hex"].as_str().unwrap();
        // Truncated to 100 bytes → 7 lines of hex dump
        let hex_lines: Vec<&str> = hex_str.lines().filter(|l| l.starts_with("0000")).collect();
        assert_eq!(hex_lines.len(), 7);
    }

    #[test]
    fn hex_mode_json_text_stream_uses_content_not_hex() {
        let mut doc = Document::new();
        let text_content = b"Hello world, this is text".to_vec();
        let stream = Stream::new(Dictionary::new(), text_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: None,
            json: true,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Text stream should use "content", not "content_hex"
        assert!(parsed["object"]["content"].is_string());
        assert!(parsed["object"]["content_hex"].is_null());
    }

    #[test]
    fn json_binary_stream_no_hex_shows_content_binary() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: None,
            json: true,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // No hex → content_binary
        assert!(parsed["object"]["content_binary"].is_string());
        assert!(parsed["object"]["content_hex"].is_null());
    }

    #[test]
    fn json_binary_stream_truncate_no_hex_shows_content_truncated() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: Some(100),
            json: true,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["content_truncated"].is_string());
    }

    #[test]
    fn print_content_data_truncate_zero() {
        // truncate=0 should truncate all binary content to 0 bytes
        let content: Vec<u8> = vec![0x80; 50];
        let config = DumpConfig { decode_streams: false, truncate: Some(0), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("truncated to 0"), "truncate=0 should truncate: {}", out);
    }

    #[test]
    fn print_content_data_truncate_one() {
        // truncate=1 should show only 1 byte of binary
        let content: Vec<u8> = vec![0x80; 100];
        let config = DumpConfig { decode_streams: false, truncate: Some(1), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("truncated to 1"), "truncate=1 should truncate: {}", out);
    }

    #[test]
    fn print_content_data_hex_with_truncation() {
        // hex mode + truncation: hex dump should be truncated too
        let content: Vec<u8> = (0..200).map(|i| i as u8).collect();
        let config = DumpConfig { decode_streams: false, truncate: Some(32), json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("truncated to 32"), "Should show truncation: {}", out);
        assert!(out.contains("00000000"), "Should have hex dump offset");
        // Hex dump of 32 bytes = 2 lines of 16 bytes each
        assert!(out.contains("00000010"), "Should have second hex line for 32 bytes");
        // Should NOT have a third hex line (offset 0x20 = 32)
        assert!(!out.contains("00000020"), "Should not have third hex line: {}", out);
    }

    #[test]
    fn print_content_data_hex_without_truncation() {
        // hex mode without truncation: full hex dump
        let content: Vec<u8> = (0..48).map(|i| i as u8).collect();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("48 bytes"), "Should show full size: {}", out);
        assert!(!out.contains("truncated"));
        // 48 bytes = 3 hex lines
        assert!(out.contains("00000020"), "Should have third hex line for 48 bytes");
    }

    #[test]
    fn print_content_data_warning_with_hex_mode() {
        // Warning should appear alongside hex dump output
        let content: Vec<u8> = vec![0x80; 32];
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, Some("FlateDecode decompression failed"));
        });
        assert!(out.contains("[WARNING: FlateDecode decompression failed]"), "Warning should appear with hex");
        assert!(out.contains("00000000"), "Hex dump should still appear");
    }

    #[test]
    fn print_content_data_warning_with_truncation() {
        // Warning + truncation should both appear
        let content: Vec<u8> = vec![0x80; 200];
        let config = DumpConfig { decode_streams: false, truncate: Some(50), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, Some("unsupported filter: DCTDecode"));
        });
        assert!(out.contains("[WARNING: unsupported filter: DCTDecode]"), "Warning should appear");
        assert!(out.contains("truncated to 50"), "Truncation should apply");
    }

    #[test]
    fn print_content_data_warning_with_hex_and_truncation() {
        // All three: warning + hex + truncation
        let content: Vec<u8> = (0..200).map(|i| i as u8).collect();
        let config = DumpConfig { decode_streams: false, truncate: Some(16), json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, Some("LZWDecode: invalid data"));
        });
        assert!(out.contains("[WARNING: LZWDecode: invalid data]"));
        assert!(out.contains("truncated to 16"));
        assert!(out.contains("00000000"), "Hex dump should appear");
        assert!(!out.contains("00000010"), "Only 16 bytes = 1 hex line");
    }

    #[test]
    fn depth_zero_json_limits_objects() {
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=0: collect_reachable_objects should only include root
        let objects_d0 = collect_reachable_objects(&doc, Some(0));
        assert!(objects_d0.contains_key("1:0"), "Root should be collected at depth 0");
        assert!(!objects_d0.contains_key("2:0"), "Child should NOT be collected at depth 0");
        assert!(!objects_d0.contains_key("3:0"), "Grandchild should NOT be at depth 0");

        // depth=2: should get everything
        let objects_d2 = collect_reachable_objects(&doc, Some(2));
        assert!(objects_d2.contains_key("1:0"));
        assert!(objects_d2.contains_key("2:0"));
        assert!(objects_d2.contains_key("3:0"), "Grandchild should be at depth 2");
    }

    #[test]
    fn depth_one_follows_all_immediate_refs() {
        // Root has 3 children, depth=1 should follow all 3 but not their children
        let mut doc = Document::new();
        let gc = Dictionary::from_iter(vec![("Type", Object::Name(b"Deep".to_vec()))]);
        doc.objects.insert((5, 0), Object::Dictionary(gc));

        let c1 = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"C1".to_vec())),
            ("Deep", Object::Reference((5, 0))),
        ]);
        let c2 = Dictionary::from_iter(vec![("Type", Object::Name(b"C2".to_vec()))]);
        let c3 = Dictionary::from_iter(vec![("Type", Object::Name(b"C3".to_vec()))]);
        doc.objects.insert((2, 0), Object::Dictionary(c1));
        doc.objects.insert((3, 0), Object::Dictionary(c2));
        doc.objects.insert((4, 0), Object::Dictionary(c3));

        let root = Dictionary::from_iter(vec![
            ("A", Object::Reference((2, 0))),
            ("B", Object::Reference((3, 0))),
            ("C", Object::Reference((4, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root");
        assert!(out.contains("Object 2 0:"), "Should follow child A");
        assert!(out.contains("Object 3 0:"), "Should follow child B");
        assert!(out.contains("Object 4 0:"), "Should follow child C");
        assert!(!out.contains("Object 5 0:"), "Should NOT follow grandchild");
        assert!(out.contains("depth limit reached"));
    }

    #[test]
    fn print_stream_content_corrupt_shows_warning() {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, b"not zlib data at all".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, false);
        });
        assert!(out.contains("[WARNING: FlateDecode decompression failed]"), "Warning should propagate: {}", out);
        assert!(out.contains("raw"), "Description should say raw for failed decode");
    }

    #[test]
    fn print_stream_content_unsupported_filter_shows_warning() {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"JBIG2Decode".to_vec()));
        let stream = Stream::new(dict, b"jbig2 data".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("[WARNING: unsupported filter: JBIG2Decode]"), "Should show unsupported warning: {}", out);
    }

    #[test]
    fn print_stream_content_successful_decode_no_warning() {
        let compressed = zlib_compress(b"success");
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, compressed);
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(!out.contains("WARNING"), "Successful decode should have no warning");
        assert!(out.contains("decoded"), "Should say decoded");
    }

    #[test]
    fn object_to_json_stream_unsupported_filter_warning() {
        let stream = make_stream(
            Some(Object::Name(b"CCITTFaxDecode".to_vec())),
            b"fax data".to_vec(),
        );
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        let warning = val.get("decode_warning");
        assert!(warning.is_some(), "Unsupported filter should produce JSON warning");
        assert!(warning.unwrap().as_str().unwrap().contains("unsupported filter"));
    }

    #[test]
    fn object_to_json_stream_pipeline_partial_failure_warning() {
        // Pipeline that partially succeeds then fails
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            b"48656c6c6f>".to_vec(), // hex("Hello"), but "Hello" is not valid zlib
        );
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        let warning = val.get("decode_warning");
        assert!(warning.is_some(), "Pipeline failure should produce JSON warning");
        assert!(warning.unwrap().as_str().unwrap().contains("FlateDecode"));
    }

    #[test]
    fn dump_page_range() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_page(w, &doc, &PageSpec::Range(1, 2), &config);
        });
        assert!(out.contains("Page 1 (Object"));
        assert!(out.contains("Page 2 (Object"));
    }

    #[test]
    fn multi_object_plain_output() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Boolean(true));
        let config = default_config();
        let out = output_of(|w| print_objects(w, &doc, &[1, 2], &config));
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
    }

    #[test]
    fn multi_object_json_wraps_in_array() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Boolean(true));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_objects_json(w, &doc, &[1, 2], &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["objects"].is_array());
        assert_eq!(parsed["objects"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn single_object_json_backward_compat() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_objects_json(w, &doc, &[1], &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Single object should NOT wrap in array
        assert!(parsed["object_number"].is_number());
    }

    #[test]
    fn multi_object_missing_reports_error_in_json() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_objects_json(w, &doc, &[1, 99], &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let objs = parsed["objects"].as_array().unwrap();
        assert_eq!(objs[1]["error"].as_str().unwrap(), "not found");
    }

    #[test]
    fn deref_shows_reference_summary() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let mut target = Dictionary::new();
        target.set("Type", Object::Name(b"Font".to_vec()));
        target.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((2, 0), Object::Dictionary(target));
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("2 0 R =>"));
        assert!(out.contains("/Type /Font"));
    }

    #[test]
    fn deref_false_no_expansion() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("2 0 R"));
        assert!(!out.contains("=>"));
    }

    #[test]
    fn deref_json_adds_resolved() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let ref_obj = &parsed["object"]["entries"]["Ref"];
        assert_eq!(ref_obj["type"], "reference");
        assert!(ref_obj["resolved"].is_object());
        assert_eq!(ref_obj["resolved"]["type"], "integer");
        assert_eq!(ref_obj["resolved"]["value"], 42);
    }

    #[test]
    fn deref_json_no_recursive_deref() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let mut inner_dict = Dictionary::new();
        inner_dict.set("Inner", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(inner_dict));
        doc.objects.insert((3, 0), Object::Integer(99));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let resolved = &parsed["object"]["entries"]["Ref"]["resolved"];
        // Inner reference should NOT be recursively resolved
        let inner_ref = &resolved["entries"]["Inner"];
        assert_eq!(inner_ref["type"], "reference");
        assert!(inner_ref.get("resolved").is_none() || inner_ref["resolved"].is_null());
    }

    #[test]
    fn deref_stream_summary() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let mut stream_dict = Dictionary::new();
        stream_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(stream_dict, vec![0u8; 100]);
        doc.objects.insert((2, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("stream, 100 bytes"));
        assert!(out.contains("FlateDecode"));
    }

    #[test]
    fn raw_shows_compressed_bytes() {
        let mut doc = Document::new();
        let original = b"Hello, World!";
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();
        let compressed_len = compressed.len();

        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, compressed.clone());
        doc.objects.insert((1, 0), Object::Stream(stream));

        // raw mode: should show the compressed bytes, not decoded
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains(&format!("{} bytes", compressed_len)));
        // Should NOT contain the decoded text
        assert!(!out.contains("Hello, World!"));
    }

    #[test]
    fn raw_with_hex() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..64).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains("00000000  "));
    }

    #[test]
    fn raw_with_truncate() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: Some(50), json: false, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains("truncated to 50"));
    }

    #[test]
    fn raw_on_non_stream_is_noop() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("42"));
        assert!(!out.contains("raw"));
    }

    #[test]
    fn raw_json_text_stream() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"Hello text content".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["object"]["raw_content"].as_str().unwrap(), "Hello text content");
        assert!(parsed["object"]["content"].is_null());
    }

    #[test]
    fn raw_json_binary_stream() {
        let mut doc = Document::new();
        let binary: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["raw_content_binary"].as_str().unwrap().contains("32 bytes"));
    }

    #[test]
    fn raw_json_binary_hex() {
        let mut doc = Document::new();
        let binary: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["raw_content_hex"].is_string());
        assert!(parsed["object"]["raw_content_hex"].as_str().unwrap().contains("00000000"));
    }

    #[test]
    fn raw_json_binary_hex_truncate() {
        let mut doc = Document::new();
        let binary: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: Some(32), json: true, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["raw_content_hex"].is_string());
        // Should only show 32 bytes = 2 full hex lines
        let hex_str = parsed["object"]["raw_content_hex"].as_str().unwrap();
        let hex_lines: Vec<&str> = hex_str.lines().filter(|l| l.starts_with("0000")).collect();
        assert_eq!(hex_lines.len(), 2);
    }

    #[test]
    fn raw_text_stream_shows_content() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"Some PDF text stream".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains("Some PDF text stream"));
    }

    #[test]
    fn raw_does_not_parse_content_stream() {
        // Even if the stream has content operations, raw should NOT parse them
        let mut doc = Document::new();
        let content = b"BT /F1 12 Tf (Hello) Tj ET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        // With raw, is_contents=false so it won't try to parse
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        // Should show raw text, not parsed operations
        assert!(out.contains("BT /F1 12 Tf"));
        assert!(!out.contains("Parsed Content Stream"));
    }

}
