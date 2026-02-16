use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::types::PageSpec;
use crate::helpers::{name_to_string, format_dict_value};

pub(crate) struct AnnotationInfo {
    pub page_num: u32,
    pub object_id: ObjectId,
    pub subtype: String,
    pub rect: String,
    pub contents: String,
}

pub(crate) fn collect_annotations(doc: &Document, page_filter: Option<&PageSpec>) -> Vec<AnnotationInfo> {
    let pages = doc.get_pages();
    let mut annotations = Vec::new();

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

            let subtype = annot_dict.get(b"Subtype").ok()
                .and_then(name_to_string)
                .unwrap_or_else(|| "-".to_string());

            let rect = annot_dict.get(b"Rect").ok()
                .map(format_dict_value)
                .unwrap_or_else(|| "-".to_string());

            let contents = annot_dict.get(b"Contents").ok()
                .map(|v| match v {
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => "-".to_string(),
                })
                .unwrap_or_default();

            annotations.push(AnnotationInfo {
                page_num,
                object_id: annot_id,
                subtype,
                rect,
                contents,
            });
        }
    }

    annotations
}

pub(crate) fn print_annotations(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let annotations = collect_annotations(doc, page_filter);
    writeln!(writer, "{} annotations found\n", annotations.len()).unwrap();
    if annotations.is_empty() { return; }
    writeln!(writer, "  {:>4}  {:>4}  {:<12} {:<30} Contents", "Page", "Obj#", "Subtype", "Rect").unwrap();
    for a in &annotations {
        writeln!(writer, "  {:>4}  {:>4}  {:<12} {:<30} {}", a.page_num, a.object_id.0, a.subtype, a.rect, a.contents).unwrap();
    }
}

pub(crate) fn print_annotations_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let annotations = collect_annotations(doc, page_filter);
    let items: Vec<Value> = annotations.iter().map(|a| {
        json!({
            "page_number": a.page_num,
            "object_number": a.object_id.0,
            "generation": a.object_id.1,
            "subtype": a.subtype,
            "rect": a.rect,
            "contents": a.contents,
        })
    }).collect();
    let output = json!({
        "annotation_count": items.len(),
        "annotations": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

    fn make_doc_with_annotations() -> Document {
        let mut doc = Document::new();

        // Annotation
        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(100), Object::Integer(50),
        ]));
        annot.set("Contents", Object::String(b"Click here".to_vec(), StringFormat::Literal));
        let annot_id = doc.add_object(Object::Dictionary(annot));

        // Content stream
        let content_stream = Stream::new(Dictionary::new(), b"BT ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content_stream));

        // Page
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Contents", Object::Reference(content_id));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        // Pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages_dict.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        // Update page /Parent
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
        }

        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        doc
    }

    #[test]
    fn annotations_link_annotation() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("1 annotations found"));
        assert!(out.contains("Link"));
        assert!(out.contains("Click here"));
    }

    #[test]
    fn annotations_page_filter() {
        let doc = make_doc_with_annotations();
        // Page 1 has annotations
        let spec1 = PageSpec::Single(1);
        let out = output_of(|w| print_annotations(w, &doc, Some(&spec1)));
        assert!(out.contains("1 annotations found"));
        // Page 2 doesn't exist, should return 0
        let spec2 = PageSpec::Single(2);
        let out2 = output_of(|w| print_annotations(w, &doc, Some(&spec2)));
        assert!(out2.contains("0 annotations found"));
    }

    #[test]
    fn annotations_no_annotations() {
        let mut doc = Document::new();
        // Page without /Annots
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages_dict.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
        }

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("0 annotations found"));
    }

    #[test]
    fn annotations_json_output() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| print_annotations_json(w, &doc, None));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["annotation_count"], 1);
        assert_eq!(val["annotations"][0]["subtype"], "Link");
        assert_eq!(val["annotations"][0]["contents"], "Click here");
    }

    #[test]
    fn annotations_text_annotation() {
        let mut doc = Document::new();

        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set("Rect", Object::Array(vec![
            Object::Integer(10), Object::Integer(20),
            Object::Integer(30), Object::Integer(40),
        ]));
        annot.set("Contents", Object::String(b"A note".to_vec(), StringFormat::Literal));
        let annot_id = doc.add_object(Object::Dictionary(annot));

        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages_dict.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
        }

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("Text"));
        assert!(out.contains("A note"));
    }

}
