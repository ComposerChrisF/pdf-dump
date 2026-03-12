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
}
