use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::helpers::{resolve_dict, resolve_array, name_to_string};

pub(crate) struct OcgInfo {
    pub object_id: ObjectId,
    pub name: String,
    pub default_state: String,
    pub page_numbers: Vec<u32>,
}

pub(crate) fn collect_layers(doc: &Document) -> Vec<OcgInfo> {
    let catalog_id = match doc.trailer.get(b"Root").ok()
        .and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return Vec::new(),
    };
    let catalog = match doc.get_object(catalog_id).ok() {
        Some(Object::Dictionary(d)) => d,
        _ => return Vec::new(),
    };

    let oc_props = match catalog.get(b"OCProperties").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return Vec::new(),
    };

    // Get OCGs array
    let ocgs_arr = match oc_props.get(b"OCGs").ok().and_then(|o| resolve_array(doc, o)) {
        Some(a) => a,
        None => return Vec::new(),
    };

    // Get default config /D
    let empty_dict = lopdf::Dictionary::new();
    let d_dict = oc_props.get(b"D").ok()
        .and_then(|o| resolve_dict(doc, o))
        .unwrap_or(&empty_dict);

    let base_state: &str = &d_dict.get(b"BaseState").ok()
        .and_then(name_to_string)
        .unwrap_or_else(|| "ON".to_string());

    // Collect ON/OFF override sets
    let on_set: BTreeSet<ObjectId> = extract_id_set(d_dict, b"ON");
    let off_set: BTreeSet<ObjectId> = extract_id_set(d_dict, b"OFF");

    // Build page_id -> page_num lookup
    let pages = doc.get_pages();
    let page_id_to_num: BTreeMap<ObjectId, u32> = pages.into_iter().map(|(num, id)| (id, num)).collect();

    // Scan pages for OCG references in /Properties
    let mut ocg_pages: BTreeMap<ObjectId, Vec<u32>> = BTreeMap::new();
    for (&page_id, &page_num) in &page_id_to_num {
        if let Ok(page_obj) = doc.get_object(page_id) {
            let page_dict = match page_obj {
                Object::Dictionary(d) => d,
                _ => continue,
            };
            scan_page_for_ocgs(doc, page_dict, page_num, &mut ocg_pages);
        }
    }

    // Build OcgInfo for each OCG
    let mut layers = Vec::new();
    for item in ocgs_arr {
        let ocg_id = match item {
            Object::Reference(id) => *id,
            _ => continue,
        };
        let ocg_dict = match doc.get_object(ocg_id).ok() {
            Some(Object::Dictionary(d)) => d,
            _ => continue,
        };

        let name = ocg_dict.get(b"Name").ok()
            .and_then(|v| if let Object::String(bytes, _) = v { Some(String::from_utf8_lossy(bytes).into_owned()) } else { None })
            .unwrap_or_else(|| "(unnamed)".to_string());

        let default_state = if off_set.contains(&ocg_id) {
            "OFF".to_string()
        } else if on_set.contains(&ocg_id) {
            "ON".to_string()
        } else {
            base_state.to_string()
        };

        let page_numbers = ocg_pages.remove(&ocg_id).unwrap_or_default();
        layers.push(OcgInfo { object_id: ocg_id, name, default_state, page_numbers });
    }

    layers
}

fn extract_id_set(dict: &lopdf::Dictionary, key: &[u8]) -> BTreeSet<ObjectId> {
    let mut set = BTreeSet::new();
    if let Ok(Object::Array(arr)) = dict.get(key) {
        for item in arr {
            if let Object::Reference(id) = item {
                set.insert(*id);
            }
        }
    }
    set
}

fn scan_page_for_ocgs(doc: &Document, page_dict: &lopdf::Dictionary, page_num: u32, ocg_pages: &mut BTreeMap<ObjectId, Vec<u32>>) {
    // Look for Resources -> Properties which holds OCG references
    let resources = match page_dict.get(b"Resources").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => {
            // Try inheriting from parent
            if let Ok(Object::Reference(parent_id)) = page_dict.get(b"Parent")
                && let Ok(Object::Dictionary(parent)) = doc.get_object(*parent_id)
                && let Some(d) = parent.get(b"Resources").ok().and_then(|o| resolve_dict(doc, o))
            {
                d
            } else {
                return;
            }
        }
    };

    let props = match resources.get(b"Properties").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return,
    };

    for (_, val) in props.iter() {
        let ocg_id = match val {
            Object::Reference(id) => *id,
            _ => continue,
        };
        // Verify it's an OCG (has /Type /OCG)
        if let Ok(obj) = doc.get_object(ocg_id) {
            let dict = match obj {
                Object::Dictionary(d) => d,
                _ => continue,
            };
            if dict.get_type().ok().is_some_and(|t| t == b"OCG") {
                ocg_pages.entry(ocg_id).or_default().push(page_num);
            }
        }
    }
}

pub(crate) fn print_layers(writer: &mut impl Write, doc: &Document) {
    let layers = collect_layers(doc);
    wln!(writer, "{} layers found\n", layers.len());
    if layers.is_empty() { return; }
    wln!(writer, "  {:>4}  {:<30} {:<8} Pages", "Obj#", "Name", "Default");
    for l in &layers {
        let pages_str = if l.page_numbers.is_empty() {
            "-".to_string()
        } else {
            l.page_numbers.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
        };
        wln!(writer, "  {:>4}  {:<30} {:<8} {}", l.object_id.0, l.name, l.default_state, pages_str);
    }
}

pub(crate) fn layers_json_value(doc: &Document) -> Value {
    let layers = collect_layers(doc);
    let items: Vec<Value> = layers.iter().map(|l| {
        json!({
            "object_number": l.object_id.0,
            "generation": l.object_id.1,
            "name": l.name,
            "default_state": l.default_state,
            "pages": l.page_numbers,
        })
    }).collect();
    json!({
        "layer_count": items.len(),
        "layers": items,
    })
}

#[cfg(test)]
pub(crate) fn print_layers_json(writer: &mut impl Write, doc: &Document) {
    let output = layers_json_value(doc);
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

    fn make_ocg_doc() -> Document {
        let mut doc = Document::new();

        // OCG objects
        let mut ocg1 = Dictionary::new();
        ocg1.set("Type", Object::Name(b"OCG".to_vec()));
        ocg1.set("Name", Object::String(b"Layer1".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg1));

        let mut ocg2 = Dictionary::new();
        ocg2.set("Type", Object::Name(b"OCG".to_vec()));
        ocg2.set("Name", Object::String(b"Layer2".to_vec(), StringFormat::Literal));
        doc.objects.insert((11, 0), Object::Dictionary(ocg2));

        // Default config
        let mut d_config = Dictionary::new();
        d_config.set("BaseState", Object::Name(b"ON".to_vec()));
        d_config.set("OFF", Object::Array(vec![Object::Reference((11, 0))]));

        // OCProperties
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        oc_props.set("D", Object::Dictionary(d_config));

        // Page with Properties referencing OCG
        let mut props = Dictionary::new();
        props.set("MC0", Object::Reference((10, 0)));

        let mut resources = Dictionary::new();
        resources.set("Properties", Object::Dictionary(props));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));

        doc.trailer.set("Root", Object::Reference((1, 0)));
        doc
    }

    #[test]
    fn layers_collects_ocgs() {
        let doc = make_ocg_doc();
        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].name, "Layer1");
        assert_eq!(layers[1].name, "Layer2");
    }

    #[test]
    fn layers_default_state() {
        let doc = make_ocg_doc();
        let layers = collect_layers(&doc);
        assert_eq!(layers[0].default_state, "ON");
        assert_eq!(layers[1].default_state, "OFF");
    }

    #[test]
    fn layers_page_references() {
        let doc = make_ocg_doc();
        let layers = collect_layers(&doc);
        // Layer1 is referenced on page 1
        assert_eq!(layers[0].page_numbers, vec![1]);
        // Layer2 is not referenced on any page
        assert!(layers[1].page_numbers.is_empty());
    }

    #[test]
    fn layers_no_ocproperties() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert!(layers.is_empty());
    }

    #[test]
    fn layers_empty_ocgs() {
        let mut doc = Document::new();
        let mut d_config = Dictionary::new();
        d_config.set("BaseState", Object::Name(b"ON".to_vec()));
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![]));
        oc_props.set("D", Object::Dictionary(d_config));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert!(layers.is_empty());
    }

    #[test]
    fn layers_base_state_off_with_on_override() {
        let mut doc = Document::new();

        let mut ocg1 = Dictionary::new();
        ocg1.set("Type", Object::Name(b"OCG".to_vec()));
        ocg1.set("Name", Object::String(b"Hidden".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg1));

        let mut ocg2 = Dictionary::new();
        ocg2.set("Type", Object::Name(b"OCG".to_vec()));
        ocg2.set("Name", Object::String(b"Visible".to_vec(), StringFormat::Literal));
        doc.objects.insert((11, 0), Object::Dictionary(ocg2));

        let mut d_config = Dictionary::new();
        d_config.set("BaseState", Object::Name(b"OFF".to_vec()));
        d_config.set("ON", Object::Array(vec![Object::Reference((11, 0))]));

        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        oc_props.set("D", Object::Dictionary(d_config));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers[0].default_state, "OFF");
        assert_eq!(layers[1].default_state, "ON");
    }

    #[test]
    fn layers_unnamed_ocg() {
        let mut doc = Document::new();
        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        // No Name key
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers[0].name, "(unnamed)");
    }

    #[test]
    fn layers_text_output() {
        let doc = make_ocg_doc();
        let out = output_of(|w| print_layers(w, &doc));
        assert!(out.contains("2 layers found"));
        assert!(out.contains("Layer1"));
        assert!(out.contains("Layer2"));
        assert!(out.contains("ON"));
        assert!(out.contains("OFF"));
    }

    #[test]
    fn layers_json_output() {
        let doc = make_ocg_doc();
        let out = output_of(|w| print_layers_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["layer_count"], 2);
        assert_eq!(parsed["layers"][0]["name"], "Layer1");
        assert_eq!(parsed["layers"][0]["default_state"], "ON");
        assert_eq!(parsed["layers"][1]["name"], "Layer2");
        assert_eq!(parsed["layers"][1]["default_state"], "OFF");
    }

    #[test]
    fn layers_ocproperties_via_reference() {
        let mut doc = Document::new();
        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"RefLayer".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));
        doc.objects.insert((20, 0), Object::Dictionary(oc_props));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Reference((20, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "RefLayer");
    }

}
