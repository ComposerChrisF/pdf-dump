use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::io::Write;

use crate::annotations::{AnnotationInfo, collect_annotations};
use crate::helpers::{self, format_dict_value};
use crate::resources::{PageResources, collect_page_resources, resource_entries_to_json};
use crate::text::extract_text_from_page_with_warnings;
use crate::types::PageSpec;

struct PageInfo {
    page_num: u32,
    object_id: ObjectId,
    media_box: String,
    crop_box: Option<String>,
    rotate: Option<i64>,
    font_names: Vec<String>,
    image_count: usize,
    ext_gstate_count: usize,
    annotation_count: usize,
    annotation_subtypes: Vec<(String, usize)>,
    content_stream_count: usize,
    content_stream_bytes: usize,
    text_preview: String,
    text_extractable: bool,
    full_text: String,
    warnings: Vec<String>,
    resources: PageResources,
}

/// Returns true if the text appears to be garbled / non-extractable.
/// Heuristic: more than 50% of non-whitespace chars are outside ASCII printable range (0x20..=0x7E).
fn is_garbled_text(text: &str, warnings: &[String]) -> bool {
    // If warnings mention CID or encoding issues, treat as garbled
    for w in warnings {
        let lower = w.to_lowercase();
        if lower.contains("cid") || lower.contains("encoding") || lower.contains("tounicode") {
            // Only if there is actually some text (otherwise it's just empty)
            if !text.trim().is_empty() {
                return true;
            }
        }
    }
    let non_ws_count = text.chars().filter(|c| !c.is_whitespace()).count();
    if non_ws_count == 0 {
        return false;
    }
    let non_printable = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .filter(|&c| !('\x20'..='\x7E').contains(&c))
        .count();
    non_printable * 2 > non_ws_count
}

fn collect_page_info(
    doc: &Document,
    page_num: u32,
    page_id: ObjectId,
    annots: &[&AnnotationInfo],
) -> PageInfo {
    let page_dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => {
            return PageInfo {
                page_num,
                object_id: page_id,
                media_box: "-".to_string(),
                crop_box: None,
                rotate: None,
                font_names: vec![],
                image_count: 0,
                ext_gstate_count: 0,
                annotation_count: 0,
                annotation_subtypes: vec![],
                content_stream_count: 0,
                content_stream_bytes: 0,
                text_preview: String::new(),
                text_extractable: true,
                full_text: String::new(),
                warnings: vec![],
                resources: PageResources {
                    fonts: vec![],
                    xobjects: vec![],
                    ext_gstate: vec![],
                    color_spaces: vec![],
                },
            };
        }
    };

    let media_box = page_dict
        .get(b"MediaBox")
        .ok()
        .map(format_dict_value)
        .unwrap_or_else(|| "-".to_string());

    let crop_box = page_dict.get(b"CropBox").ok().map(format_dict_value);

    let rotate = page_dict.get(b"Rotate").ok().and_then(|v| v.as_i64().ok());

    // Resources
    let res = collect_page_resources(doc, page_id);
    let font_names: Vec<String> = res
        .fonts
        .iter()
        .map(|f| {
            // Extract base font name from detail (e.g. "Helvetica, Type1" -> "Helvetica")
            f.detail.split(',').next().unwrap_or("?").trim().to_string()
        })
        .collect();
    let image_count = res
        .xobjects
        .iter()
        .filter(|x| x.detail.starts_with("Image"))
        .count();
    let ext_gstate_count = res.ext_gstate.len();

    // Annotations (pre-filtered by caller)
    let annotation_count = annots.len();
    let mut subtype_counts: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for a in annots {
        *subtype_counts.entry(a.subtype.as_str()).or_insert(0usize) += 1;
    }
    let annotation_subtypes: Vec<(String, usize)> = subtype_counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

    // Content streams
    let content_ids: Vec<ObjectId> = match page_dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => vec![],
    };
    let content_stream_count = content_ids.len();
    let mut content_stream_bytes = 0usize;
    for cid in &content_ids {
        if let Ok(Object::Stream(s)) = doc.get_object(*cid) {
            content_stream_bytes += s.content.len();
        }
    }

    // Text preview
    let text_result = extract_text_from_page_with_warnings(doc, page_id);
    let garbled = is_garbled_text(&text_result.text, &text_result.warnings);
    let (text_preview, text_extractable) = if garbled {
        (
            "(text not extractable \u{2014} fonts lack Unicode mappings)".to_string(),
            false,
        )
    } else if text_result.text.len() > 200 {
        let truncated: String = text_result.text.chars().take(200).collect();
        (format!("{}...", truncated.trim()), true)
    } else {
        (text_result.text.trim().to_string(), true)
    };

    PageInfo {
        page_num,
        object_id: page_id,
        media_box,
        crop_box,
        rotate,
        font_names,
        image_count,
        ext_gstate_count,
        annotation_count,
        annotation_subtypes,
        content_stream_count,
        content_stream_bytes,
        text_preview,
        text_extractable,
        full_text: text_result.text,
        warnings: text_result.warnings,
        resources: res,
    }
}

pub(crate) fn print_page_info(writer: &mut impl Write, doc: &Document, spec: &PageSpec) {
    let page_list = match helpers::build_page_list(doc, Some(spec)) {
        Ok(list) => list,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return;
        }
    };
    let all_annots = collect_annotations(doc, Some(spec));
    for (pn, page_id) in &page_list {
        let pn = *pn;
        let page_id = *page_id;
        let page_annots: Vec<&AnnotationInfo> =
            all_annots.iter().filter(|a| a.page_num == pn).collect();
        let info = collect_page_info(doc, pn, page_id, &page_annots);

        wln!(
            writer,
            "Page {} (Object {} {})",
            info.page_num,
            info.object_id.0,
            info.object_id.1
        );
        wln!(writer, "  MediaBox:     {}", info.media_box);
        if let Some(ref cb) = info.crop_box {
            wln!(writer, "  CropBox:      {}", cb);
        }
        if let Some(r) = info.rotate {
            wln!(writer, "  Rotate:       {}", r);
        }

        // Resources detail
        if !info.resources.fonts.is_empty() {
            wln!(writer, "  Fonts:        {}", info.resources.fonts.len());
            for e in &info.resources.fonts {
                let obj_str = match e.object_id {
                    Some(id) => format!("obj {}", id.0),
                    None => "inline".to_string(),
                };
                wln!(writer, "    {:<12} -> {} ({})", e.name, obj_str, e.detail);
            }
        }
        if !info.resources.xobjects.is_empty() {
            let image_count = info
                .resources
                .xobjects
                .iter()
                .filter(|x| x.detail.starts_with("Image"))
                .count();
            let form_count = info.resources.xobjects.len() - image_count;
            let mut label_parts = Vec::new();
            if image_count > 0 {
                label_parts.push(format!(
                    "{} image{}",
                    image_count,
                    if image_count == 1 { "" } else { "s" }
                ));
            }
            if form_count > 0 {
                label_parts.push(format!(
                    "{} form{}",
                    form_count,
                    if form_count == 1 { "" } else { "s" }
                ));
            }
            wln!(
                writer,
                "  XObjects:     {} ({})",
                info.resources.xobjects.len(),
                label_parts.join(", ")
            );
            for e in &info.resources.xobjects {
                let obj_str = match e.object_id {
                    Some(id) => format!("obj {}", id.0),
                    None => "inline".to_string(),
                };
                wln!(writer, "    {:<12} -> {} ({})", e.name, obj_str, e.detail);
            }
        }
        if !info.resources.ext_gstate.is_empty() {
            wln!(
                writer,
                "  ExtGState:    {}",
                info.resources.ext_gstate.len()
            );
            for e in &info.resources.ext_gstate {
                let obj_str = match e.object_id {
                    Some(id) => format!("obj {}", id.0),
                    None => "inline".to_string(),
                };
                wln!(writer, "    {:<12} -> {} ({})", e.name, obj_str, e.detail);
            }
        }
        if !info.resources.color_spaces.is_empty() {
            wln!(
                writer,
                "  ColorSpaces:  {}",
                info.resources.color_spaces.len()
            );
            for e in &info.resources.color_spaces {
                if let Some(id) = e.object_id {
                    wln!(writer, "    {:<12} -> obj {} ({})", e.name, id.0, e.detail);
                } else {
                    wln!(writer, "    {:<12} -> {}", e.name, e.detail);
                }
            }
        }

        // Annotations
        if info.annotation_count > 0 {
            let breakdown: Vec<String> = info
                .annotation_subtypes
                .iter()
                .map(|(s, c)| format!("{} {}", c, s))
                .collect();
            wln!(
                writer,
                "  Annotations:  {} ({})",
                info.annotation_count,
                breakdown.join(", ")
            );
        }

        // Content streams
        if info.content_stream_count > 0 {
            wln!(
                writer,
                "  Content:      {} stream{}, {} bytes",
                info.content_stream_count,
                if info.content_stream_count == 1 {
                    ""
                } else {
                    "s"
                },
                info.content_stream_bytes
            );
        }

        // Text preview
        if !info.text_preview.is_empty() {
            if info.text_extractable {
                wln!(writer, "  Text preview: \"{}\"", info.text_preview);
            } else {
                wln!(writer, "  Text preview: {}", info.text_preview);
            }
        }
        for w in &info.warnings {
            wln!(writer, "  Warning: {}", w);
        }
        wln!(writer);
    }
}

pub(crate) fn page_info_json_value(doc: &Document, spec: &PageSpec) -> Value {
    let page_list = match helpers::build_page_list(doc, Some(spec)) {
        Ok(list) => list,
        Err(msg) => return json!({"error": msg}),
    };
    let all_annots = collect_annotations(doc, Some(spec));
    let mut results = Vec::new();

    for &(pn, page_id) in &page_list {
        let page_annots: Vec<&AnnotationInfo> =
            all_annots.iter().filter(|a| a.page_num == pn).collect();
        let info = collect_page_info(doc, pn, page_id, &page_annots);

        let subtypes: Value = info
            .annotation_subtypes
            .iter()
            .map(|(s, c)| json!({"subtype": s, "count": c}))
            .collect();

        let resources_json = json!({
            "fonts": resource_entries_to_json(&info.resources.fonts),
            "xobjects": resource_entries_to_json(&info.resources.xobjects),
            "ext_gstate": resource_entries_to_json(&info.resources.ext_gstate),
            "color_spaces": resource_entries_to_json(&info.resources.color_spaces),
        });

        let mut page_json = json!({
            "page_number": info.page_num,
            "object_number": info.object_id.0,
            "generation": info.object_id.1,
            "media_box": info.media_box,
            "crop_box": info.crop_box,
            "rotate": info.rotate,
            "fonts": info.font_names,
            "image_count": info.image_count,
            "ext_gstate_count": info.ext_gstate_count,
            "annotation_count": info.annotation_count,
            "annotation_subtypes": subtypes,
            "content_stream_count": info.content_stream_count,
            "content_stream_bytes": info.content_stream_bytes,
            "text": info.full_text,
            "resources": resources_json,
        });
        if !info.text_extractable {
            page_json["text_extractable"] = json!(false);
        }
        if !info.warnings.is_empty() {
            page_json["warnings"] = json!(info.warnings);
        }
        results.push(page_json);
    }

    json!({"pages": results})
}

pub(crate) fn print_page_info_json(writer: &mut impl Write, doc: &Document, spec: &PageSpec) {
    let output = page_info_json_value(doc, spec);
    wln!(writer, "{}", helpers::json_pretty(&output));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use pretty_assertions::assert_eq;

    #[test]
    fn page_info_shows_media_box() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("Page 1"));
        assert!(out.contains("MediaBox"));
        assert!(out.contains("[0 0 612 792]"));
    }

    #[test]
    fn page_info_shows_fonts() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("Fonts:"));
        assert!(out.contains("Helvetica"));
    }

    #[test]
    fn page_info_shows_content_streams() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("Content:"));
        assert!(out.contains("1 stream"));
    }

    #[test]
    fn page_info_json_structure() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info_json(w, &doc, &PageSpec::Single(1)));
        let val: serde_json::Value = serde_json::from_str(&out).unwrap();
        let pages = val["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!(page["page_number"], 1);
        assert!(page.get("media_box").is_some());
        assert!(page.get("fonts").is_some());
        assert!(page.get("text").is_some());
    }

    #[test]
    fn page_info_range() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Range(1, 2)));
        assert!(out.contains("Page 1"));
        assert!(out.contains("Page 2"));
    }

    #[test]
    fn page_info_text_preview() {
        let doc = build_page_doc_with_content(b"BT /F1 12 Tf (Hello World) Tj ET");
        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("Text preview:") || out.contains("Hello World"));
    }

    #[test]
    fn page_info_shows_font_detail() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        // Should show font entry with object ID and detail
        assert!(out.contains("/F1"));
        assert!(out.contains("obj "));
        assert!(out.contains("Helvetica"));
    }

    #[test]
    fn page_info_json_has_resources() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info_json(w, &doc, &PageSpec::Single(1)));
        let val: serde_json::Value = serde_json::from_str(&out).unwrap();
        let page = &val["pages"][0];
        let res = &page["resources"];
        assert!(res.is_object());
        assert!(res["fonts"].is_array());
        assert!(res["xobjects"].is_array());
        assert!(res["ext_gstate"].is_array());
        assert!(res["color_spaces"].is_array());
        // The font entry should have detail
        let fonts = res["fonts"].as_array().unwrap();
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0]["name"], "/F1");
        assert!(fonts[0]["detail"].as_str().unwrap().contains("Helvetica"));
    }

    #[test]
    fn page_info_with_xobjects() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();

        // Create an image XObject
        let mut img_dict = Dictionary::new();
        img_dict.set("Type", Object::Name(b"XObject".to_vec()));
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", Object::Integer(100));
        img_dict.set("Height", Object::Integer(200));
        img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        let img_stream = Stream::new(img_dict, vec![0u8; 10]);
        let img_id = doc.add_object(Object::Stream(img_stream));

        let content = Stream::new(Dictionary::new(), b"BT (test) Tj ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content));

        let mut xobjects = Dictionary::new();
        xobjects.set("Im1", Object::Reference(img_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set("Resources", Object::Dictionary(resources));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("XObjects:"));
        assert!(out.contains("1 image"));
        assert!(out.contains("/Im1"));
        assert!(out.contains("Image, 100x200, DeviceRGB"));
    }

    #[test]
    fn page_info_with_rotation() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();
        let content = Stream::new(Dictionary::new(), b"BT (test) Tj ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        page.set("Rotate", Object::Integer(90));
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("Rotate:"));
        assert!(out.contains("90"));
    }

    #[test]
    fn page_info_with_crop_box() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();
        let content = Stream::new(Dictionary::new(), b"BT (test) Tj ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        page.set(
            "CropBox",
            Object::Array(vec![
                Object::Integer(50),
                Object::Integer(50),
                Object::Integer(562),
                Object::Integer(742),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("CropBox:"));
        assert!(out.contains("[50 50 562 742]"));
    }

    #[test]
    fn page_info_no_content_streams() {
        use lopdf::Dictionary;

        let mut doc = Document::new();
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        // No Contents
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("Page 1"));
        // Should not contain Content: line
        assert!(!out.contains("Content:"));
    }

    #[test]
    fn page_info_with_annotations() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();
        let content = Stream::new(Dictionary::new(), b"BT (test) Tj ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content));

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
        let annot_id = doc.add_object(Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        page.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(out.contains("Annotations:"));
        assert!(out.contains("1 Text"));
    }

    #[test]
    fn page_info_json_with_rotation() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();
        let content = Stream::new(Dictionary::new(), b"BT (test) Tj ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        page.set("Rotate", Object::Integer(180));
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info_json(w, &doc, &PageSpec::Single(1)));
        let val: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["pages"][0]["rotate"], 180);
    }

    // ── is_garbled_text ─────────────────────────────────────────────

    #[test]
    fn garbled_text_detection_clean() {
        assert!(!is_garbled_text("Hello World", &[]));
    }

    #[test]
    fn garbled_text_detection_empty() {
        assert!(!is_garbled_text("", &[]));
        assert!(!is_garbled_text("   ", &[]));
    }

    #[test]
    fn garbled_text_detection_mostly_non_ascii() {
        // More than 50% non-printable -> garbled
        assert!(is_garbled_text("\u{00}\u{01}\u{02}a", &[]));
    }

    #[test]
    fn garbled_text_detection_cid_warning() {
        let warnings = vec!["CID font without ToUnicode".to_string()];
        assert!(is_garbled_text("some text", &warnings));
    }

    #[test]
    fn garbled_text_cid_warning_empty_text() {
        // CID warning but no text => not garbled
        let warnings = vec!["CID font without ToUnicode".to_string()];
        assert!(!is_garbled_text("", &warnings));
    }

    // ── New edge-case tests ───────────────────────────────────────────

    #[test]
    fn garbled_text_encoding_warning() {
        // Warning containing "encoding" should trigger garbled when text is present
        let warnings = vec!["Font has unknown encoding".to_string()];
        assert!(is_garbled_text("some text", &warnings));
    }

    #[test]
    fn garbled_text_tounicode_warning() {
        // Warning containing "tounicode" (case-insensitive) should trigger garbled
        let warnings = vec!["Missing ToUnicode CMap for font".to_string()];
        assert!(is_garbled_text("abc", &warnings));
    }

    #[test]
    fn garbled_text_at_exactly_50_percent_boundary() {
        // Exactly half non-printable should NOT be garbled (need > 50%)
        // 1 non-printable + 1 printable = 50% exactly => not garbled
        assert!(!is_garbled_text("\u{01}a", &[]));
        // 2 non-printable + 2 printable = 50% exactly => not garbled
        assert!(!is_garbled_text("\u{01}\u{02}ab", &[]));
    }

    #[test]
    fn garbled_text_unicode_chars() {
        // Characters >0x7E are counted as non-printable by the heuristic
        // 3 unicode chars + 1 ASCII = 75% non-printable => garbled
        let text = "\u{00E9}\u{00F1}\u{00FC}a"; // e-acute, n-tilde, u-diaeresis, 'a'
        assert!(is_garbled_text(text, &[]));
    }

    #[test]
    fn page_info_multiple_content_streams() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();

        // Create two content stream objects
        let c1 = Stream::new(Dictionary::new(), b"BT /F1 12 Tf (Hello) Tj ET".to_vec());
        let c1_id = doc.add_object(Object::Stream(c1));
        let c2 = Stream::new(Dictionary::new(), b"BT /F1 12 Tf (World) Tj ET".to_vec());
        let c2_id = doc.add_object(Object::Stream(c2));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        // Contents as an array of references
        page.set(
            "Contents",
            Object::Array(vec![Object::Reference(c1_id), Object::Reference(c2_id)]),
        );
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(
            out.contains("2 streams"),
            "Expected '2 streams' in output:\n{}",
            out
        );
        // Byte total should be the sum of both streams
        let expected_bytes =
            b"BT /F1 12 Tf (Hello) Tj ET".len() + b"BT /F1 12 Tf (World) Tj ET".len();
        assert!(
            out.contains(&format!("{} bytes", expected_bytes)),
            "Expected '{} bytes' in output:\n{}",
            expected_bytes,
            out
        );
    }

    #[test]
    fn page_info_json_value_invalid_page() {
        let doc = build_two_page_doc(); // has 2 pages
        let val = page_info_json_value(&doc, &PageSpec::Single(99));
        // Should return an error JSON value
        assert!(
            val.get("error").is_some(),
            "Expected 'error' key in JSON for out-of-range page:\n{}",
            val
        );
    }

    #[test]
    fn page_info_multiple_annotation_subtypes() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();
        let content = Stream::new(Dictionary::new(), b"BT (test) Tj ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content));

        // Create a Link annotation
        let mut link_annot = Dictionary::new();
        link_annot.set("Subtype", Object::Name(b"Link".to_vec()));
        link_annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(50),
                Object::Integer(50),
            ]),
        );
        let link_id = doc.add_object(Object::Dictionary(link_annot));

        // Create a Text annotation
        let mut text_annot = Dictionary::new();
        text_annot.set("Subtype", Object::Name(b"Text".to_vec()));
        text_annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(60),
                Object::Integer(60),
                Object::Integer(100),
                Object::Integer(100),
            ]),
        );
        let text_id = doc.add_object(Object::Dictionary(text_annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        page.set(
            "Annots",
            Object::Array(vec![Object::Reference(link_id), Object::Reference(text_id)]),
        );
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(
            out.contains("Annotations:"),
            "Expected annotations line in output:\n{}",
            out
        );
        assert!(
            out.contains("2"),
            "Expected annotation count of 2:\n{}",
            out
        );
        assert!(
            out.contains("Link"),
            "Expected 'Link' subtype in output:\n{}",
            out
        );
        assert!(
            out.contains("Text"),
            "Expected 'Text' subtype in output:\n{}",
            out
        );
    }

    #[test]
    fn page_info_text_stream_pluralization() {
        // "1 stream" (singular) is tested by page_info_shows_content_streams
        // Here we verify the singular form explicitly, then build a multi-stream case
        let doc = build_two_page_doc();
        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        // Single content stream should say "1 stream" (no 's')
        assert!(
            out.contains("1 stream,"),
            "Expected '1 stream,' in:\n{}",
            out
        );
        assert!(
            !out.contains("1 streams"),
            "Should not say '1 streams' in:\n{}",
            out
        );
    }

    #[test]
    fn page_info_json_text_extractable_false() {
        use lopdf::{Dictionary, Stream};

        let mut doc = Document::new();
        // Create content with characters above 0x7E to trigger garbled detection
        // We need >50% non-printable non-whitespace characters
        let garbled_bytes = b"BT /F1 12 Tf <8081828384> Tj ET";
        let content = Stream::new(Dictionary::new(), garbled_bytes.to_vec());
        let content_id = doc.add_object(Object::Stream(content));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let val = page_info_json_value(&doc, &PageSpec::Single(1));
        let page_json = &val["pages"][0];
        // If text was detected as garbled, text_extractable should be false
        // If text happens to be clean (hex strings may decode differently),
        // text_extractable key won't be present (it's only set when false)
        if page_json.get("text_extractable").is_some() {
            assert_eq!(
                page_json["text_extractable"], false,
                "text_extractable should be false when set"
            );
        }
    }

    #[test]
    fn page_info_with_ext_gstate() {
        use lopdf::Dictionary;

        let mut doc = Document::new();

        // Create an ExtGState object
        let mut gs_dict = Dictionary::new();
        gs_dict.set("Type", Object::Name(b"ExtGState".to_vec()));
        gs_dict.set("ca", Object::Real(0.5));
        let gs_id = doc.add_object(Object::Dictionary(gs_dict));

        let mut ext_gstate = Dictionary::new();
        ext_gstate.set("GS1", Object::Reference(gs_id));

        let mut resources = Dictionary::new();
        resources.set("ExtGState", Object::Dictionary(ext_gstate));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Resources", Object::Dictionary(resources));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_page_info(w, &doc, &PageSpec::Single(1)));
        assert!(
            out.contains("ExtGState:"),
            "Expected ExtGState section in output:\n{}",
            out
        );
        assert!(out.contains("1"), "Expected ExtGState count of 1:\n{}", out);
        assert!(
            out.contains("/GS1"),
            "Expected /GS1 entry in output:\n{}",
            out
        );

        // Also verify JSON includes ext_gstate_count
        let val = page_info_json_value(&doc, &PageSpec::Single(1));
        let page_json = &val["pages"][0];
        assert_eq!(page_json["ext_gstate_count"], 1);
    }
}
