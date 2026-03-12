use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write;

use crate::helpers::get_catalog;


pub(crate) struct OutlineItem {
    pub object_id: ObjectId,
    pub title: String,
    pub destination: String,
    pub children: Vec<OutlineItem>,
}

pub(crate) fn collect_outline_items(doc: &Document, first_id: ObjectId) -> Vec<OutlineItem> {
    let mut visited = BTreeSet::new();
    collect_outline_items_inner(doc, first_id, &mut visited)
}

fn collect_outline_items_inner(doc: &Document, first_id: ObjectId, visited: &mut BTreeSet<ObjectId>) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut current_id = Some(first_id);

    while let Some(id) = current_id {
        if visited.contains(&id) { break; }
        visited.insert(id);

        let dict = match doc.get_object(id) {
            Ok(Object::Dictionary(d)) => d,
            _ => break,
        };

        let title = dict.get(b"Title").ok()
            .map(|v| match v {
                Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                _ => "(untitled)".to_string(),
            })
            .unwrap_or_else(|| "(untitled)".to_string());

        let destination = format_destination(doc, dict);

        let children = dict.get(b"First").ok()
            .and_then(|v| v.as_reference().ok())
            .map(|child_id| collect_outline_items_inner(doc, child_id, visited))
            .unwrap_or_default();

        items.push(OutlineItem { object_id: id, title, destination, children });

        current_id = dict.get(b"Next").ok()
            .and_then(|v| v.as_reference().ok());
    }

    items
}

fn format_destination(doc: &Document, dict: &lopdf::Dictionary) -> String {
    // Check /Dest first
    if let Ok(dest) = dict.get(b"Dest") {
        return format_dest_value(doc, dest);
    }
    // Check /A (action)
    if let Ok(action_obj) = dict.get(b"A") {
        let action_dict = match action_obj {
            Object::Dictionary(d) => d,
            Object::Reference(id) => {
                match doc.get_object(*id) {
                    Ok(Object::Dictionary(d)) => d,
                    _ => return format!("Action({} {} R)", id.0, id.1),
                }
            }
            _ => return "-".to_string(),
        };
        let action_type = action_dict.get(b"S").ok()
            .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
            .unwrap_or_else(|| "?".to_string());
        match action_type.as_str() {
            "GoTo" => {
                if let Ok(d) = action_dict.get(b"D") {
                    return format!("GoTo({})", format_dest_value(doc, d));
                }
                "GoTo(?)".to_string()
            }
            "URI" => {
                let uri = action_dict.get(b"URI").ok()
                    .map(|v| match v {
                        Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                        _ => "?".to_string(),
                    })
                    .unwrap_or_else(|| "?".to_string());
                format!("URI({})", uri)
            }
            other => format!("Action({})", other),
        }
    } else {
        "-".to_string()
    }
}

pub(crate) fn format_dest_value(doc: &Document, dest: &Object) -> String {
    match dest {
        Object::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(|item| match item {
                Object::Reference(id) => format!("{} {} R", id.0, id.1),
                Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
                Object::Integer(i) => i.to_string(),
                Object::Real(r) => r.to_string(),
                Object::Null => "null".to_string(),
                _ => "?".to_string(),
            }).collect();
            format!("[{}]", parts.join(" "))
        }
        Object::String(bytes, _) => format!("({})", String::from_utf8_lossy(bytes)),
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::Reference(id) => {
            if let Ok(resolved) = doc.get_object(*id) {
                format_dest_value(doc, resolved)
            } else {
                format!("{} {} R", id.0, id.1)
            }
        }
        _ => "-".to_string(),
    }
}

fn count_outline_items(items: &[OutlineItem]) -> usize {
    items.iter().map(|item| 1 + count_outline_items(&item.children)).sum()
}

fn get_first_outline_id(doc: &Document) -> Option<ObjectId> {
    let catalog = get_catalog(doc)?;
    let outlines_ref = catalog.get(b"Outlines").ok()?.as_reference().ok()?;
    let outlines_dict = match doc.get_object(outlines_ref).ok()? {
        Object::Dictionary(d) => d,
        _ => return None,
    };
    outlines_dict.get(b"First").ok()?.as_reference().ok()
}

pub(crate) fn count_bookmarks(doc: &Document) -> usize {
    match get_first_outline_id(doc) {
        Some(id) => count_outline_items(&collect_outline_items(doc, id)),
        None => 0,
    }
}

pub(crate) fn print_bookmarks(writer: &mut impl Write, doc: &Document) {
    let first_id = match get_first_outline_id(doc) {
        Some(id) => id,
        None => {
            wln!(writer, "No bookmarks.");
            return;
        }
    };

    let items = collect_outline_items(doc, first_id);
    let total = count_outline_items(&items);
    wln!(writer, "{} bookmarks\n", total);
    print_outline_tree(writer, &items, 0);
}

fn print_outline_tree(writer: &mut impl Write, items: &[OutlineItem], depth: usize) {
    let indent = "  ".repeat(depth);
    for item in items {
        wln!(writer, "{}[{}] {} -> {}", indent, item.object_id.0, item.title, item.destination);
        if !item.children.is_empty() {
            print_outline_tree(writer, &item.children, depth + 1);
        }
    }
}

pub(crate) fn bookmarks_json_value(doc: &Document) -> Value {
    let items = match get_first_outline_id(doc) {
        Some(id) => collect_outline_items(doc, id),
        None => vec![],
    };
    let total = count_outline_items(&items);

    fn items_to_json(items: &[OutlineItem]) -> Vec<Value> {
        items.iter().map(|item| {
            let mut obj = json!({
                "object_number": item.object_id.0,
                "title": item.title,
                "destination": item.destination,
            });
            if !item.children.is_empty() {
                obj["children"] = json!(items_to_json(&item.children));
            }
            obj
        }).collect()
    }

    json!({
        "bookmark_count": total,
        "bookmarks": items_to_json(&items),
    })
}

#[cfg(test)]
pub(crate) fn print_bookmarks_json(writer: &mut impl Write, doc: &Document) {
    use crate::helpers::json_pretty;
    let output = bookmarks_json_value(doc);
    writeln!(writer, "{}", json_pretty(&output)).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::{Dictionary, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

    fn make_doc_with_bookmarks() -> Document {
        let mut doc = Document::new();

        // Two bookmark items: "Chapter 1" -> "Chapter 2"
        let mut bm2 = Dictionary::new();
        bm2.set("Title", Object::String(b"Chapter 2".to_vec(), StringFormat::Literal));
        bm2.set("Dest", Object::Array(vec![Object::Integer(0), Object::Name(b"Fit".to_vec())]));
        let bm2_id = doc.add_object(Object::Dictionary(bm2));

        let mut bm1 = Dictionary::new();
        bm1.set("Title", Object::String(b"Chapter 1".to_vec(), StringFormat::Literal));
        bm1.set("Dest", Object::Array(vec![Object::Integer(0), Object::Name(b"Fit".to_vec())]));
        bm1.set("Next", Object::Reference(bm2_id));
        let bm1_id = doc.add_object(Object::Dictionary(bm1));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm1_id));
        outlines.set("Last", Object::Reference(bm2_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));

        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc
    }

    #[test]
    fn bookmarks_siblings() {
        let doc = make_doc_with_bookmarks();
        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("2 bookmarks"));
        assert!(out.contains("Chapter 1"));
        assert!(out.contains("Chapter 2"));
    }

    #[test]
    fn bookmarks_no_outlines() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("No bookmarks"));
    }

    #[test]
    fn bookmarks_nested_children() {
        let mut doc = Document::new();

        // Child bookmark
        let mut child = Dictionary::new();
        child.set("Title", Object::String(b"Section 1.1".to_vec(), StringFormat::Literal));
        let child_id = doc.add_object(Object::Dictionary(child));

        // Parent bookmark with /First pointing to child
        let mut parent = Dictionary::new();
        parent.set("Title", Object::String(b"Chapter 1".to_vec(), StringFormat::Literal));
        parent.set("First", Object::Reference(child_id));
        let parent_id = doc.add_object(Object::Dictionary(parent));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(parent_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("2 bookmarks"));
        assert!(out.contains("Chapter 1"));
        assert!(out.contains("Section 1.1"));
    }

    #[test]
    fn bookmarks_with_dest_array() {
        let doc = make_doc_with_bookmarks();
        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("[0 /Fit]"));
    }

    #[test]
    fn bookmarks_with_uri_action() {
        let mut doc = Document::new();

        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"URI".to_vec()));
        action.set("URI", Object::String(b"https://example.com".to_vec(), StringFormat::Literal));
        let action_id = doc.add_object(Object::Dictionary(action));

        let mut bm = Dictionary::new();
        bm.set("Title", Object::String(b"Link".to_vec(), StringFormat::Literal));
        bm.set("A", Object::Reference(action_id));
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("URI(https://example.com)"));
    }

    #[test]
    fn bookmarks_missing_title() {
        let mut doc = Document::new();

        let bm = Dictionary::new(); // No /Title
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("(untitled)"));
    }

    #[test]
    fn bookmarks_json_output() {
        let doc = make_doc_with_bookmarks();
        let out = output_of(|w| print_bookmarks_json(w, &doc));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["bookmark_count"], 2);
        assert_eq!(val["bookmarks"][0]["title"], "Chapter 1");
        assert_eq!(val["bookmarks"][1]["title"], "Chapter 2");
    }

    #[test]
    fn bookmarks_json_no_outlines() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks_json(w, &doc));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["bookmark_count"], 0);
    }

    #[test]
    fn bookmarks_cycle_detection() {
        // Arrange: bookmark A -> Next -> B -> Next -> A (cycle)
        let mut doc = Document::new();
        let bm_a_id = (10, 0u16);
        let bm_b_id = (11, 0u16);

        let mut bm_a = Dictionary::new();
        bm_a.set("Title", Object::String(b"A".to_vec(), StringFormat::Literal));
        bm_a.set("Next", Object::Reference(bm_b_id));
        doc.objects.insert(bm_a_id, Object::Dictionary(bm_a));

        let mut bm_b = Dictionary::new();
        bm_b.set("Title", Object::String(b"B".to_vec(), StringFormat::Literal));
        bm_b.set("Next", Object::Reference(bm_a_id));
        doc.objects.insert(bm_b_id, Object::Dictionary(bm_b));

        // Act - should not hang
        let items = collect_outline_items(&doc, bm_a_id);

        // Assert: should visit each once
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn bookmarks_with_goto_action() {
        let mut doc = Document::new();

        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"GoTo".to_vec()));
        action.set("D", Object::Array(vec![
            Object::Reference((50, 0)),
            Object::Name(b"XYZ".to_vec()),
        ]));

        let mut bm = Dictionary::new();
        bm.set("Title", Object::String(b"Internal".to_vec(), StringFormat::Literal));
        bm.set("A", Object::Dictionary(action));
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("GoTo("));
        assert!(out.contains("/XYZ"));
    }

    #[test]
    fn bookmarks_with_named_dest() {
        let mut doc = Document::new();

        let mut bm = Dictionary::new();
        bm.set("Title", Object::String(b"Named".to_vec(), StringFormat::Literal));
        bm.set("Dest", Object::Name(b"chapter1".to_vec()));
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("/chapter1"));
    }

    #[test]
    fn bookmarks_no_dest_no_action() {
        let mut doc = Document::new();

        let mut bm = Dictionary::new();
        bm.set("Title", Object::String(b"Nowhere".to_vec(), StringFormat::Literal));
        // No /Dest, no /A
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("Nowhere"));
        assert!(out.contains("-> -")); // No destination
    }

    #[test]
    fn bookmarks_json_nested_children() {
        let mut doc = Document::new();

        let mut child = Dictionary::new();
        child.set("Title", Object::String(b"Section 1.1".to_vec(), StringFormat::Literal));
        let child_id = doc.add_object(Object::Dictionary(child));

        let mut parent = Dictionary::new();
        parent.set("Title", Object::String(b"Chapter 1".to_vec(), StringFormat::Literal));
        parent.set("First", Object::Reference(child_id));
        let parent_id = doc.add_object(Object::Dictionary(parent));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(parent_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks_json(w, &doc));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["bookmark_count"], 2);
        let bm = &val["bookmarks"][0];
        assert_eq!(bm["title"], "Chapter 1");
        let children = bm["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["title"], "Section 1.1");
    }

    #[test]
    fn bookmarks_count_function() {
        let doc = make_doc_with_bookmarks();
        assert_eq!(count_bookmarks(&doc), 2);
    }

    #[test]
    fn bookmarks_count_no_outlines() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));
        assert_eq!(count_bookmarks(&doc), 0);
    }

    #[test]
    fn bookmarks_with_string_dest() {
        let mut doc = Document::new();

        let mut bm = Dictionary::new();
        bm.set("Title", Object::String(b"StrDest".to_vec(), StringFormat::Literal));
        bm.set("Dest", Object::String(b"page1dest".to_vec(), StringFormat::Literal));
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("(page1dest)"));
    }

    #[test]
    fn bookmarks_with_unknown_action() {
        let mut doc = Document::new();

        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"JavaScript".to_vec()));

        let mut bm = Dictionary::new();
        bm.set("Title", Object::String(b"JSAction".to_vec(), StringFormat::Literal));
        bm.set("A", Object::Dictionary(action));
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("Action(JavaScript)"));
    }

}
