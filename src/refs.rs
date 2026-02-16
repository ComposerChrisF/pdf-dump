use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::helpers::{object_type_label, format_dict_value};

pub(crate) fn collect_references_in_object(obj: &Object, target_id: ObjectId, path: &str) -> Vec<String> {
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

pub(crate) struct ReverseRef {
    pub obj_num: u32,
    pub generation: u16,
    pub kind: String,
    pub type_label: String,
    pub paths: Vec<String>,
}

pub(crate) fn collect_reverse_refs(doc: &Document, target_id: ObjectId) -> Vec<ReverseRef> {
    let mut refs = Vec::new();
    for (&(obj_num, generation), object) in &doc.objects {
        let paths = collect_references_in_object(object, target_id, "");
        if !paths.is_empty() {
            refs.push(ReverseRef {
                obj_num,
                generation,
                kind: object.enum_variant().to_string(),
                type_label: object_type_label(object),
                paths,
            });
        }
    }
    refs
}

pub(crate) fn reverse_refs_to_json(refs: &[ReverseRef]) -> Vec<Value> {
    refs.iter().map(|r| json!({
        "object_number": r.obj_num,
        "generation": r.generation,
        "kind": r.kind,
        "type": r.type_label,
        "via_keys": r.paths,
    })).collect()
}

pub(crate) fn collect_forward_refs_json(doc: &Document, object: &Object) -> Vec<Value> {
    collect_refs_with_paths(object).iter().map(|(path, ref_id)| {
        let mut entry = json!({
            "path": path,
            "object_number": ref_id.0,
            "generation": ref_id.1,
        });
        if let Ok(resolved) = doc.get_object(*ref_id) {
            entry["summary"] = json!(deref_summary(resolved, doc));
        }
        entry
    }).collect()
}

pub(crate) fn print_refs_to(writer: &mut impl Write, doc: &Document, target_num: u32) {
    let target_id = (target_num, 0);
    writeln!(writer, "Objects referencing {} 0 R:\n", target_num).unwrap();

    let rev_refs = collect_reverse_refs(doc, target_id);
    for r in &rev_refs {
        writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} via {}", r.obj_num, r.generation, r.kind, r.type_label, r.paths.join(", ")).unwrap();
    }
    writeln!(writer, "\nFound {} objects referencing {} 0 R.", rev_refs.len(), target_num).unwrap();
}

pub(crate) fn print_refs_to_json(writer: &mut impl Write, doc: &Document, target_num: u32) {
    let target_id = (target_num, 0);
    let references = reverse_refs_to_json(&collect_reverse_refs(doc, target_id));
    let output = json!({
        "target_object": target_num,
        "reference_count": references.len(),
        "references": references,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

pub(crate) fn collect_refs_from_dict(dict: &lopdf::Dictionary) -> Vec<(String, ObjectId)> {
    let mut refs = Vec::new();
    for (key, val) in dict.iter() {
        let key_str = format!("/{}", String::from_utf8_lossy(key));
        collect_refs_recursive(val, &key_str, &mut refs);
    }
    refs
}

pub(crate) fn collect_refs_with_paths(obj: &Object) -> Vec<(String, ObjectId)> {
    match obj {
        Object::Dictionary(dict) => collect_refs_from_dict(dict),
        Object::Stream(stream) => collect_refs_from_dict(&stream.dict),
        Object::Array(arr) => {
            let mut refs = Vec::new();
            for (i, val) in arr.iter().enumerate() {
                let key_str = format!("[{}]", i);
                collect_refs_recursive(val, &key_str, &mut refs);
            }
            refs
        }
        _ => Vec::new(),
    }
}

fn collect_refs_recursive(obj: &Object, path: &str, refs: &mut Vec<(String, ObjectId)>) {
    match obj {
        Object::Reference(id) => {
            refs.push((path.to_string(), *id));
        }
        Object::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                collect_refs_recursive(val, &child_path, refs);
            }
        }
        _ => {}
    }
}

fn deref_summary(obj: &Object, _doc: &Document) -> String {
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

    #[test]
    fn collect_refs_direct_reference() {
        let target = (5, 0);
        let obj = Object::Reference(target);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "");
    }

    #[test]
    fn collect_refs_in_dict() {
        let target = (5, 0);
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target));
        let obj = Object::Dictionary(dict);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Font");
    }

    #[test]
    fn collect_refs_in_array() {
        let target = (5, 0);
        let obj = Object::Array(vec![
            Object::Integer(1),
            Object::Reference(target),
        ]);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "[1]");
    }

    #[test]
    fn collect_refs_nested_dict() {
        let target = (5, 0);
        let mut inner = Dictionary::new();
        inner.set(b"Ref", Object::Reference(target));
        let mut outer = Dictionary::new();
        outer.set(b"Resources", Object::Dictionary(inner));
        let obj = Object::Dictionary(outer);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Resources/Ref");
    }

    #[test]
    fn collect_refs_in_stream_dict() {
        let target = (5, 0);
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target));
        let stream = Stream::new(dict, vec![]);
        let obj = Object::Stream(stream);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Font");
    }

    #[test]
    fn collect_refs_no_match() {
        let target = (5, 0);
        let obj = Object::Reference((99, 0));
        let paths = collect_references_in_object(&obj, target, "");
        assert!(paths.is_empty());
    }

    #[test]
    fn print_refs_to_finds_referencing_objects() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Found 1 objects referencing 5 0 R."));
        assert!(out.contains("/Font"));
    }

    #[test]
    fn print_refs_to_no_references() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));
        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Found 0 objects referencing 5 0 R."));
    }

    #[test]
    fn print_refs_to_json_produces_valid_json() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to_json(w, &doc, 5));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["target_object"], 5);
        assert_eq!(parsed["reference_count"], 1);
        assert!(parsed["references"].is_array());
    }

    #[test]
    fn collect_refs_multiple_paths_same_object() {
        // Object references target from two different dict keys
        let target = (5, 0);
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target));
        dict.set(b"ExtGState", Object::Reference(target));
        let obj = Object::Dictionary(dict);

        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"/ExtGState".to_string()));
        assert!(paths.contains(&"/Font".to_string()));
    }

    #[test]
    fn collect_refs_mixed_containers_dict_array_ref() {
        // Dict → Array → Reference
        let target = (7, 0);
        let inner_array = Object::Array(vec![
            Object::Integer(42),
            Object::Reference(target),
        ]);
        let mut dict = Dictionary::new();
        dict.set(b"Kids", inner_array);
        let obj = Object::Dictionary(dict);

        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Kids[1]");
    }

    #[test]
    fn collect_refs_deeply_nested() {
        // Dict → Dict → Array → Dict → Reference
        let target = (10, 0);
        let mut innermost = Dictionary::new();
        innermost.set(b"Ref", Object::Reference(target));
        let arr = Object::Array(vec![Object::Dictionary(innermost)]);
        let mut mid = Dictionary::new();
        mid.set(b"Items", arr);
        let mut outer = Dictionary::new();
        outer.set(b"Resources", Object::Dictionary(mid));
        let obj = Object::Dictionary(outer);

        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Resources/Items[0]/Ref");
    }

    #[test]
    fn collect_refs_non_matching_reference_ignored() {
        let target = (5, 0);
        let obj = Object::Array(vec![
            Object::Reference((1, 0)),
            Object::Reference((2, 0)),
            Object::Integer(99),
        ]);
        let paths = collect_references_in_object(&obj, target, "");
        assert!(paths.is_empty());
    }

    #[test]
    fn collect_refs_primitive_types_return_empty() {
        let target = (5, 0);
        assert!(collect_references_in_object(&Object::Null, target, "").is_empty());
        assert!(collect_references_in_object(&Object::Boolean(true), target, "").is_empty());
        assert!(collect_references_in_object(&Object::Integer(42), target, "").is_empty());
        assert!(collect_references_in_object(&Object::Real(2.72), target, "").is_empty());
        assert!(collect_references_in_object(&Object::Name(b"Test".to_vec()), target, "").is_empty());
        assert!(collect_references_in_object(
            &Object::String(b"test".to_vec(), StringFormat::Literal), target, ""
        ).is_empty());
    }

    #[test]
    fn print_refs_to_multiple_referencing_objects() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        // Two different objects reference the target
        let mut dict1 = Dictionary::new();
        dict1.set(b"Font", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));

        let mut dict2 = Dictionary::new();
        dict2.set(b"XObject", Object::Reference(target_id));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Found 2 objects referencing 5 0 R."));
        assert!(out.contains("/Font"));
        assert!(out.contains("/XObject"));
    }

    #[test]
    fn print_refs_to_nonexistent_target() {
        // Target object doesn't exist — should still work, just find 0 refs
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(10));

        let out = output_of(|w| print_refs_to(w, &doc, 999));
        assert!(out.contains("Found 0 objects referencing 999 0 R."));
    }

    #[test]
    fn print_refs_to_json_multiple_via_keys() {
        // Single object has two paths to the target
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        let mut dict = Dictionary::new();
        dict.set(b"A", Object::Reference(target_id));
        dict.set(b"B", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to_json(w, &doc, 5));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["reference_count"], 1);
        let via_keys = parsed["references"][0]["via_keys"].as_array().unwrap();
        assert_eq!(via_keys.len(), 2);
    }

    #[test]
    fn print_refs_to_json_no_references() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));

        let out = output_of(|w| print_refs_to_json(w, &doc, 99));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["target_object"], 99);
        assert_eq!(parsed["reference_count"], 0);
        assert!(parsed["references"].as_array().unwrap().is_empty());
    }

    #[test]
    fn print_refs_to_shows_object_type_label() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        // Dict with /Type = Page
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Page".to_vec()));
        dict.set(b"Contents", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Page"));
        assert!(out.contains("Dictionary"));
    }

    #[test]
    fn collect_refs_with_paths_from_dict() {
        let dict = Dictionary::from_iter(vec![
            ("A", Object::Reference((1, 0))),
            ("B", Object::Integer(42)),
            ("C", Object::Reference((2, 0))),
        ]);
        let refs = collect_refs_with_paths(&Object::Dictionary(dict));
        assert_eq!(refs.len(), 2);
        // Should have /A and /C paths
        let paths: Vec<&str> = refs.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"/A"));
        assert!(paths.contains(&"/C"));
    }

    #[test]
    fn collect_refs_with_paths_array_in_dict() {
        let dict = Dictionary::from_iter(vec![
            ("Kids", Object::Array(vec![
                Object::Reference((1, 0)),
                Object::Reference((2, 0)),
            ])),
        ]);
        let refs = collect_refs_with_paths(&Object::Dictionary(dict));
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].0, "/Kids[0]");
        assert_eq!(refs[1].0, "/Kids[1]");
    }

    #[test]
    fn collect_refs_with_paths_from_stream() {
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((5, 0)));
        dict.set("Length", Object::Integer(100));
        let stream = Stream::new(dict, vec![1, 2, 3]);
        let refs = collect_refs_with_paths(&Object::Stream(stream));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "/Font");
        assert_eq!(refs[0].1, (5, 0));
    }

    #[test]
    fn collect_refs_with_paths_from_bare_array() {
        let arr = Object::Array(vec![
            Object::Reference((1, 0)),
            Object::Integer(42),
            Object::Reference((3, 0)),
        ]);
        let refs = collect_refs_with_paths(&arr);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].0, "[0]");
        assert_eq!(refs[0].1, (1, 0));
        assert_eq!(refs[1].0, "[2]");
        assert_eq!(refs[1].1, (3, 0));
    }

    #[test]
    fn collect_refs_with_paths_no_refs() {
        let dict = Dictionary::from_iter(vec![
            ("A", Object::Integer(1)),
            ("B", Object::Name(b"Foo".to_vec())),
        ]);
        let refs = collect_refs_with_paths(&Object::Dictionary(dict));
        assert!(refs.is_empty());
    }

    #[test]
    fn collect_refs_with_paths_scalar_object() {
        // Scalars have no refs
        let refs = collect_refs_with_paths(&Object::Integer(42));
        assert!(refs.is_empty());
        let refs = collect_refs_with_paths(&Object::Null);
        assert!(refs.is_empty());
        let refs = collect_refs_with_paths(&Object::Boolean(true));
        assert!(refs.is_empty());
    }

    #[test]
    fn collect_refs_with_paths_nested_array_in_array() {
        // Array containing a nested array with references
        let arr = Object::Array(vec![
            Object::Array(vec![
                Object::Reference((1, 0)),
                Object::Reference((2, 0)),
            ]),
        ]);
        let refs = collect_refs_with_paths(&arr);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].0, "[0][0]");
        assert_eq!(refs[1].0, "[0][1]");
    }

}
