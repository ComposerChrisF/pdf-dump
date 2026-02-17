use lopdf::{Document, Object};
use serde_json::{json, Value};
use std::io::Write;

use crate::helpers::{resolve_dict, obj_to_string_lossy, name_to_string, walk_number_tree, get_catalog};

pub(crate) struct PageLabelEntry {
    pub physical_page: u32,
    pub label: String,
    pub style: String,
    pub prefix: String,
    pub start: i64,
}

fn int_to_roman(mut n: i64, uppercase: bool) -> String {
    if n <= 0 { return n.to_string(); }
    let table: &[(i64, &str)] = &[
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"),
        (100, "c"), (90, "xc"), (50, "l"), (40, "xl"),
        (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut result = String::new();
    for &(value, numeral) in table {
        while n >= value {
            result.push_str(numeral);
            n -= value;
        }
    }
    if uppercase { result.to_uppercase() } else { result }
}

fn int_to_alpha(n: i64, uppercase: bool) -> String {
    if n <= 0 { return n.to_string(); }
    let mut result = String::new();
    let mut remaining = n - 1;
    loop {
        let ch = (remaining % 26) as u8;
        let letter = if uppercase { b'A' + ch } else { b'a' + ch };
        result.insert(0, letter as char);
        remaining = remaining / 26 - 1;
        if remaining < 0 { break; }
    }
    result
}

fn format_page_label(style: &str, prefix: &str, value: i64) -> String {
    let number_part = match style {
        "D" => value.to_string(),
        "r" => int_to_roman(value, false),
        "R" => int_to_roman(value, true),
        "a" => int_to_alpha(value, false),
        "A" => int_to_alpha(value, true),
        _ => String::new(),
    };
    format!("{}{}", prefix, number_part)
}

pub(crate) fn collect_page_labels(doc: &Document) -> Vec<PageLabelEntry> {
    let mut entries = Vec::new();

    let catalog = match get_catalog(doc) {
        Some(c) => c,
        None => return entries,
    };

    let page_labels_dict = match catalog.get(b"PageLabels").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return entries,
    };

    let mut ranges = walk_number_tree(doc, page_labels_dict);
    ranges.sort_by_key(|(k, _)| *k);

    // Pre-parse ranges into (range_start, style, prefix, start_val) tuples
    let parsed_ranges: Vec<(i64, String, String, i64)> = ranges.iter().map(|(range_key, value)| {
        let label_dict = resolve_dict(doc, value);
        match label_dict {
            Some(d) => {
                let s = d.get(b"S").ok()
                    .and_then(name_to_string)
                    .unwrap_or_else(|| "-".to_string());
                let p = d.get(b"P").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_default();
                let st = d.get(b"St").ok()
                    .and_then(|v| if let Object::Integer(i) = v { Some(*i) } else { None })
                    .unwrap_or(1);
                (*range_key, s, p, st)
            }
            None => (*range_key, "D".to_string(), String::new(), 1),
        }
    }).collect();

    let page_count = doc.get_pages().len() as u32;

    for phys in 0..page_count {
        // Find the applicable range: the last parsed range whose key <= phys
        let (range_start, style, prefix, start_val) = parsed_ranges.iter().rev()
            .find(|(k, _, _, _)| *k as u32 <= phys)
            .map(|(k, s, p, st)| (*k, s.as_str(), p.as_str(), *st))
            .unwrap_or((0, "D", "", 1));

        let offset = phys as i64 - range_start;
        let value = start_val + offset;
        let label = format_page_label(style, prefix, value);

        entries.push(PageLabelEntry {
            physical_page: phys + 1,
            label,
            style: style.to_string(),
            prefix: prefix.to_string(),
            start: start_val,
        });
    }

    entries
}

pub(crate) fn print_page_labels(writer: &mut impl Write, doc: &Document) {
    let labels = collect_page_labels(doc);
    if labels.is_empty() {
        wln!(writer, "No page labels defined.");
        return;
    }
    wln!(writer, "{} pages with labels\n", labels.len());
    wln!(writer, "  {:>8}  Label", "Physical");
    for entry in &labels {
        wln!(writer, "  {:>8}  {}", entry.physical_page, entry.label);
    }
}

pub(crate) fn labels_json_value(doc: &Document) -> Value {
    let labels = collect_page_labels(doc);
    let items: Vec<Value> = labels.iter().map(|e| {
        json!({
            "physical_page": e.physical_page,
            "label": e.label,
            "style": e.style,
            "prefix": e.prefix,
            "start": e.start,
        })
    }).collect();
    json!({
        "page_count": items.len(),
        "page_labels": items,
    })
}

#[cfg(test)]
pub(crate) fn print_page_labels_json(writer: &mut impl Write, doc: &Document) {
    use crate::helpers::json_pretty;
    let output = labels_json_value(doc);
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

    #[test]
    fn int_to_roman_basic() {
        assert_eq!(int_to_roman(1, false), "i");
        assert_eq!(int_to_roman(4, false), "iv");
        assert_eq!(int_to_roman(9, true), "IX");
        assert_eq!(int_to_roman(14, false), "xiv");
        assert_eq!(int_to_roman(1999, true), "MCMXCIX");
    }

    #[test]
    fn int_to_alpha_basic() {
        assert_eq!(int_to_alpha(1, true), "A");
        assert_eq!(int_to_alpha(26, true), "Z");
        assert_eq!(int_to_alpha(27, true), "AA");
        assert_eq!(int_to_alpha(1, false), "a");
    }

    #[test]
    fn format_page_label_decimal() {
        assert_eq!(format_page_label("D", "", 5), "5");
        assert_eq!(format_page_label("D", "P-", 3), "P-3");
    }

    #[test]
    fn format_page_label_roman() {
        assert_eq!(format_page_label("r", "", 3), "iii");
        assert_eq!(format_page_label("R", "", 4), "IV");
    }

    #[test]
    fn format_page_label_alpha() {
        assert_eq!(format_page_label("a", "", 1), "a");
        assert_eq!(format_page_label("A", "", 2), "B");
    }

    #[test]
    fn format_page_label_prefix_only() {
        // Style "-" means no number, only prefix
        assert_eq!(format_page_label("-", "Cover", 1), "Cover");
    }

    #[test]
    fn page_labels_no_labels() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let labels = collect_page_labels(&doc);
        assert!(labels.is_empty());
    }

    #[test]
    fn page_labels_roman_then_decimal() {
        let mut doc = Document::new();

        // Create a simple page tree with 5 pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(5));
        let mut kids = Vec::new();
        for i in 10..15 {
            let mut page = Dictionary::new();
            page.set("Type", Object::Name(b"Page".to_vec()));
            page.set("Parent", Object::Reference((2, 0)));
            page.set("MediaBox", Object::Array(vec![
                Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
            ]));
            doc.objects.insert((i, 0), Object::Dictionary(page));
            kids.push(Object::Reference((i, 0)));
        }
        pages_dict.set("Kids", Object::Array(kids));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        // PageLabels: pages 0-2 are roman lowercase, pages 3-4 are decimal starting at 1
        let mut rule_roman = Dictionary::new();
        rule_roman.set("S", Object::Name(b"r".to_vec()));
        let mut rule_decimal = Dictionary::new();
        rule_decimal.set("S", Object::Name(b"D".to_vec()));
        rule_decimal.set("St", Object::Integer(1));

        let mut pl_dict = Dictionary::new();
        pl_dict.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Dictionary(rule_roman),
            Object::Integer(3), Object::Dictionary(rule_decimal),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(pl_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("PageLabels", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let labels = collect_page_labels(&doc);
        assert_eq!(labels.len(), 5);
        assert_eq!(labels[0].label, "i");
        assert_eq!(labels[1].label, "ii");
        assert_eq!(labels[2].label, "iii");
        assert_eq!(labels[3].label, "1");
        assert_eq!(labels[4].label, "2");
    }

    #[test]
    fn page_labels_with_prefix() {
        let mut doc = Document::new();
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(2));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Parent", Object::Reference((2, 0)));
        page1.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((10, 0), Object::Dictionary(page1));
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Parent", Object::Reference((2, 0)));
        page2.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((11, 0), Object::Dictionary(page2));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0)), Object::Reference((11, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut rule = Dictionary::new();
        rule.set("S", Object::Name(b"D".to_vec()));
        rule.set("P", Object::String(b"A-".to_vec(), StringFormat::Literal));
        rule.set("St", Object::Integer(1));
        let mut pl_dict = Dictionary::new();
        pl_dict.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Dictionary(rule),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(pl_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("PageLabels", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let labels = collect_page_labels(&doc);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].label, "A-1");
        assert_eq!(labels[1].label, "A-2");
    }

    #[test]
    fn page_labels_print_no_labels() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_page_labels(w, &doc));
        assert!(out.contains("No page labels defined."));
    }

    #[test]
    fn page_labels_json() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_page_labels_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["page_count"], 0);
    }

}
