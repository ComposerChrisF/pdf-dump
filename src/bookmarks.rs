use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write;


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

pub(crate) fn count_bookmarks(doc: &Document) -> usize {
    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return 0,
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return 0,
    };
    let first_id = catalog.get(b"Outlines").ok()
        .and_then(|v| v.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|obj| if let Object::Dictionary(d) = obj { Some(d) } else { None })
        .and_then(|d| d.get(b"First").ok())
        .and_then(|v| v.as_reference().ok());
    match first_id {
        Some(id) => count_outline_items(&collect_outline_items(doc, id)),
        None => 0,
    }
}

pub(crate) fn print_bookmarks(writer: &mut impl Write, doc: &Document) {
    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => {
            writeln!(writer, "No bookmarks (no /Root in trailer).").unwrap();
            return;
        }
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => {
            writeln!(writer, "No bookmarks (could not read catalog).").unwrap();
            return;
        }
    };
    let outlines_ref = match catalog.get(b"Outlines").ok().and_then(|v| {
        match v {
            Object::Reference(id) => Some(*id),
            _ => None,
        }
    }) {
        Some(id) => id,
        None => {
            writeln!(writer, "No bookmarks.").unwrap();
            return;
        }
    };
    let outlines_dict = match doc.get_object(outlines_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => {
            writeln!(writer, "No bookmarks (could not read /Outlines).").unwrap();
            return;
        }
    };
    let first_id = match outlines_dict.get(b"First").ok().and_then(|v| v.as_reference().ok()) {
        Some(id) => id,
        None => {
            writeln!(writer, "No bookmarks.").unwrap();
            return;
        }
    };

    let items = collect_outline_items(doc, first_id);
    let total = count_outline_items(&items);
    writeln!(writer, "{} bookmarks\n", total).unwrap();
    print_outline_tree(writer, &items, 0);
}

fn print_outline_tree(writer: &mut impl Write, items: &[OutlineItem], depth: usize) {
    let indent = "  ".repeat(depth);
    for item in items {
        writeln!(writer, "{}[{}] {} -> {}", indent, item.object_id.0, item.title, item.destination).unwrap();
        if !item.children.is_empty() {
            print_outline_tree(writer, &item.children, depth + 1);
        }
    }
}

pub(crate) fn bookmarks_json_value(doc: &Document) -> Value {
    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return json!({"bookmark_count": 0, "bookmarks": []}),
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return json!({"bookmark_count": 0, "bookmarks": []}),
    };
    let first_id = catalog.get(b"Outlines").ok()
        .and_then(|v| v.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|obj| if let Object::Dictionary(d) = obj { Some(d) } else { None })
        .and_then(|d| d.get(b"First").ok())
        .and_then(|v| v.as_reference().ok());

    let items = match first_id {
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
    let output = bookmarks_json_value(doc);
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
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

}
