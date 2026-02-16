use lopdf::{content::Content, Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use crate::types::PageSpec;
use crate::stream::decode_stream;
use crate::helpers::{format_dict_value, format_operation};

pub(crate) struct DiffResult {
    pub metadata_diffs: Vec<String>,
    pub page_diffs: Vec<PageDiff>,
    pub font_diffs: FontDiff,
    pub object_count: (usize, usize),
}

pub(crate) struct PageDiff {
    pub page_number: u32,
    pub identical: bool,
    pub dict_diffs: Vec<String>,
    pub resource_diffs: Vec<String>,
    pub content_diffs: Vec<String>,
}

pub(crate) struct FontDiff {
    pub only_in_first: Vec<String>,
    pub only_in_second: Vec<String>,
}

pub(crate) fn compare_pdfs(doc1: &Document, doc2: &Document, page_filter: Option<&PageSpec>) -> DiffResult {
    let metadata_diffs = compare_metadata(doc1, doc2);
    let font_diffs = compare_fonts(doc1, doc2);

    let pages1 = doc1.get_pages();
    let pages2 = doc2.get_pages();

    let mut page_diffs = Vec::new();

    let page_numbers: Vec<u32> = if let Some(spec) = page_filter {
        spec.pages()
    } else {
        let max_pages = pages1.len().max(pages2.len()) as u32;
        (1..=max_pages).collect()
    };

    for pn in page_numbers {
        let id1 = pages1.get(&pn);
        let id2 = pages2.get(&pn);
        match (id1, id2) {
            (Some(&id1), Some(&id2)) => {
                page_diffs.push(compare_page(doc1, doc2, id1, id2, pn));
            }
            (Some(_), None) => {
                page_diffs.push(PageDiff {
                    page_number: pn,
                    identical: false,
                    dict_diffs: vec![format!("Page {} only in first file", pn)],
                    resource_diffs: vec![],
                    content_diffs: vec![],
                });
            }
            (None, Some(_)) => {
                page_diffs.push(PageDiff {
                    page_number: pn,
                    identical: false,
                    dict_diffs: vec![format!("Page {} only in second file", pn)],
                    resource_diffs: vec![],
                    content_diffs: vec![],
                });
            }
            (None, None) if page_filter.is_some() => {
                page_diffs.push(PageDiff {
                    page_number: pn,
                    identical: false,
                    dict_diffs: vec![format!("Page {} not found in either file", pn)],
                    resource_diffs: vec![],
                    content_diffs: vec![],
                });
            }
            _ => {}
        }
    }

    DiffResult {
        metadata_diffs,
        page_diffs,
        font_diffs,
        object_count: (doc1.objects.len(), doc2.objects.len()),
    }
}

pub(crate) fn compare_metadata(doc1: &Document, doc2: &Document) -> Vec<String> {
    let mut diffs = Vec::new();

    if doc1.version != doc2.version {
        diffs.push(format!("Version: {} vs {}", doc1.version, doc2.version));
    }

    let pages1 = doc1.get_pages().len();
    let pages2 = doc2.get_pages().len();
    if pages1 != pages2 {
        diffs.push(format!("Pages: {} vs {}", pages1, pages2));
    }

    // Compare /Info fields
    let info_fields = [
        b"Title".as_slice(), b"Author", b"Subject", b"Keywords",
        b"Creator", b"Producer", b"CreationDate", b"ModDate",
    ];
    let get_info = |doc: &Document, field: &[u8]| -> Option<String> {
        let info_ref = doc.trailer.get(b"Info").ok()?;
        let (_, obj) = doc.dereference(info_ref).ok()?;
        if let Object::Dictionary(d) = obj
            && let Ok(Object::String(bytes, _)) = d.get(field)
        {
            return Some(String::from_utf8_lossy(bytes).into_owned());
        }
        None
    };

    for field in info_fields {
        let v1 = get_info(doc1, field);
        let v2 = get_info(doc2, field);
        if v1 != v2 {
            let name = String::from_utf8_lossy(field);
            let s1 = v1.unwrap_or_else(|| "(none)".to_string());
            let s2 = v2.unwrap_or_else(|| "(none)".to_string());
            diffs.push(format!("{}: \"{}\" vs \"{}\"", name, s1, s2));
        }
    }

    diffs
}

pub(crate) fn compare_page(doc1: &Document, doc2: &Document, page_id1: ObjectId, page_id2: ObjectId, page_num: u32) -> PageDiff {
    let dict1 = match doc1.get_object(page_id1) {
        Ok(Object::Dictionary(d)) => d,
        _ => return PageDiff { page_number: page_num, identical: false, dict_diffs: vec!["Could not read page from first file".into()], resource_diffs: vec![], content_diffs: vec![] },
    };
    let dict2 = match doc2.get_object(page_id2) {
        Ok(Object::Dictionary(d)) => d,
        _ => return PageDiff { page_number: page_num, identical: false, dict_diffs: vec!["Could not read page from second file".into()], resource_diffs: vec![], content_diffs: vec![] },
    };

    let mut dict_diffs = Vec::new();
    let mut resource_diffs = Vec::new();

    // Compare page dict entries (skip Parent, Contents, Resources)
    let skip_keys: &[&[u8]] = &[b"Parent", b"Contents", b"Resources"];
    for (key, val1) in dict1.iter() {
        if skip_keys.contains(&key.as_slice()) { continue; }
        let v1_str = format_dict_value(val1);
        match dict2.get(key) {
            Ok(val2) => {
                let v2_str = format_dict_value(val2);
                if v1_str != v2_str {
                    dict_diffs.push(format!("/{}: {} vs {}", String::from_utf8_lossy(key), v1_str, v2_str));
                }
            }
            Err(_) => {
                dict_diffs.push(format!("/{}: {} vs (missing)", String::from_utf8_lossy(key), v1_str));
            }
        }
    }
    // Keys only in dict2
    for (key, val2) in dict2.iter() {
        if skip_keys.contains(&key.as_slice()) { continue; }
        if dict1.get(key).is_err() {
            dict_diffs.push(format!("/{}: (missing) vs {}", String::from_utf8_lossy(key), format_dict_value(val2)));
        }
    }

    // Compare resources across multiple categories
    let get_resource_names = |doc: &Document, dict: &lopdf::Dictionary, category: &[u8]| -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let resources = match dict.get(b"Resources") {
            Ok(Object::Reference(id)) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { d } else { return names; }
            }
            Ok(Object::Dictionary(d)) => d,
            _ => return names,
        };
        if let Ok(cat_obj) = resources.get(category) {
            let cat_dict = match cat_obj {
                Object::Dictionary(d) => d,
                Object::Reference(id) => {
                    if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { d } else { return names; }
                }
                _ => return names,
            };
            for (k, _) in cat_dict.iter() {
                names.insert(String::from_utf8_lossy(k).into_owned());
            }
        }
        names
    };

    for category in &[b"Font" as &[u8], b"XObject", b"ColorSpace", b"ExtGState", b"Pattern", b"Shading"] {
        let cat_name = String::from_utf8_lossy(category);
        let names1 = get_resource_names(doc1, dict1, category);
        let names2 = get_resource_names(doc2, dict2, category);
        if names1 != names2 {
            for n in names1.difference(&names2) {
                resource_diffs.push(format!("{} {} only in first file", cat_name, n));
            }
            for n in names2.difference(&names1) {
                resource_diffs.push(format!("{} {} only in second file", cat_name, n));
            }
        }
    }

    // Compare content streams
    let content_diffs = compare_content_streams(doc1, doc2, page_id1, page_id2);

    let identical = dict_diffs.is_empty() && resource_diffs.is_empty() && content_diffs.is_empty();

    PageDiff {
        page_number: page_num,
        identical,
        dict_diffs,
        resource_diffs,
        content_diffs,
    }
}

pub(crate) fn get_content_ops(doc: &Document, page_id: ObjectId) -> Vec<String> {
    let dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return vec![],
    };

    let content_ids: Vec<ObjectId> = match dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return vec![],
    };

    let mut all_bytes = Vec::new();
    for cid in &content_ids {
        if let Ok(Object::Stream(stream)) = doc.get_object(*cid) {
            let (decoded, _warning) = decode_stream(stream);
            all_bytes.extend_from_slice(&decoded);
        }
    }

    match Content::decode(&all_bytes) {
        Ok(content) => content.operations.iter().map(format_operation).collect(),
        Err(_) => vec![],
    }
}

pub(crate) fn compare_content_streams(doc1: &Document, doc2: &Document, page_id1: ObjectId, page_id2: ObjectId) -> Vec<String> {
    let ops1 = get_content_ops(doc1, page_id1);
    let ops2 = get_content_ops(doc2, page_id2);

    if ops1 == ops2 {
        return vec![];
    }

    // Simple line-based diff
    let mut diffs = Vec::new();
    let max = ops1.len().max(ops2.len());
    for i in 0..max {
        match (ops1.get(i), ops2.get(i)) {
            (Some(a), Some(b)) if a != b => {
                diffs.push(format!("- {}", a));
                diffs.push(format!("+ {}", b));
            }
            (Some(a), None) => {
                diffs.push(format!("- {}", a));
            }
            (None, Some(b)) => {
                diffs.push(format!("+ {}", b));
            }
            _ => {}
        }
    }
    diffs
}

pub(crate) fn collect_all_font_names(doc: &Document) -> BTreeSet<String> {
    let mut fonts = BTreeSet::new();
    for obj in doc.objects.values() {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        if dict.get_type().ok().is_some_and(|t| t == b"Font")
            && let Ok(Object::Name(name)) = dict.get(b"BaseFont")
        {
            fonts.insert(String::from_utf8_lossy(name).into_owned());
        }
    }
    fonts
}

pub(crate) fn compare_fonts(doc1: &Document, doc2: &Document) -> FontDiff {
    let fonts1 = collect_all_font_names(doc1);
    let fonts2 = collect_all_font_names(doc2);
    FontDiff {
        only_in_first: fonts1.difference(&fonts2).cloned().collect(),
        only_in_second: fonts2.difference(&fonts1).cloned().collect(),
    }
}

pub(crate) fn print_diff(writer: &mut impl Write, result: &DiffResult, file1: &Path, file2: &Path) {
    writeln!(writer, "Comparing: {} vs {}", file1.display(), file2.display()).unwrap();
    writeln!(writer, "Objects: {} vs {}\n", result.object_count.0, result.object_count.1).unwrap();

    if !result.metadata_diffs.is_empty() {
        writeln!(writer, "--- Metadata ---").unwrap();
        for d in &result.metadata_diffs {
            writeln!(writer, "  {}", d).unwrap();
        }
        writeln!(writer).unwrap();
    }

    for page in &result.page_diffs {
        writeln!(writer, "--- Page {} ---", page.page_number).unwrap();
        if page.identical {
            writeln!(writer, "  (identical)").unwrap();
        } else {
            for d in &page.dict_diffs {
                writeln!(writer, "  {}", d).unwrap();
            }
            for d in &page.resource_diffs {
                writeln!(writer, "  {}", d).unwrap();
            }
            if !page.content_diffs.is_empty() {
                writeln!(writer, "  Content stream: differs").unwrap();
                for d in &page.content_diffs {
                    writeln!(writer, "    {}", d).unwrap();
                }
            }
        }
        writeln!(writer).unwrap();
    }

    if !result.font_diffs.only_in_first.is_empty() || !result.font_diffs.only_in_second.is_empty() {
        writeln!(writer, "--- Fonts ---").unwrap();
        for f in &result.font_diffs.only_in_first {
            writeln!(writer, "  Only in {}: {}", file1.display(), f).unwrap();
        }
        for f in &result.font_diffs.only_in_second {
            writeln!(writer, "  Only in {}: {}", file2.display(), f).unwrap();
        }
        writeln!(writer).unwrap();
    }
}

pub(crate) fn print_diff_json(writer: &mut impl Write, result: &DiffResult, file1: &Path, file2: &Path) {
    let pages: Vec<Value> = result.page_diffs.iter().map(|p| {
        json!({
            "page_number": p.page_number,
            "identical": p.identical,
            "dict_diffs": p.dict_diffs,
            "resource_diffs": p.resource_diffs,
            "content_diffs": p.content_diffs,
        })
    }).collect();

    let output = json!({
        "file1": file1.display().to_string(),
        "file2": file2.display().to_string(),
        "object_count": {"file1": result.object_count.0, "file2": result.object_count.1},
        "metadata_diffs": result.metadata_diffs,
        "page_diffs": pages,
        "font_diffs": {
            "only_in_first": result.font_diffs.only_in_first,
            "only_in_second": result.font_diffs.only_in_second,
        },
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;
    use lopdf::Document;
    use std::path::PathBuf;

    #[test]
    fn diff_identical_docs() {
        let doc = build_two_page_doc();
        let result = compare_pdfs(&doc, &doc, None);
        assert!(result.metadata_diffs.is_empty());
        for page in &result.page_diffs {
            assert!(page.identical, "Page {} should be identical", page.page_number);
        }
    }

    #[test]
    fn diff_different_page_counts() {
        let doc1 = build_two_page_doc();
        let mut doc2 = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc2.add_object(Object::Dictionary(catalog));
        doc2.trailer.set("Root", Object::Reference(catalog_id));

        let result = compare_pdfs(&doc1, &doc2, None);
        assert!(result.metadata_diffs.iter().any(|d| d.contains("Pages")));
    }

    #[test]
    fn diff_with_page_filter() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let result = compare_pdfs(&doc, &doc, Some(&spec));
        assert_eq!(result.page_diffs.len(), 1);
        assert_eq!(result.page_diffs[0].page_number, 1);
        assert!(result.page_diffs[0].identical);
    }

    #[test]
    fn compare_fonts_identical() {
        let doc = build_two_page_doc();
        let fd = compare_fonts(&doc, &doc);
        assert!(fd.only_in_first.is_empty());
        assert!(fd.only_in_second.is_empty());
    }

    #[test]
    fn print_diff_produces_output() {
        let doc = build_two_page_doc();
        let result = compare_pdfs(&doc, &doc, None);
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("Comparing:"));
        assert!(out.contains("Objects:"));
    }

    #[test]
    fn print_diff_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let result = compare_pdfs(&doc, &doc, None);
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff_json(w, &result, &file1, &file2));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert!(parsed.get("page_diffs").is_some());
        assert!(parsed.get("metadata_diffs").is_some());
    }

    #[test]
    fn compare_metadata_identical() {
        let doc = Document::new();
        let diffs = compare_metadata(&doc, &doc);
        assert!(diffs.is_empty());
    }

    #[test]
    fn compare_metadata_different_page_counts() {
        let doc1 = build_two_page_doc();
        let doc2 = Document::new();
        let diffs = compare_metadata(&doc1, &doc2);
        assert!(diffs.iter().any(|d| d.contains("Pages")), "Should report page count diff, got: {:?}", diffs);
    }

    #[test]
    fn compare_metadata_info_field_diffs() {
        let mut doc1 = Document::new();
        let mut info1 = Dictionary::new();
        info1.set("Title", Object::String(b"Title A".to_vec(), StringFormat::Literal));
        let info1_id = doc1.add_object(Object::Dictionary(info1));
        doc1.trailer.set("Info", Object::Reference(info1_id));

        let mut doc2 = Document::new();
        let mut info2 = Dictionary::new();
        info2.set("Title", Object::String(b"Title B".to_vec(), StringFormat::Literal));
        let info2_id = doc2.add_object(Object::Dictionary(info2));
        doc2.trailer.set("Info", Object::Reference(info2_id));

        let diffs = compare_metadata(&doc1, &doc2);
        assert!(diffs.iter().any(|d| d.contains("Title")), "Should report title diff, got: {:?}", diffs);
        assert!(diffs.iter().any(|d| d.contains("Title A") && d.contains("Title B")));
    }

    #[test]
    fn compare_metadata_info_present_vs_absent() {
        let mut doc1 = Document::new();
        let mut info = Dictionary::new();
        info.set("Author", Object::String(b"Someone".to_vec(), StringFormat::Literal));
        let info_id = doc1.add_object(Object::Dictionary(info));
        doc1.trailer.set("Info", Object::Reference(info_id));

        let doc2 = Document::new();

        let diffs = compare_metadata(&doc1, &doc2);
        assert!(diffs.iter().any(|d| d.contains("Author")), "Should report author diff, got: {:?}", diffs);
    }

    #[test]
    fn compare_content_streams_identical() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"BT\n(Hello) Tj\nET".to_vec());
        let s_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(s_id));
        let p_id = doc.add_object(Object::Dictionary(page));

        let diffs = compare_content_streams(&doc, &doc, p_id, p_id);
        assert!(diffs.is_empty(), "Identical streams should have no diffs");
    }

    #[test]
    fn compare_content_streams_different() {
        let mut doc1 = Document::new();
        let s1 = Stream::new(Dictionary::new(), b"BT\n(Hello) Tj\nET".to_vec());
        let s1_id = doc1.add_object(Object::Stream(s1));
        let mut page1 = Dictionary::new();
        page1.set("Contents", Object::Reference(s1_id));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let s2 = Stream::new(Dictionary::new(), b"BT\n(World) Tj\nET".to_vec());
        let s2_id = doc2.add_object(Object::Stream(s2));
        let mut page2 = Dictionary::new();
        page2.set("Contents", Object::Reference(s2_id));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let diffs = compare_content_streams(&doc1, &doc2, p1_id, p2_id);
        assert!(!diffs.is_empty(), "Different streams should have diffs");
        assert!(diffs.iter().any(|d| d.starts_with("- ") || d.starts_with("+ ")));
    }

    #[test]
    fn compare_content_streams_one_longer() {
        let mut doc1 = Document::new();
        let s1 = Stream::new(Dictionary::new(), b"BT\n(A) Tj\n(B) Tj\nET".to_vec());
        let s1_id = doc1.add_object(Object::Stream(s1));
        let mut page1 = Dictionary::new();
        page1.set("Contents", Object::Reference(s1_id));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let s2 = Stream::new(Dictionary::new(), b"BT\n(A) Tj\nET".to_vec());
        let s2_id = doc2.add_object(Object::Stream(s2));
        let mut page2 = Dictionary::new();
        page2.set("Contents", Object::Reference(s2_id));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let diffs = compare_content_streams(&doc1, &doc2, p1_id, p2_id);
        assert!(!diffs.is_empty(), "Different-length streams should have diffs");
    }

    #[test]
    fn compare_content_streams_no_contents() {
        let mut doc = Document::new();
        let page = Dictionary::new();
        let p_id = doc.add_object(Object::Dictionary(page));
        let diffs = compare_content_streams(&doc, &doc, p_id, p_id);
        assert!(diffs.is_empty(), "Pages with no contents should have no diffs");
    }

    #[test]
    fn get_content_ops_valid_stream() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"BT\n(Hello) Tj\nET".to_vec());
        let s_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(s_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let ops = get_content_ops(&doc, p_id);
        assert!(!ops.is_empty(), "Should have operations");
        assert!(ops.contains(&"BT".to_string()), "Should contain BT, got: {:?}", ops);
        assert!(ops.contains(&"(Hello) Tj".to_string()), "Should contain readable Tj, got: {:?}", ops);
        assert!(ops.contains(&"ET".to_string()), "Should contain ET, got: {:?}", ops);
    }

    #[test]
    fn get_content_ops_no_contents() {
        let mut doc = Document::new();
        let page = Dictionary::new();
        let p_id = doc.add_object(Object::Dictionary(page));
        let ops = get_content_ops(&doc, p_id);
        assert!(ops.is_empty());
    }

    #[test]
    fn get_content_ops_non_dict_page() {
        let mut doc = Document::new();
        let p_id = doc.add_object(Object::Integer(42));
        let ops = get_content_ops(&doc, p_id);
        assert!(ops.is_empty());
    }

    #[test]
    fn collect_all_font_names_finds_fonts() {
        let doc = build_two_page_doc();
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.contains("Helvetica"), "Should find Helvetica font, got: {:?}", fonts);
    }

    #[test]
    fn collect_all_font_names_no_fonts() {
        let doc = Document::new();
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.is_empty());
    }

    #[test]
    fn collect_all_font_names_ignores_non_font() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        dict.set("BaseFont", Object::Name(b"NotAFont".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.is_empty(), "Should only collect fonts with Type=Font");
    }

    #[test]
    fn collect_all_font_names_stream_font() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"CourierNew".to_vec()));
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.contains("CourierNew"), "Should find font in stream object");
    }

    #[test]
    fn compare_fonts_different() {
        let mut doc1 = Document::new();
        let mut f1 = Dictionary::new();
        f1.set("Type", Object::Name(b"Font".to_vec()));
        f1.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc1.objects.insert((1, 0), Object::Dictionary(f1));

        let mut doc2 = Document::new();
        let mut f2 = Dictionary::new();
        f2.set("Type", Object::Name(b"Font".to_vec()));
        f2.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc2.objects.insert((1, 0), Object::Dictionary(f2));

        let fd = compare_fonts(&doc1, &doc2);
        assert!(fd.only_in_first.contains(&"Helvetica".to_string()));
        assert!(fd.only_in_second.contains(&"Courier".to_string()));
    }

    #[test]
    fn compare_page_identical() {
        let doc = build_two_page_doc();
        let pages = doc.get_pages();
        let p1_id = *pages.get(&1).unwrap();
        let pd = compare_page(&doc, &doc, p1_id, p1_id, 1);
        assert!(pd.identical, "Same page should be identical");
        assert!(pd.dict_diffs.is_empty());
        assert!(pd.resource_diffs.is_empty());
        assert!(pd.content_diffs.is_empty());
    }

    #[test]
    fn compare_page_different_dict_entries() {
        let mut doc1 = Document::new();
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(595), Object::Integer(842),
        ]));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.dict_diffs.iter().any(|d| d.contains("MediaBox")), "Should show MediaBox diff, got: {:?}", pd.dict_diffs);
    }

    #[test]
    fn compare_page_key_only_in_first() {
        let mut doc1 = Document::new();
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Rotate", Object::Integer(90));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.dict_diffs.iter().any(|d| d.contains("Rotate") && d.contains("(missing)")));
    }

    #[test]
    fn compare_page_key_only_in_second() {
        let mut doc1 = Document::new();
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("CropBox", Object::Array(vec![Object::Integer(0)]));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.dict_diffs.iter().any(|d| d.contains("CropBox") && d.contains("(missing)")));
    }

    #[test]
    fn compare_page_non_dict_page() {
        let mut doc = Document::new();
        let p_id = doc.add_object(Object::Integer(42));
        let pd = compare_page(&doc, &doc, p_id, p_id, 1);
        assert!(!pd.identical);
        assert!(!pd.dict_diffs.is_empty());
    }

    #[test]
    fn compare_page_xobject_resource_diff() {
        let mut doc1 = Document::new();
        let mut res1 = Dictionary::new();
        let mut xobj1 = Dictionary::new();
        xobj1.set("Im0", Object::Null);
        res1.set("XObject", Object::Dictionary(xobj1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(res1));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut res2 = Dictionary::new();
        let mut xobj2 = Dictionary::new();
        xobj2.set("Im0", Object::Null);
        xobj2.set("Im1", Object::Null);
        res2.set("XObject", Object::Dictionary(xobj2));
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Resources", Object::Dictionary(res2));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("XObject") && d.contains("Im1") && d.contains("second")),
            "Should detect XObject Im1 only in second file, got: {:?}", pd.resource_diffs);
    }

    #[test]
    fn compare_page_extgstate_resource_diff() {
        let mut doc1 = Document::new();
        let mut res1 = Dictionary::new();
        let mut gs1 = Dictionary::new();
        gs1.set("GS0", Object::Null);
        res1.set("ExtGState", Object::Dictionary(gs1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(res1));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("ExtGState") && d.contains("GS0") && d.contains("first")),
            "Should detect ExtGState GS0 only in first file, got: {:?}", pd.resource_diffs);
    }

    #[test]
    fn compare_page_colorspace_resource_diff() {
        let mut doc1 = Document::new();
        let mut res1 = Dictionary::new();
        let mut cs1 = Dictionary::new();
        cs1.set("CS0", Object::Null);
        res1.set("ColorSpace", Object::Dictionary(cs1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(res1));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut res2 = Dictionary::new();
        let mut cs2 = Dictionary::new();
        cs2.set("CS1", Object::Null);
        res2.set("ColorSpace", Object::Dictionary(cs2));
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Resources", Object::Dictionary(res2));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("ColorSpace") && d.contains("CS0") && d.contains("first")),
            "Should detect CS0 only in first, got: {:?}", pd.resource_diffs);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("ColorSpace") && d.contains("CS1") && d.contains("second")),
            "Should detect CS1 only in second, got: {:?}", pd.resource_diffs);
    }

    #[test]
    fn compare_pdfs_page_only_in_first() {
        let doc1 = build_two_page_doc();
        let doc2 = Document::new();
        let spec = PageSpec::Single(1);
        let result = compare_pdfs(&doc1, &doc2, Some(&spec));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(!result.page_diffs[0].identical);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("only in first")));
    }

    #[test]
    fn compare_pdfs_page_only_in_second() {
        let doc1 = Document::new();
        let doc2 = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let result = compare_pdfs(&doc1, &doc2, Some(&spec));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(!result.page_diffs[0].identical);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("only in second")));
    }

    #[test]
    fn compare_pdfs_page_not_in_either() {
        let doc = Document::new();
        let spec = PageSpec::Single(999);
        let result = compare_pdfs(&doc, &doc, Some(&spec));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("not found in either")));
    }

    #[test]
    fn compare_pdfs_no_filter_different_page_counts() {
        let doc1 = build_two_page_doc();
        let mut doc2 = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc2.add_object(Object::Dictionary(catalog));
        doc2.trailer.set("Root", Object::Reference(catalog_id));

        let result = compare_pdfs(&doc1, &doc2, None);
        // Pages only in doc1 should be reported
        assert!(result.page_diffs.iter().any(|p| p.dict_diffs.iter().any(|d| d.contains("only in first"))));
    }

    #[test]
    fn print_diff_shows_metadata_diffs() {
        let result = DiffResult {
            metadata_diffs: vec!["Version: 1.4 vs 1.7".to_string()],
            page_diffs: vec![],
            font_diffs: FontDiff { only_in_first: vec![], only_in_second: vec![] },
            object_count: (5, 6),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("--- Metadata ---"), "Should show metadata section");
        assert!(out.contains("Version: 1.4 vs 1.7"));
    }

    #[test]
    fn print_diff_shows_font_diffs() {
        let result = DiffResult {
            metadata_diffs: vec![],
            page_diffs: vec![],
            font_diffs: FontDiff {
                only_in_first: vec!["Helvetica".to_string()],
                only_in_second: vec!["Courier".to_string()],
            },
            object_count: (5, 5),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("--- Fonts ---"), "Should show fonts section");
        assert!(out.contains("Helvetica"));
        assert!(out.contains("Courier"));
    }

    #[test]
    fn print_diff_shows_page_content_diffs() {
        let result = DiffResult {
            metadata_diffs: vec![],
            page_diffs: vec![PageDiff {
                page_number: 1,
                identical: false,
                dict_diffs: vec!["/MediaBox: [0 0 612 792] vs [0 0 595 842]".to_string()],
                resource_diffs: vec!["Font F1 only in first file".to_string()],
                content_diffs: vec!["- (Hello) Tj".to_string(), "+ (World) Tj".to_string()],
            }],
            font_diffs: FontDiff { only_in_first: vec![], only_in_second: vec![] },
            object_count: (5, 5),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("--- Page 1 ---"));
        assert!(out.contains("MediaBox"));
        assert!(out.contains("Font F1 only in first file"));
        assert!(out.contains("Content stream: differs"));
        assert!(out.contains("- (Hello) Tj"));
        assert!(out.contains("+ (World) Tj"));
    }

    #[test]
    fn print_diff_identical_page() {
        let result = DiffResult {
            metadata_diffs: vec![],
            page_diffs: vec![PageDiff {
                page_number: 1,
                identical: true,
                dict_diffs: vec![],
                resource_diffs: vec![],
                content_diffs: vec![],
            }],
            font_diffs: FontDiff { only_in_first: vec![], only_in_second: vec![] },
            object_count: (5, 5),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("(identical)"));
    }

    #[test]
    fn print_diff_json_with_diffs() {
        let result = DiffResult {
            metadata_diffs: vec!["Version: 1.4 vs 1.7".to_string()],
            page_diffs: vec![PageDiff {
                page_number: 1,
                identical: false,
                dict_diffs: vec!["diff1".to_string()],
                resource_diffs: vec!["rdiff1".to_string()],
                content_diffs: vec!["cdiff1".to_string()],
            }],
            font_diffs: FontDiff {
                only_in_first: vec!["Helvetica".to_string()],
                only_in_second: vec![],
            },
            object_count: (5, 6),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff_json(w, &result, &file1, &file2));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["metadata_diffs"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["page_diffs"][0]["identical"], false);
        assert_eq!(parsed["font_diffs"]["only_in_first"][0], "Helvetica");
        assert_eq!(parsed["object_count"]["file1"], 5);
        assert_eq!(parsed["object_count"]["file2"], 6);
    }

}
