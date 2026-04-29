use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;

use crate::refs::{collect_refs_from_dict, collect_refs_with_paths};
use crate::types::DumpConfig;

fn tree_node_label(obj: &Object) -> String {
    match obj {
        Object::Dictionary(dict) => {
            if let Ok(Object::Name(type_name)) = dict.get(b"Type") {
                let name = String::from_utf8_lossy(type_name);
                name.into_owned()
            } else {
                format!("Dictionary, {} keys", dict.len())
            }
        }
        Object::Stream(stream) => {
            if let Ok(Object::Name(type_name)) = stream.dict.get(b"Type") {
                let name = String::from_utf8_lossy(type_name);
                format!("{}, {} bytes", name, stream.content.len())
            } else {
                format!("Stream, {} bytes", stream.content.len())
            }
        }
        Object::Array(arr) => format!("Array, {} items", arr.len()),
        Object::Boolean(b) => format!("Boolean({})", b),
        Object::Integer(i) => format!("Integer({})", i),
        Object::Real(r) => format!("Real({})", r),
        Object::Name(n) => format!("Name({})", String::from_utf8_lossy(n)),
        Object::String(s, _) => format!("String({})", String::from_utf8_lossy(s)),
        Object::Null => "Null".to_string(),
        Object::Reference(id) => format!("Reference({} {})", id.0, id.1),
    }
}

pub(crate) fn print_tree(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    wln!(writer, "Reference Tree:\n");
    wln!(writer, "Trailer");

    let mut visited = BTreeSet::new();
    let trailer_refs = collect_refs_from_dict(&doc.trailer);

    for (path, ref_id) in trailer_refs {
        print_tree_node(writer, ref_id, doc, &mut visited, 1, &path, config);
    }
}

fn print_tree_node(
    writer: &mut impl Write,
    obj_id: ObjectId,
    doc: &Document,
    visited: &mut BTreeSet<ObjectId>,
    depth: usize,
    key_path: &str,
    config: &DumpConfig,
) {
    let indent = "  ".repeat(depth);

    if visited.contains(&obj_id) {
        wln!(
            writer,
            "{}{} -> {} {} (visited)",
            indent,
            key_path,
            obj_id.0,
            obj_id.1
        );
        return;
    }

    if let Some(max_depth) = config.depth
        && depth > max_depth
    {
        wln!(
            writer,
            "{}{} -> {} {} (depth limit reached)",
            indent,
            key_path,
            obj_id.0,
            obj_id.1
        );
        return;
    }

    visited.insert(obj_id);

    match doc.get_object(obj_id) {
        Ok(object) => {
            let label = tree_node_label(object);
            wln!(
                writer,
                "{}{} -> {} {} ({})",
                indent,
                key_path,
                obj_id.0,
                obj_id.1,
                label
            );

            let child_refs = collect_refs_with_paths(object);
            for (path, child_id) in child_refs {
                print_tree_node(writer, child_id, doc, visited, depth + 1, &path, config);
            }
        }
        Err(_) => {
            wln!(
                writer,
                "{}{} -> {} {} (missing)",
                indent,
                key_path,
                obj_id.0,
                obj_id.1
            );
        }
    }
}

pub(crate) fn tree_json_value(doc: &Document, config: &DumpConfig) -> Value {
    let mut visited = BTreeSet::new();
    let trailer_refs = collect_refs_from_dict(&doc.trailer);

    let children: Vec<Value> = trailer_refs
        .iter()
        .map(|(path, ref_id)| tree_node_to_json(*ref_id, doc, &mut visited, 1, path, config))
        .collect();

    json!({
        "tree": {
            "node": "Trailer",
            "children": children,
        }
    })
}

fn tree_node_to_json(
    obj_id: ObjectId,
    doc: &Document,
    visited: &mut BTreeSet<ObjectId>,
    depth: usize,
    key_path: &str,
    config: &DumpConfig,
) -> Value {
    if visited.contains(&obj_id) {
        return json!({
            "key": key_path,
            "object": format!("{} {}", obj_id.0, obj_id.1),
            "status": "visited",
        });
    }

    if let Some(max_depth) = config.depth
        && depth > max_depth
    {
        return json!({
            "key": key_path,
            "object": format!("{} {}", obj_id.0, obj_id.1),
            "status": "depth_limit_reached",
        });
    }

    visited.insert(obj_id);

    match doc.get_object(obj_id) {
        Ok(object) => {
            let label = tree_node_label(object);
            let child_refs = collect_refs_with_paths(object);
            let children: Vec<Value> = child_refs
                .iter()
                .map(|(path, ref_id)| {
                    tree_node_to_json(*ref_id, doc, visited, depth + 1, path, config)
                })
                .collect();
            let mut node = json!({
                "key": key_path,
                "object": format!("{} {}", obj_id.0, obj_id.1),
                "label": label,
            });
            if !children.is_empty() {
                node["children"] = json!(children);
            }
            node
        }
        Err(_) => {
            json!({
                "key": key_path,
                "object": format!("{} {}", obj_id.0, obj_id.1),
                "status": "missing",
            })
        }
    }
}

// ── DOT output for tree ──────────────────────────────────────────────

fn escape_dot(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn print_tree_dot(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    wln!(writer, "digraph pdf {{");
    wln!(writer, "  rankdir=LR;");
    wln!(writer, "  node [shape=box, fontname=\"monospace\"];");
    wln!(writer, "  \"trailer\" [label=\"Trailer\"];");

    let mut visited = BTreeSet::new();
    let trailer_refs = collect_refs_from_dict(&doc.trailer);

    for (path, ref_id) in trailer_refs {
        emit_dot_node(
            writer,
            ref_id,
            doc,
            &mut visited,
            1,
            &path,
            "trailer",
            config,
        );
    }

    wln!(writer, "}}");
}

#[allow(clippy::too_many_arguments)]
fn emit_dot_node(
    writer: &mut impl Write,
    obj_id: ObjectId,
    doc: &Document,
    visited: &mut BTreeSet<ObjectId>,
    depth: usize,
    key_path: &str,
    parent_node: &str,
    config: &DumpConfig,
) {
    let node_name = format!("obj_{}_{}", obj_id.0, obj_id.1);
    let edge_label = escape_dot(key_path);

    if visited.contains(&obj_id) {
        wln!(
            writer,
            "  \"{}\" -> \"{}\" [label=\"{}\"];",
            parent_node,
            node_name,
            edge_label
        );
        return;
    }

    if let Some(max_depth) = config.depth
        && depth > max_depth
    {
        return;
    }

    visited.insert(obj_id);

    match doc.get_object(obj_id) {
        Ok(object) => {
            let label = escape_dot(&tree_node_label(object));
            let node_label = format!("{} {}: {}", obj_id.0, obj_id.1, label);
            wln!(writer, "  \"{}\" [label=\"{}\"];", node_name, node_label);
            wln!(
                writer,
                "  \"{}\" -> \"{}\" [label=\"{}\"];",
                parent_node,
                node_name,
                edge_label
            );

            let child_refs = collect_refs_with_paths(object);
            for (path, child_id) in child_refs {
                emit_dot_node(
                    writer,
                    child_id,
                    doc,
                    visited,
                    depth + 1,
                    &path,
                    &node_name,
                    config,
                );
            }
        }
        Err(_) => {
            wln!(
                writer,
                "  \"{}\" [label=\"{} {} (missing)\", style=dashed];",
                node_name,
                obj_id.0,
                obj_id.1
            );
            wln!(
                writer,
                "  \"{}\" -> \"{}\" [label=\"{}\"];",
                parent_node,
                node_name,
                edge_label
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::DumpConfig;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    #[test]
    fn tree_node_label_catalog() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"Catalog".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Catalog");
    }

    #[test]
    fn tree_node_label_page() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"Page".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Page");
    }

    #[test]
    fn tree_node_label_dict_no_type() {
        let dict = Dictionary::from_iter(vec![
            ("Foo", Object::Integer(1)),
            ("Bar", Object::Integer(2)),
        ]);
        assert_eq!(
            tree_node_label(&Object::Dictionary(dict)),
            "Dictionary, 2 keys"
        );
    }

    #[test]
    fn tree_node_label_stream() {
        let stream = Stream::new(Dictionary::new(), vec![1, 2, 3, 4, 5]);
        assert_eq!(tree_node_label(&Object::Stream(stream)), "Stream, 5 bytes");
    }

    #[test]
    fn tree_node_label_stream_with_type() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XRef".to_vec()));
        let stream = Stream::new(dict, vec![1, 2, 3]);
        assert_eq!(tree_node_label(&Object::Stream(stream)), "XRef, 3 bytes");
    }

    #[test]
    fn tree_basic_output() {
        let mut doc = Document::new();
        let pages_dict = Dictionary::from_iter(vec![("Type", Object::Name(b"Pages".to_vec()))]);
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        let catalog = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("Reference Tree:"));
        assert!(out.contains("Trailer"));
        assert!(out.contains("/Root -> 1 0 (Catalog)"));
        assert!(out.contains("/Pages -> 2 0 (Pages)"));
    }

    #[test]
    fn tree_visited_nodes_show_visited() {
        let mut doc = Document::new();
        let shared = Dictionary::from_iter(vec![("Type", Object::Name(b"Font".to_vec()))]);
        doc.objects.insert((3, 0), Object::Dictionary(shared));
        // Two objects reference the same child
        let a = Dictionary::from_iter(vec![("Font", Object::Reference((3, 0)))]);
        let b = Dictionary::from_iter(vec![("Font", Object::Reference((3, 0)))]);
        doc.objects.insert((1, 0), Object::Dictionary(a));
        doc.objects.insert((2, 0), Object::Dictionary(b));
        doc.trailer.set("A", Object::Reference((1, 0)));
        doc.trailer.set("B", Object::Reference((2, 0)));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_tree(w, &doc, &config));
        // Object 3 should appear once normally and once as visited
        assert!(out.contains("3 0 (Font)"));
        assert!(out.contains("(visited)"));
    }

    #[test]
    fn tree_depth_limit_respected() {
        let mut doc = Document::new();
        let child = Dictionary::from_iter(vec![("Type", Object::Name(b"Child".to_vec()))]);
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![("Child", Object::Reference((2, 0)))]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=1: trailer -> root (depth 1), child would be depth 2 → limited
        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: Some(1),
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("1 0"));
        assert!(out.contains("depth limit reached"));
    }

    #[test]
    fn tree_json_output_valid() {
        let mut doc = Document::new();
        let pages = Dictionary::from_iter(vec![("Type", Object::Name(b"Pages".to_vec()))]);
        doc.objects.insert((2, 0), Object::Dictionary(pages));
        let catalog = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: true,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| render_json(w, &tree_json_value(&doc, &config)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["tree"]["node"], "Trailer");
        assert!(parsed["tree"]["children"].is_array());
        let children = parsed["tree"]["children"].as_array().unwrap();
        assert!(!children.is_empty());
        // First child should be the /Root ref
        let root_child = &children[0];
        assert_eq!(root_child["key"], "/Root");
        assert_eq!(root_child["label"], "Catalog");
    }

    #[test]
    fn tree_json_visited_status() {
        let mut doc = Document::new();
        let shared = Dictionary::new();
        doc.objects.insert((2, 0), Object::Dictionary(shared));
        let dict_a = Dictionary::from_iter(vec![("Ref", Object::Reference((2, 0)))]);
        let dict_b = Dictionary::from_iter(vec![("Ref", Object::Reference((2, 0)))]);
        doc.objects.insert((1, 0), Object::Dictionary(dict_a));
        doc.objects.insert((3, 0), Object::Dictionary(dict_b));
        doc.trailer.set("A", Object::Reference((1, 0)));
        doc.trailer.set("B", Object::Reference((3, 0)));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: true,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| render_json(w, &tree_json_value(&doc, &config)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Should contain "visited" status somewhere in the tree
        let tree_str = serde_json::to_string(&parsed).unwrap();
        assert!(tree_str.contains("\"visited\""));
    }

    #[test]
    fn tree_json_depth_limit() {
        let mut doc = Document::new();
        let child = Dictionary::new();
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![("Child", Object::Reference((2, 0)))]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: true,
            hex: false,
            depth: Some(1),
            deref: false,
            raw: false,
        };
        let out = output_of(|w| render_json(w, &tree_json_value(&doc, &config)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let tree_str = serde_json::to_string(&parsed).unwrap();
        assert!(tree_str.contains("depth_limit_reached"));
    }

    #[test]
    fn tree_node_label_array() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ]);
        assert_eq!(tree_node_label(&arr), "Array, 3 items");
    }

    #[test]
    fn tree_node_label_empty_array() {
        let arr = Object::Array(vec![]);
        assert_eq!(tree_node_label(&arr), "Array, 0 items");
    }

    #[test]
    fn tree_node_label_boolean_true() {
        assert_eq!(tree_node_label(&Object::Boolean(true)), "Boolean(true)");
    }

    #[test]
    fn tree_node_label_boolean_false() {
        assert_eq!(tree_node_label(&Object::Boolean(false)), "Boolean(false)");
    }

    #[test]
    fn tree_node_label_integer() {
        assert_eq!(tree_node_label(&Object::Integer(42)), "Integer(42)");
    }

    #[test]
    fn tree_node_label_negative_integer() {
        assert_eq!(tree_node_label(&Object::Integer(-1)), "Integer(-1)");
    }

    #[test]
    fn tree_node_label_real() {
        assert_eq!(tree_node_label(&Object::Real(2.72)), "Real(2.72)");
    }

    #[test]
    fn tree_node_label_name() {
        assert_eq!(
            tree_node_label(&Object::Name(b"Helvetica".to_vec())),
            "Name(Helvetica)"
        );
    }

    #[test]
    fn tree_node_label_string() {
        assert_eq!(
            tree_node_label(&Object::String(b"Hello".to_vec(), StringFormat::Literal)),
            "String(Hello)"
        );
    }

    #[test]
    fn tree_node_label_null() {
        assert_eq!(tree_node_label(&Object::Null), "Null");
    }

    #[test]
    fn tree_node_label_reference() {
        assert_eq!(
            tree_node_label(&Object::Reference((5, 0))),
            "Reference(5 0)"
        );
    }

    #[test]
    fn tree_node_label_pages() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"Pages".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Pages");
    }

    #[test]
    fn tree_node_label_font() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"Font".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Font");
    }

    #[test]
    fn tree_node_label_annot() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"Annot".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Annot");
    }

    #[test]
    fn tree_node_label_xobject() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"XObject".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "XObject");
    }

    #[test]
    fn tree_node_label_encoding() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"Encoding".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Encoding");
    }

    #[test]
    fn tree_node_label_custom_type() {
        let dict = Dictionary::from_iter(vec![("Type", Object::Name(b"CustomFoo".to_vec()))]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "CustomFoo");
    }

    #[test]
    fn tree_node_label_empty_dict() {
        let dict = Dictionary::new();
        assert_eq!(
            tree_node_label(&Object::Dictionary(dict)),
            "Dictionary, 0 keys"
        );
    }

    #[test]
    fn tree_missing_object_shows_missing() {
        // Trailer references an object that doesn't exist
        let mut doc = Document::new();
        doc.trailer.set("Root", Object::Reference((99, 0)));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(
            out.contains("99 0 (missing)"),
            "Missing objects should be labeled: {}",
            out
        );
    }

    #[test]
    fn tree_json_missing_object_shows_status() {
        let mut doc = Document::new();
        doc.trailer.set("Root", Object::Reference((99, 0)));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: true,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| render_json(w, &tree_json_value(&doc, &config)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let tree_str = serde_json::to_string(&parsed).unwrap();
        assert!(
            tree_str.contains("\"missing\""),
            "JSON should contain missing status"
        );
    }

    #[test]
    fn tree_depth_zero_shows_only_trailer_refs() {
        let mut doc = Document::new();
        let child = Dictionary::from_iter(vec![("Type", Object::Name(b"Child".to_vec()))]);
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=0: Trailer shows, but no children at all (trailer is depth 0)
        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: Some(0),
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("Trailer"));
        // /Root -> 1 0 should show as depth limit reached (depth 1 > max_depth 0)
        assert!(
            out.contains("depth limit reached"),
            "Should hit depth limit: {}",
            out
        );
    }

    #[test]
    fn tree_depth_two_shows_three_levels() {
        let mut doc = Document::new();
        let gc = Dictionary::from_iter(vec![("Type", Object::Name(b"Grandchild".to_vec()))]);
        doc.objects.insert((3, 0), Object::Dictionary(gc));
        let child = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=2: should show Trailer, Root (depth 1), Child (depth 2), but not Grandchild (depth 3)
        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: Some(2),
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("Catalog"), "Should show Root/Catalog");
        assert!(out.contains("Child"), "Should show Child at depth 2");
        assert!(
            out.contains("depth limit reached"),
            "Grandchild should be depth-limited"
        );
    }

    #[test]
    fn escape_dot_quotes_and_backslash() {
        assert_eq!(escape_dot("hello \"world\""), "hello \\\"world\\\"");
        assert_eq!(escape_dot("a\\b"), "a\\\\b");
    }

    #[test]
    fn dot_basic_output() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let config = default_config();
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        assert!(out.contains("digraph pdf {"));
        assert!(out.contains("->"));
        assert!(out.contains("}"));
        assert!(out.contains("Catalog"));
    }

    #[test]
    fn dot_revisited_nodes() {
        let mut doc = Document::new();
        // Two dict entries referencing the same object
        let shared = doc.add_object(Object::Integer(42));
        let mut root = Dictionary::new();
        root.set("Type", Object::Name(b"Catalog".to_vec()));
        root.set("A", Object::Reference(shared));
        root.set("B", Object::Reference(shared));
        let root_id = doc.add_object(Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference(root_id));

        let config = default_config();
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        // The shared node should be defined once, but have two edges pointing to it
        let node_name = format!("obj_{}_{}", shared.0, shared.1);
        let edge_count = out.matches(&format!("-> \"{}\"", node_name)).count();
        assert!(
            edge_count >= 2,
            "Should have at least 2 edges to shared node, got {}",
            edge_count
        );
    }

    #[test]
    fn dot_depth_limiting() {
        let mut doc = Document::new();
        let deep = doc.add_object(Object::Integer(99));
        let mut child = Dictionary::new();
        child.set("Deep", Object::Reference(deep));
        let child_id = doc.add_object(Object::Dictionary(child));
        let mut root = Dictionary::new();
        root.set("Type", Object::Name(b"Catalog".to_vec()));
        root.set("Child", Object::Reference(child_id));
        let root_id = doc.add_object(Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference(root_id));

        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: Some(1),
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        // Should include root and child, but not the deep object
        let deep_node = format!("obj_{}_{}", deep.0, deep.1);
        assert!(
            !out.contains(&deep_node),
            "Deep node should not appear with depth limit 1"
        );
    }

    #[test]
    fn dot_empty_tree() {
        let doc = Document::new();
        let config = default_config();
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        assert!(out.contains("digraph pdf {"));
        assert!(out.contains("}"));
    }
}
