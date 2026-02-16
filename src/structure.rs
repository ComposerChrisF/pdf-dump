use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::types::DumpConfig;
use crate::helpers::{resolve_dict, obj_to_string_lossy};

pub(crate) struct StructElemInfo {
    pub object_id: ObjectId,
    pub role: String,
    pub page: Option<u32>,
    pub mcid: Option<i64>,
    pub title: Option<String>,
    pub alt: Option<String>,
    pub children: Vec<StructElemInfo>,
}

pub(crate) fn collect_structure_tree(doc: &Document) -> (bool, Vec<StructElemInfo>) {
    let catalog_id = match doc.trailer.get(b"Root").ok()
        .and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return (false, Vec::new()),
    };
    let catalog = match doc.get_object(catalog_id).ok() {
        Some(Object::Dictionary(d)) => d,
        _ => return (false, Vec::new()),
    };

    // Check MarkInfo
    let is_marked = catalog.get(b"MarkInfo").ok()
        .and_then(|v| resolve_dict(doc, v))
        .and_then(|d| d.get(b"Marked").ok())
        .and_then(|m| if let Object::Boolean(b) = m { Some(*b) } else { None })
        .unwrap_or(false);

    let struct_tree_root = match catalog.get(b"StructTreeRoot").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return (is_marked, Vec::new()),
    };

    // Build page_id -> page_num lookup
    let pages = doc.get_pages();
    let page_lookup: BTreeMap<ObjectId, u32> = pages.into_iter().map(|(num, id)| (id, num)).collect();

    let mut visited = BTreeSet::new();
    let children = collect_struct_children(doc, struct_tree_root, &page_lookup, &mut visited);

    (is_marked, children)
}

fn collect_struct_children(doc: &Document, dict: &lopdf::Dictionary, page_lookup: &BTreeMap<ObjectId, u32>, visited: &mut BTreeSet<ObjectId>) -> Vec<StructElemInfo> {
    let k = match dict.get(b"K").ok() {
        Some(v) => v,
        None => return Vec::new(),
    };

    let items: &[Object] = match k {
        Object::Array(arr) => arr,
        other => std::slice::from_ref(other),
    };

    let mut result = Vec::new();
    for item in items {
        match item {
            Object::Reference(id) => {
                if visited.contains(id) { continue; }
                visited.insert(*id);
                if let Ok(Object::Dictionary(child_dict)) = doc.get_object(*id)
                    && let Ok(role_obj) = child_dict.get(b"S") {
                    let role = if let Object::Name(n) = role_obj {
                        String::from_utf8_lossy(n).into_owned()
                    } else {
                        "-".to_string()
                    };

                    let page = child_dict.get(b"Pg").ok()
                        .and_then(|v| v.as_reference().ok())
                        .and_then(|pg_id| page_lookup.get(&pg_id).copied());

                    let mcid = extract_mcid(child_dict);

                    let title = child_dict.get(b"T").ok()
                        .and_then(obj_to_string_lossy);

                    let alt = child_dict.get(b"Alt").ok()
                        .and_then(obj_to_string_lossy);

                    let children = collect_struct_children(doc, child_dict, page_lookup, visited);

                    result.push(StructElemInfo {
                        object_id: *id,
                        role,
                        page,
                        mcid,
                        title,
                        alt,
                        children,
                    });
                }
            }
            Object::Dictionary(d) => {
                // Inline struct element
                if let Ok(role_obj) = d.get(b"S") {
                    let role = if let Object::Name(n) = role_obj {
                        String::from_utf8_lossy(n).into_owned()
                    } else {
                        "-".to_string()
                    };
                    let mcid = extract_mcid(d);
                    result.push(StructElemInfo {
                        object_id: (0, 0),
                        role,
                        page: None,
                        mcid,
                        title: None,
                        alt: None,
                        children: Vec::new(),
                    });
                }
            }
            _ => {
                // Bare MCID integers and other items are captured via extract_mcid on the parent
            }
        }
    }
    result
}

fn extract_mcid(dict: &lopdf::Dictionary) -> Option<i64> {
    // MCID can be in /K as integer, or in /K as dict with /MCID
    match dict.get(b"K").ok()? {
        Object::Integer(n) => Some(*n),
        Object::Dictionary(d) => d.get(b"MCID").ok()?.as_i64().ok(),
        Object::Array(arr) => {
            // Find first MCID in array
            for item in arr {
                match item {
                    Object::Integer(n) => return Some(*n),
                    Object::Dictionary(d) => {
                        if let Ok(mcid) = d.get(b"MCID")
                            && let Ok(n) = mcid.as_i64() {
                            return Some(n);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

fn count_struct_elems(items: &[StructElemInfo]) -> usize {
    items.iter().map(|e| 1 + count_struct_elems(&e.children)).sum()
}

pub(crate) fn print_structure(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let (is_marked, tree) = collect_structure_tree(doc);
    writeln!(writer, "Tagged PDF: {}", if is_marked { "yes" } else { "no" }).unwrap();
    let count = count_struct_elems(&tree);
    writeln!(writer, "Structure elements: {}\n", count).unwrap();
    if tree.is_empty() { return; }
    for elem in &tree {
        print_struct_elem(writer, elem, 0, config);
    }
}

fn print_struct_elem(writer: &mut impl Write, elem: &StructElemInfo, depth: usize, config: &DumpConfig) {
    if let Some(max_depth) = config.depth
        && depth > max_depth {
        return;
    }

    let indent = "  ".repeat(depth);
    let mut line = if elem.object_id != (0, 0) {
        format!("{}[{}] /{}", indent, elem.object_id.0, elem.role)
    } else {
        format!("{}/{}", indent, elem.role)
    };

    if let Some(page) = elem.page {
        line.push_str(&format!(" (page {})", page));
    }
    if let Some(mcid) = elem.mcid {
        line.push_str(&format!(" MCID={}", mcid));
    }
    if let Some(ref title) = elem.title {
        line.push_str(&format!(" \"{}\"", title));
    }
    if let Some(ref alt) = elem.alt {
        line.push_str(&format!(" alt=\"{}\"", alt));
    }

    // At depth limit, show children count instead of recursing
    if let Some(max_depth) = config.depth
        && depth == max_depth && !elem.children.is_empty() {
        line.push_str(&format!(" ({} children)", count_struct_elems(&elem.children)));
    }

    writeln!(writer, "{}", line).unwrap();

    for child in &elem.children {
        print_struct_elem(writer, child, depth + 1, config);
    }
}

pub(crate) fn print_structure_json(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let (is_marked, tree) = collect_structure_tree(doc);
    let count = count_struct_elems(&tree);
    let items: Vec<Value> = tree.iter().map(|e| struct_elem_to_json(e, 0, config)).collect();
    let output = json!({
        "tagged": is_marked,
        "element_count": count,
        "structure": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

fn struct_elem_to_json(elem: &StructElemInfo, depth: usize, config: &DumpConfig) -> Value {
    let mut obj = json!({
        "role": elem.role,
    });
    if elem.object_id != (0, 0) {
        obj["object_number"] = json!(elem.object_id.0);
        obj["generation"] = json!(elem.object_id.1);
    }
    if let Some(page) = elem.page {
        obj["page"] = json!(page);
    }
    if let Some(mcid) = elem.mcid {
        obj["mcid"] = json!(mcid);
    }
    if let Some(ref title) = elem.title {
        obj["title"] = json!(title);
    }
    if let Some(ref alt) = elem.alt {
        obj["alt"] = json!(alt);
    }

    if let Some(max_depth) = config.depth
        && depth >= max_depth {
        if !elem.children.is_empty() {
            obj["children_count"] = json!(count_struct_elems(&elem.children));
        }
        return obj;
    }

    if !elem.children.is_empty() {
        let children: Vec<Value> = elem.children.iter()
            .map(|c| struct_elem_to_json(c, depth + 1, config))
            .collect();
        obj["children"] = json!(children);
    }
    obj
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::DumpConfig;
    use lopdf::{Dictionary, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

    fn make_struct_doc() -> Document {
        let mut doc = Document::new();

        // Page
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        // Structure elements
        let mut span = Dictionary::new();
        span.set("Type", Object::Name(b"StructElem".to_vec()));
        span.set("S", Object::Name(b"Span".to_vec()));
        span.set("Pg", Object::Reference((3, 0)));
        span.set("K", Object::Integer(0)); // MCID
        doc.objects.insert((12, 0), Object::Dictionary(span));

        let mut p_elem = Dictionary::new();
        p_elem.set("Type", Object::Name(b"StructElem".to_vec()));
        p_elem.set("S", Object::Name(b"P".to_vec()));
        p_elem.set("K", Object::Array(vec![Object::Reference((12, 0))]));
        p_elem.set("T", Object::String(b"My Paragraph".to_vec(), StringFormat::Literal));
        doc.objects.insert((11, 0), Object::Dictionary(p_elem));

        let mut doc_elem = Dictionary::new();
        doc_elem.set("Type", Object::Name(b"StructElem".to_vec()));
        doc_elem.set("S", Object::Name(b"Document".to_vec()));
        doc_elem.set("K", Object::Array(vec![Object::Reference((11, 0))]));
        doc.objects.insert((10, 0), Object::Dictionary(doc_elem));

        // StructTreeRoot
        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        // MarkInfo
        let mut mark_info = Dictionary::new();
        mark_info.set("Marked", Object::Boolean(true));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        catalog.set("MarkInfo", Object::Dictionary(mark_info));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));

        doc.trailer.set("Root", Object::Reference((1, 0)));
        doc
    }

    #[test]
    fn structure_collects_tree() {
        let doc = make_struct_doc();
        let (is_marked, tree) = collect_structure_tree(&doc);
        assert!(is_marked);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].role, "Document");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].role, "P");
        assert_eq!(tree[0].children[0].title.as_deref(), Some("My Paragraph"));
        assert_eq!(tree[0].children[0].children.len(), 1);
        assert_eq!(tree[0].children[0].children[0].role, "Span");
        assert_eq!(tree[0].children[0].children[0].mcid, Some(0));
    }

    #[test]
    fn structure_page_refs() {
        let doc = make_struct_doc();
        let (_, tree) = collect_structure_tree(&doc);
        let span = &tree[0].children[0].children[0];
        assert_eq!(span.page, Some(1));
    }

    #[test]
    fn structure_no_struct_tree_root() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (is_marked, tree) = collect_structure_tree(&doc);
        assert!(!is_marked);
        assert!(tree.is_empty());
    }

    #[test]
    fn structure_mark_info_false() {
        let mut doc = Document::new();
        let mut mark_info = Dictionary::new();
        mark_info.set("Marked", Object::Boolean(false));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("MarkInfo", Object::Dictionary(mark_info));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (is_marked, _) = collect_structure_tree(&doc);
        assert!(!is_marked);
    }

    #[test]
    fn structure_text_output() {
        let doc = make_struct_doc();
        let config = default_config();
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("Tagged PDF: yes"));
        assert!(out.contains("Structure elements: 3"));
        assert!(out.contains("/Document"));
        assert!(out.contains("/P"));
        assert!(out.contains("/Span"));
        assert!(out.contains("MCID=0"));
        assert!(out.contains("\"My Paragraph\""));
    }

    #[test]
    fn structure_json_output() {
        let doc = make_struct_doc();
        let config = default_config();
        let out = output_of(|w| print_structure_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["tagged"], true);
        assert_eq!(parsed["element_count"], 3);
        let root = &parsed["structure"][0];
        assert_eq!(root["role"], "Document");
        assert!(root["children"].is_array());
    }

    #[test]
    fn structure_depth_limit_0() {
        let doc = make_struct_doc();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("/Document"));
        assert!(out.contains("children"));
        // /P and /Span should NOT appear since depth=0 only shows root level
        assert!(!out.contains("/P"));
    }

    #[test]
    fn structure_depth_limit_1() {
        let doc = make_struct_doc();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("/Document"));
        assert!(out.contains("/P"));
        // /Span at depth 2 should NOT appear
        assert!(!out.contains("/Span"));
    }

    #[test]
    fn structure_json_with_depth_limit() {
        let doc = make_struct_doc();
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| print_structure_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let root = &parsed["structure"][0];
        // At depth 0, children should be represented as children_count
        assert!(root.get("children_count").is_some());
        assert!(root.get("children").is_none());
    }

    #[test]
    fn structure_cycle_detection() {
        let mut doc = Document::new();

        // Create a cycle: elem1 -> elem2 -> elem1
        let mut elem2 = Dictionary::new();
        elem2.set("S", Object::Name(b"Span".to_vec()));
        elem2.set("K", Object::Reference((10, 0))); // back to elem1
        doc.objects.insert((11, 0), Object::Dictionary(elem2));

        let mut elem1 = Dictionary::new();
        elem1.set("S", Object::Name(b"P".to_vec()));
        elem1.set("K", Object::Reference((11, 0)));
        doc.objects.insert((10, 0), Object::Dictionary(elem1));

        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (_, tree) = collect_structure_tree(&doc);
        // Should not infinite-loop, should have 2 elements (one is visited, stops)
        let count = count_struct_elems(&tree);
        assert!(count <= 2);
    }

    #[test]
    fn structure_empty_tree() {
        let mut doc = Document::new();
        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut mark_info = Dictionary::new();
        mark_info.set("Marked", Object::Boolean(true));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        catalog.set("MarkInfo", Object::Dictionary(mark_info));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (is_marked, tree) = collect_structure_tree(&doc);
        assert!(is_marked);
        assert!(tree.is_empty());
    }

    #[test]
    fn structure_alt_text() {
        let mut doc = Document::new();

        let mut elem = Dictionary::new();
        elem.set("S", Object::Name(b"Figure".to_vec()));
        elem.set("Alt", Object::String(b"A photo of sunset".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(elem));

        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = default_config();
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("alt=\"A photo of sunset\""));
    }

    #[test]
    fn structure_k_as_array() {
        let mut doc = Document::new();

        let mut elem1 = Dictionary::new();
        elem1.set("S", Object::Name(b"P".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(elem1));

        let mut elem2 = Dictionary::new();
        elem2.set("S", Object::Name(b"Span".to_vec()));
        doc.objects.insert((11, 0), Object::Dictionary(elem2));

        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (_, tree) = collect_structure_tree(&doc);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].role, "P");
        assert_eq!(tree[1].role, "Span");
    }

}
