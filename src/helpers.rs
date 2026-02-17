use lopdf::{Document, Object, ObjectId};
use std::collections::BTreeSet;

use crate::types::PageSpec;

pub(crate) fn build_page_list(
    doc: &Document,
    page_filter: Option<&PageSpec>,
) -> Result<Vec<(u32, ObjectId)>, String> {
    let pages = doc.get_pages();
    if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            pages.get(&pn).map(|&id| (pn, id))
                .ok_or_else(|| format!("Page {} not found. Document has {} pages.", pn, pages.len()))
        }).collect()
    } else {
        Ok(pages.iter().map(|(&pn, &id)| (pn, id)).collect())
    }
}

pub(crate) fn resolve_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a lopdf::Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Dictionary(d) => Some(d),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn resolve_array<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a [Object]> {
    match obj {
        Object::Array(a) => Some(a),
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Array(a) => Some(a),
            _ => None,
        },
        _ => None,
    }
}

/// Extracts a String from an Object::String or Object::Name (lossy UTF-8 conversion).
pub(crate) fn obj_to_string_lossy(obj: &Object) -> Option<String> {
    match obj {
        Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

/// Extracts a String from an Object::Name only (lossy UTF-8 conversion).
pub(crate) fn name_to_string(obj: &Object) -> Option<String> {
    match obj {
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

pub(crate) fn format_dict_value(obj: &Object) -> String {
    match obj {
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::Integer(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::Boolean(b) => b.to_string(),
        Object::String(bytes, _) => format!("({})", String::from_utf8_lossy(bytes)),
        Object::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_dict_value).collect();
            format!("[{}]", items.join(" "))
        }
        Object::Reference(id) => format!("{} {} R", id.0, id.1),
        Object::Null => "null".to_string(),
        Object::Dictionary(_) => "<<...>>".to_string(),
        Object::Stream(_) => "<<stream>>".to_string(),
    }
}

pub(crate) fn format_operation(op: &lopdf::content::Operation) -> String {
    if op.operands.is_empty() {
        return op.operator.clone();
    }
    let operands: Vec<String> = op.operands.iter().map(format_dict_value).collect();
    format!("{} {}", operands.join(" "), op.operator)
}

pub(crate) fn format_color_space(obj: &Object, doc: &Document) -> String {
    match obj {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Array(arr) => {
            let names: Vec<String> = arr.iter().map(|item| match item {
                Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                Object::Reference(id) => format!("{} {} R", id.0, id.1),
                Object::Integer(i) => i.to_string(),
                _ => "?".to_string(),
            }).collect();
            format!("[{}]", names.join(" "))
        }
        Object::Reference(id) => {
            if let Ok(resolved) = doc.get_object(*id) {
                format_color_space(resolved, doc)
            } else {
                format!("{} {} R", id.0, id.1)
            }
        }
        _ => "-".to_string(),
    }
}

pub(crate) fn format_filter(obj: &Object) -> String {
    match obj {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Array(arr) => {
            let names: Vec<String> = arr.iter().map(|item| match item {
                Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                _ => "?".to_string(),
            }).collect();
            names.join(", ")
        }
        _ => "-".to_string(),
    }
}

pub(crate) fn object_type_label(obj: &Object) -> String {
    let dict = match obj {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return "-".to_string(),
    };
    match dict.get_type() {
        Ok(name) => String::from_utf8_lossy(name).into_owned(),
        Err(_) => "-".to_string(),
    }
}

pub(crate) fn walk_name_tree(doc: &Document, dict: &lopdf::Dictionary) -> Vec<(String, Object)> {
    let mut results = Vec::new();
    let mut visited = BTreeSet::new();
    walk_name_tree_inner(doc, dict, &mut results, &mut visited);
    results
}

fn walk_name_tree_inner(
    doc: &Document,
    dict: &lopdf::Dictionary,
    results: &mut Vec<(String, Object)>,
    visited: &mut BTreeSet<ObjectId>,
) {
    if let Ok(Object::Array(names)) = dict.get(b"Names") {
        let mut i = 0;
        while i + 1 < names.len() {
            let key = match &names[i] {
                Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                _ => { i += 2; continue; }
            };
            results.push((key, names[i + 1].clone()));
            i += 2;
        }
    }
    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        for kid in kids {
            let kid_id = match kid {
                Object::Reference(id) => *id,
                _ => continue,
            };
            if visited.contains(&kid_id) { continue; }
            visited.insert(kid_id);
            if let Ok(Object::Dictionary(kid_dict)) = doc.get_object(kid_id) {
                walk_name_tree_inner(doc, kid_dict, results, visited);
            }
        }
    }
}

pub(crate) fn walk_number_tree(doc: &Document, dict: &lopdf::Dictionary) -> Vec<(i64, Object)> {
    let mut results = Vec::new();
    let mut visited = BTreeSet::new();
    walk_number_tree_inner(doc, dict, &mut results, &mut visited);
    results
}

fn walk_number_tree_inner(
    doc: &Document,
    dict: &lopdf::Dictionary,
    results: &mut Vec<(i64, Object)>,
    visited: &mut BTreeSet<ObjectId>,
) {
    if let Ok(Object::Array(nums)) = dict.get(b"Nums") {
        let mut i = 0;
        while i + 1 < nums.len() {
            let key = match &nums[i] {
                Object::Integer(n) => *n,
                _ => { i += 2; continue; }
            };
            results.push((key, nums[i + 1].clone()));
            i += 2;
        }
    }
    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        for kid in kids {
            let kid_id = match kid {
                Object::Reference(id) => *id,
                _ => continue,
            };
            if visited.contains(&kid_id) { continue; }
            visited.insert(kid_id);
            if let Ok(Object::Dictionary(kid_dict)) = doc.get_object(kid_id) {
                walk_number_tree_inner(doc, kid_dict, results, visited);
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
    use lopdf::Object;

    #[test]
    fn object_type_label_dictionary_with_type() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        assert_eq!(object_type_label(&Object::Dictionary(dict)), "Page");
    }

    #[test]
    fn object_type_label_dictionary_without_type() {
        let dict = Dictionary::new();
        assert_eq!(object_type_label(&Object::Dictionary(dict)), "-");
    }

    #[test]
    fn object_type_label_stream_with_type() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, vec![]);
        assert_eq!(object_type_label(&Object::Stream(stream)), "XObject");
    }

    #[test]
    fn object_type_label_stream_without_type() {
        let stream = Stream::new(Dictionary::new(), vec![]);
        assert_eq!(object_type_label(&Object::Stream(stream)), "-");
    }

    #[test]
    fn object_type_label_integer() {
        assert_eq!(object_type_label(&Object::Integer(42)), "-");
    }

    #[test]
    fn object_type_label_null() {
        assert_eq!(object_type_label(&Object::Null), "-");
    }

    #[test]
    fn format_dict_value_name() {
        let val = format_dict_value(&Object::Name(b"Page".to_vec()));
        assert_eq!(val, "/Page");
    }

    #[test]
    fn format_dict_value_integer() {
        assert_eq!(format_dict_value(&Object::Integer(42)), "42");
    }

    #[test]
    fn format_dict_value_real() {
        assert_eq!(format_dict_value(&Object::Real(2.72)), "2.72");
    }

    #[test]
    fn format_dict_value_boolean() {
        assert_eq!(format_dict_value(&Object::Boolean(true)), "true");
        assert_eq!(format_dict_value(&Object::Boolean(false)), "false");
    }

    #[test]
    fn format_dict_value_string() {
        let val = format_dict_value(&Object::String(b"hello".to_vec(), StringFormat::Literal));
        assert_eq!(val, "(hello)");
    }

    #[test]
    fn format_dict_value_array() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        assert_eq!(format_dict_value(&arr), "[1 2]");
    }

    #[test]
    fn format_dict_value_reference() {
        assert_eq!(format_dict_value(&Object::Reference((5, 0))), "5 0 R");
    }

    #[test]
    fn format_dict_value_null() {
        assert_eq!(format_dict_value(&Object::Null), "null");
    }

    #[test]
    fn format_dict_value_dictionary() {
        let dict = Dictionary::new();
        assert_eq!(format_dict_value(&Object::Dictionary(dict)), "<<...>>");
    }

    #[test]
    fn format_dict_value_stream() {
        let stream = make_stream(None, vec![]);
        assert_eq!(format_dict_value(&Object::Stream(stream)), "<<stream>>");
    }

    #[test]
    fn format_dict_value_nested_array() {
        let inner = Object::Array(vec![Object::Name(b"X".to_vec())]);
        let outer = Object::Array(vec![inner, Object::Integer(3)]);
        let val = format_dict_value(&outer);
        assert_eq!(val, "[[/X] 3]");
    }

    #[test]
    fn format_operation_no_operands() {
        let op = lopdf::content::Operation::new("BT", vec![]);
        assert_eq!(format_operation(&op), "BT");
    }

    #[test]
    fn format_operation_string_tj() {
        let op = lopdf::content::Operation::new("Tj", vec![Object::String(b"Hello".to_vec(), StringFormat::Literal)]);
        assert_eq!(format_operation(&op), "(Hello) Tj");
    }

    #[test]
    fn format_operation_name_and_int() {
        let op = lopdf::content::Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), Object::Integer(12)]);
        assert_eq!(format_operation(&op), "/F1 12 Tf");
    }

    #[test]
    fn format_operation_tj_array() {
        let arr = Object::Array(vec![
            Object::String(b"H".to_vec(), StringFormat::Literal),
            Object::Integer(-20),
            Object::String(b"ello".to_vec(), StringFormat::Literal),
        ]);
        let op = lopdf::content::Operation::new("TJ", vec![arr]);
        assert_eq!(format_operation(&op), "[(H) -20 (ello)] TJ");
    }

    #[test]
    fn format_operation_reference() {
        let op = lopdf::content::Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]);
        assert_eq!(format_operation(&op), "/Im0 Do");
    }

    #[test]
    fn format_color_space_name() {
        let doc = Document::new();
        let obj = Object::Name(b"DeviceGray".to_vec());
        assert_eq!(format_color_space(&obj, &doc), "DeviceGray");
    }

    #[test]
    fn format_color_space_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Integer(5),
        ]);
        assert_eq!(format_color_space(&obj, &doc), "[ICCBased 5]");
    }

    #[test]
    fn format_filter_name() {
        let obj = Object::Name(b"DCTDecode".to_vec());
        assert_eq!(format_filter(&obj), "DCTDecode");
    }

    #[test]
    fn format_filter_array() {
        let obj = Object::Array(vec![
            Object::Name(b"FlateDecode".to_vec()),
            Object::Name(b"ASCII85Decode".to_vec()),
        ]);
        assert_eq!(format_filter(&obj), "FlateDecode, ASCII85Decode");
    }

    #[test]
    fn format_color_space_reference_in_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Reference((7, 0)),
        ]);
        assert_eq!(format_color_space(&obj, &doc), "[ICCBased 7 0 R]");
    }

    #[test]
    fn format_color_space_unknown_type_shows_dash() {
        let doc = Document::new();
        let obj = Object::Integer(42);
        assert_eq!(format_color_space(&obj, &doc), "-");
    }

    #[test]
    fn format_color_space_array_with_unknown_item() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Name(b"Indexed".to_vec()),
            Object::Boolean(true), // unusual
        ]);
        assert_eq!(format_color_space(&obj, &doc), "[Indexed ?]");
    }

    #[test]
    fn format_filter_unknown_type_shows_dash() {
        let obj = Object::Integer(42);
        assert_eq!(format_filter(&obj), "-");
    }

    #[test]
    fn format_filter_array_with_unknown_item() {
        let obj = Object::Array(vec![
            Object::Name(b"FlateDecode".to_vec()),
            Object::Integer(99), // unusual
        ]);
        assert_eq!(format_filter(&obj), "FlateDecode, ?");
    }

    #[test]
    fn walk_name_tree_leaf_only() {
        let mut doc = Document::new();
        let mut leaf = Dictionary::new();
        leaf.set("Names", Object::Array(vec![
            Object::String(b"file1.pdf".to_vec(), StringFormat::Literal),
            Object::Integer(1),
            Object::String(b"file2.pdf".to_vec(), StringFormat::Literal),
            Object::Integer(2),
        ]));
        doc.objects.insert((1, 0), Object::Dictionary(leaf.clone()));

        let results = walk_name_tree(&doc, &leaf);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "file1.pdf");
        assert_eq!(results[1].0, "file2.pdf");
    }

    #[test]
    fn walk_name_tree_with_kids() {
        let mut doc = Document::new();
        let mut child = Dictionary::new();
        child.set("Names", Object::Array(vec![
            Object::String(b"a.txt".to_vec(), StringFormat::Literal),
            Object::Integer(10),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let mut root = Dictionary::new();
        root.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(root.clone()));

        let results = walk_name_tree(&doc, &root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "a.txt");
    }

    #[test]
    fn walk_name_tree_empty() {
        let doc = Document::new();
        let dict = Dictionary::new();
        let results = walk_name_tree(&doc, &dict);
        assert!(results.is_empty());
    }

    #[test]
    fn walk_name_tree_cycle_protection() {
        let mut doc = Document::new();
        // Create two nodes that reference each other
        let mut node_a = Dictionary::new();
        node_a.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(node_a.clone()));
        let mut node_b = Dictionary::new();
        node_b.set("Kids", Object::Array(vec![Object::Reference((1, 0))]));
        node_b.set("Names", Object::Array(vec![
            Object::String(b"found".to_vec(), StringFormat::Literal),
            Object::Integer(1),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(node_b));

        let results = walk_name_tree(&doc, &node_a);
        // Should find "found" without infinite loop
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "found");
    }

    #[test]
    fn walk_number_tree_leaf_only() {
        let doc = Document::new();
        let mut leaf = Dictionary::new();
        leaf.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Name(b"D".to_vec()),
            Object::Integer(5), Object::Name(b"r".to_vec()),
        ]));
        let results = walk_number_tree(&doc, &leaf);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 0);
        assert_eq!(results[1].0, 5);
    }

    #[test]
    fn walk_number_tree_with_kids() {
        let mut doc = Document::new();
        let mut child = Dictionary::new();
        child.set("Nums", Object::Array(vec![
            Object::Integer(3), Object::Integer(99),
        ]));
        doc.objects.insert((5, 0), Object::Dictionary(child));
        let mut root = Dictionary::new();
        root.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        doc.objects.insert((4, 0), Object::Dictionary(root.clone()));

        let results = walk_number_tree(&doc, &root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 3);
    }

    #[test]
    fn walk_number_tree_empty() {
        let doc = Document::new();
        let dict = Dictionary::new();
        let results = walk_number_tree(&doc, &dict);
        assert!(results.is_empty());
    }

    #[test]
    fn walk_number_tree_cycle_protection() {
        let mut doc = Document::new();
        // Two nodes that reference each other
        let mut node_a = Dictionary::new();
        node_a.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(node_a.clone()));
        let mut node_b = Dictionary::new();
        node_b.set("Kids", Object::Array(vec![Object::Reference((1, 0))]));
        node_b.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Integer(1),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(node_b));

        let results = walk_number_tree(&doc, &node_a);
        // Should find the one entry from node_b without infinite loop
        assert_eq!(results.len(), 1);
    }

}
