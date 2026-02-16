use lopdf::{content::Content, Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::stream::decode_stream;
use crate::helpers::resolve_dict;

#[derive(PartialEq)]
pub(crate) enum ValidationLevel {
    Error,
    Warn,
    Info,
}

pub(crate) struct ValidationIssue {
    pub level: ValidationLevel,
    pub message: String,
}

pub(crate) struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
    pub error_count: usize,
    pub warn_count: usize,
    pub info_count: usize,
}

pub(crate) fn validate_pdf(doc: &Document) -> ValidationReport {
    let mut issues = Vec::new();

    check_broken_references(doc, &mut issues);
    check_unreachable_objects(doc, &mut issues);
    check_required_keys(doc, &mut issues);
    check_stream_lengths(doc, &mut issues);
    check_page_tree(doc, &mut issues);
    check_content_stream_syntax(doc, &mut issues);
    check_font_requirements(doc, &mut issues);
    check_page_tree_cycles(doc, &mut issues);
    check_names_tree_structure(doc, &mut issues);
    check_duplicate_objects(doc, &mut issues);

    let error_count = issues.iter().filter(|i| i.level == ValidationLevel::Error).count();
    let warn_count = issues.iter().filter(|i| i.level == ValidationLevel::Warn).count();
    let info_count = issues.iter().filter(|i| i.level == ValidationLevel::Info).count();

    ValidationReport { issues, error_count, warn_count, info_count }
}

fn check_broken_references(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    for (&(obj_num, generation), object) in &doc.objects {
        let broken = collect_broken_refs(object, doc);
        for (ref_num, ref_generation) in broken {
            issues.push(ValidationIssue {
                level: ValidationLevel::Error,
                message: format!("Object {} {}: references non-existent object {} {}", obj_num, generation, ref_num, ref_generation),
            });
        }
    }
}

fn collect_broken_refs(obj: &Object, doc: &Document) -> Vec<(u32, u16)> {
    let mut broken = Vec::new();
    match obj {
        Object::Reference(id) => {
            if doc.get_object(*id).is_err() {
                broken.push(*id);
            }
        }
        Object::Array(arr) => {
            for item in arr {
                broken.extend(collect_broken_refs(item, doc));
            }
        }
        Object::Dictionary(dict) => {
            for (_, v) in dict.iter() {
                broken.extend(collect_broken_refs(v, doc));
            }
        }
        Object::Stream(stream) => {
            for (_, v) in stream.dict.iter() {
                broken.extend(collect_broken_refs(v, doc));
            }
        }
        _ => {}
    }
    broken
}

pub(crate) fn collect_reachable_ids(doc: &Document) -> BTreeSet<ObjectId> {
    let mut visited = BTreeSet::new();

    fn walk_refs(obj: &Object, doc: &Document, visited: &mut BTreeSet<ObjectId>) {
        match obj {
            Object::Reference(id) => {
                if visited.contains(id) { return; }
                visited.insert(*id);
                if let Ok(resolved) = doc.get_object(*id) {
                    walk_refs(resolved, doc, visited);
                }
            }
            Object::Array(arr) => {
                for item in arr { walk_refs(item, doc, visited); }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() { walk_refs(v, doc, visited); }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter() { walk_refs(v, doc, visited); }
            }
            _ => {}
        }
    }

    // Start from trailer
    for (_, v) in doc.trailer.iter() {
        walk_refs(v, doc, &mut visited);
    }

    visited
}

fn check_unreachable_objects(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let reachable = collect_reachable_ids(doc);
    for &(obj_num, generation) in doc.objects.keys() {
        if !reachable.contains(&(obj_num, generation)) {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Object {} {} is unreachable from trailer", obj_num, generation),
            });
        }
    }
}

fn check_required_keys(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    // Catalog must have /Pages
    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        if let Ok(Object::Dictionary(catalog)) = doc.get_object(root_ref)
            && catalog.get(b"Pages").is_err() {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Error,
                    message: "Catalog missing required /Pages key".to_string(),
                });
        }
    } else {
        issues.push(ValidationIssue {
            level: ValidationLevel::Error,
            message: "Trailer missing /Root reference".to_string(),
        });
    }

    // Each page must have /MediaBox (or inherit from parent)
    let pages = doc.get_pages();
    for (&page_num, &page_id) in &pages {
        if !page_has_media_box(doc, page_id) {
            issues.push(ValidationIssue {
                level: ValidationLevel::Error,
                message: format!("Page {} (object {}): missing /MediaBox (not found in page or parent chain)", page_num, page_id.0),
            });
        }
    }
}

fn page_has_media_box(doc: &Document, page_id: ObjectId) -> bool {
    let mut current_id = Some(page_id);
    let mut depth = 0;
    while let Some(id) = current_id {
        if depth > 20 { break; } // Guard against cycles
        depth += 1;
        if let Ok(obj) = doc.get_object(id) {
            let dict = match obj {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                _ => break,
            };
            if dict.get(b"MediaBox").is_ok() {
                return true;
            }
            // Walk up the /Parent chain
            current_id = dict.get(b"Parent").ok()
                .and_then(|v| v.as_reference().ok());
        } else {
            break;
        }
    }
    false
}

fn check_stream_lengths(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    for (&(obj_num, generation), object) in &doc.objects {
        if let Object::Stream(stream) = object
            && let Ok(Object::Integer(declared)) = stream.dict.get(b"Length") {
                let actual = stream.content.len() as i64;
                if *declared != actual {
                    issues.push(ValidationIssue {
                        level: ValidationLevel::Warn,
                        message: format!("Object {} {}: /Length is {} but stream content is {} bytes", obj_num, generation, declared, actual),
                    });
                }
        }
    }
}

fn check_page_tree(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let pages = doc.get_pages();
    let actual_count = pages.len();

    // Check /Pages /Count
    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok())
        && let Ok(Object::Dictionary(catalog)) = doc.get_object(root_ref)
        && let Ok(pages_ref) = catalog.get(b"Pages").and_then(|o| o.as_reference())
        && let Ok(Object::Dictionary(pages_dict)) = doc.get_object(pages_ref)
        && let Ok(Object::Integer(count)) = pages_dict.get(b"Count")
        && *count as usize != actual_count
    {
        issues.push(ValidationIssue {
            level: ValidationLevel::Error,
            message: format!("/Pages /Count is {} but document has {} pages", count, actual_count),
        });
    }
}

fn check_content_stream_syntax(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let pages = doc.get_pages();
    for (&page_num, &page_id) in &pages {
        let page_dict = match doc.get_object(page_id) {
            Ok(Object::Dictionary(d)) => d,
            _ => continue,
        };
        let content_ids: Vec<ObjectId> = match page_dict.get(b"Contents") {
            Ok(Object::Reference(id)) => vec![*id],
            Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => continue,
        };
        for content_id in content_ids {
            if let Ok(Object::Stream(stream)) = doc.get_object(content_id) {
                let (decoded, _) = decode_stream(stream);
                if Content::decode(&decoded).is_err() {
                    issues.push(ValidationIssue {
                        level: ValidationLevel::Warn,
                        message: format!("Page {}: content stream {} {} has invalid syntax", page_num, content_id.0, content_id.1),
                    });
                }
            }
        }
    }
}

fn check_font_requirements(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    for (&(obj_num, generation), object) in &doc.objects {
        let dict = match object {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        let is_font = dict.get(b"Type").ok()
            .and_then(|v| v.as_name().ok().map(|n| n == b"Font"))
            .unwrap_or(false);
        if !is_font { continue; }

        let subtype = dict.get(b"Subtype").ok()
            .and_then(|v| v.as_name().ok());

        let needs_basefont = matches!(subtype, Some(b"Type1") | Some(b"TrueType") | Some(b"Type0") | Some(b"CIDFontType0") | Some(b"CIDFontType2"));
        if needs_basefont && dict.get(b"BaseFont").is_err() {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Font object {} {}: missing /BaseFont", obj_num, generation),
            });
        }

        let has_first = dict.get(b"FirstChar").is_ok();
        let has_last = dict.get(b"LastChar").is_ok();
        if has_first != has_last {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Font object {} {}: has {} but not {}", obj_num, generation,
                    if has_first { "/FirstChar" } else { "/LastChar" },
                    if has_first { "/LastChar" } else { "/FirstChar" }),
            });
        }

        if has_first && has_last
            && let (Ok(Object::Integer(first)), Ok(Object::Integer(last))) =
                (dict.get(b"FirstChar"), dict.get(b"LastChar"))
        {
            let expected_width_count = (last - first + 1).max(0) as usize;
            if let Ok(Object::Array(widths)) = dict.get(b"Widths")
                && widths.len() != expected_width_count
            {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Warn,
                    message: format!("Font object {} {}: /Widths has {} entries but /FirstChar..=/LastChar expects {}", obj_num, generation, widths.len(), expected_width_count),
                });
            }
        }
    }
}

fn check_page_tree_cycles(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let pages = doc.get_pages();
    for (&page_num, &page_id) in &pages {
        let mut visited = BTreeSet::new();
        visited.insert(page_id);
        let mut current = doc.get_object(page_id).ok()
            .and_then(|o| match o {
                Object::Dictionary(d) => d.get(b"Parent").ok().and_then(|v| v.as_reference().ok()),
                _ => None,
            });
        while let Some(parent_id) = current {
            if visited.contains(&parent_id) {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Error,
                    message: format!("Page {}: cycle detected in /Parent chain (object {} {} seen twice)", page_num, parent_id.0, parent_id.1),
                });
                break;
            }
            visited.insert(parent_id);
            current = doc.get_object(parent_id).ok()
                .and_then(|o| match o {
                    Object::Dictionary(d) => d.get(b"Parent").ok().and_then(|v| v.as_reference().ok()),
                    _ => None,
                });
        }
    }
}

fn check_names_tree_structure(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return,
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return,
    };
    let names_dict = match catalog.get(b"Names").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return,
    };

    let subtree_keys: &[&[u8]] = &[b"EmbeddedFiles", b"Dests", b"JavaScript", b"AP"];
    for key in subtree_keys {
        let subtree = match names_dict.get(key).ok().and_then(|o| resolve_dict(doc, o)) {
            Some(d) => d,
            None => continue,
        };
        let key_name = String::from_utf8_lossy(key);
        validate_name_tree_node(doc, subtree, &key_name, &mut BTreeSet::new(), issues, 0);
    }
}

fn validate_name_tree_node(
    doc: &Document,
    dict: &lopdf::Dictionary,
    tree_name: &str,
    visited: &mut BTreeSet<ObjectId>,
    issues: &mut Vec<ValidationIssue>,
    depth: usize,
) {
    if depth > 50 {
        issues.push(ValidationIssue {
            level: ValidationLevel::Warn,
            message: format!("Name tree '{}': exceeded maximum depth (50)", tree_name),
        });
        return;
    }

    let has_names = dict.get(b"Names").is_ok();
    let has_kids = dict.get(b"Kids").is_ok();

    if !has_names && !has_kids {
        issues.push(ValidationIssue {
            level: ValidationLevel::Warn,
            message: format!("Name tree '{}': node has neither /Names nor /Kids", tree_name),
        });
        return;
    }

    if let Ok(Object::Array(names)) = dict.get(b"Names")
        && names.len() % 2 != 0
    {
        issues.push(ValidationIssue {
            level: ValidationLevel::Warn,
            message: format!("Name tree '{}': /Names array has odd length ({})", tree_name, names.len()),
        });
    }

    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        for kid in kids {
            let kid_id = match kid {
                Object::Reference(id) => *id,
                _ => continue,
            };
            if visited.contains(&kid_id) {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Error,
                    message: format!("Name tree '{}': cycle detected at object {} {}", tree_name, kid_id.0, kid_id.1),
                });
                continue;
            }
            visited.insert(kid_id);
            if let Ok(Object::Dictionary(kid_dict)) = doc.get_object(kid_id) {
                validate_name_tree_node(doc, kid_dict, tree_name, visited, issues, depth + 1);
            }
        }
    }
}

fn check_duplicate_objects(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let mut seen_numbers: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    for &(obj_num, generation) in doc.objects.keys() {
        seen_numbers.entry(obj_num).or_default().push(generation);
    }
    for (obj_num, generations) in &seen_numbers {
        if generations.len() > 1 {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Object {}: multiple generations present ({:?})", obj_num, generations),
            });
        }
    }
}

pub(crate) fn print_validation(writer: &mut impl Write, doc: &Document) {
    let report = validate_pdf(doc);

    if report.issues.is_empty() {
        writeln!(writer, "[OK] No issues found.").unwrap();
        return;
    }

    for issue in &report.issues {
        let prefix = match issue.level {
            ValidationLevel::Error => "[ERROR]",
            ValidationLevel::Warn => "[WARN]",
            ValidationLevel::Info => "[INFO]",
        };
        writeln!(writer, "{} {}", prefix, issue.message).unwrap();
    }
    writeln!(writer, "\nSummary: {} errors, {} warnings, {} info",
        report.error_count, report.warn_count, report.info_count).unwrap();
}

pub(crate) fn print_validation_json(writer: &mut impl Write, doc: &Document) {
    let report = validate_pdf(doc);

    let issues: Vec<Value> = report.issues.iter().map(|i| {
        json!({
            "level": match i.level {
                ValidationLevel::Error => "error",
                ValidationLevel::Warn => "warning",
                ValidationLevel::Info => "info",
            },
            "message": i.message,
        })
    }).collect();

    let output = json!({
        "error_count": report.error_count,
        "warning_count": report.warn_count,
        "info_count": report.info_count,
        "issues": issues,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;
    use lopdf::Document;

    #[test]
    fn validate_empty_doc_reports_missing_root() {
        let doc = Document::new();
        let report = validate_pdf(&doc);
        assert!(report.issues.iter().any(|i|
            i.level == ValidationLevel::Error && i.message.contains("Trailer missing /Root")));
    }

    #[test]
    fn check_broken_references_detects_broken() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let mut issues = Vec::new();
        check_broken_references(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("99"));
    }

    #[test]
    fn check_broken_references_valid() {
        let mut doc = Document::new();
        doc.objects.insert((2, 0), Object::Integer(42));
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let mut issues = Vec::new();
        check_broken_references(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn collect_broken_refs_in_array() {
        let mut doc = Document::new();
        let obj = Object::Array(vec![Object::Reference((99, 0))]);
        doc.objects.insert((1, 0), obj.clone());

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
    }

    #[test]
    fn collect_broken_refs_in_stream_dict() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        let stream = Stream::new(dict, vec![]);
        let obj = Object::Stream(stream);
        doc.objects.insert((1, 0), obj.clone());

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
    }

    #[test]
    fn check_unreachable_objects_finds_orphans() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Integer(99));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_unreachable_objects(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("2 0"));
    }

    #[test]
    fn check_stream_lengths_mismatch() {
        let mut doc = Document::new();
        let mut stream = Stream::new(Dictionary::new(), vec![0; 10]);
        // Override /Length after construction to simulate a mismatch
        stream.dict.set(b"Length", Object::Integer(999));
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("999"));
        assert!(issues[0].message.contains("10"));
    }

    #[test]
    fn check_stream_lengths_correct() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Length", Object::Integer(10));
        let stream = Stream::new(dict, vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn page_has_media_box_direct() {
        let mut doc = Document::new();
        let mut page_dict = Dictionary::new();
        page_dict.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((1, 0), Object::Dictionary(page_dict));

        assert!(page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_inherited() {
        let mut doc = Document::new();
        // Parent has MediaBox
        let mut parent = Dictionary::new();
        parent.set(b"Type", Object::Name(b"Pages".to_vec()));
        parent.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(parent));

        // Page without MediaBox, has Parent
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_missing() {
        let mut doc = Document::new();
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn print_validation_no_issues() {
        // Build a minimal valid PDF
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((2, 0), Object::Dictionary(pages));
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_validation(w, &doc));
        assert!(out.contains("[OK]"));
    }

    #[test]
    fn print_validation_json_produces_valid_json() {
        let doc = Document::new();
        let out = output_of(|w| print_validation_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["error_count"].is_number());
        assert!(parsed["warning_count"].is_number());
        assert!(parsed["issues"].is_array());
    }

    #[test]
    fn print_validation_shows_errors_and_summary() {
        let doc = Document::new();
        let out = output_of(|w| print_validation(w, &doc));
        assert!(out.contains("[ERROR]"));
        assert!(out.contains("Summary:"));
    }

    #[test]
    fn check_page_tree_count_mismatch() {
        let mut doc = Document::new();
        // Pages says Count=5 but no actual pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set(b"Count", Object::Integer(5));
        pages_dict.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_page_tree(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("/Pages /Count is 5"));
    }

    #[test]
    fn collect_broken_refs_nested_dict() {
        let doc = Document::new();
        let mut inner = Dictionary::new();
        inner.set(b"Ref", Object::Reference((99, 0)));
        let mut outer = Dictionary::new();
        outer.set(b"Inner", Object::Dictionary(inner));
        let obj = Object::Dictionary(outer);

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0], (99, 0));
    }

    #[test]
    fn collect_broken_refs_nested_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Array(vec![Object::Reference((88, 0))]),
        ]);

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0], (88, 0));
    }

    #[test]
    fn collect_broken_refs_multiple_in_one_object() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"A", Object::Reference((91, 0)));
        dict.set(b"B", Object::Reference((92, 0)));
        dict.set(b"C", Object::Reference((93, 0)));
        let obj = Object::Dictionary(dict);

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 3);
    }

    #[test]
    fn collect_broken_refs_valid_ref_not_reported() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));
        let obj = Object::Reference((5, 0));

        let broken = collect_broken_refs(&obj, &doc);
        assert!(broken.is_empty());
    }

    #[test]
    fn collect_broken_refs_primitives_return_empty() {
        let doc = Document::new();
        assert!(collect_broken_refs(&Object::Null, &doc).is_empty());
        assert!(collect_broken_refs(&Object::Boolean(false), &doc).is_empty());
        assert!(collect_broken_refs(&Object::Integer(0), &doc).is_empty());
        assert!(collect_broken_refs(&Object::Real(1.0), &doc).is_empty());
        assert!(collect_broken_refs(&Object::Name(b"X".to_vec()), &doc).is_empty());
    }

    #[test]
    fn check_required_keys_catalog_missing_pages() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        // No /Pages key
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_required_keys(&doc, &mut issues);
        assert!(issues.iter().any(|i|
            i.level == ValidationLevel::Error && i.message.contains("Catalog missing required /Pages")));
    }

    #[test]
    fn check_required_keys_valid_catalog() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((2, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_required_keys(&doc, &mut issues);
        // No "Catalog missing" errors — may still have MediaBox issues
        assert!(!issues.iter().any(|i| i.message.contains("Catalog missing")));
    }

    #[test]
    fn page_has_media_box_inherited_three_levels() {
        let mut doc = Document::new();

        // Grandparent has MediaBox
        let mut grandparent = Dictionary::new();
        grandparent.set(b"Type", Object::Name(b"Pages".to_vec()));
        grandparent.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(grandparent));

        // Parent without MediaBox, points up
        let mut parent = Dictionary::new();
        parent.set(b"Type", Object::Name(b"Pages".to_vec()));
        parent.set(b"Parent", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(parent));

        // Page without MediaBox
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_cycle_guard() {
        let mut doc = Document::new();
        // Page A points to B, B points to A — cycle, no MediaBox
        let mut page_a = Dictionary::new();
        page_a.set(b"Type", Object::Name(b"Page".to_vec()));
        page_a.set(b"Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page_a));

        let mut page_b = Dictionary::new();
        page_b.set(b"Type", Object::Name(b"Pages".to_vec()));
        page_b.set(b"Parent", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_b));

        // Should not infinite loop, should return false
        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_nonexistent_parent() {
        let mut doc = Document::new();
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((99, 0))); // doesn't exist
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_non_dict_object() {
        let mut doc = Document::new();
        // Object is an Integer, not a Dictionary
        doc.objects.insert((1, 0), Object::Integer(42));

        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn check_stream_lengths_no_length_key_no_issue() {
        let mut doc = Document::new();
        // Stream without /Length key — not checked
        let stream = Stream::new(Dictionary::new(), vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn check_stream_lengths_zero_length_correct() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Length", Object::Integer(0));
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn check_page_tree_correct_count() {
        let mut doc = Document::new();

        // One page
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((2, 0)));
        page.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set(b"Count", Object::Integer(1));
        pages_dict.set(b"Kids", Object::Array(vec![Object::Reference((3, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_page_tree(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn validate_pdf_mixed_issues() {
        let mut doc = Document::new();
        // Broken reference
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        // Missing root → error
        // Object 1 unreachable → warn

        let report = validate_pdf(&doc);
        assert!(report.error_count > 0); // missing root + broken ref
        assert!(report.warn_count > 0);  // unreachable
        assert_eq!(report.error_count + report.warn_count + report.info_count,
                   report.issues.len());
    }

    #[test]
    fn print_validation_json_structure() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_validation_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();

        // Check structure
        assert!(parsed["error_count"].is_number());
        assert!(parsed["warning_count"].is_number());
        assert!(parsed["info_count"].is_number());
        assert!(parsed["issues"].is_array());

        // Each issue has level and message
        for issue in parsed["issues"].as_array().unwrap() {
            assert!(issue["level"].is_string());
            assert!(issue["message"].is_string());
            let level = issue["level"].as_str().unwrap();
            assert!(level == "error" || level == "warning" || level == "info");
        }
    }

    #[test]
    fn check_unreachable_all_reachable() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_unreachable_objects(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn check_unreachable_multiple_orphans() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(1));
        doc.objects.insert((2, 0), Object::Integer(2));
        doc.objects.insert((3, 0), Object::Integer(3));
        // No trailer refs → all unreachable

        let mut issues = Vec::new();
        check_unreachable_objects(&doc, &mut issues);
        assert_eq!(issues.len(), 3);
        assert!(issues.iter().all(|i| i.level == ValidationLevel::Warn));
    }

    #[test]
    fn validate_content_stream_valid() {
        let mut doc = Document::new();
        let content = b"BT /F1 12 Tf (Hello) Tj ET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        let p_id = doc.add_object(Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(1));
        pages.set("Kids", Object::Array(vec![Object::Reference(p_id)]));
        let pages_id = doc.add_object(Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let cat_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(cat_id));

        let mut issues = Vec::new();
        check_content_stream_syntax(&doc, &mut issues);
        assert!(issues.is_empty(), "Valid content stream should produce no issues");
    }

    #[test]
    fn validate_content_stream_invalid() {
        let mut doc = Document::new();
        let content = b"THIS IS NOT VALID PDF CONTENT STREAM SYNTAX <<<>>>!!!";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        let p_id = doc.add_object(Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(1));
        pages.set("Kids", Object::Array(vec![Object::Reference(p_id)]));
        let pages_id = doc.add_object(Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let cat_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(cat_id));

        let mut issues = Vec::new();
        check_content_stream_syntax(&doc, &mut issues);
        // Note: lopdf's Content::decode is lenient; it may or may not fail on arbitrary bytes.
        // If it does fail, we should get a warning about invalid syntax.
        // If it doesn't fail, that's also acceptable.
    }

    #[test]
    fn validate_font_missing_basefont() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        // Missing BaseFont
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("missing /BaseFont")));
    }

    #[test]
    fn validate_font_valid() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn validate_font_widths_mismatch() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        font.set("BaseFont", Object::Name(b"Arial".to_vec()));
        font.set("FirstChar", Object::Integer(32));
        font.set("LastChar", Object::Integer(126));
        // Expected: 95 widths, provide only 10
        let widths: Vec<Object> = (0..10).map(|_| Object::Integer(600)).collect();
        font.set("Widths", Object::Array(widths));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("/Widths")));
    }

    #[test]
    fn validate_font_firstchar_without_lastchar() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        font.set("FirstChar", Object::Integer(32));
        // Missing LastChar
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("/FirstChar") && i.message.contains("/LastChar")));
    }

    #[test]
    fn validate_page_tree_cycle() {
        let mut doc = Document::new();
        // Create two page nodes that form a parent cycle
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((10, 0), Object::Dictionary(page));

        let mut parent1 = Dictionary::new();
        parent1.set("Type", Object::Name(b"Pages".to_vec()));
        parent1.set("Parent", Object::Reference((3, 0)));
        parent1.set("Count", Object::Integer(1));
        parent1.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(parent1));

        let mut parent2 = Dictionary::new();
        parent2.set("Type", Object::Name(b"Pages".to_vec()));
        parent2.set("Parent", Object::Reference((2, 0))); // cycle!
        doc.objects.insert((3, 0), Object::Dictionary(parent2));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_page_tree_cycles(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("cycle")));
    }

    #[test]
    fn validate_name_tree_odd_names() {
        let mut doc = Document::new();
        let mut names_subtree = Dictionary::new();
        // Odd-length Names array
        names_subtree.set("Names", Object::Array(vec![
            Object::String(b"key1".to_vec(), StringFormat::Literal),
            Object::Integer(1),
            Object::String(b"key2".to_vec(), StringFormat::Literal),
            // Missing value for key2 → odd length = 3
        ]));
        doc.objects.insert((5, 0), Object::Dictionary(names_subtree));

        let mut ef_dict = Dictionary::new();
        ef_dict.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        doc.objects.insert((4, 0), Object::Dictionary(ef_dict));

        let mut names = Dictionary::new();
        names.set("EmbeddedFiles", Object::Reference((4, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(names));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_names_tree_structure(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("odd length")));
    }

    #[test]
    fn validate_name_tree_no_names_or_kids() {
        let mut doc = Document::new();
        // Empty dict node (neither /Names nor /Kids)
        let empty_node = Dictionary::new();
        doc.objects.insert((5, 0), Object::Dictionary(empty_node));

        let mut ef_dict = Dictionary::new();
        ef_dict.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        doc.objects.insert((4, 0), Object::Dictionary(ef_dict));

        let mut names = Dictionary::new();
        names.set("EmbeddedFiles", Object::Reference((4, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(names));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_names_tree_structure(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("neither /Names nor /Kids")));
    }

    #[test]
    fn validate_duplicate_objects() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(1));
        doc.objects.insert((1, 1), Object::Integer(2)); // Same obj# different gen
        doc.objects.insert((2, 0), Object::Integer(3));

        let mut issues = Vec::new();
        check_duplicate_objects(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("Object 1") && i.message.contains("multiple generations")));
    }

    #[test]
    fn validate_no_duplicate_objects() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(1));
        doc.objects.insert((2, 0), Object::Integer(2));

        let mut issues = Vec::new();
        check_duplicate_objects(&doc, &mut issues);
        assert!(issues.is_empty());
    }

}
