use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};

use crate::helpers::object_type_label;
use crate::object::deref_summary;

pub(crate) fn collect_references_in_object(obj: &Object, target_id: ObjectId, path: &str) -> Vec<String> {
    let mut found = Vec::new();
    collect_references_in_object_into(obj, target_id, path, &mut found);
    found
}

pub(crate) fn collect_references_in_object_into(obj: &Object, target_id: ObjectId, path: &str, found: &mut Vec<String>) {
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
    pub kind: &'static str,
    pub type_label: String,
    pub paths: Vec<String>,
}

pub(crate) fn collect_reverse_refs(doc: &Document, target_id: ObjectId) -> Vec<ReverseRef> {
    let mut refs = Vec::new();
    let mut paths = Vec::new();
    for (&(obj_num, generation), object) in &doc.objects {
        paths.clear();
        collect_references_in_object_into(object, target_id, "", &mut paths);
        if !paths.is_empty() {
            refs.push(ReverseRef {
                obj_num,
                generation,
                kind: object.enum_variant(),
                type_label: object_type_label(object),
                paths: std::mem::take(&mut paths),
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
            entry["summary"] = json!(deref_summary(resolved));
        }
        entry
    }).collect()
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
        Object::Dictionary(dict) => {
            for (key, val) in dict.iter() {
                let child_path = format!("{}/{}", path, String::from_utf8_lossy(key));
                collect_refs_recursive(val, &child_path, refs);
            }
        }
        _ => {}
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
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
