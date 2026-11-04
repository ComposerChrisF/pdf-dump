use lopdf::{Document, Object};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;

#[cfg(test)]
use crate::helpers::json_pretty;
use crate::helpers::object_type_label;
use crate::object::{object_header_label, object_to_json, print_object};
use crate::stream::decode_stream;
use crate::summary::summary_detail;
use crate::types::DumpConfig;

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
        if part.is_empty() {
            continue;
        }
        if let Some((left, right)) = part.split_once('=') {
            let left = left.trim();
            let right = right.trim();
            if right.is_empty() {
                return Err(format!("Empty value in '{}'", part));
            }
            if left.eq_ignore_ascii_case("key") {
                conditions.push(SearchCondition::HasKey {
                    key: right.as_bytes().to_vec(),
                });
            } else if left.eq_ignore_ascii_case("value") {
                conditions.push(SearchCondition::ValueContains {
                    text: right.to_string(),
                });
            } else if left.eq_ignore_ascii_case("stream") {
                conditions.push(SearchCondition::StreamContains {
                    text: right.to_string(),
                });
            } else if left.eq_ignore_ascii_case("regex") {
                let re =
                    Regex::new(right).map_err(|e| format!("Invalid regex '{}': {}", right, e))?;
                conditions.push(SearchCondition::RegexMatch { pattern: re });
            } else {
                conditions.push(SearchCondition::KeyEquals {
                    key: left.as_bytes().to_vec(),
                    value: right.as_bytes().to_vec(),
                });
            }
        } else {
            return Err(format!(
                "Invalid condition '{}'. Expected Key=Value, key=Key, or value=Text",
                part
            ));
        }
    }
    if conditions.is_empty() {
        return Err("Empty search expression".to_string());
    }
    Ok(conditions)
}

/// What a text string says, for matching beside its raw bytes: a string that cannot be decoded has
/// only its bytes to match.
fn decoded_text(bytes: &[u8]) -> Option<String> {
    let decoded = crate::text_string::decode_text_string(bytes);
    decoded.decoded.then_some(decoded.text)
}

/// Lowercased a character at a time.  `str::to_lowercase` also applies context rules, under which a
/// final sigma differs from the same letter inside a longer string.
fn folded(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

pub(crate) fn object_matches(doc: &Document, obj: &Object, conditions: &[SearchCondition]) -> bool {
    let dict = match obj {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return false,
    };

    // Lazily decode stream content once for conditions that need it
    let needs_stream = conditions.iter().any(|c| {
        matches!(
            c,
            SearchCondition::StreamContains { .. } | SearchCondition::RegexMatch { .. }
        )
    });
    let decoded_content = if needs_stream {
        if let Object::Stream(stream) = obj {
            let (decoded, _) = decode_stream(doc, stream);
            Some(decoded)
        } else {
            None
        }
    } else {
        None
    };

    conditions.iter().all(|cond| match cond {
        SearchCondition::KeyEquals { key, value } => dict.get(key).ok().is_some_and(|v| match v {
            Object::Name(n) => n.eq_ignore_ascii_case(value),
            Object::String(bytes, _) => {
                let v_lower = value.to_ascii_lowercase();
                bytes.to_ascii_lowercase() == v_lower
                    || decoded_text(bytes).is_some_and(|text| {
                        folded(&text) == folded(&String::from_utf8_lossy(value))
                    })
            }
            _ => false,
        }),
        SearchCondition::HasKey { key } => dict.get(key).is_ok(),
        SearchCondition::ValueContains { text } => {
            let needle = text.as_bytes();
            let in_bytes = |haystack: &[u8]| {
                haystack
                    .windows(needle.len())
                    .any(|w| w.eq_ignore_ascii_case(needle))
            };
            dict.iter().any(|(_, v)| match v {
                Object::Name(n) => in_bytes(n),
                Object::String(bytes, _) => {
                    in_bytes(bytes)
                        || decoded_text(bytes)
                            .is_some_and(|decoded| folded(&decoded).contains(&folded(text)))
                }
                _ => false,
            })
        }
        SearchCondition::StreamContains { text } => {
            if let Some(ref decoded) = decoded_content {
                let needle = text.as_bytes();
                decoded
                    .windows(needle.len())
                    .any(|w| w.eq_ignore_ascii_case(needle))
            } else {
                false
            }
        }
        SearchCondition::RegexMatch { pattern } => {
            // Match against dict key names, Name values, String values
            let key_or_value_match = dict.iter().any(|(k, v)| {
                if pattern.is_match(&String::from_utf8_lossy(k)) {
                    return true;
                }
                match v {
                    Object::Name(n) => pattern.is_match(&String::from_utf8_lossy(n)),
                    Object::String(bytes, _) => {
                        pattern.is_match(&String::from_utf8_lossy(bytes))
                            || decoded_text(bytes).is_some_and(|s| pattern.is_match(&s))
                    }
                    _ => false,
                }
            });
            if key_or_value_match {
                return true;
            }
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

pub(crate) fn search_objects(
    writer: &mut impl Write,
    doc: &Document,
    conditions: &[SearchCondition],
    config: &DumpConfig,
    summary_mode: bool,
) {
    let mut count = 0;

    if summary_mode {
        wln!(
            writer,
            "  {:>4}  {:>3}  {:<13} {:<14} Detail",
            "Obj#",
            "Gen",
            "Kind",
            "/Type"
        );
    }

    for (&(obj_num, generation), object) in &doc.objects {
        if object_matches(doc, object, conditions) {
            count += 1;
            if summary_mode {
                let kind = object.enum_variant();
                let type_label = object_type_label(object);
                let detail = summary_detail(object);
                wln!(
                    writer,
                    "  {:>4}  {:>3}  {:<13} {:<14} {}",
                    obj_num,
                    generation,
                    kind,
                    type_label,
                    detail
                );
            } else {
                wln!(
                    writer,
                    "Object {} {} ({}):",
                    obj_num,
                    generation,
                    object_header_label(object)
                );
                let visited = BTreeSet::new();
                let mut child_refs = BTreeSet::new();
                print_object(
                    writer,
                    object,
                    doc,
                    &visited,
                    1,
                    config,
                    false,
                    &mut child_refs,
                );
                wln!(writer);
            }
        }
    }
    wln!(writer, "Found {} matching objects.", count);
}

#[cfg(test)]
pub(crate) fn search_objects_json(
    writer: &mut impl Write,
    doc: &Document,
    expr: &str,
    conditions: &[SearchCondition],
    config: &DumpConfig,
) {
    wln!(
        writer,
        "{}",
        json_pretty(&search_json_value(doc, expr, conditions, config))
    );
}

pub(crate) fn search_json_value(
    doc: &Document,
    expr: &str,
    conditions: &[SearchCondition],
    config: &DumpConfig,
) -> Value {
    let mut matches = Vec::new();
    for (&(obj_num, generation), object) in &doc.objects {
        if object_matches(doc, object, conditions) {
            matches.push(json!({
                "object_number": obj_num,
                "generation": generation,
                "object": object_to_json(object, doc, config),
            }));
        }
    }
    json!({
        "query": expr,
        "match_count": matches.len(),
        "matches": matches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use regex::Regex;
    use serde_json::Value;

    #[test]
    fn search_matches_what_a_string_says_and_its_bytes() {
        let doc = empty_doc();
        let utf16 = b"\xFE\xFF\x00H\x00e\x00l\x00l\x00o\x20\x18".to_vec();
        let mut dict = Dictionary::new();
        dict.set("Title", Object::String(utf16, StringFormat::Literal));
        let object = Object::Dictionary(dict);
        for expr in [
            "value=hello",
            "value=\u{2018}",
            "Title=hello\u{2018}",
            "regex=H.llo",
        ] {
            let conditions = parse_search_expr(expr).unwrap();
            assert!(object_matches(&doc, &object, &conditions), "{expr}");
        }
        assert!(!object_matches(
            &doc,
            &object,
            &parse_search_expr("value=goodbye").unwrap()
        ));
    }

    /// A search that matched at v0.31.6 still does: these match only in their bytes.
    #[test]
    fn search_still_matches_raw_bytes() {
        let doc = empty_doc();
        let string_object = |bytes: &[u8]| {
            let mut dict = Dictionary::new();
            dict.set(
                "Title",
                Object::String(bytes.to_vec(), StringFormat::Literal),
            );
            Object::Dictionary(dict)
        };
        // Raw UTF-8 with no mark decodes to mojibake, so only its bytes say this.
        let raw_utf8 = string_object("\u{e9}".as_bytes());
        for expr in ["value=\u{e9}", "Title=\u{e9}", "regex=\u{e9}"] {
            let conditions = parse_search_expr(expr).unwrap();
            assert!(object_matches(&doc, &raw_utf8, &conditions), "{expr}");
        }
        // This cannot be decoded at all, and was skipped by regex= before.
        let undecodable = string_object(&[0x01, 0xFF]);
        let conditions = parse_search_expr(r"regex=^\x01").unwrap();
        assert!(object_matches(&doc, &undecodable, &conditions));
    }

    #[test]
    fn search_regex_reads_names_and_keys_that_are_not_utf8() {
        let doc = empty_doc();
        let mut dict = Dictionary::new();
        dict.set(b"Bar\xE9".to_vec(), Object::Name(b"Caf\xE9".to_vec()));
        let object = Object::Dictionary(dict);
        for expr in ["regex=^Bar", "regex=^Caf"] {
            let conditions = parse_search_expr(expr).unwrap();
            assert!(object_matches(&doc, &object, &conditions), "{expr}");
        }
    }

    #[test]
    fn search_folds_case_a_letter_at_a_time() {
        let doc = empty_doc();
        let utf16: Vec<u8> = [0xFEFFu16, 0x0391, 0x03A3, 0x0392]
            .iter()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        let mut dict = Dictionary::new();
        dict.set("Key", Object::String(utf16, StringFormat::Literal));
        let object = Object::Dictionary(dict);
        let conditions = parse_search_expr("value=\u{3b1}\u{3a3}").unwrap();
        assert!(object_matches(&doc, &object, &conditions));
    }

    #[test]
    fn parse_search_expr_key_value() {
        let conds = parse_search_expr("Type=Font").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(
            matches!(&conds[0], SearchCondition::KeyEquals { key, value } if key == b"Type" && value == b"Font")
        );
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
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"Font".to_vec(),
        }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_key_value_case_insensitive() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"font".to_vec(),
        }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_has_key() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Integer(0));
        let conds = vec![SearchCondition::HasKey {
            key: b"MediaBox".to_vec(),
        }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_has_key_missing() {
        let dict = Dictionary::new();
        let conds = vec![SearchCondition::HasKey {
            key: b"MediaBox".to_vec(),
        }];
        assert!(!object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_value_contains() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let conds = vec![SearchCondition::ValueContains {
            text: "helvet".to_string(),
        }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_non_dict_returns_false() {
        let conds = vec![SearchCondition::HasKey {
            key: b"Type".to_vec(),
        }];
        assert!(!object_matches(&empty_doc(), &Object::Integer(42), &conds));
    }

    #[test]
    fn object_matches_stream() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, vec![]);
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"XObject".to_vec(),
        }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Stream(stream),
            &conds
        ));
    }

    #[test]
    fn object_matches_and_logic() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        let conds = vec![
            SearchCondition::KeyEquals {
                key: b"Type".to_vec(),
                value: b"Font".to_vec(),
            },
            SearchCondition::KeyEquals {
                key: b"Subtype".to_vec(),
                value: b"Type1".to_vec(),
            },
        ];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_and_logic_partial_fail() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![
            SearchCondition::KeyEquals {
                key: b"Type".to_vec(),
                value: b"Font".to_vec(),
            },
            SearchCondition::KeyEquals {
                key: b"Subtype".to_vec(),
                value: b"Type1".to_vec(),
            },
        ];
        assert!(!object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn search_objects_finds_match() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"Font".to_vec(),
        }];
        let config = default_config();
        let out = output_of(|w| search_objects(w, &doc, &conds, &config, false));
        assert!(
            out.contains("Object 1 0 (Dictionary, /Font):"),
            "got: {}",
            out
        );
        assert!(out.contains("Found 1 matching objects."));
    }

    #[test]
    fn search_objects_no_match() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"Font".to_vec(),
        }];
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
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"Font".to_vec(),
        }];
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
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"Font".to_vec(),
        }];
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
        dict.set(
            "Title",
            Object::String(b"MyTitle".to_vec(), StringFormat::Literal),
        );
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Title".to_vec(),
            value: b"mytitle".to_vec(),
        }];
        assert!(
            object_matches(&empty_doc(), &Object::Dictionary(dict), &conds),
            "String matching should be case-insensitive"
        );
    }

    #[test]
    fn object_matches_key_equals_non_matching_type() {
        // KeyEquals where value is neither Name nor String → should not match
        let mut dict = Dictionary::new();
        dict.set("Count", Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Count".to_vec(),
            value: b"42".to_vec(),
        }];
        assert!(
            !object_matches(&empty_doc(), &Object::Dictionary(dict), &conds),
            "Integer values should not match KeyEquals"
        );
    }

    #[test]
    fn object_matches_value_contains_string_object() {
        // ValueContains should match Object::String values too
        let mut dict = Dictionary::new();
        dict.set(
            "Title",
            Object::String(b"Hello World".to_vec(), StringFormat::Literal),
        );
        let conds = vec![SearchCondition::ValueContains {
            text: "world".to_string(),
        }];
        assert!(
            object_matches(&empty_doc(), &Object::Dictionary(dict), &conds),
            "ValueContains should match String objects"
        );
    }

    #[test]
    fn object_matches_value_contains_no_match() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        let conds = vec![SearchCondition::ValueContains {
            text: "nonexistent".to_string(),
        }];
        assert!(!object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_value_contains_non_string_values_skipped() {
        // Dict with only Integer values → ValueContains should not match
        let mut dict = Dictionary::new();
        dict.set("Count", Object::Integer(42));
        let conds = vec![SearchCondition::ValueContains {
            text: "42".to_string(),
        }];
        assert!(!object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
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
        assert!(
            matches!(&conds[0], SearchCondition::KeyEquals { key, value } if key == b"Type" && value == b"Font")
        );
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
        let conds = vec![SearchCondition::KeyEquals {
            key: b"Type".to_vec(),
            value: b"Font".to_vec(),
        }];
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
        let conditions = vec![SearchCondition::StreamContains {
            text: "world".to_string(),
        }];
        assert!(object_matches(
            &empty_doc(),
            doc.get_object((1, 0)).unwrap(),
            &conditions
        ));
        assert!(!object_matches(
            &empty_doc(),
            doc.get_object((2, 0)).unwrap(),
            &conditions
        ));
    }

    #[test]
    fn search_stream_contains_case_insensitive() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"FlateDecode Content".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![SearchCondition::StreamContains {
            text: "flatedecode".to_string(),
        }];
        assert!(object_matches(
            &empty_doc(),
            doc.get_object((1, 0)).unwrap(),
            &conditions
        ));
    }

    #[test]
    fn search_stream_no_match() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"ABC".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![SearchCondition::StreamContains {
            text: "XYZ".to_string(),
        }];
        assert!(!object_matches(
            &empty_doc(),
            doc.get_object((1, 0)).unwrap(),
            &conditions
        ));
    }

    #[test]
    fn parse_search_stream_condition() {
        let conditions = parse_search_expr("stream=Hello").unwrap();
        assert_eq!(conditions.len(), 1);
        assert!(
            matches!(&conditions[0], SearchCondition::StreamContains { text } if text == "Hello")
        );
    }

    #[test]
    fn search_stream_and_key_combined() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, b"q 1 0 0 1 0 0 cm Q".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![
            SearchCondition::KeyEquals {
                key: b"Type".to_vec(),
                value: b"XObject".to_vec(),
            },
            SearchCondition::StreamContains {
                text: "cm".to_string(),
            },
        ];
        assert!(object_matches(
            &empty_doc(),
            doc.get_object((1, 0)).unwrap(),
            &conditions
        ));
    }

    #[test]
    fn search_stream_on_dict_returns_false() {
        let mut doc = Document::new();
        doc.objects
            .insert((1, 0), Object::Dictionary(Dictionary::new()));
        let conditions = vec![SearchCondition::StreamContains {
            text: "anything".to_string(),
        }];
        assert!(!object_matches(
            &empty_doc(),
            doc.get_object((1, 0)).unwrap(),
            &conditions
        ));
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
            Err(e) => assert!(
                e.contains("Invalid regex"),
                "Error should mention invalid regex: {}",
                e
            ),
            Ok(_) => panic!("Expected error for invalid regex"),
        }
    }

    #[test]
    fn object_matches_regex_name_value() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica-Bold".to_vec()));
        let re = Regex::new("Helvetica").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_regex_key_name() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Integer(0));
        let re = Regex::new("^Media").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_regex_string_value() {
        let mut dict = Dictionary::new();
        dict.set(
            "Title",
            Object::String(b"Chapter 5 - Results".to_vec(), StringFormat::Literal),
        );
        let re = Regex::new(r"Chapter \d+").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_regex_no_match() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        let re = Regex::new("Font").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(!object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_regex_combined_with_other() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let re = Regex::new("Helv").unwrap();
        let conds = vec![
            SearchCondition::KeyEquals {
                key: b"Type".to_vec(),
                value: b"Font".to_vec(),
            },
            SearchCondition::RegexMatch { pattern: re },
        ];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }

    #[test]
    fn object_matches_regex_stream_content() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, b"BT /F1 12 Tf (Hello World) Tj ET".to_vec());
        let re = Regex::new("Hello").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Stream(stream),
            &conds
        ));
    }

    #[test]
    fn object_matches_regex_case_flag() {
        // Use (?i) for case-insensitive matching
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"HELVETICA".to_vec()));
        let re = Regex::new("(?i)helvetica").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(
            &empty_doc(),
            &Object::Dictionary(dict),
            &conds
        ));
    }
}
