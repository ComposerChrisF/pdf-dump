use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;


pub(crate) struct FormFieldInfo {
    pub object_id: ObjectId,
    pub qualified_name: String,
    pub field_type: String,
    pub value: String,
    pub page_number: Option<u32>,
    pub flags: u32,
}

pub(crate) fn collect_form_fields(doc: &Document) -> (Option<ObjectId>, bool, Vec<FormFieldInfo>) {
    // Find AcroForm in catalog
    let catalog_id = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return (None, false, vec![]),
    };
    let catalog = match doc.get_object(catalog_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return (None, false, vec![]),
    };
    let acroform_ref = match catalog.get(b"AcroForm") {
        Ok(Object::Reference(id)) => *id,
        Ok(Object::Dictionary(_)) => {
            // Inline AcroForm - use catalog_id as placeholder
            // We need to work with the dict directly
            return collect_form_fields_from_dict(doc, catalog, catalog_id);
        }
        _ => return (None, false, vec![]),
    };
    let acroform_dict = match doc.get_object(acroform_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return (Some(acroform_ref), false, vec![]),
    };
    collect_form_fields_from_dict(doc, acroform_dict, acroform_ref)
}

pub(crate) fn collect_form_fields_from_dict(doc: &Document, acroform_dict: &lopdf::Dictionary, acroform_id: ObjectId) -> (Option<ObjectId>, bool, Vec<FormFieldInfo>) {
    let need_appearances = acroform_dict.get(b"NeedAppearances")
        .ok()
        .and_then(|v| match v { Object::Boolean(b) => Some(*b), _ => None })
        .unwrap_or(false);

    let fields_array = match acroform_dict.get(b"Fields") {
        Ok(Object::Array(arr)) => arr,
        Ok(Object::Reference(id)) => {
            match doc.get_object(*id) {
                Ok(Object::Array(arr)) => arr,
                _ => return (Some(acroform_id), need_appearances, vec![]),
            }
        }
        _ => return (Some(acroform_id), need_appearances, vec![]),
    };

    // Build page annotation map: widget object_id -> page number
    let pages = doc.get_pages();
    let mut widget_to_page: BTreeMap<ObjectId, u32> = BTreeMap::new();
    for (&page_num, &page_id) in &pages {
        if let Ok(Object::Dictionary(page_dict)) = doc.get_object(page_id)
            && let Ok(Object::Array(annots)) = page_dict.get(b"Annots") {
            for annot in annots {
                if let Ok(id) = annot.as_reference() {
                    widget_to_page.insert(id, page_num);
                }
            }
        }
    }

    let mut fields = Vec::new();
    for field_obj in fields_array {
        if let Ok(field_id) = field_obj.as_reference() {
            collect_field_recursive(doc, field_id, "", &widget_to_page, &mut fields);
        }
    }

    (Some(acroform_id), need_appearances, fields)
}

pub(crate) fn collect_field_recursive(
    doc: &Document,
    field_id: ObjectId,
    parent_name: &str,
    widget_to_page: &BTreeMap<ObjectId, u32>,
    fields: &mut Vec<FormFieldInfo>,
) {
    let dict = match doc.get_object(field_id) {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Stream(s)) => &s.dict,
        _ => return,
    };

    // Get field name (T)
    let partial_name = dict.get(b"T").ok()
        .and_then(|v| match v {
            Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        })
        .unwrap_or_default();

    let qualified_name = if parent_name.is_empty() {
        partial_name.clone()
    } else if partial_name.is_empty() {
        parent_name.to_string()
    } else {
        format!("{}.{}", parent_name, partial_name)
    };

    // Check for Kids
    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        // Check if kids are fields (have T) or widgets (no T)
        let mut has_field_kids = false;
        for kid in kids {
            if let Ok(kid_id) = kid.as_reference()
                && let Ok(kid_obj) = doc.get_object(kid_id) {
                let kid_dict = match kid_obj {
                    Object::Dictionary(d) => d,
                    Object::Stream(s) => &s.dict,
                    _ => continue,
                };
                if kid_dict.get(b"T").is_ok() {
                    has_field_kids = true;
                    break;
                }
            }
        }
        if has_field_kids {
            for kid in kids {
                if let Ok(kid_id) = kid.as_reference() {
                    collect_field_recursive(doc, kid_id, &qualified_name, widget_to_page, fields);
                }
            }
            return;
        }
        // Kids are widgets — fall through to collect this field
    }

    // Determine field type (FT) — may be inherited
    let field_type = dict.get(b"FT").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
        .unwrap_or_else(|| "-".to_string());

    // Get value (V)
    let value = dict.get(b"V").ok()
        .map(|v| match v {
            Object::String(bytes, _) => format!("\"{}\"", String::from_utf8_lossy(bytes)),
            Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
            Object::Integer(i) => i.to_string(),
            Object::Boolean(b) => b.to_string(),
            Object::Array(_) => "[array]".to_string(),
            _ => "(empty)".to_string(),
        })
        .unwrap_or_else(|| "(empty)".to_string());

    // Get flags (Ff)
    let flags = dict.get(b"Ff").ok()
        .and_then(|v| v.as_i64().ok())
        .unwrap_or(0) as u32;

    // Determine page number
    let page_number = widget_to_page.get(&field_id).copied();

    fields.push(FormFieldInfo {
        object_id: field_id,
        qualified_name,
        field_type,
        value,
        page_number,
        flags,
    });
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

pub(crate) fn print_forms(writer: &mut impl Write, doc: &Document) {
    let (acroform_id, need_appearances, fields) = collect_form_fields(doc);

    match acroform_id {
        None => {
            wln!(writer, "No AcroForm found in document.");
            return;
        }
        Some(id) => {
            wln!(writer, "AcroForm found (obj {}), NeedAppearances: {}", id.0, need_appearances);
        }
    }

    wln!(writer, "{} form fields\n", fields.len());
    if fields.is_empty() { return; }

    wln!(writer, "  {:>4}  {:<24} {:<6}  {:<20} Page", "Obj#", "FieldName", "Type", "Value");
    for f in &fields {
        let page_str = f.page_number.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
        wln!(writer, "  {:>4}  {:<24} {:<6}  {:<20} {}",
            f.object_id.0,
            truncate_str(&f.qualified_name, 24),
            &f.field_type,
            truncate_str(&f.value, 20),
            page_str,
        );
    }
}

pub(crate) fn forms_json_value(doc: &Document) -> Value {
    let (acroform_id, need_appearances, fields) = collect_form_fields(doc);

    let items: Vec<Value> = fields.iter().map(|f| {
        json!({
            "object_number": f.object_id.0,
            "generation": f.object_id.1,
            "field_name": f.qualified_name,
            "field_type": f.field_type,
            "value": f.value,
            "flags": f.flags,
            "page_number": f.page_number,
        })
    }).collect();

    json!({
        "acroform_object": acroform_id.map(|id| id.0),
        "need_appearances": need_appearances,
        "field_count": items.len(),
        "fields": items,
    })
}

#[cfg(test)]
pub(crate) fn print_forms_json(writer: &mut impl Write, doc: &Document) {
    let output = forms_json_value(doc);
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

    fn build_form_doc() -> Document {
        let mut doc = Document::new();
        // Form field 1 - text field
        let mut field1 = Dictionary::new();
        field1.set("T", Object::String(b"FirstName".to_vec(), StringFormat::Literal));
        field1.set("FT", Object::Name(b"Tx".to_vec()));
        field1.set("V", Object::String(b"John".to_vec(), StringFormat::Literal));
        doc.objects.insert((20, 0), Object::Dictionary(field1));
        // Form field 2 - button
        let mut field2 = Dictionary::new();
        field2.set("T", Object::String(b"Subscribe".to_vec(), StringFormat::Literal));
        field2.set("FT", Object::Name(b"Btn".to_vec()));
        field2.set("V", Object::Name(b"Yes".to_vec()));
        doc.objects.insert((22, 0), Object::Dictionary(field2));
        // AcroForm
        let mut acroform = Dictionary::new();
        acroform.set("NeedAppearances", Object::Boolean(true));
        acroform.set("Fields", Object::Array(vec![
            Object::Reference((20, 0)),
            Object::Reference((22, 0)),
        ]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        // Need Pages for page mapping
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((30, 0))]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        // Page with annotations pointing to fields
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((5, 0)));
        page.set("Annots", Object::Array(vec![
            Object::Reference((20, 0)),
            Object::Reference((22, 0)),
        ]));
        doc.objects.insert((30, 0), Object::Dictionary(page));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        doc
    }

    #[test]
    fn forms_shows_fields() {
        let doc = build_form_doc();
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("AcroForm found"));
        assert!(out.contains("NeedAppearances: true"));
        assert!(out.contains("2 form fields"));
        assert!(out.contains("FirstName"));
        assert!(out.contains("Tx"));
        assert!(out.contains("\"John\""));
        assert!(out.contains("Subscribe"));
        assert!(out.contains("Btn"));
        assert!(out.contains("/Yes"));
    }

    #[test]
    fn forms_json_structure() {
        let doc = build_form_doc();
        let out = output_of(|w| print_forms_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["acroform_object"].is_number());
        assert_eq!(parsed["need_appearances"], true);
        assert_eq!(parsed["field_count"], 2);
        assert!(parsed["fields"].is_array());
        let fields = parsed["fields"].as_array().unwrap();
        assert_eq!(fields[0]["field_name"], "FirstName");
        assert_eq!(fields[0]["field_type"], "Tx");
    }

    #[test]
    fn forms_page_mapping() {
        let doc = build_form_doc();
        let out = output_of(|w| print_forms_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let fields = parsed["fields"].as_array().unwrap();
        // Fields should be mapped to page 1 via Annots
        assert_eq!(fields[0]["page_number"], 1);
    }

    #[test]
    fn forms_no_acroform() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("No AcroForm found"));
    }

    #[test]
    fn forms_empty_fields() {
        let mut doc = Document::new();
        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("0 form fields"));
    }

    #[test]
    fn forms_hierarchical_fields() {
        let mut doc = Document::new();
        // Child field
        let mut child = Dictionary::new();
        child.set("T", Object::String(b"FirstName".to_vec(), StringFormat::Literal));
        child.set("FT", Object::Name(b"Tx".to_vec()));
        child.set("V", Object::String(b"Alice".to_vec(), StringFormat::Literal));
        doc.objects.insert((21, 0), Object::Dictionary(child));
        // Parent field with Kids
        let mut parent = Dictionary::new();
        parent.set("T", Object::String(b"Person".to_vec(), StringFormat::Literal));
        parent.set("Kids", Object::Array(vec![Object::Reference((21, 0))]));
        doc.objects.insert((20, 0), Object::Dictionary(parent));
        // AcroForm
        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        // Should show qualified name Person.FirstName
        assert!(out.contains("Person.FirstName"));
    }

    #[test]
    fn forms_json_no_acroform() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_forms_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["acroform_object"].is_null());
        assert_eq!(parsed["field_count"], 0);
    }

}
