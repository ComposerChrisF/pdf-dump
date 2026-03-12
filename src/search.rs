use lopdf::{Document, Object};
use regex::Regex;
use serde_json::json;
use std::collections::BTreeSet;
use std::io::Write;

use crate::types::DumpConfig;
use crate::stream::decode_stream;
use crate::helpers::{object_type_label, json_pretty};
use crate::object::{object_to_json, print_object, object_header_label};
use crate::summary::summary_detail;

pub(crate) enum SearchCondition {
    KeyEquals { key: Vec<u8>, value: Vec<u8> },
    HasKey { key: Vec<u8> },
    ValueContains { text: String },
    StreamContains { text: String },
    RegexMatch { pattern: Regex },
}

pub(crate) fn parse_search_expr(expr: &str) -> Result<Vec<SearchCondition>, String> {
    let mut conditions = Vec::new();
    for part in expr.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        if let Some((left, right)) = part.split_once('=') {
            let left = left.trim();
            let right = right.trim();
            if right.is_empty() {
                return Err(format!("Empty value in '{}'", part));
            }
            if left.eq_ignore_ascii_case("key") {
                conditions.push(SearchCondition::HasKey { key: right.as_bytes().to_vec() });
            } else if left.eq_ignore_ascii_case("value") {
                conditions.push(SearchCondition::ValueContains { text: right.to_string() });
            } else if left.eq_ignore_ascii_case("stream") {
                conditions.push(SearchCondition::StreamContains { text: right.to_string() });
            } else if left.eq_ignore_ascii_case("regex") {
                let re = Regex::new(right)
                    .map_err(|e| format!("Invalid regex '{}': {}", right, e))?;
                conditions.push(SearchCondition::RegexMatch { pattern: re });
            } else {
                conditions.push(SearchCondition::KeyEquals {
                    key: left.as_bytes().to_vec(),
                    value: right.as_bytes().to_vec(),
                });
            }
        } else {
            return Err(format!("Invalid condition '{}'. Expected Key=Value, key=Key, or value=Text", part));
        }
    }
    if conditions.is_empty() {
        return Err("Empty search expression".to_string());
    }
    Ok(conditions)
}

pub(crate) fn object_matches(obj: &Object, conditions: &[SearchCondition]) -> bool {
    let dict = match obj {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return false,
    };

    // Lazily decode stream content once for conditions that need it
    let needs_stream = conditions.iter().any(|c| matches!(c, SearchCondition::StreamContains { .. } | SearchCondition::RegexMatch { .. }));
    let decoded_content = if needs_stream {
        if let Object::Stream(stream) = obj {
            let (decoded, _) = decode_stream(stream);
            Some(decoded)
        } else {
            None
        }
    } else {
        None
    };

    conditions.iter().all(|cond| match cond {
        SearchCondition::KeyEquals { key, value } => {
            dict.get(key).ok().is_some_and(|v| {
                match v {
                    Object::Name(n) => n.eq_ignore_ascii_case(value),
                    Object::String(bytes, _) => {
                        let v_lower = value.to_ascii_lowercase();
                        bytes.to_ascii_lowercase() == v_lower
                    }
                    _ => false,
                }
            })
        }
        SearchCondition::HasKey { key } => dict.get(key).is_ok(),
        SearchCondition::ValueContains { text } => {
            let needle = text.to_lowercase();
            let needle_bytes = needle.as_bytes();
            dict.iter().any(|(_, v)| {
                let haystack: &[u8] = match v {
                    Object::Name(n) => n,
                    Object::String(bytes, _) => bytes,
                    _ => return false,
                };
                // Case-insensitive byte search: zero-allocation byte-by-byte compare
                haystack.windows(needle_bytes.len()).any(|w|
                    w.iter().zip(needle_bytes).all(|(a, b)| a.to_ascii_lowercase() == *b)
                )
            })
        }
        SearchCondition::StreamContains { text } => {
            if let Some(ref decoded) = decoded_content {
                let needle = text.as_bytes();
                decoded.windows(needle.len()).any(|w|
                    w.eq_ignore_ascii_case(needle)
                )
            } else {
                false
            }
        }
        SearchCondition::RegexMatch { pattern } => {
            // Match against dict key names, Name values, String values
            let key_or_value_match = dict.iter().any(|(k, v)| {
                if let Ok(key_str) = std::str::from_utf8(k)
                    && pattern.is_match(key_str) { return true; }
                match v {
                    Object::Name(n) => std::str::from_utf8(n).is_ok_and(|s| pattern.is_match(s)),
                    Object::String(bytes, _) => std::str::from_utf8(bytes).is_ok_and(|s| pattern.is_match(s)),
                    _ => false,
                }
            });
            if key_or_value_match { return true; }
            // Also match decoded stream content
            if let Some(ref decoded) = decoded_content {
                let content_str = String::from_utf8_lossy(decoded);
                pattern.is_match(&content_str)
            } else {
                false
            }
        }
    })
}

pub(crate) fn search_objects(writer: &mut impl Write, doc: &Document, conditions: &[SearchCondition], config: &DumpConfig, summary_mode: bool) {
    let mut count = 0;

    if summary_mode {
        wln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} Detail", "Obj#", "Gen", "Kind", "/Type");
    }

    for (&(obj_num, generation), object) in &doc.objects {
        if object_matches(object, conditions) {
            count += 1;
            if summary_mode {
                let kind = object.enum_variant();
                let type_label = object_type_label(object);
                let detail = summary_detail(object);
                wln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} {}", obj_num, generation, kind, type_label, detail);
            } else {
                wln!(writer, "Object {} {} ({}):", obj_num, generation, object_header_label(object));
                let visited = BTreeSet::new();
                let mut child_refs = BTreeSet::new();
                print_object(writer, object, doc, &visited, 1, config, false, &mut child_refs);
                wln!(writer);
            }
        }
    }
    wln!(writer, "Found {} matching objects.", count);
}

pub(crate) fn search_objects_json(writer: &mut impl Write, doc: &Document, expr: &str, conditions: &[SearchCondition], config: &DumpConfig) {
    let mut matches = Vec::new();
    for (&(obj_num, generation), object) in &doc.objects {
        if object_matches(object, conditions) {
            matches.push(json!({
                "object_number": obj_num,
                "generation": generation,
                "object": object_to_json(object, doc, config),
            }));
        }
    }
    let output = json!({
        "query": expr,
        "match_count": matches.len(),
        "matches": matches,
    });
    wln!(writer, "{}", json_pretty(&output));
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;
    use regex::Regex;

    #[test]
    fn parse_search_expr_key_value() {
        let conds = parse_search_expr("Type=Font").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::KeyEquals { key, value } if key == b"Type" && value == b"Font"));
    }

    #[test]
    fn parse_search_expr_has_key() {
        let conds = parse_search_expr("key=MediaBox").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::HasKey { key } if key == b"MediaBox"));
    }

    #[test]
    fn parse_search_expr_value_contains() {
        let conds = parse_search_expr("value=Hello").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::ValueContains { text } if text == "Hello"));
    }

    #[test]
    fn parse_search_expr_multiple() {
        let conds = parse_search_expr("Type=Font,Subtype=Type1").unwrap();
        assert_eq!(conds.len(), 2);
    }

    #[test]
    fn parse_search_expr_empty_fails() {
        assert!(parse_search_expr("").is_err());
    }

    #[test]
    fn parse_search_expr_no_equals_fails() {
        assert!(parse_search_expr("badexpr").is_err());
    }

    #[test]
    fn parse_search_expr_empty_value_fails() {
        assert!(parse_search_expr("Type=").is_err());
    }

    #[test]
    fn object_matches_key_value() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_key_value_case_insensitive() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"font".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_has_key() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Integer(0));
        let conds = vec![SearchCondition::HasKey { key: b"MediaBox".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_has_key_missing() {
        let dict = Dictionary::new();
        let conds = vec![SearchCondition::HasKey { key: b"MediaBox".to_vec() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_value_contains() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let conds = vec![SearchCondition::ValueContains { text: "helvet".to_string() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_non_dict_returns_false() {
        let conds = vec![SearchCondition::HasKey { key: b"Type".to_vec() }];
        assert!(!object_matches(&Object::Integer(42), &conds));
    }

    #[test]
    fn object_matches_stream() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, vec![]);
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"XObject".to_vec() }];
        assert!(object_matches(&Object::Stream(stream), &conds));
    }

    #[test]
    fn object_matches_and_logic() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        let conds = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() },
            SearchCondition::KeyEquals { key: b"Subtype".to_vec(), value: b"Type1".to_vec() },
        ];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_and_logic_partial_fail() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() },
            SearchCondition::KeyEquals { key: b"Subtype".to_vec(), value: b"Type1".to_vec() },
        ];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn search_objects_finds_match() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = default_config();
        let out = output_of(|w| search_objects(w, &doc, &conds, &config, false));
        assert!(out.contains("Object 1 0 (Dictionary, /Font):"), "got: {}", out);
        assert!(out.contains("Found 1 matching objects."));
    }

    #[test]
    fn search_objects_no_match() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = default_config();
        let out = output_of(|w| search_objects(w, &doc, &conds, &config, false));
        assert!(out.contains("Found 0 matching objects."));
    }

    #[test]
    fn search_objects_summary_mode() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = default_config();
        let out = output_of(|w| search_objects(w, &doc, &conds, &config, true));
        assert!(out.contains("Obj#"));
        assert!(out.contains("Found 1 matching objects."));
    }

    #[test]
    fn search_objects_json_produces_valid_json() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = json_config();
        let out = output_of(|w| search_objects_json(w, &doc, "Type=Font", &conds, &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["match_count"], 1);
        assert_eq!(parsed["query"], "Type=Font");
    }

    #[test]
    fn object_matches_key_equals_string_value() {
        // KeyEquals with Object::String value (not Name)
        let mut dict = Dictionary::new();
        dict.set("Title", Object::String(b"MyTitle".to_vec(), StringFormat::Literal));
        let conds = vec![SearchCondition::KeyEquals { key: b"Title".to_vec(), value: b"mytitle".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds), "String matching should be case-insensitive");
    }

    #[test]
    fn object_matches_key_equals_non_matching_type() {
        // KeyEquals where value is neither Name nor String → should not match
        let mut dict = Dictionary::new();
        dict.set("Count", Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals { key: b"Count".to_vec(), value: b"42".to_vec() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds), "Integer values should not match KeyEquals");
    }

    #[test]
    fn object_matches_value_contains_string_object() {
        // ValueContains should match Object::String values too
        let mut dict = Dictionary::new();
        dict.set("Title", Object::String(b"Hello World".to_vec(), StringFormat::Literal));
        let conds = vec![SearchCondition::ValueContains { text: "world".to_string() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds), "ValueContains should match String objects");
    }

    #[test]
    fn object_matches_value_contains_no_match() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        let conds = vec![SearchCondition::ValueContains { text: "nonexistent".to_string() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_value_contains_non_string_values_skipped() {
        // Dict with only Integer values → ValueContains should not match
        let mut dict = Dictionary::new();
        dict.set("Count", Object::Integer(42));
        let conds = vec![SearchCondition::ValueContains { text: "42".to_string() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn parse_search_expr_key_case_insensitive() {
        // "KEY=..." should be treated as HasKey
        let conds = parse_search_expr("KEY=MediaBox").unwrap();
        assert!(matches!(&conds[0], SearchCondition::HasKey { key } if key == b"MediaBox"));
    }

    #[test]
    fn parse_search_expr_value_case_insensitive() {
        // "VALUE=..." should be treated as ValueContains
        let conds = parse_search_expr("VALUE=Hello").unwrap();
        assert!(matches!(&conds[0], SearchCondition::ValueContains { text } if text == "Hello"));
    }

    #[test]
    fn parse_search_expr_whitespace_trimmed() {
        let conds = parse_search_expr("  Type = Font  ").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::KeyEquals { key, value } if key == b"Type" && value == b"Font"));
    }

    #[test]
    fn parse_search_expr_multiple_with_whitespace() {
        let conds = parse_search_expr("Type=Font , Subtype=Type1").unwrap();
        assert_eq!(conds.len(), 2);
    }

    #[test]
    fn parse_search_expr_empty_parts_skipped() {
        // Trailing comma should be OK
        let conds = parse_search_expr("Type=Font,").unwrap();
        assert_eq!(conds.len(), 1);
    }

    #[test]
    fn parse_search_expr_only_commas_fails() {
        assert!(parse_search_expr(",,,").is_err());
    }

    #[test]
    fn search_objects_json_no_matches() {
        let doc = Document::new();
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = json_config();
        let out = output_of(|w| search_objects_json(w, &doc, "Type=Font", &conds, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["match_count"], 0);
        assert!(parsed["matches"].as_array().unwrap().is_empty());
    }

    #[test]
    fn search_stream_contains_matches() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"Hello World stream content".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        doc.objects.insert((2, 0), Object::Integer(42));
        let conditions = vec![SearchCondition::StreamContains { text: "world".to_string() }];
        assert!(object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
        assert!(!object_matches(doc.get_object((2, 0)).unwrap(), &conditions));
    }

    #[test]
    fn search_stream_contains_case_insensitive() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"FlateDecode Content".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![SearchCondition::StreamContains { text: "flatedecode".to_string() }];
        assert!(object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    #[test]
    fn search_stream_no_match() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"ABC".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![SearchCondition::StreamContains { text: "XYZ".to_string() }];
        assert!(!object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    #[test]
    fn parse_search_stream_condition() {
        let conditions = parse_search_expr("stream=Hello").unwrap();
        assert_eq!(conditions.len(), 1);
        assert!(matches!(&conditions[0], SearchCondition::StreamContains { text } if text == "Hello"));
    }

    #[test]
    fn search_stream_and_key_combined() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, b"q 1 0 0 1 0 0 cm Q".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"XObject".to_vec() },
            SearchCondition::StreamContains { text: "cm".to_string() },
        ];
        assert!(object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    #[test]
    fn search_stream_on_dict_returns_false() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Dictionary(Dictionary::new()));
        let conditions = vec![SearchCondition::StreamContains { text: "anything".to_string() }];
        assert!(!object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    #[test]
    fn parse_search_expr_regex() {
        let conds = parse_search_expr("regex=Font\\d+").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::RegexMatch { .. }));
    }

    #[test]
    fn parse_search_expr_regex_case_insensitive() {
        let conds = parse_search_expr("REGEX=test").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::RegexMatch { .. }));
    }

    #[test]
    fn parse_search_expr_regex_invalid() {
        let result = parse_search_expr("regex=[invalid");
        match result {
            Err(e) => assert!(e.contains("Invalid regex"), "Error should mention invalid regex: {}", e),
            Ok(_) => panic!("Expected error for invalid regex"),
        }
    }

    #[test]
    fn object_matches_regex_name_value() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica-Bold".to_vec()));
        let re = Regex::new("Helvetica").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_key_name() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Integer(0));
        let re = Regex::new("^Media").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_string_value() {
        let mut dict = Dictionary::new();
        dict.set("Title", Object::String(b"Chapter 5 - Results".to_vec(), StringFormat::Literal));
        let re = Regex::new(r"Chapter \d+").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_no_match() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        let re = Regex::new("Font").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_combined_with_other() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let re = Regex::new("Helv").unwrap();
        let conds = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() },
            SearchCondition::RegexMatch { pattern: re },
        ];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_stream_content() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, b"BT /F1 12 Tf (Hello World) Tj ET".to_vec());
        let re = Regex::new("Hello").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Stream(stream), &conds));
    }

    #[test]
    fn object_matches_regex_case_flag() {
        // Use (?i) for case-insensitive matching
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"HELVETICA".to_vec()));
        let re = Regex::new("(?i)helvetica").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

}
