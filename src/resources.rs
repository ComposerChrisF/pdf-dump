use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::BTreeSet;

use crate::helpers::{resolve_dict, format_color_space, find_font_file_id};

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
    let mut visited = BTreeSet::new();
    while let Ok(Object::Dictionary(dict)) = doc.get_object(current_id) {
        if !visited.insert(current_id) { break; }
        if let Ok(res) = dict.get(b"Resources") {
            return resolve_dict(doc, res);
        }
        // Walk up to parent
        if let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference()) {
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
    let embedded = find_font_file_id(doc, dict).map(|_| true);
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
    if let Ok(font_obj) = res_dict.get(b"Font")
        && let Some(fd) = resolve_dict(doc, font_obj)
    {
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
    fonts.sort_by(|a, b| a.name.cmp(&b.name));

    let mut xobjects = Vec::new();
    if let Ok(xobj_obj) = res_dict.get(b"XObject")
        && let Some(xd) = resolve_dict(doc, xobj_obj)
    {
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
    xobjects.sort_by(|a, b| a.name.cmp(&b.name));

    let mut ext_gstate = Vec::new();
    if let Ok(gs_obj) = res_dict.get(b"ExtGState")
        && let Some(gd) = resolve_dict(doc, gs_obj)
    {
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
    ext_gstate.sort_by(|a, b| a.name.cmp(&b.name));

    let mut color_spaces = Vec::new();
    if let Ok(cs_obj) = res_dict.get(b"ColorSpace")
        && let Some(cd) = resolve_dict(doc, cs_obj)
    {
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
    color_spaces.sort_by(|a, b| a.name.cmp(&b.name));

    PageResources { fonts, xobjects, ext_gstate, color_spaces }
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

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Dictionary, Object, Stream};
    use pretty_assertions::assert_eq;

    // ── resolve_page_resources ───────────────────────────────────────

    #[test]
    fn resolve_resources_direct_on_page() {
        // Arrange: page has Resources dict directly
        let mut doc = Document::new();
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(Dictionary::new()));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        // Act
        let result = resolve_page_resources(&doc, (1, 0));

        // Assert
        assert!(result.is_some());
        assert!(result.unwrap().has(b"Font"));
    }

    #[test]
    fn resolve_resources_via_reference() {
        // Arrange: page has Resources as a reference
        let mut doc = Document::new();
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(Dictionary::new()));
        doc.objects.insert((10, 0), Object::Dictionary(resources));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Reference((10, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        // Act
        let result = resolve_page_resources(&doc, (1, 0));

        // Assert
        assert!(result.is_some());
        assert!(result.unwrap().has(b"Font"));
    }

    #[test]
    fn resolve_resources_inherited_from_parent() {
        // Arrange: page lacks Resources, but parent Pages node has them
        let mut doc = Document::new();
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(Dictionary::new()));

        let mut parent = Dictionary::new();
        parent.set("Type", Object::Name(b"Pages".to_vec()));
        parent.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((2, 0), Object::Dictionary(parent));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        // Act
        let result = resolve_page_resources(&doc, (1, 0));

        // Assert
        assert!(result.is_some());
        assert!(result.unwrap().has(b"Font"));
    }

    #[test]
    fn resolve_resources_missing_entirely() {
        // Arrange: no Resources on page or parents
        let mut doc = Document::new();
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        // Act
        let result = resolve_page_resources(&doc, (1, 0));

        // Assert
        assert!(result.is_none());
    }

    #[test]
    fn resolve_resources_cycle_detection() {
        // Arrange: circular parent chain
        let mut doc = Document::new();
        let mut page_a = Dictionary::new();
        page_a.set("Type", Object::Name(b"Page".to_vec()));
        page_a.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page_a));

        let mut page_b = Dictionary::new();
        page_b.set("Type", Object::Name(b"Page".to_vec()));
        page_b.set("Parent", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_b));

        // Act - should not hang
        let result = resolve_page_resources(&doc, (1, 0));

        // Assert
        assert!(result.is_none());
    }

    #[test]
    fn resolve_resources_nonexistent_page() {
        let doc = Document::new();

        let result = resolve_page_resources(&doc, (999, 0));

        assert!(result.is_none());
    }

    // ── font_detail ─────────────────────────────────────────────────

    #[test]
    fn font_detail_full_info() {
        // Arrange: font dict with BaseFont, Subtype
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        // Act
        let detail = font_detail(&doc, (5, 0));

        // Assert
        assert!(detail.contains("Helvetica"));
        assert!(detail.contains("Type1"));
    }

    #[test]
    fn font_detail_missing_basefont() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let detail = font_detail(&doc, (5, 0));

        assert_eq!(detail, "TrueType");
    }

    #[test]
    fn font_detail_empty_dict() {
        let mut doc = Document::new();
        let font = Dictionary::new();
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let detail = font_detail(&doc, (5, 0));

        assert_eq!(detail, "?");
    }

    #[test]
    fn font_detail_nonexistent_object() {
        let doc = Document::new();

        let detail = font_detail(&doc, (999, 0));

        assert_eq!(detail, "?");
    }

    #[test]
    fn font_detail_from_stream_object() {
        // Some font objects are streams (e.g., CIDFont with embedded data)
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"ArialMT".to_vec()));
        dict.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 10]);
        doc.objects.insert((5, 0), Object::Stream(stream));

        let detail = font_detail(&doc, (5, 0));

        assert!(detail.contains("ArialMT"));
        assert!(detail.contains("CIDFontType2"));
    }

    // ── xobject_detail ──────────────────────────────────────────────

    #[test]
    fn xobject_detail_image() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        dict.set("Width", Object::Integer(640));
        dict.set("Height", Object::Integer(480));
        dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 10]);
        doc.objects.insert((5, 0), Object::Stream(stream));

        let detail = xobject_detail(&doc, (5, 0));

        assert_eq!(detail, "Image, 640x480, DeviceRGB");
    }

    #[test]
    fn xobject_detail_form() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 10]);
        doc.objects.insert((5, 0), Object::Stream(stream));

        let detail = xobject_detail(&doc, (5, 0));

        assert_eq!(detail, "Form");
    }

    #[test]
    fn xobject_detail_image_missing_dimensions() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        // No Width, Height, or ColorSpace
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((5, 0), Object::Stream(stream));

        let detail = xobject_detail(&doc, (5, 0));

        assert_eq!(detail, "Image, 0x0, -");
    }

    #[test]
    fn xobject_detail_missing_subtype() {
        let mut doc = Document::new();
        let dict = Dictionary::new();
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((5, 0), Object::Stream(stream));

        let detail = xobject_detail(&doc, (5, 0));

        assert_eq!(detail, "?");
    }

    #[test]
    fn xobject_detail_nonexistent() {
        let doc = Document::new();

        let detail = xobject_detail(&doc, (999, 0));

        assert_eq!(detail, "?");
    }

    #[test]
    fn xobject_detail_non_stream_object() {
        // If object is a dictionary (not stream), should return "?"
        let mut doc = Document::new();
        let dict = Dictionary::new();
        doc.objects.insert((5, 0), Object::Dictionary(dict));

        let detail = xobject_detail(&doc, (5, 0));

        assert_eq!(detail, "?");
    }

    // ── collect_page_resources ───────────────────────────────────────

    #[test]
    fn collect_resources_no_resources() {
        let mut doc = Document::new();
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert!(res.fonts.is_empty());
        assert!(res.xobjects.is_empty());
        assert!(res.ext_gstate.is_empty());
        assert!(res.color_spaces.is_empty());
    }

    #[test]
    fn collect_resources_with_fonts() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("BaseFont", Object::Name(b"Courier".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font));

        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((10, 0)));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.fonts.len(), 1);
        assert_eq!(res.fonts[0].name, "/F1");
        assert!(res.fonts[0].detail.contains("Courier"));
        assert_eq!(res.fonts[0].object_id, Some((10, 0)));
    }

    #[test]
    fn collect_resources_inline_font() {
        // Font dict is inline (not a reference)
        let mut doc = Document::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Integer(42)); // Not a reference
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.fonts.len(), 1);
        assert_eq!(res.fonts[0].name, "/F1");
        assert_eq!(res.fonts[0].detail, "inline");
        assert!(res.fonts[0].object_id.is_none());
    }

    #[test]
    fn collect_resources_with_xobjects() {
        let mut doc = Document::new();
        let mut img_dict = Dictionary::new();
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", Object::Integer(100));
        img_dict.set("Height", Object::Integer(200));
        img_dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        let img = Stream::new(img_dict, vec![0u8; 5]);
        doc.objects.insert((10, 0), Object::Stream(img));

        let mut xobj_dict = Dictionary::new();
        xobj_dict.set("Im0", Object::Reference((10, 0)));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobj_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.xobjects.len(), 1);
        assert_eq!(res.xobjects[0].name, "/Im0");
        assert!(res.xobjects[0].detail.contains("Image"));
        assert!(res.xobjects[0].detail.contains("100x200"));
    }

    #[test]
    fn collect_resources_with_ext_gstate() {
        let mut doc = Document::new();
        let mut gs = Dictionary::new();
        gs.set("ca", Object::Real(0.5));
        gs.set("CA", Object::Real(0.8));
        doc.objects.insert((10, 0), Object::Dictionary(gs));

        let mut gs_dict = Dictionary::new();
        gs_dict.set("GS0", Object::Reference((10, 0)));
        let mut resources = Dictionary::new();
        resources.set("ExtGState", Object::Dictionary(gs_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.ext_gstate.len(), 1);
        assert_eq!(res.ext_gstate[0].name, "/GS0");
        assert_eq!(res.ext_gstate[0].detail, "2 keys");
    }

    #[test]
    fn collect_resources_with_color_spaces_name() {
        let mut doc = Document::new();
        let mut cs_dict = Dictionary::new();
        cs_dict.set("CS0", Object::Name(b"DeviceRGB".to_vec()));
        let mut resources = Dictionary::new();
        resources.set("ColorSpace", Object::Dictionary(cs_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.color_spaces.len(), 1);
        assert_eq!(res.color_spaces[0].name, "/CS0");
        assert_eq!(res.color_spaces[0].detail, "DeviceRGB");
    }

    #[test]
    fn collect_resources_with_color_spaces_array() {
        let mut doc = Document::new();
        let cs_array = Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Reference((20, 0)),
        ]);
        let mut cs_dict = Dictionary::new();
        cs_dict.set("CS1", cs_array);
        let mut resources = Dictionary::new();
        resources.set("ColorSpace", Object::Dictionary(cs_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.color_spaces.len(), 1);
        assert_eq!(res.color_spaces[0].detail, "ICCBased");
    }

    #[test]
    fn collect_resources_with_color_spaces_reference() {
        let mut doc = Document::new();
        // Resolved reference target is a Name
        doc.objects.insert((20, 0), Object::Name(b"DeviceCMYK".to_vec()));

        let mut cs_dict = Dictionary::new();
        cs_dict.set("CS2", Object::Reference((20, 0)));
        let mut resources = Dictionary::new();
        resources.set("ColorSpace", Object::Dictionary(cs_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.color_spaces.len(), 1);
        assert_eq!(res.color_spaces[0].detail, "DeviceCMYK");
        assert_eq!(res.color_spaces[0].object_id, Some((20, 0)));
    }

    #[test]
    fn collect_resources_color_space_broken_reference() {
        let mut doc = Document::new();
        let mut cs_dict = Dictionary::new();
        cs_dict.set("CS3", Object::Reference((999, 0))); // dangling ref
        let mut resources = Dictionary::new();
        resources.set("ColorSpace", Object::Dictionary(cs_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.color_spaces.len(), 1);
        assert_eq!(res.color_spaces[0].detail, "999 0 R");
    }

    #[test]
    fn collect_resources_sorts_by_name() {
        let mut doc = Document::new();
        let mut f1 = Dictionary::new();
        f1.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(f1));
        let mut f2 = Dictionary::new();
        f2.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((11, 0), Object::Dictionary(f2));

        let mut font_dict = Dictionary::new();
        font_dict.set("F2", Object::Reference((11, 0)));
        font_dict.set("F1", Object::Reference((10, 0)));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_dict));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        let res = collect_page_resources(&doc, (1, 0));

        assert_eq!(res.fonts.len(), 2);
        assert_eq!(res.fonts[0].name, "/F1");
        assert_eq!(res.fonts[1].name, "/F2");
    }

    // ── resource_entries_to_json ─────────────────────────────────────

    #[test]
    fn entries_to_json_with_object_id() {
        let entries = vec![ResourceEntry {
            name: "/F1".to_string(),
            object_id: Some((10, 0)),
            detail: "Helvetica, Type1".to_string(),
        }];

        let json = resource_entries_to_json(&entries);

        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["name"], "/F1");
        assert_eq!(json[0]["detail"], "Helvetica, Type1");
        assert_eq!(json[0]["object_number"], 10);
        assert_eq!(json[0]["generation"], 0);
    }

    #[test]
    fn entries_to_json_without_object_id() {
        let entries = vec![ResourceEntry {
            name: "/F1".to_string(),
            object_id: None,
            detail: "inline".to_string(),
        }];

        let json = resource_entries_to_json(&entries);

        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["name"], "/F1");
        assert!(json[0].get("object_number").is_none());
    }

    #[test]
    fn entries_to_json_empty() {
        let entries: Vec<ResourceEntry> = vec![];
        let json = resource_entries_to_json(&entries);
        assert!(json.is_empty());
    }
}
