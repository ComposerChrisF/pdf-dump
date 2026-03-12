use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::helpers::{resolve_dict, resolve_array, name_to_string, obj_to_string_lossy, get_catalog};

pub(crate) struct OcgInfo {
    pub object_id: ObjectId,
    pub name: String,
    pub default_state: String,
    pub page_numbers: Vec<u32>,
}

pub(crate) fn collect_layers(doc: &Document) -> Vec<OcgInfo> {
    let catalog = match get_catalog(doc) {
        Some(c) => c,
        None => return Vec::new(),
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

    let base_state = d_dict.get(b"BaseState").ok()
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
            .and_then(obj_to_string_lossy)
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
    use crate::helpers::json_pretty;
    let output = layers_json_value(doc);
    writeln!(writer, "{}", json_pretty(&output)).unwrap();
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

    #[test]
    fn layers_no_d_config_defaults_base_state_on() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"NoConfig".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        // OCProperties with OCGs but no /D config dict
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        // Intentionally no "D" key

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "NoConfig");
        assert_eq!(layers[0].default_state, "ON");
    }

    #[test]
    fn layers_ocg_on_multiple_pages() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"MultiPage".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));

        // Page 1 references the OCG
        let mut props1 = Dictionary::new();
        props1.set("MC0", Object::Reference((10, 0)));
        let mut resources1 = Dictionary::new();
        resources1.set("Properties", Object::Dictionary(props1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(resources1));
        page1.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page1));

        // Page 2 also references the same OCG
        let mut props2 = Dictionary::new();
        props2.set("MC1", Object::Reference((10, 0)));
        let mut resources2 = Dictionary::new();
        resources2.set("Properties", Object::Dictionary(props2));
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Resources", Object::Dictionary(resources2));
        page2.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(page2));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![
            Object::Reference((3, 0)),
            Object::Reference((4, 0)),
        ]));
        pages.set("Count", Object::Integer(2));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].page_numbers, vec![1, 2]);
    }

    #[test]
    fn layers_non_reference_items_in_ocgs_array_skipped() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"Real".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        // Mix of reference and non-reference items
        oc_props.set("OCGs", Object::Array(vec![
            Object::Integer(42),
            Object::Reference((10, 0)),
            Object::Name(b"Bogus".to_vec()),
        ]));
        oc_props.set("D", Object::Dictionary(d_config));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        // Only the valid reference should produce a layer
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "Real");
    }

    #[test]
    fn layers_ocg_object_missing_from_doc_skipped() {
        let mut doc = Document::new();

        // OCG (10,0) exists
        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"Exists".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        // OCG (99,0) does NOT exist in doc.objects

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((99, 0)), // dangling reference
        ]));
        oc_props.set("D", Object::Dictionary(d_config));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        // Only the existing OCG should appear
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "Exists");
    }

    #[test]
    fn layers_resources_inherited_from_parent() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"Inherited".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));

        // Page has NO Resources — they come from the parent Pages node
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        // Parent Pages node carries Resources with Properties referencing the OCG
        let mut props = Dictionary::new();
        props.set("MC0", Object::Reference((10, 0)));
        let mut resources = Dictionary::new();
        resources.set("Properties", Object::Dictionary(props));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages.set("Count", Object::Integer(1));
        pages.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "Inherited");
        assert_eq!(layers[0].page_numbers, vec![1]);
    }

    #[test]
    fn layers_properties_entry_non_ocg_not_counted() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"RealOCG".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        // A dictionary that is NOT an OCG (different /Type)
        let mut non_ocg = Dictionary::new();
        non_ocg.set("Type", Object::Name(b"OCMD".to_vec()));
        doc.objects.insert((11, 0), Object::Dictionary(non_ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));

        // Page Properties references both the OCG and the non-OCG
        let mut props = Dictionary::new();
        props.set("MC0", Object::Reference((10, 0)));
        props.set("MC1", Object::Reference((11, 0)));
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

        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 1);
        // Only the real OCG should have page references; non-OCG should not create entries
        assert_eq!(layers[0].page_numbers, vec![1]);
    }

    #[test]
    fn layers_ocgs_array_via_reference() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"IndirectArr".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        // Store the OCGs array as a separate object
        doc.objects.insert((20, 0), Object::Array(vec![Object::Reference((10, 0))]));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        // OCGs key is a reference to the array object
        oc_props.set("OCGs", Object::Reference((20, 0)));
        oc_props.set("D", Object::Dictionary(d_config));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "IndirectArr");
    }

    #[test]
    fn layers_print_no_layers_shows_zero_count_no_header() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_layers(w, &doc));
        assert!(out.contains("0 layers found"));
        // Should NOT contain the table header since there are no layers
        assert!(!out.contains("Obj#"));
        assert!(!out.contains("Default"));
    }

    #[test]
    fn layers_json_includes_generation_number() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"GenTest".to_vec(), StringFormat::Literal));
        // Use generation 0 (standard)
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

        let json_val = layers_json_value(&doc);
        assert_eq!(json_val["layers"][0]["generation"], 0);
        assert_eq!(json_val["layers"][0]["object_number"], 10);
        // Verify the field actually exists (not null)
        assert!(json_val["layers"][0].get("generation").is_some());
    }

    #[test]
    fn layers_print_shows_multiple_page_numbers() {
        let mut doc = Document::new();

        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"TwoPages".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));

        // Page 1 references the OCG
        let mut props1 = Dictionary::new();
        props1.set("MC0", Object::Reference((10, 0)));
        let mut resources1 = Dictionary::new();
        resources1.set("Properties", Object::Dictionary(props1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(resources1));
        page1.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page1));

        // Page 2 does NOT reference the OCG (no Properties)
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(page2));

        // Page 3 references the OCG
        let mut props3 = Dictionary::new();
        props3.set("MC0", Object::Reference((10, 0)));
        let mut resources3 = Dictionary::new();
        resources3.set("Properties", Object::Dictionary(props3));
        let mut page3 = Dictionary::new();
        page3.set("Type", Object::Name(b"Page".to_vec()));
        page3.set("Resources", Object::Dictionary(resources3));
        page3.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(page3));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![
            Object::Reference((3, 0)),
            Object::Reference((4, 0)),
            Object::Reference((5, 0)),
        ]));
        pages.set("Count", Object::Integer(3));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_layers(w, &doc));
        assert!(out.contains("1, 3"), "Expected pages '1, 3' in output, got:\n{}", out);
    }

}
