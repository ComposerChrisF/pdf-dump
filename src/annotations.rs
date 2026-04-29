use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::io::Write;

use crate::bookmarks::format_dest_value;
use crate::helpers::{format_dict_value, name_to_string, obj_to_string_lossy};
use crate::types::PageSpec;

pub(crate) struct AnnotationInfo {
    pub page_num: u32,
    pub object_id: ObjectId,
    pub subtype: String,
    pub rect: String,
    pub contents: String,
    pub link_type: Option<String>,
    pub target: Option<String>,
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
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(Object::Dictionary(d)) => d,
                _ => return (Cow::Borrowed("Unknown"), format!("{} {} R", id.0, id.1)),
            },
            _ => return (Cow::Borrowed("Unknown"), "-".to_string()),
        };
        let action_type = action_dict.get(b"S").ok().and_then(|v| v.as_name().ok());
        match action_type {
            Some(b"GoTo") => {
                let target = action_dict
                    .get(b"D")
                    .ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("GoTo"), target)
            }
            Some(b"GoToR") => {
                let file = action_dict
                    .get(b"F")
                    .ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                let dest = action_dict
                    .get(b"D")
                    .ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_default();
                (
                    Cow::Borrowed("GoToR"),
                    format!("{} {}", file, dest).trim().to_string(),
                )
            }
            Some(b"URI") => {
                let uri = action_dict
                    .get(b"URI")
                    .ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("URI"), uri)
            }
            Some(b"Named") => {
                let n = action_dict
                    .get(b"N")
                    .ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Named"), n)
            }
            Some(b"Launch") => {
                let f = action_dict
                    .get(b"F")
                    .ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Launch"), f)
            }
            Some(other) => (
                Cow::Owned(String::from_utf8_lossy(other).into_owned()),
                "-".to_string(),
            ),
            None => (Cow::Borrowed("Unknown"), "-".to_string()),
        }
    } else {
        (Cow::Borrowed("Unknown"), "-".to_string())
    }
}

pub(crate) fn collect_annotations(
    doc: &Document,
    page_filter: Option<&PageSpec>,
) -> Vec<AnnotationInfo> {
    let pages = doc.get_pages();
    let mut annotations = Vec::new();

    for (&page_num, &page_id) in &pages {
        if let Some(spec) = page_filter
            && !spec.contains(page_num)
        {
            continue;
        }

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

            let subtype = annot_dict
                .get(b"Subtype")
                .ok()
                .and_then(name_to_string)
                .unwrap_or_else(|| "-".to_string());

            let rect = annot_dict
                .get(b"Rect")
                .ok()
                .map(format_dict_value)
                .unwrap_or_else(|| "-".to_string());

            let contents = annot_dict
                .get(b"Contents")
                .ok()
                .map(|v| match v {
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => "-".to_string(),
                })
                .unwrap_or_default();

            let (link_type, target) = if subtype == "Link" {
                let (lt, tgt) = classify_link(doc, annot_dict);
                (Some(lt.into_owned()), Some(tgt))
            } else {
                (None, None)
            };

            annotations.push(AnnotationInfo {
                page_num,
                object_id: annot_id,
                subtype,
                rect,
                contents,
                link_type,
                target,
            });
        }
    }

    annotations
}

pub(crate) fn print_annotations(
    writer: &mut impl Write,
    doc: &Document,
    page_filter: Option<&PageSpec>,
) {
    let annotations = collect_annotations(doc, page_filter);
    wln!(writer, "{} annotations found\n", annotations.len());
    if annotations.is_empty() {
        return;
    }
    wln!(
        writer,
        "  {:>4}  {:>4}  {:<12} {:<8} {:<30} {:<30} Contents",
        "Page",
        "Obj#",
        "Subtype",
        "Type",
        "Rect",
        "Target"
    );
    for a in &annotations {
        let link_type = a.link_type.as_deref().unwrap_or("-");
        let target = a.target.as_deref().unwrap_or("-");
        wln!(
            writer,
            "  {:>4}  {:>4}  {:<12} {:<8} {:<30} {:<30} {}",
            a.page_num,
            a.object_id.0,
            a.subtype,
            link_type,
            a.rect,
            target,
            a.contents
        );
    }
}

pub(crate) fn annotations_json_value(doc: &Document, page_filter: Option<&PageSpec>) -> Value {
    let annotations = collect_annotations(doc, page_filter);
    let items: Vec<Value> = annotations
        .iter()
        .map(|a| {
            json!({
                "page_number": a.page_num,
                "object_number": a.object_id.0,
                "generation": a.object_id.1,
                "subtype": a.subtype,
                "rect": a.rect,
                "contents": a.contents,
                "link_type": a.link_type,
                "target": a.target,
            })
        })
        .collect();
    json!({
        "annotation_count": items.len(),
        "annotations": items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    fn make_doc_with_annotations() -> Document {
        let mut doc = Document::new();

        // Annotation
        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(50),
            ]),
        );
        annot.set(
            "Contents",
            Object::String(b"Click here".to_vec(), StringFormat::Literal),
        );
        let annot_id = doc.add_object(Object::Dictionary(annot));

        // Content stream
        let content_stream = Stream::new(Dictionary::new(), b"BT ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content_stream));

        // Page
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Contents", Object::Reference(content_id));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
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
        page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
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
        let out = output_of(|w| render_json(w, &annotations_json_value(&doc, None)));
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
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(10),
                Object::Integer(20),
                Object::Integer(30),
                Object::Integer(40),
            ]),
        );
        annot.set(
            "Contents",
            Object::String(b"A note".to_vec(), StringFormat::Literal),
        );
        let annot_id = doc.add_object(Object::Dictionary(annot));

        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
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

    #[test]
    fn annotations_link_uri_type_and_target() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"URI".to_vec()));
        action.set(
            "URI",
            Object::String(b"https://example.com".to_vec(), StringFormat::Literal),
        );

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].link_type.as_deref(), Some("URI"));
        assert_eq!(
            annotations[0].target.as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn annotations_link_goto_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set(
            "Dest",
            Object::Array(vec![
                Object::Reference((10, 0)),
                Object::Name(b"Fit".to_vec()),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].link_type.as_deref(), Some("GoTo"));
    }

    #[test]
    fn annotations_non_link_has_no_link_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert!(annotations[0].link_type.is_none());
        assert!(annotations[0].target.is_none());
    }

    #[test]
    fn annotations_json_includes_link_fields() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| render_json(w, &annotations_json_value(&doc, None)));
        let val: Value = serde_json::from_str(&out).unwrap();
        // Link annotation should have link_type populated
        let annot = &val["annotations"][0];
        assert_eq!(annot["subtype"], "Link");
        assert!(annot.get("link_type").is_some());
        assert!(annot.get("target").is_some());
    }

    #[test]
    fn annotations_link_gotor_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"GoToR".to_vec()));
        action.set(
            "F",
            Object::String(b"other.pdf".to_vec(), StringFormat::Literal),
        );
        action.set(
            "D",
            Object::Array(vec![Object::Integer(0), Object::Name(b"Fit".to_vec())]),
        );

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("GoToR"));
        assert!(
            annotations[0]
                .target
                .as_ref()
                .unwrap()
                .contains("other.pdf")
        );
    }

    #[test]
    fn annotations_link_named_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"Named".to_vec()));
        action.set("N", Object::Name(b"NextPage".to_vec()));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("Named"));
        assert_eq!(annotations[0].target.as_deref(), Some("NextPage"));
    }

    #[test]
    fn annotations_link_launch_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"Launch".to_vec()));
        action.set(
            "F",
            Object::String(b"app.exe".to_vec(), StringFormat::Literal),
        );

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("Launch"));
        assert_eq!(annotations[0].target.as_deref(), Some("app.exe"));
    }

    #[test]
    fn annotations_missing_subtype() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        // No Subtype
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(50),
                Object::Integer(50),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].subtype, "-");
    }

    #[test]
    fn annotations_missing_rect() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        // No Rect
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].rect, "-");
    }

    #[test]
    fn annotations_highlight_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Highlight".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(10),
                Object::Integer(20),
                Object::Integer(30),
                Object::Integer(40),
            ]),
        );
        annot.set(
            "Contents",
            Object::String(b"Highlighted text".to_vec(), StringFormat::Literal),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].subtype, "Highlight");
        assert_eq!(annotations[0].contents, "Highlighted text");
        // Highlight is not a Link, so no link_type
        assert!(annotations[0].link_type.is_none());
    }

    #[test]
    fn annotations_multiple_on_same_page() {
        let mut doc = Document::new();

        let mut a1 = Dictionary::new();
        a1.set("Subtype", Object::Name(b"Text".to_vec()));
        a1.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(50),
                Object::Integer(50),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(a1));

        let mut a2 = Dictionary::new();
        a2.set("Subtype", Object::Name(b"Link".to_vec()));
        a2.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(60),
                Object::Integer(60),
                Object::Integer(100),
                Object::Integer(100),
            ]),
        );
        doc.objects.insert((21, 0), Object::Dictionary(a2));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0), (21, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 2);
    }

    #[test]
    fn annotations_annots_as_reference() {
        // Arrange: /Annots is an indirect reference to an array
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(50),
                Object::Integer(50),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        // Annots array as its own object
        let annots_array = Object::Array(vec![Object::Reference((20, 0))]);
        doc.objects.insert((30, 0), annots_array);

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        page.set("Annots", Object::Reference((30, 0))); // Reference to array
        doc.objects.insert((10, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].subtype, "Text");
    }

    #[test]
    fn annotations_page_range_filter() {
        let doc = make_doc_with_annotations();
        let spec = PageSpec::Range(1, 1);
        let annotations = collect_annotations(&doc, Some(&spec));
        assert_eq!(annotations.len(), 1);
    }

    #[test]
    fn annotations_link_unknown_action() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"CustomAction".to_vec()));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("CustomAction"));
    }
}
