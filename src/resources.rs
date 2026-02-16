use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::types::PageSpec;
use crate::helpers::{resolve_dict, format_color_space};

pub(crate) struct ResourceEntry {
    pub name: String,
    pub object_id: Option<ObjectId>,
    pub detail: String,
}

pub(crate) struct PageResources {
    pub fonts: Vec<ResourceEntry>,
    pub xobjects: Vec<ResourceEntry>,
    pub ext_gstate: Vec<ResourceEntry>,
    pub color_spaces: Vec<ResourceEntry>,
}

pub(crate) fn resolve_page_resources(doc: &Document, page_id: ObjectId) -> Option<&lopdf::Dictionary> {
    // Walk up the page tree to find inherited Resources
    let mut current_id = page_id;
    while let Ok(Object::Dictionary(dict)) = doc.get_object(current_id) {
        if let Ok(res) = dict.get(b"Resources") {
            return resolve_dict(doc, res);
        }
        // Walk up to parent
        if let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference()) {
            if parent_ref == current_id { break; }
            current_id = parent_ref;
        } else {
            break;
        }
    }
    None
}

pub(crate) fn font_detail(doc: &Document, obj_id: ObjectId) -> String {
    let dict = match doc.get_object(obj_id) {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Stream(s)) => &s.dict,
        _ => return "?".to_string(),
    };
    let base_font = dict.get(b"BaseFont").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));
    let subtype = dict.get(b"Subtype").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));
    let embedded = dict.get(b"FontDescriptor").ok()
        .and_then(|v| v.as_reference().ok())
        .and_then(|fd_id| doc.get_object(fd_id).ok())
        .and_then(|fd_obj| {
            let fd_dict = match fd_obj {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                _ => return None,
            };
            for key in &[b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                if fd_dict.get(key).ok().and_then(|v| v.as_reference().ok()).is_some() {
                    return Some(true);
                }
            }
            None
        });
    let mut parts = Vec::new();
    if let Some(bf) = base_font { parts.push(bf); }
    if let Some(st) = subtype { parts.push(st); }
    if embedded == Some(true) { parts.push("embedded".to_string()); }
    if parts.is_empty() { "?".to_string() } else { parts.join(", ") }
}

pub(crate) fn xobject_detail(doc: &Document, obj_id: ObjectId) -> String {
    match doc.get_object(obj_id) {
        Ok(Object::Stream(s)) => {
            let subtype = s.dict.get(b"Subtype").ok()
                .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                .unwrap_or_else(|| "?".to_string());
            if subtype == "Image" {
                let w = s.dict.get(b"Width").ok().and_then(|v| v.as_i64().ok()).unwrap_or(0);
                let h = s.dict.get(b"Height").ok().and_then(|v| v.as_i64().ok()).unwrap_or(0);
                let cs = s.dict.get(b"ColorSpace").ok()
                    .map(|v| format_color_space(v, doc))
                    .unwrap_or_else(|| "-".to_string());
                format!("Image, {}x{}, {}", w, h, cs)
            } else {
                subtype
            }
        }
        _ => "?".to_string(),
    }
}

pub(crate) fn collect_page_resources(doc: &Document, page_id: ObjectId) -> PageResources {
    let empty = PageResources {
        fonts: vec![], xobjects: vec![], ext_gstate: vec![], color_spaces: vec![],
    };

    let res_dict = match resolve_page_resources(doc, page_id) {
        Some(d) => d,
        None => return empty,
    };

    let mut fonts = Vec::new();
    if let Ok(font_obj) = res_dict.get(b"Font") {
        let font_dict = match font_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(fd) = font_dict {
            for (name, val) in fd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                if let Ok(id) = val.as_reference() {
                    fonts.push(ResourceEntry {
                        name: name_str,
                        object_id: Some(id),
                        detail: font_detail(doc, id),
                    });
                } else {
                    fonts.push(ResourceEntry { name: name_str, object_id: None, detail: "inline".to_string() });
                }
            }
        }
    }
    fonts.sort_by(|a, b| a.name.cmp(&b.name));

    let mut xobjects = Vec::new();
    if let Ok(xobj_obj) = res_dict.get(b"XObject") {
        let xobj_dict = match xobj_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(xd) = xobj_dict {
            for (name, val) in xd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                if let Ok(id) = val.as_reference() {
                    xobjects.push(ResourceEntry {
                        name: name_str,
                        object_id: Some(id),
                        detail: xobject_detail(doc, id),
                    });
                }
            }
        }
    }
    xobjects.sort_by(|a, b| a.name.cmp(&b.name));

    let mut ext_gstate = Vec::new();
    if let Ok(gs_obj) = res_dict.get(b"ExtGState") {
        let gs_dict = match gs_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(gd) = gs_dict {
            for (name, val) in gd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                if let Ok(id) = val.as_reference() {
                    let key_count = match doc.get_object(id) {
                        Ok(Object::Dictionary(d)) => d.len(),
                        Ok(Object::Stream(s)) => s.dict.len(),
                        _ => 0,
                    };
                    ext_gstate.push(ResourceEntry {
                        name: name_str,
                        object_id: Some(id),
                        detail: format!("{} keys", key_count),
                    });
                }
            }
        }
    }
    ext_gstate.sort_by(|a, b| a.name.cmp(&b.name));

    let mut color_spaces = Vec::new();
    if let Ok(cs_obj) = res_dict.get(b"ColorSpace") {
        let cs_dict = match cs_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(cd) = cs_dict {
            for (name, val) in cd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                let detail = match val {
                    Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                    Object::Array(arr) => {
                        arr.first().and_then(|v| v.as_name().ok())
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                            .unwrap_or_else(|| format!("[{} items]", arr.len()))
                    }
                    Object::Reference(id) => {
                        if let Ok(resolved) = doc.get_object(*id) {
                            match resolved {
                                Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                                Object::Array(arr) => {
                                    arr.first().and_then(|v| v.as_name().ok())
                                        .map(|n| String::from_utf8_lossy(n).into_owned())
                                        .unwrap_or_else(|| format!("[{} items]", arr.len()))
                                }
                                _ => format!("obj {}", id.0),
                            }
                        } else {
                            format!("{} {} R", id.0, id.1)
                        }
                    }
                    _ => "?".to_string(),
                };
                let obj_id = val.as_reference().ok();
                color_spaces.push(ResourceEntry { name: name_str, object_id: obj_id, detail });
            }
        }
    }
    color_spaces.sort_by(|a, b| a.name.cmp(&b.name));

    PageResources { fonts, xobjects, ext_gstate, color_spaces }
}

pub(crate) fn print_resource_section(writer: &mut impl Write, label: &str, entries: &[ResourceEntry]) {
    if entries.is_empty() { return; }
    writeln!(writer, "{}:", label).unwrap();
    for e in entries {
        let obj_str = match e.object_id {
            Some(id) => format!("obj {}", id.0),
            None => "inline".to_string(),
        };
        writeln!(writer, "  {:<6} -> {} ({})", e.name, obj_str, e.detail).unwrap();
    }
}

pub(crate) fn print_resources(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    for (pn, page_id) in &page_list {
        let res = collect_page_resources(doc, *page_id);
        writeln!(writer, "--- Page {} Resources ---", pn).unwrap();
        print_resource_section(writer, "Fonts", &res.fonts);
        print_resource_section(writer, "XObjects", &res.xobjects);
        print_resource_section(writer, "ExtGState", &res.ext_gstate);
        print_resource_section(writer, "ColorSpaces", &res.color_spaces);
        writeln!(writer).unwrap();
    }
}

pub(crate) fn resource_entries_to_json(entries: &[ResourceEntry]) -> Vec<Value> {
    entries.iter().map(|e| {
        let mut obj = json!({
            "name": e.name,
            "detail": e.detail,
        });
        if let Some(id) = e.object_id {
            obj["object_number"] = json!(id.0);
            obj["generation"] = json!(id.1);
        }
        obj
    }).collect()
}

pub(crate) fn print_resources_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    let mut page_results = Vec::new();
    for (pn, page_id) in &page_list {
        let res = collect_page_resources(doc, *page_id);
        page_results.push(json!({
            "page_number": pn,
            "fonts": resource_entries_to_json(&res.fonts),
            "xobjects": resource_entries_to_json(&res.xobjects),
            "ext_gstate": resource_entries_to_json(&res.ext_gstate),
            "color_spaces": resource_entries_to_json(&res.color_spaces),
        }));
    }

    let output = json!({"pages": page_results});
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::{Dictionary, Stream};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

    fn build_page_doc_with_resources() -> Document {
        let mut doc = Document::new();
        // Font object
        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name(b"Font".to_vec()));
        font_dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        font_dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font_dict));
        // XObject image
        let mut img_dict = Dictionary::new();
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", Object::Integer(640));
        img_dict.set("Height", Object::Integer(480));
        img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        let img_stream = Stream::new(img_dict, vec![0u8; 100]);
        doc.objects.insert((20, 0), Object::Stream(img_stream));
        // Resources dict
        let mut font_res = Dictionary::new();
        font_res.set("F1", Object::Reference((10, 0)));
        let mut xobj_res = Dictionary::new();
        xobj_res.set("Im1", Object::Reference((20, 0)));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_res));
        resources.set("XObject", Object::Dictionary(xobj_res));
        doc.objects.insert((5, 0), Object::Dictionary(resources));
        // Page
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Resources", Object::Reference((5, 0)));
        page_dict.set("Parent", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_dict));
        // Pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((3, 0), Object::Dictionary(pages_dict));
        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((3, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        doc
    }

    #[test]
    fn resources_shows_fonts_and_xobjects() {
        let doc = build_page_doc_with_resources();
        let out = output_of(|w| print_resources(w, &doc, None));
        assert!(out.contains("Page 1 Resources"));
        assert!(out.contains("Fonts:"));
        assert!(out.contains("/F1"));
        assert!(out.contains("obj 10"));
        assert!(out.contains("Helvetica"));
        assert!(out.contains("XObjects:"));
        assert!(out.contains("/Im1"));
        assert!(out.contains("obj 20"));
        assert!(out.contains("Image"));
    }

    #[test]
    fn resources_json_structure() {
        let doc = build_page_doc_with_resources();
        let out = output_of(|w| print_resources_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["pages"].is_array());
        let page = &parsed["pages"][0];
        assert_eq!(page["page_number"], 1);
        assert!(!page["fonts"].as_array().unwrap().is_empty());
        assert!(!page["xobjects"].as_array().unwrap().is_empty());
    }

    #[test]
    fn resources_with_page_filter() {
        let doc = build_page_doc_with_resources();
        let spec = PageSpec::Single(1);
        let out = output_of(|w| print_resources(w, &doc, Some(&spec)));
        assert!(out.contains("Page 1 Resources"));
    }

    #[test]
    fn resources_inherits_from_parent() {
        let mut doc = Document::new();
        // Font
        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name(b"Font".to_vec()));
        font_dict.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font_dict));
        // Resources on Pages (parent), not on Page
        let mut font_res = Dictionary::new();
        font_res.set("F1", Object::Reference((10, 0)));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_res));
        // Page without Resources
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_dict));
        // Pages with Resources
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        pages_dict.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((3, 0), Object::Dictionary(pages_dict));
        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((3, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let out = output_of(|w| print_resources(w, &doc, None));
        assert!(out.contains("Fonts:"));
        assert!(out.contains("Courier"));
    }

    #[test]
    fn resources_no_resources_empty() {
        let mut doc = Document::new();
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page_dict));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((1, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((3, 0)));
        let out = output_of(|w| print_resources(w, &doc, None));
        assert!(out.contains("Page 1 Resources"));
        // No Fonts: or XObjects: sections should appear
        assert!(!out.contains("Fonts:"));
    }

}
