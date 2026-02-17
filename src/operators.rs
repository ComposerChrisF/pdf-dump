use lopdf::{content::Content, Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::types::PageSpec;
use crate::stream::decode_stream;
use crate::helpers::{self, format_operation, format_dict_value};

pub(crate) struct OpsResult {
    pub operations: Vec<lopdf::content::Operation>,
    pub warnings: Vec<String>,
}

pub(crate) fn get_page_operations_with_warnings(doc: &Document, page_id: ObjectId) -> OpsResult {
    let dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return OpsResult { operations: vec![], warnings: vec![] },
    };

    let content_ids: Vec<ObjectId> = match dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return OpsResult { operations: vec![], warnings: vec![] },
    };

    let mut all_bytes = Vec::new();
    let mut warnings = Vec::new();
    for cid in &content_ids {
        match doc.get_object(*cid) {
            Ok(Object::Stream(stream)) => {
                let (decoded, warning) = decode_stream(stream);
                if let Some(warn) = warning {
                    warnings.push(format!("Content stream {} {}: {}", cid.0, cid.1, warn));
                }
                all_bytes.extend_from_slice(&decoded);
            }
            _ => {
                warnings.push(format!("Content stream {} {} could not be read", cid.0, cid.1));
            }
        }
    }

    match Content::decode(&all_bytes) {
        Ok(content) => OpsResult { operations: content.operations, warnings },
        Err(_) => {
            warnings.push("Content stream has syntax errors".to_string());
            OpsResult { operations: vec![], warnings }
        }
    }
}

pub(crate) fn print_operators(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => { eprintln!("Error: {}", msg); return; }
    };

    for (pn, page_id) in &page_list {
        let result = get_page_operations_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        wln!(writer, "--- Page {} ({} operations) ---", pn, result.operations.len());
        for op in &result.operations {
            wln!(writer, "{}", format_operation(op));
        }
        wln!(writer);
    }
}

pub(crate) fn operators_json_value(doc: &Document, page_filter: Option<&PageSpec>) -> Value {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => { return json!({"error": msg}); }
    };

    let mut page_results = Vec::new();
    for (pn, page_id) in &page_list {
        let result = get_page_operations_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        let json_ops: Vec<Value> = result.operations.iter().map(|op| {
            let operands: Vec<Value> = op.operands.iter().map(|o| json!(format_dict_value(o))).collect();
            json!({
                "operator": op.operator,
                "operands": operands,
            })
        }).collect();
        let mut entry = serde_json::Map::new();
        entry.insert("page_number".to_string(), json!(pn));
        entry.insert("operation_count".to_string(), json!(result.operations.len()));
        entry.insert("operations".to_string(), json!(json_ops));
        if !result.warnings.is_empty() {
            entry.insert("warnings".to_string(), json!(result.warnings));
        }
        page_results.push(Value::Object(entry));
    }

    json!({"pages": page_results})
}

#[cfg(test)]
pub(crate) fn print_operators_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    use crate::helpers::json_pretty;
    let output = operators_json_value(doc, page_filter);
    writeln!(writer, "{}", json_pretty(&output)).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::{Dictionary};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;
    use lopdf::Document;

    #[test]
    fn operators_shows_operations() {
        let doc = build_page_doc_with_content(b"BT /F1 12 Tf (Hello) Tj ET");
        let out = output_of(|w| print_operators(w, &doc, None));
        assert!(out.contains("Page 1"));
        assert!(out.contains("operations"));
        assert!(out.contains("BT"));
        assert!(out.contains("/F1 12 Tf"));
        assert!(out.contains("(Hello) Tj"));
        assert!(out.contains("ET"));
    }

    #[test]
    fn operators_json_structure() {
        let doc = build_page_doc_with_content(b"BT (Test) Tj ET");
        let out = output_of(|w| print_operators_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["pages"].is_array());
        let page = &parsed["pages"][0];
        assert_eq!(page["page_number"], 1);
        assert!(page["operation_count"].as_u64().unwrap() > 0);
        assert!(page["operations"].is_array());
    }

    #[test]
    fn operators_with_page_filter() {
        let doc = build_page_doc_with_content(b"BT (Hello) Tj ET");
        let spec = PageSpec::Single(1);
        let out = output_of(|w| print_operators(w, &doc, Some(&spec)));
        assert!(out.contains("Page 1"));
    }

    #[test]
    fn operators_json_has_operator_and_operands() {
        let doc = build_page_doc_with_content(b"BT /F1 12 Tf ET");
        let out = output_of(|w| print_operators_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let ops = parsed["pages"][0]["operations"].as_array().unwrap();
        // Find the Tf operation
        let tf_op = ops.iter().find(|o| o["operator"] == "Tf").unwrap();
        assert!(tf_op["operands"].is_array());
        assert_eq!(tf_op["operands"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn operators_empty_page() {
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
        let out = output_of(|w| print_operators(w, &doc, None));
        assert!(out.contains("0 operations"));
    }

}
