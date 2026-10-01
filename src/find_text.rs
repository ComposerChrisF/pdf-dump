use lopdf::Document;
use serde_json::{Value, json};
use std::io::Write;

use crate::text::extract_text_from_page_with_warnings;
use crate::types::PageSpec;

struct PageMatch {
    page_number: u32,
    snippets: Vec<String>,
}

/// Characters of context shown on each side of a match.
const CONTEXT_CHARS: usize = 40;

/// Lowercases `s` one character at a time, returning the folded string and,
/// for each of its bytes, the byte span in `s` of the character it came from.
///
/// Per-character folding (rather than `str::to_lowercase`) applies the same
/// mapping to the text and the pattern, so matching never depends on context
/// such as Greek final sigma.
fn fold_case(s: &str) -> (String, Vec<(usize, usize)>) {
    let mut folded = String::with_capacity(s.len());
    let mut spans = Vec::with_capacity(s.len());
    for (start, c) in s.char_indices() {
        let end = start + c.len_utf8();
        let before = folded.len();
        folded.extend(c.to_lowercase());
        spans.resize(spans.len() + (folded.len() - before), (start, end));
    }
    (folded, spans)
}

/// The byte offset `n` characters before `pos` in `s` (or 0).
fn chars_before(s: &str, pos: usize, n: usize) -> usize {
    s[..pos]
        .char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map_or(0, |(i, _)| i)
}

/// The byte offset `n` characters after `pos` in `s` (or `s.len()`).
fn chars_after(s: &str, pos: usize, n: usize) -> usize {
    s[pos..]
        .char_indices()
        .nth(n)
        .map_or(s.len(), |(i, _)| pos + i)
}

fn find_matches(doc: &Document, pattern: &str, page_filter: Option<&PageSpec>) -> Vec<PageMatch> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let pages = doc.get_pages();
    let (lower_pattern, _) = fold_case(pattern);

    let page_list: Vec<(u32, lopdf::ObjectId)> = if let Some(spec) = page_filter {
        pages
            .iter()
            .filter(|(pn, _)| spec.contains(**pn))
            .map(|(&pn, &id)| (pn, id))
            .collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    let mut results = Vec::new();

    for (pn, page_id) in &page_list {
        let result = extract_text_from_page_with_warnings(doc, *page_id);
        let text = &result.text;
        let (lower_text, spans) = fold_case(text);

        let mut snippets = Vec::new();
        let mut search_start = 0;

        // All search offsets live in `lower_text`; `spans` maps a match back
        // onto `text`, since lowercasing can change a character's byte length
        // (İ is 2 bytes and folds to 3; ẞ is 3 and folds to 2).
        while let Some(pos) = lower_text[search_start..].find(&lower_pattern) {
            let lower_start = search_start + pos;
            let lower_end = lower_start + lower_pattern.len();
            let match_start = spans[lower_start].0;
            let match_end = spans[lower_end - 1].1;
            let ctx_start = chars_before(text, match_start, CONTEXT_CHARS);
            let ctx_end = chars_after(text, match_end, CONTEXT_CHARS);
            let snippet = text[ctx_start..ctx_end].replace('\n', " ");
            let mut formatted = String::new();
            if ctx_start > 0 {
                formatted.push_str("...");
            }
            formatted.push_str(snippet.trim());
            if ctx_end < text.len() {
                formatted.push_str("...");
            }
            snippets.push(formatted);
            search_start = lower_end;
        }

        if !snippets.is_empty() {
            results.push(PageMatch {
                page_number: *pn,
                snippets,
            });
        }
    }

    results
}

pub(crate) fn print_find_text(
    writer: &mut impl Write,
    doc: &Document,
    pattern: &str,
    page_filter: Option<&PageSpec>,
) {
    let matches = find_matches(doc, pattern, page_filter);

    if matches.is_empty() {
        wln!(writer, "No matches for \"{}\".", pattern);
        return;
    }

    let total_matches: usize = matches.iter().map(|m| m.snippets.len()).sum();

    for page_match in &matches {
        for snippet in &page_match.snippets {
            wln!(writer, "Page {}: \"{}\"", page_match.page_number, snippet);
        }
    }

    let page_count = matches.len();
    wln!(writer);
    wln!(
        writer,
        "Found \"{}\" {} time{} on {} page{}.",
        pattern,
        total_matches,
        if total_matches == 1 { "" } else { "s" },
        page_count,
        if page_count == 1 { "" } else { "s" },
    );
}

pub(crate) fn find_text_json_value(
    doc: &Document,
    pattern: &str,
    page_filter: Option<&PageSpec>,
) -> Value {
    let matches = find_matches(doc, pattern, page_filter);
    let total_matches: usize = matches.iter().map(|m| m.snippets.len()).sum();

    let pages: Vec<Value> = matches
        .iter()
        .map(|m| {
            json!({
                "page_number": m.page_number,
                "matches": m.snippets,
            })
        })
        .collect();

    json!({
        "pattern": pattern,
        "match_count": total_matches,
        "pages": pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        build_page_doc_with_content, build_two_page_doc, output_of, render_json,
    };
    use crate::types::PageSpec;

    #[test]
    fn find_text_no_matches() {
        let doc = build_page_doc_with_content(b"BT (Hello World) Tj ET");
        let out = output_of(|w| print_find_text(w, &doc, "xyz", None));
        assert!(out.contains("No matches"));
    }

    #[test]
    fn find_text_single_match() {
        let doc = build_page_doc_with_content(b"BT (Hello World) Tj ET");
        let out = output_of(|w| print_find_text(w, &doc, "Hello", None));
        assert!(out.contains("Page 1:"));
        assert!(out.contains("Hello"));
        assert!(out.contains("Found \"Hello\" 1 time on 1 page"));
    }

    #[test]
    fn find_text_case_insensitive() {
        let doc = build_page_doc_with_content(b"BT (Hello World) Tj ET");
        let out = output_of(|w| print_find_text(w, &doc, "hello", None));
        assert!(out.contains("Page 1:"));
        assert!(out.contains("Found \"hello\" 1 time"));
    }

    #[test]
    fn find_text_multiple_pages() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_find_text(w, &doc, "Page", None));
        assert!(out.contains("Page 1:"));
        assert!(out.contains("Page 2:"));
        assert!(out.contains("2 pages"));
    }

    #[test]
    fn find_text_with_page_filter() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let out = output_of(|w| print_find_text(w, &doc, "Page", Some(&spec)));
        assert!(out.contains("Page 1:"));
        assert!(!out.contains("Page 2:"));
        assert!(out.contains("1 page"));
    }

    #[test]
    fn find_text_json_output() {
        let doc = build_page_doc_with_content(b"BT (Hello World) Tj ET");
        let out = output_of(|w| render_json(w, &find_text_json_value(&doc, "Hello", None)));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["pattern"], "Hello");
        assert_eq!(v["match_count"], 1);
        assert!(v["pages"].is_array());
        assert_eq!(v["pages"][0]["page_number"], 1);
    }

    #[test]
    fn find_text_json_no_matches() {
        let doc = build_page_doc_with_content(b"BT (Hello) Tj ET");
        let out = output_of(|w| render_json(w, &find_text_json_value(&doc, "xyz", None)));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["match_count"], 0);
        assert_eq!(v["pages"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn find_text_multiple_matches_on_same_page() {
        let doc = build_page_doc_with_content(b"BT (foo bar foo baz foo) Tj ET");
        let out = output_of(|w| print_find_text(w, &doc, "foo", None));
        assert!(out.contains("3 times"));
        assert!(out.contains("1 page"));
    }

    #[test]
    fn find_text_context_shown() {
        let doc = build_page_doc_with_content(b"BT (The quick brown fox jumps) Tj ET");
        let out = output_of(|w| print_find_text(w, &doc, "brown", None));
        // Snippet should include surrounding context
        assert!(out.contains("quick"));
        assert!(out.contains("fox"));
    }

    #[test]
    fn find_text_empty_pattern() {
        // Empty pattern should return no matches (avoids infinite loop)
        let doc = build_page_doc_with_content(b"BT (Hi) Tj ET");
        let out = output_of(|w| print_find_text(w, &doc, "", None));
        assert!(out.contains("No matches"));
    }

    #[test]
    fn find_text_page_range_filter() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Range(1, 2);
        let out = output_of(|w| print_find_text(w, &doc, "Page", Some(&spec)));
        assert!(out.contains("Page 1:"));
        assert!(out.contains("Page 2:"));
    }

    #[test]
    fn find_text_page_filter_nonexistent_page() {
        let doc = build_page_doc_with_content(b"BT (Hello) Tj ET");
        let spec = PageSpec::Single(99);
        let out = output_of(|w| print_find_text(w, &doc, "Hello", Some(&spec)));
        assert!(out.contains("No matches"));
    }

    #[test]
    fn find_text_json_multiple_pages() {
        let doc = build_two_page_doc();
        let out = output_of(|w| render_json(w, &find_text_json_value(&doc, "Page", None)));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["match_count"].as_u64().unwrap() >= 2);
        assert!(v["pages"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn find_text_case_fold_that_grows_does_not_panic() {
        // bug-0010: İ (2 bytes) folds to "i\u{307}" (3 bytes).
        let doc = build_page_doc_with_content("BT (İaİb) Tj ET".as_bytes());
        let out = output_of(|w| print_find_text(w, &doc, "İ", None));
        assert!(out.contains("Found \"İ\" 2 times on 1 page"), "{out}");
    }

    #[test]
    fn find_text_case_fold_that_shrinks_does_not_panic() {
        // bug-0010: ẞ (3 bytes) folds to ß (2 bytes).
        let doc = build_page_doc_with_content("BT (ẞxẞ) Tj ET".as_bytes());
        let out = output_of(|w| print_find_text(w, &doc, "ẞ", None));
        assert!(out.contains("Found \"ẞ\" 2 times on 1 page"), "{out}");
    }

    #[test]
    fn find_text_snippet_contains_match_after_length_changing_fold() {
        // bug-0010: offsets in the folded text used to be applied to the
        // original, so 50 İs before the match pushed the snippet past it.
        let text = format!("BT ({}needle tail) Tj ET", "İ".repeat(50));
        let doc = build_page_doc_with_content(text.as_bytes());
        let v = find_text_json_value(&doc, "needle", None);
        let snippet = v["pages"][0]["matches"][0].as_str().unwrap();
        assert!(snippet.contains("needle tail"), "{snippet}");
        assert!(snippet.starts_with(&format!("...{}", "İ".repeat(CONTEXT_CHARS))));
    }

    #[test]
    fn fold_case_maps_each_folded_byte_to_its_source_char() {
        let (folded, spans) = fold_case("Aİ");
        assert_eq!(folded, "ai\u{307}");
        assert_eq!(spans, vec![(0, 1), (1, 3), (1, 3), (1, 3)]);
    }

    #[test]
    fn find_text_singular_plural() {
        // 1 time on 1 page
        let doc = build_page_doc_with_content(b"BT (Hello World) Tj ET");
        let out = output_of(|w| print_find_text(w, &doc, "Hello", None));
        assert!(out.contains("1 time on 1 page"));

        // 2 times on 2 pages
        let doc2 = build_two_page_doc();
        let out2 = output_of(|w| print_find_text(w, &doc2, "Page", None));
        assert!(out2.contains("times"));
        assert!(out2.contains("pages"));
    }
}
