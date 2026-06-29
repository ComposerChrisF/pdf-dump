//! Lenient recovery of content streams that `lopdf` parsed as bare
//! dictionaries because the PDF declared an incorrect `/Length`.
//!
//! lopdf trusts `/Length`: it reads that many bytes after the `stream` keyword,
//! and when `endstream` is not where the length says it should be, it gives up
//! on the stream and stores the object as a plain `Object::Dictionary` — the
//! body is silently discarded. Real writers get `/Length` wrong (e.g. a tool
//! that rewrites a content stream without updating it), so the page text simply
//! vanishes with no error.
//!
//! This module re-reads the original file bytes, relocates each such object by
//! its cross-reference offset, extracts the true body delimited by the
//! `stream`/`endstream` keywords, and promotes the object back to a stream.
//! Every recovery is surfaced through a loud stderr banner that states exactly
//! which object was malformed and how (declared vs. actual length, file offset).
//!
//! It is deliberately conservative: an object is only touched when its raw bytes
//! really are a stream, so a genuine dictionary that merely happens to carry a
//! `/Length` key is left untouched.

use lopdf::xref::XrefEntry;
use lopdf::{Document, Object, ObjectId, Stream};
use serde_json::{Value, json};

/// One recovered stream — enough to explain (loudly) what was malformed.
pub(crate) struct StreamRecovery {
    pub id: ObjectId,
    /// File offset (from the xref) where the object begins.
    pub offset: usize,
    /// The bogus `/Length` the dictionary declared (`None` if it had none).
    pub declared_length: Option<i64>,
    /// The true body length recovered by scanning to `endstream`.
    pub recovered_length: usize,
}

/// Whether the document has any object that looks like a stream lopdf
/// mis-parsed as a dictionary (a `Dictionary` carrying a `/Length`). A cheap
/// pre-check so callers can skip re-reading the file for well-formed PDFs.
pub(crate) fn has_candidates(doc: &Document) -> bool {
    doc.objects.values().any(is_candidate_dict)
}

fn is_candidate_dict(obj: &Object) -> bool {
    matches!(obj, Object::Dictionary(d) if d.has(b"Length"))
}

/// Read-only scan: find every stream lopdf dropped to a bare dictionary
/// because of a wrong `/Length`, returning one record per recoverable stream
/// paired with its true body bytes. Does NOT mutate `doc` — both the repairing
/// path (`recover_malformed_streams`) and the strict detect-only path
/// (`detect_malformed_streams`) share this scan.
fn scan_malformed_streams(doc: &Document, raw: &[u8]) -> Vec<(StreamRecovery, Vec<u8>)> {
    let candidates: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter(|(_, o)| is_candidate_dict(o))
        .map(|(id, _)| *id)
        .collect();

    let mut found = Vec::new();
    for id in candidates {
        // Only regular (uncompressed) objects have a file offset we can scan to;
        // objects packed into an object stream can't be recovered from raw bytes.
        let offset = match doc.reference_table.entries.get(&id.0) {
            Some(XrefEntry::Normal { offset, .. }) => *offset as usize,
            _ => continue,
        };
        let Some(body) = extract_stream_body(raw, offset) else {
            // No stream actually lives here — a genuine dictionary. Leave it be.
            continue;
        };

        let Some(Object::Dictionary(dict)) = doc.objects.get(&id) else {
            continue;
        };
        let declared_length = dict.get(b"Length").ok().and_then(|v| v.as_i64().ok());
        let recovered_length = body.len();
        found.push((
            StreamRecovery {
                id,
                offset,
                declared_length,
                recovered_length,
            },
            body,
        ));
    }
    found
}

/// Scan `raw` (the original file bytes) for streams lopdf dropped to bare
/// dictionaries because of a wrong `/Length`, promote each back to
/// `Object::Stream` in `doc`, and return one record per recovery.
pub(crate) fn recover_malformed_streams(doc: &mut Document, raw: &[u8]) -> Vec<StreamRecovery> {
    let found = scan_malformed_streams(doc, raw);
    let mut recoveries = Vec::with_capacity(found.len());
    for (rec, body) in found {
        // The dict is still present (the scan only read it). Clone it and
        // rebuild as a stream: `Stream::new` resets `/Length` to the real body
        // length, so every downstream consumer (text, --list, --object,
        // extract-stream, …) sees a correct stream. Any `/Filter` in the dict
        // is preserved, so the recovered (still-encoded) body decodes later.
        if let Some(Object::Dictionary(dict)) = doc.objects.get(&rec.id) {
            let dict = dict.clone();
            doc.objects
                .insert(rec.id, Object::Stream(Stream::new(dict, body)));
            recoveries.push(rec);
        }
    }
    recoveries
}

/// Detect-only counterpart of [`recover_malformed_streams`]: find the same
/// malformed streams but leave `doc` untouched. Used by `--strict`, where a
/// spec-conformant reader reports the malformation and refuses to silently
/// repair it.
pub(crate) fn detect_malformed_streams(doc: &Document, raw: &[u8]) -> Vec<StreamRecovery> {
    scan_malformed_streams(doc, raw)
        .into_iter()
        .map(|(rec, _)| rec)
        .collect()
}

/// Machine-readable record of what was recovered (or, under `--strict`,
/// detected-but-not-recovered), surfaced as a top-level `recovery` object in
/// `--json` output so consumers never mistake repaired output for the original
/// document. `repaired` is true on the default path and false under `--strict`.
pub(crate) fn recovery_json_value(
    recoveries: &[StreamRecovery],
    repaired: bool,
    strict: bool,
) -> Value {
    let streams: Vec<Value> = recoveries
        .iter()
        .map(|r| {
            json!({
                "object": r.id.0,
                "generation": r.id.1,
                "file_offset": r.offset,
                "declared_length": r.declared_length,
                "actual_length": r.recovered_length,
            })
        })
        .collect();
    json!({
        "repaired": repaired,
        "strict": strict,
        "count": recoveries.len(),
        "streams": streams,
    })
}

/// Extract a stream body from `raw` starting at an object's file `offset`.
/// Returns the bytes between the `stream` and `endstream` keywords (with the
/// single conventional EOL stripped on each side), or `None` when no stream is
/// present there. The opening-keyword search is bounded to this object (up to
/// its `endobj`) so a genuine dictionary cannot accidentally capture the next
/// object's stream.
fn extract_stream_body(raw: &[u8], offset: usize) -> Option<Vec<u8>> {
    if offset >= raw.len() {
        return None;
    }
    let region = &raw[offset..];
    let limit = find_subslice(region, b"endobj").unwrap_or(region.len());
    let head = &region[..limit];

    let stream_kw = b"stream";
    let mut from = 0usize;
    let body_start = loop {
        let pos = from + find_subslice(&head[from..], stream_kw)?;
        // A real stream marker is a standalone token (`>>stream`, `>> stream`,
        // or whitespace-prefixed), not the tail of `endstream`, and is followed
        // by CRLF or LF (PDF 32000-1 §7.3.8.1).
        let prev_ok = pos == 0 || head[pos - 1] == b'>' || head[pos - 1].is_ascii_whitespace();
        let not_endstream = !(pos >= 3 && &head[pos - 3..pos] == b"end");
        let mut b = pos + stream_kw.len();
        if head.get(b) == Some(&b'\r') {
            b += 1;
        }
        let has_lf = head.get(b) == Some(&b'\n');
        if prev_ok && not_endstream && has_lf {
            break b + 1; // body begins right after the EOL
        }
        from = pos + stream_kw.len();
    };

    let mut body_end = body_start + find_subslice(&region[body_start..], b"endstream")?;
    // Strip the single EOL that conventionally precedes `endstream`.
    if body_end > body_start && region[body_end - 1] == b'\n' {
        body_end -= 1;
        if body_end > body_start && region[body_end - 1] == b'\r' {
            body_end -= 1;
        }
    } else if body_end > body_start && region[body_end - 1] == b'\r' {
        body_end -= 1;
    }
    Some(region[body_start..body_end].to_vec())
}

/// First index of `needle` within `haystack`, or `None`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// One indented detail line per recovered/detected stream, shared by both
/// banners: declared vs. actual `/Length` and the file offset.
fn detail_lines(recoveries: &[StreamRecovery]) -> String {
    let mut s = String::new();
    for r in recoveries {
        let declared = match r.declared_length {
            Some(n) => n.to_string(),
            None => "absent".to_string(),
        };
        s.push_str(&format!(
            "    - object {} {} (file offset {}): declared /Length {}, \
             actual body {} bytes\n",
            r.id.0, r.id.1, r.offset, declared, r.recovered_length
        ));
    }
    s
}

/// Build the loud stderr banner describing the recovered streams (default,
/// repairing path), or `None` when nothing was recovered (the silent happy
/// path).
pub(crate) fn recovery_banner(recoveries: &[StreamRecovery]) -> Option<String> {
    if recoveries.is_empty() {
        return None;
    }
    let bar = "=".repeat(60);
    let rule = "-".repeat(60);
    let mut s = String::new();
    s.push_str(&bar);
    s.push('\n');
    s.push_str("[WARN] MALFORMED PDF: incorrect stream /Length(s) recovered\n");
    s.push_str(&rule);
    s.push('\n');
    s.push_str(&format!(
        "  {} content stream(s) declared a /Length that does not match the\n",
        recoveries.len()
    ));
    s.push_str("  bytes between their `stream` and `endstream` keywords. A strict\n");
    s.push_str("  reader trusts /Length, fails to find `endstream`, and DROPS the\n");
    s.push_str("  stream body — so the affected content (e.g. page text) silently\n");
    s.push_str("  disappears. The writer that produced this PDF emitted a wrong\n");
    s.push_str("  /Length. pdf-dump recovered each body by scanning to `endstream`:\n");
    s.push_str(&detail_lines(recoveries));
    s.push_str(&bar);
    s.push('\n');
    Some(s)
}

/// Build the loud stderr banner for `--strict`: the same malformation was
/// detected, but pdf-dump refused to repair it, so the affected content is
/// missing from the output and the tool will exit 3. `None` when nothing was
/// detected.
pub(crate) fn strict_banner(recoveries: &[StreamRecovery]) -> Option<String> {
    if recoveries.is_empty() {
        return None;
    }
    let bar = "=".repeat(60);
    let rule = "-".repeat(60);
    let mut s = String::new();
    s.push_str(&bar);
    s.push('\n');
    s.push_str("[ERROR] MALFORMED PDF: incorrect stream /Length(s) detected (--strict)\n");
    s.push_str(&rule);
    s.push('\n');
    s.push_str(&format!(
        "  {} content stream(s) declared a /Length that does not match the\n",
        recoveries.len()
    ));
    s.push_str("  bytes between their `stream` and `endstream` keywords. Because\n");
    s.push_str("  --strict is set, pdf-dump did NOT repair them: the affected content\n");
    s.push_str("  (e.g. page text) is MISSING from this output, exactly as a strict\n");
    s.push_str("  spec-conformant reader would see it. Re-run without --strict to\n");
    s.push_str("  recover each body by scanning to `endstream`. Exit code: 3.\n");
    s.push_str(&detail_lines(recoveries));
    s.push_str(&bar);
    s.push('\n');
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::xref::XrefEntry;
    use lopdf::{Dictionary, Document, Object};
    use pretty_assertions::assert_eq;

    /// Build a doc with a single object that lopdf would have mis-parsed: a
    /// `Dictionary` carrying a (wrong) `/Length`, plus a Normal xref entry at
    /// `offset`. The raw file bytes are supplied separately by each test.
    fn doc_with_misparsed(offset: u32, declared_len: i64) -> Document {
        let mut doc = Document::new();
        let mut d = Dictionary::new();
        d.set("Length", Object::Integer(declared_len));
        doc.objects.insert((3, 0), Object::Dictionary(d));
        doc.reference_table.entries.insert(
            3,
            XrefEntry::Normal {
                offset,
                generation: 0,
            },
        );
        doc
    }

    #[test]
    fn recovers_stream_with_wrong_length() {
        // Declared /Length 2, but the real body is "HELLO WORLD" (11 bytes).
        let raw = b"3 0 obj\n<</Length 2>>stream\nHELLO WORLD\nendstream\nendobj\n";
        let mut doc = doc_with_misparsed(0, 2);

        let recs = recover_malformed_streams(&mut doc, raw);

        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, (3, 0));
        assert_eq!(recs[0].declared_length, Some(2));
        assert_eq!(recs[0].recovered_length, 11);
        match doc.objects.get(&(3, 0)) {
            Some(Object::Stream(s)) => {
                assert_eq!(s.content, b"HELLO WORLD");
                // /Length was corrected to the true body length.
                assert_eq!(s.dict.get(b"Length").unwrap().as_i64().unwrap(), 11);
            }
            other => panic!("expected promoted Stream, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn recovers_with_crlf_line_endings() {
        let raw = b"3 0 obj\r\n<</Length 1>>stream\r\nq Q\r\nendstream\r\nendobj\r\n";
        let mut doc = doc_with_misparsed(0, 1);
        let recs = recover_malformed_streams(&mut doc, raw);
        assert_eq!(recs.len(), 1);
        match doc.objects.get(&(3, 0)) {
            Some(Object::Stream(s)) => assert_eq!(s.content, b"q Q"),
            _ => panic!("expected promoted Stream"),
        }
    }

    #[test]
    fn leaves_genuine_dictionary_untouched() {
        // A real dictionary that has a /Length key but is NOT a stream in the
        // file (no `stream` keyword) must not be promoted.
        let raw = b"3 0 obj\n<</Type /Foo /Length 5>>\nendobj\n";
        let mut doc = doc_with_misparsed(0, 5);

        let recs = recover_malformed_streams(&mut doc, raw);

        assert!(recs.is_empty());
        assert!(matches!(
            doc.objects.get(&(3, 0)),
            Some(Object::Dictionary(_))
        ));
    }

    #[test]
    fn does_not_capture_next_objects_stream() {
        // Object 3 is a genuine dictionary; object 4 (later) is a stream. The
        // bounded search must not reach into object 4's stream.
        let raw =
            b"3 0 obj\n<</Length 5>>\nendobj\n4 0 obj\n<</Length 1>>stream\nXX\nendstream\nendobj\n";
        let mut doc = doc_with_misparsed(0, 5); // offset 0 = object 3
        let recs = recover_malformed_streams(&mut doc, raw);
        assert!(recs.is_empty(), "must not steal object 4's stream");
        assert!(matches!(
            doc.objects.get(&(3, 0)),
            Some(Object::Dictionary(_))
        ));
    }

    #[test]
    fn dictionary_without_length_is_not_a_candidate() {
        let mut doc = Document::new();
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"Foo".to_vec()));
        doc.objects.insert((3, 0), Object::Dictionary(d));
        assert!(!has_candidates(&doc));
        let recs = recover_malformed_streams(&mut doc, b"");
        assert!(recs.is_empty());
    }

    #[test]
    fn skips_candidate_without_xref_offset() {
        // Candidate dict but no Normal xref entry (e.g. it lived in an object
        // stream) → cannot scan raw bytes → skipped, not promoted.
        let raw = b"3 0 obj\n<</Length 2>>stream\nHI\nendstream\nendobj\n";
        let mut doc = Document::new();
        let mut d = Dictionary::new();
        d.set("Length", Object::Integer(2));
        doc.objects.insert((3, 0), Object::Dictionary(d));
        // no reference_table entry inserted
        let recs = recover_malformed_streams(&mut doc, raw);
        assert!(recs.is_empty());
        assert!(matches!(
            doc.objects.get(&(3, 0)),
            Some(Object::Dictionary(_))
        ));
    }

    #[test]
    fn has_candidates_detects_dict_with_length() {
        let doc = doc_with_misparsed(0, 2);
        assert!(has_candidates(&doc));
    }

    #[test]
    fn banner_is_none_when_nothing_recovered() {
        assert!(recovery_banner(&[]).is_none());
    }

    #[test]
    fn banner_states_declared_and_actual_length() {
        let recs = vec![StreamRecovery {
            id: (10, 0),
            offset: 808,
            declared_length: Some(54),
            recovered_length: 58,
        }];
        let banner = recovery_banner(&recs).expect("banner");
        assert!(banner.contains("MALFORMED PDF"));
        assert!(banner.contains("object 10 0"));
        assert!(banner.contains("declared /Length 54"));
        assert!(banner.contains("actual body 58 bytes"));
        assert!(banner.contains("file offset 808"));
    }

    #[test]
    fn detect_finds_without_mutating() {
        // Same malformed input as `recovers_stream_with_wrong_length`, but
        // detect-only must leave the object a Dictionary.
        let raw = b"3 0 obj\n<</Length 2>>stream\nHELLO WORLD\nendstream\nendobj\n";
        let mut doc = doc_with_misparsed(0, 2);

        let recs = detect_malformed_streams(&doc, raw);

        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, (3, 0));
        assert_eq!(recs[0].recovered_length, 11);
        // Crucially: the document was NOT repaired.
        assert!(matches!(
            doc.objects.get(&(3, 0)),
            Some(Object::Dictionary(_))
        ));
        // And a follow-up repair on the same (untouched) doc still works.
        let applied = recover_malformed_streams(&mut doc, raw);
        assert_eq!(applied.len(), 1);
        assert!(matches!(doc.objects.get(&(3, 0)), Some(Object::Stream(_))));
    }

    #[test]
    fn recovery_json_value_repaired_shape() {
        let recs = vec![StreamRecovery {
            id: (10, 0),
            offset: 808,
            declared_length: Some(54),
            recovered_length: 58,
        }];
        let v = recovery_json_value(&recs, true, false);
        assert_eq!(v["repaired"], json!(true));
        assert_eq!(v["strict"], json!(false));
        assert_eq!(v["count"], json!(1));
        assert_eq!(v["streams"][0]["object"], json!(10));
        assert_eq!(v["streams"][0]["generation"], json!(0));
        assert_eq!(v["streams"][0]["file_offset"], json!(808));
        assert_eq!(v["streams"][0]["declared_length"], json!(54));
        assert_eq!(v["streams"][0]["actual_length"], json!(58));
    }

    #[test]
    fn recovery_json_value_strict_and_absent_length() {
        let recs = vec![StreamRecovery {
            id: (5, 0),
            offset: 100,
            declared_length: None,
            recovered_length: 12,
        }];
        let v = recovery_json_value(&recs, false, true);
        assert_eq!(v["repaired"], json!(false));
        assert_eq!(v["strict"], json!(true));
        // An absent declared /Length is surfaced as JSON null, not omitted.
        assert_eq!(v["streams"][0]["declared_length"], Value::Null);
    }

    #[test]
    fn strict_banner_states_refusal_and_exit() {
        let recs = vec![StreamRecovery {
            id: (10, 0),
            offset: 808,
            declared_length: Some(54),
            recovered_length: 58,
        }];
        let banner = strict_banner(&recs).expect("banner");
        assert!(banner.contains("--strict"));
        assert!(banner.contains("did NOT repair"));
        assert!(banner.contains("Exit code: 3"));
        assert!(banner.contains("object 10 0"));
    }

    #[test]
    fn strict_banner_is_none_when_nothing_detected() {
        assert!(strict_banner(&[]).is_none());
    }

    #[test]
    fn extract_stream_body_returns_none_without_stream() {
        let raw = b"3 0 obj\n<</Type /Foo>>\nendobj\n";
        assert!(extract_stream_body(raw, 0).is_none());
    }

    #[test]
    fn extract_stream_body_handles_offset_past_eof() {
        assert!(extract_stream_body(b"short", 999).is_none());
    }
}
