use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::types::PageSpec;
use crate::resources::collect_page_resources;
use crate::annotations::collect_annotations;
use crate::text::extract_text_from_page_with_warnings;
use crate::helpers::format_dict_value;

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
}

fn collect_page_info(doc: &Document, page_num: u32, page_id: ObjectId) -> PageInfo {
    let page_dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return PageInfo {
            page_num, object_id: page_id,
            media_box: "-".to_string(), crop_box: None, rotate: None,
            font_names: vec![], image_count: 0, ext_gstate_count: 0,
            annotation_count: 0, annotation_subtypes: vec![],
            content_stream_count: 0, content_stream_bytes: 0,
            text_preview: String::new(),
        },
    };

    let media_box = page_dict.get(b"MediaBox").ok()
        .map(format_dict_value)
        .unwrap_or_else(|| "-".to_string());

    let crop_box = page_dict.get(b"CropBox").ok()
        .map(format_dict_value);

    let rotate = page_dict.get(b"Rotate").ok()
        .and_then(|v| v.as_i64().ok());

    // Resources
    let res = collect_page_resources(doc, page_id);
    let font_names: Vec<String> = res.fonts.iter().map(|f| {
        // Extract base font name from detail (e.g. "Helvetica, Type1" -> "Helvetica")
        f.detail.split(',').next().unwrap_or("?").trim().to_string()
    }).collect();
    let image_count = res.xobjects.iter().filter(|x| x.detail.starts_with("Image")).count();
    let ext_gstate_count = res.ext_gstate.len();

    // Annotations
    let spec = PageSpec::Single(page_num);
    let annots = collect_annotations(doc, Some(&spec));
    let annotation_count = annots.len();
    let mut subtype_counts = std::collections::BTreeMap::new();
    for a in &annots {
        *subtype_counts.entry(a.subtype.clone()).or_insert(0usize) += 1;
    }
    let annotation_subtypes: Vec<(String, usize)> = subtype_counts.into_iter().collect();

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
    let text_preview = if text_result.text.len() > 200 {
        let truncated: String = text_result.text.chars().take(200).collect();
        format!("{}...", truncated.trim())
    } else {
        text_result.text.trim().to_string()
    };

    PageInfo {
        page_num, object_id: page_id,
        media_box, crop_box, rotate,
        font_names, image_count, ext_gstate_count,
        annotation_count, annotation_subtypes,
        content_stream_count, content_stream_bytes,
        text_preview,
    }
}

pub(crate) fn print_page_info(writer: &mut impl Write, doc: &Document, spec: &PageSpec) {
    let pages = doc.get_pages();
    for pn in spec.pages() {
        let page_id = match pages.get(&pn) {
            Some(&id) => id,
            None => {
                eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                std::process::exit(1);
            }
        };
        let info = collect_page_info(doc, pn, page_id);

        writeln!(writer, "Page {} (Object {} {})", info.page_num, info.object_id.0, info.object_id.1).unwrap();
        writeln!(writer, "  MediaBox:     {}", info.media_box).unwrap();
        if let Some(ref cb) = info.crop_box {
            writeln!(writer, "  CropBox:      {}", cb).unwrap();
        }
        if let Some(r) = info.rotate {
            writeln!(writer, "  Rotate:       {}", r).unwrap();
        }

        // Resources summary
        if !info.font_names.is_empty() {
            writeln!(writer, "  Fonts:        {} ({})", info.font_names.len(),
                info.font_names.join(", ")).unwrap();
        }
        if info.image_count > 0 {
            writeln!(writer, "  Images:       {}", info.image_count).unwrap();
        }
        if info.ext_gstate_count > 0 {
            writeln!(writer, "  ExtGState:    {}", info.ext_gstate_count).unwrap();
        }

        // Annotations
        if info.annotation_count > 0 {
            let breakdown: Vec<String> = info.annotation_subtypes.iter()
                .map(|(s, c)| format!("{} {}", c, s))
                .collect();
            writeln!(writer, "  Annotations:  {} ({})", info.annotation_count, breakdown.join(", ")).unwrap();
        }

        // Content streams
        if info.content_stream_count > 0 {
            writeln!(writer, "  Content:      {} stream{}, {} bytes",
                info.content_stream_count,
                if info.content_stream_count == 1 { "" } else { "s" },
                info.content_stream_bytes).unwrap();
        }

        // Text preview
        if !info.text_preview.is_empty() {
            writeln!(writer, "  Text preview: \"{}\"", info.text_preview).unwrap();
        }
        writeln!(writer).unwrap();
    }
}

pub(crate) fn print_page_info_json(writer: &mut impl Write, doc: &Document, spec: &PageSpec) {
    let pages = doc.get_pages();
    let mut results = Vec::new();

    for pn in spec.pages() {
        let page_id = match pages.get(&pn) {
            Some(&id) => id,
            None => {
                eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                std::process::exit(1);
            }
        };
        let info = collect_page_info(doc, pn, page_id);

        let text_result = extract_text_from_page_with_warnings(doc, page_id);

        let subtypes: Value = info.annotation_subtypes.iter()
            .map(|(s, c)| json!({"subtype": s, "count": c}))
            .collect();

        results.push(json!({
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
            "text": text_result.text,
        }));
    }

    if results.len() == 1 {
        writeln!(writer, "{}", serde_json::to_string_pretty(&results[0]).unwrap()).unwrap();
    } else {
        let output = json!({"pages": results});
        writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
    }
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
        assert_eq!(val["page_number"], 1);
        assert!(val.get("media_box").is_some());
        assert!(val.get("fonts").is_some());
        assert!(val.get("text").is_some());
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
}
