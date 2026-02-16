use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::io::Write;

use crate::types::PageSpec;
use crate::helpers::{obj_to_string_lossy, format_dict_value};

pub(crate) struct LinkInfo {
    pub page_num: u32,
    pub object_id: ObjectId,
    pub link_type: Cow<'static, str>,
    pub target: String,
    pub rect: String,
}

fn format_dest_value(doc: &Document, dest: &Object) -> String {
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

fn classify_link(doc: &Document, dict: &lopdf::Dictionary) -> (Cow<'static, str>, String) {
    // Check /Dest first (direct destination)
    if let Ok(dest) = dict.get(b"Dest") {
        return (Cow::Borrowed("GoTo"), format_dest_value(doc, dest));
    }
    // Check /A (action dictionary)
    if let Ok(action_obj) = dict.get(b"A") {
        let action_dict = match action_obj {
            Object::Dictionary(d) => d,
            Object::Reference(id) => {
                match doc.get_object(*id) {
                    Ok(Object::Dictionary(d)) => d,
                    _ => return (Cow::Borrowed("Unknown"), format!("{} {} R", id.0, id.1)),
                }
            }
            _ => return (Cow::Borrowed("Unknown"), "-".to_string()),
        };
        let action_type = action_dict.get(b"S").ok()
            .and_then(|v| v.as_name().ok());
        match action_type {
            Some(b"GoTo") => {
                let target = action_dict.get(b"D").ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("GoTo"), target)
            }
            Some(b"GoToR") => {
                let file = action_dict.get(b"F").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                let dest = action_dict.get(b"D").ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_default();
                (Cow::Borrowed("GoToR"), format!("{} {}", file, dest).trim().to_string())
            }
            Some(b"URI") => {
                let uri = action_dict.get(b"URI").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("URI"), uri)
            }
            Some(b"Named") => {
                let n = action_dict.get(b"N").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Named"), n)
            }
            Some(b"Launch") => {
                let f = action_dict.get(b"F").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Launch"), f)
            }
            Some(other) => (Cow::Owned(String::from_utf8_lossy(other).into_owned()), "-".to_string()),
            None => (Cow::Borrowed("Unknown"), "-".to_string()),
        }
    } else {
        (Cow::Borrowed("Unknown"), "-".to_string())
    }
}

pub(crate) fn collect_links(doc: &Document, page_filter: Option<&PageSpec>) -> Vec<LinkInfo> {
    let pages = doc.get_pages();
    let mut links = Vec::new();

    for (&page_num, &page_id) in &pages {
        if let Some(spec) = page_filter
            && !spec.contains(page_num) { continue; }

        let page_dict = match doc.get_object(page_id) {
            Ok(Object::Dictionary(d)) => d,
            _ => continue,
        };

        let annot_refs: Vec<ObjectId> = match page_dict.get(b"Annots") {
            Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
            Ok(Object::Reference(id)) => {
                if let Ok(Object::Array(arr)) = doc.get_object(*id) {
                    arr.iter().filter_map(|o| o.as_reference().ok()).collect()
                } else {
                    continue;
                }
            }
            _ => continue,
        };

        for annot_id in annot_refs {
            let annot_dict = match doc.get_object(annot_id) {
                Ok(Object::Dictionary(d)) => d,
                _ => continue,
            };

            let is_link = annot_dict.get(b"Subtype").ok()
                .and_then(|v| v.as_name().ok())
                .is_some_and(|n| n == b"Link");

            if !is_link { continue; }

            let rect = annot_dict.get(b"Rect").ok()
                .map(format_dict_value)
                .unwrap_or_else(|| "-".to_string());

            let (link_type, target) = classify_link(doc, annot_dict);

            links.push(LinkInfo {
                page_num,
                object_id: annot_id,
                link_type,
                target,
                rect,
            });
        }
    }

    links
}

pub(crate) fn print_links(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let links = collect_links(doc, page_filter);
    writeln!(writer, "{} links found\n", links.len()).unwrap();
    if links.is_empty() { return; }
    writeln!(writer, "  {:>4}  {:>4}  {:<8} Target", "Page", "Obj#", "Type").unwrap();
    for l in &links {
        writeln!(writer, "  {:>4}  {:>4}  {:<8} {}", l.page_num, l.object_id.0, l.link_type, l.target).unwrap();
    }
}

pub(crate) fn print_links_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let links = collect_links(doc, page_filter);
    let items: Vec<Value> = links.iter().map(|l| {
        json!({
            "page_number": l.page_num,
            "object_number": l.object_id.0,
            "generation": l.object_id.1,
            "link_type": l.link_type,
            "target": l.target,
            "rect": l.rect,
        })
    }).collect();
    let output = json!({
        "link_count": items.len(),
        "links": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::{Dictionary, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

    #[test]
    fn links_uri_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"URI".to_vec()));
        action.set("URI", Object::String(b"https://example.com".to_vec(), StringFormat::Literal));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "URI");
        assert_eq!(links[0].target, "https://example.com");
    }

    #[test]
    fn links_goto_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("Dest", Object::Array(vec![Object::Reference((10, 0)), Object::Name(b"Fit".to_vec())]));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "GoTo");
    }

    #[test]
    fn links_no_links() {
        let mut doc = Document::new();
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((10, 0), Object::Dictionary(page));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert!(links.is_empty());
    }

    #[test]
    fn links_mixed_annotations() {
        // Has both Link and Text annotations, should only return Links
        let mut doc = Document::new();
        let mut link_annot = Dictionary::new();
        link_annot.set("Subtype", Object::Name(b"Link".to_vec()));
        link_annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        link_annot.set("Dest", Object::Name(b"dest1".to_vec()));
        doc.objects.insert((20, 0), Object::Dictionary(link_annot));

        let mut text_annot = Dictionary::new();
        text_annot.set("Subtype", Object::Name(b"Text".to_vec()));
        text_annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        doc.objects.insert((21, 0), Object::Dictionary(text_annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0), (21, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "GoTo");
    }

    #[test]
    fn links_page_filter() {
        let mut doc = Document::new();
        let mut link1 = Dictionary::new();
        link1.set("Subtype", Object::Name(b"Link".to_vec()));
        link1.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        link1.set("Dest", Object::Name(b"d1".to_vec()));
        doc.objects.insert((20, 0), Object::Dictionary(link1));

        let mut link2 = Dictionary::new();
        link2.set("Subtype", Object::Name(b"Link".to_vec()));
        link2.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        link2.set("Dest", Object::Name(b"d2".to_vec()));
        doc.objects.insert((21, 0), Object::Dictionary(link2));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(2));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0)), Object::Reference((11, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);
        make_page_with_annots(&mut doc, (11, 0), (2, 0), vec![(21, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let spec = PageSpec::Single(1);
        let links = collect_links(&doc, Some(&spec));
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn links_named_action() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"Named".to_vec()));
        action.set("N", Object::Name(b"NextPage".to_vec()));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "Named");
        assert_eq!(links[0].target, "NextPage");
    }

    #[test]
    fn links_goto_r_action() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"GoToR".to_vec()));
        action.set("F", Object::String(b"other.pdf".to_vec(), StringFormat::Literal));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "GoToR");
        assert!(links[0].target.contains("other.pdf"));
    }

    #[test]
    fn links_print_output() {
        let doc = Document::new();
        let out = output_of(|w| print_links(w, &doc, None));
        assert!(out.contains("0 links found"));
    }

    #[test]
    fn links_json_output() {
        let doc = Document::new();
        let out = output_of(|w| print_links_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["link_count"], 0);
        assert_eq!(parsed["links"].as_array().unwrap().len(), 0);
    }

}
