use lopdf::{Document, ObjectId, content::Content};
use serde_json::{Value, json};
use std::io::Write;

use crate::helpers::{self, format_dict_value, format_operation};
use crate::types::PageSpec;

pub(crate) struct OpsResult {
    pub operations: Vec<lopdf::content::Operation>,
    pub warnings: Vec<String>,
}

pub(crate) fn get_page_operations_with_warnings(doc: &Document, page_id: ObjectId) -> OpsResult {
    let stream_data = match helpers::read_content_streams(doc, page_id) {
        Some(data) => data,
        None => {
            return OpsResult {
                operations: vec![],
                warnings: vec![],
            };
        }
    };

    let mut warnings = stream_data.warnings;

    match Content::decode(&stream_data.bytes) {
        Ok(content) => OpsResult {
            operations: content.operations,
            warnings,
        },
        Err(_) => {
            warnings.push("Content stream has syntax errors".to_string());
            OpsResult {
                operations: vec![],
                warnings,
            }
        }
    }
}

pub(crate) fn print_operators(
    writer: &mut impl Write,
    doc: &Document,
    page_filter: Option<&PageSpec>,
) {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return;
        }
    };

    for (pn, page_id) in &page_list {
        let result = get_page_operations_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        wln!(
            writer,
            "--- Page {} ({} operations) ---",
            pn,
            result.operations.len()
        );
        for op in &result.operations {
            wln!(writer, "{}", format_operation(op));
        }
        wln!(writer);
    }
}

pub(crate) fn operators_json_value(doc: &Document, page_filter: Option<&PageSpec>) -> Value {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => {
            return json!({"error": msg});
        }
    };

    let mut page_results = Vec::new();
    for (pn, page_id) in &page_list {
        let result = get_page_operations_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        let json_ops: Vec<Value> = result
            .operations
            .iter()
            .map(|op| {
                let operands: Vec<Value> = op
                    .operands
                    .iter()
                    .map(|o| json!(format_dict_value(o)))
                    .collect();
                json!({
                    "operator": op.operator,
                    "operands": operands,
                })
            })
            .collect();
        let mut entry = serde_json::Map::new();
        entry.insert("page_number".to_string(), json!(pn));
        entry.insert(
            "operation_count".to_string(),
            json!(result.operations.len()),
        );
        entry.insert("operations".to_string(), json!(json_ops));
        if !result.warnings.is_empty() {
            entry.insert("warnings".to_string(), json!(result.warnings));
        }
        page_results.push(Value::Object(entry));
    }

    json!({"pages": page_results})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::Dictionary;
    use lopdf::Document;
    use lopdf::Object;
    use pretty_assertions::assert_eq;
    use serde_json::Value;

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
        let out = output_of(|w| render_json(w, &operators_json_value(&doc, None)));
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
        let out = output_of(|w| render_json(w, &operators_json_value(&doc, None)));
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

    #[test]
    fn operators_get_page_operations_direct() {
        // Arrange
        let doc = build_page_doc_with_content(b"BT /F1 12 Tf (Hello) Tj ET");
        let pages = doc.get_pages();
        let page_id = *pages.get(&1).unwrap();

        // Act
        let result = get_page_operations_with_warnings(&doc, page_id);

        // Assert
        assert!(result.warnings.is_empty());
        assert!(result.operations.len() >= 3); // BT, Tf, Tj, ET
        let op_names: Vec<&str> = result
            .operations
            .iter()
            .map(|o| o.operator.as_str())
            .collect();
        assert!(op_names.contains(&"BT"));
        assert!(op_names.contains(&"Tj"));
        assert!(op_names.contains(&"ET"));
    }

    #[test]
    fn operators_get_page_operations_no_content() {
        // Arrange: page without any content stream
        let mut doc = Document::new();
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page_dict));

        // Act
        let result = get_page_operations_with_warnings(&doc, (1, 0));

        // Assert
        assert!(result.operations.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn operators_syntax_error_in_stream() {
        // Arrange: content stream with malformed operator data
        let doc = build_page_doc_with_content(b"<<<INVALID>>>");
        let pages = doc.get_pages();
        let page_id = *pages.get(&1).unwrap();

        // Act
        let result = get_page_operations_with_warnings(&doc, page_id);

        // Assert: either it parses partially or reports a warning
        // The behavior depends on lopdf's Content::decode tolerance
        // At minimum, it should not panic
        let _ = result;
    }

    #[test]
    fn operators_multiple_pages() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_operators(w, &doc, None));
        assert!(out.contains("Page 1"));
        assert!(out.contains("Page 2"));
    }

    #[test]
    fn operators_page_filter_excludes() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Single(2);
        let out = output_of(|w| print_operators(w, &doc, Some(&spec)));
        assert!(!out.contains("Page 1"));
        assert!(out.contains("Page 2"));
    }

    #[test]
    fn operators_json_no_warnings_key_when_clean() {
        let doc = build_page_doc_with_content(b"BT (Test) Tj ET");
        let val = operators_json_value(&doc, None);
        let page = &val["pages"][0];
        // No warnings key when there are none
        assert!(page.get("warnings").is_none());
    }

    #[test]
    fn operators_json_page_filter() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let val = operators_json_value(&doc, Some(&spec));
        let pages = val["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["page_number"], 1);
    }
}
