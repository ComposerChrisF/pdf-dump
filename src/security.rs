use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

pub(crate) struct SecurityInfo {
    pub encrypted: bool,
    pub algorithm: String,
    pub version: i64,
    pub revision: i64,
    pub key_length: i64,
    pub permissions_raw: i64,
    pub permissions: BTreeMap<String, bool>,
    pub encrypt_object: Option<u32>,
}

fn algorithm_name(v: i64, length: i64) -> String {
    match v {
        0 => "Undocumented".to_string(),
        1 => "RC4, 40-bit".to_string(),
        2 => format!("RC4, {}-bit", if length > 0 { length } else { 40 }),
        3 => "Unpublished".to_string(),
        4 => "AES-128".to_string(),
        5 => "AES-256".to_string(),
        _ => format!("Unknown (V={})", v),
    }
}

fn decode_permissions(p: i64) -> BTreeMap<String, bool> {
    let mut perms = BTreeMap::new();
    let bits = p as u32;
    perms.insert("Print".to_string(), bits & (1 << 2) != 0);
    perms.insert("Modify".to_string(), bits & (1 << 3) != 0);
    perms.insert("Copy/extract text".to_string(), bits & (1 << 4) != 0);
    perms.insert("Annotate".to_string(), bits & (1 << 5) != 0);
    perms.insert("Fill forms".to_string(), bits & (1 << 8) != 0);
    perms.insert("Accessibility extract".to_string(), bits & (1 << 9) != 0);
    perms.insert("Assemble".to_string(), bits & (1 << 10) != 0);
    perms.insert("Print high quality".to_string(), bits & (1 << 11) != 0);
    perms
}

pub(crate) fn collect_security(doc: &Document, file_path: Option<&Path>) -> SecurityInfo {
    // Try trailer first (works for traditional xref tables)
    let encrypt_ref = doc.trailer.get(b"Encrypt").ok().and_then(|v| match v {
        Object::Reference(id) => Some(*id),
        _ => None,
    });

    let encrypt_dict =
        encrypt_ref
            .and_then(|id| doc.get_object(id).ok())
            .and_then(|obj| match obj {
                Object::Dictionary(d) => Some(d),
                _ => None,
            });

    // Also check if /Encrypt is an inline dictionary in the trailer
    let encrypt_dict = encrypt_dict.or_else(|| {
        doc.trailer.get(b"Encrypt").ok().and_then(|v| match v {
            Object::Dictionary(d) => Some(d),
            _ => None,
        })
    });

    if let Some(dict) = encrypt_dict {
        return build_security_info(dict, encrypt_ref);
    }

    // Fallback: lopdf consumes /Encrypt from the trailer during loading and
    // removes the encrypt dict object. For cross-reference stream PDFs, the
    // XRef stream object still has /Encrypt in its dict. Scan for it.
    let mut found_encrypt_obj: Option<u32> = None;
    for object in doc.objects.values() {
        let stream_dict = match object {
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        let is_xref = stream_dict
            .get(b"Type")
            .ok()
            .and_then(|v| v.as_name().ok())
            .is_some_and(|n| n == b"XRef");
        if !is_xref {
            continue;
        }

        let enc_ref = stream_dict.get(b"Encrypt").ok().and_then(|v| match v {
            Object::Reference(id) => Some(*id),
            _ => None,
        });

        if let Some(ref_id) = enc_ref {
            // Try to resolve the encrypt dict (usually consumed by lopdf)
            if let Ok(Object::Dictionary(d)) = doc.get_object(ref_id) {
                return build_security_info(d, Some(ref_id));
            }
            found_encrypt_obj = Some(ref_id.0);
            break;
        }

        // Check for inline /Encrypt dict
        if let Ok(Object::Dictionary(d)) = stream_dict.get(b"Encrypt") {
            return build_security_info(d, None);
        }
    }

    // Last resort: read the raw file bytes to find the encrypt dict that
    // lopdf consumed during loading.
    if let Some(obj_num) = found_encrypt_obj
        && let Some(path) = file_path
        && let Some(info) = parse_encrypt_from_raw_file(path, obj_num)
    {
        return info;
    }

    SecurityInfo {
        encrypted: false,
        algorithm: "-".to_string(),
        version: 0,
        revision: 0,
        key_length: 0,
        permissions_raw: 0,
        permissions: BTreeMap::new(),
        encrypt_object: None,
    }
}

fn dict_int(dict: &lopdf::Dictionary, key: &[u8], default: i64) -> i64 {
    dict.get(key)
        .ok()
        .and_then(|o| {
            if let Object::Integer(i) = o {
                Some(*i)
            } else {
                None
            }
        })
        .unwrap_or(default)
}

fn build_security_info(dict: &lopdf::Dictionary, encrypt_ref: Option<ObjectId>) -> SecurityInfo {
    let v = dict_int(dict, b"V", 0);
    let r = dict_int(dict, b"R", 0);
    let length = dict_int(dict, b"Length", 40);
    let p = dict_int(dict, b"P", 0);

    SecurityInfo {
        encrypted: true,
        algorithm: algorithm_name(v, length),
        version: v,
        revision: r,
        key_length: length,
        permissions_raw: p,
        permissions: decode_permissions(p),
        encrypt_object: encrypt_ref.map(|id| id.0),
    }
}

/// Parse the encrypt dictionary directly from the raw PDF file bytes.
/// lopdf consumes this object during loading, so we find it by searching
/// for "{obj_num} 0 obj" and extracting the integer-valued keys we need.
fn parse_encrypt_from_raw_file(path: &Path, obj_num: u32) -> Option<SecurityInfo> {
    let data = std::fs::read(path).ok()?;
    let marker = format!("{} 0 obj", obj_num);
    let marker_bytes = marker.as_bytes();

    // Find the object in the raw bytes
    let pos = data
        .windows(marker_bytes.len())
        .position(|w| w == marker_bytes)?;

    // Extract a window after the marker — encrypt dicts are small
    let start = pos + marker_bytes.len();
    let end = (start + 1024).min(data.len());
    let window = &data[start..end];

    // Find the dictionary start
    let dict_start = window.windows(2).position(|w| w == b"<<")?;
    let dict_bytes = &window[dict_start..];

    let v = extract_int_after_key(dict_bytes, b"/V").unwrap_or(0);
    let r = extract_int_after_key(dict_bytes, b"/R").unwrap_or(0);
    let length = extract_int_after_key(dict_bytes, b"/Length").unwrap_or(40);
    let p = extract_int_after_key(dict_bytes, b"/P").unwrap_or(0);

    Some(SecurityInfo {
        encrypted: true,
        algorithm: algorithm_name(v, length),
        version: v,
        revision: r,
        key_length: length,
        permissions_raw: p,
        permissions: decode_permissions(p),
        encrypt_object: Some(obj_num),
    })
}

/// Find a PDF key name in raw bytes and parse the integer that follows it.
/// Handles negative integers (e.g. /P -1084).
fn extract_int_after_key(data: &[u8], key: &[u8]) -> Option<i64> {
    let pos = data.windows(key.len()).position(|w| w == key)?;
    let after = &data[pos + key.len()..];

    // Skip whitespace and any non-digit characters (but allow '-')
    let mut i = 0;
    while i < after.len() && (after[i] == b' ' || after[i] == b'\r' || after[i] == b'\n') {
        i += 1;
    }
    if i >= after.len() {
        return None;
    }

    // Parse the integer (possibly negative)
    let mut end = i;
    if after[end] == b'-' || after[end] == b'+' {
        end += 1;
    }
    while end < after.len() && after[end].is_ascii_digit() {
        end += 1;
    }
    if end == i {
        return None;
    }
    // If we only got a sign with no digits, bail
    if end == i + 1 && (after[i] == b'-' || after[i] == b'+') {
        return None;
    }

    let num_str = std::str::from_utf8(&after[i..end]).ok()?;
    num_str.parse().ok()
}

pub(crate) fn print_security(writer: &mut impl Write, doc: &Document, file_path: &Path) {
    let info = collect_security(doc, Some(file_path));
    if !info.encrypted {
        wln!(writer, "Encryption: No");
        return;
    }
    wln!(writer, "Encryption: Yes");
    wln!(writer, "Algorithm:  {}", info.algorithm);
    wln!(writer, "Version:    {}", info.version);
    wln!(writer, "Revision:   {}", info.revision);
    wln!(writer, "Key Length: {} bits", info.key_length);
    if let Some(obj) = info.encrypt_object {
        wln!(writer, "Encrypt Object: {}", obj);
    }
    wln!(writer, "\nPermissions (raw: {}):", info.permissions_raw);
    let perm_order = [
        "Print",
        "Modify",
        "Copy/extract text",
        "Annotate",
        "Fill forms",
        "Accessibility extract",
        "Assemble",
        "Print high quality",
    ];
    for name in &perm_order {
        if let Some(&allowed) = info.permissions.get(*name) {
            let tag = if allowed { "YES" } else { " NO" };
            wln!(writer, "  [{}] {}", tag, name);
        }
    }
}

pub(crate) fn security_json_value(doc: &Document, file_path: &Path) -> Value {
    let info = collect_security(doc, Some(file_path));
    let perms: Value = info
        .permissions
        .iter()
        .map(|(k, v)| (k.clone(), json!(*v)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    json!({
        "encrypted": info.encrypted,
        "algorithm": info.algorithm,
        "version": info.version,
        "revision": info.revision,
        "key_length": info.key_length,
        "permissions_raw": info.permissions_raw,
        "permissions": perms,
        "encrypt_object": info.encrypt_object,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::Dictionary;
    use lopdf::Object;
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    #[test]
    fn security_unencrypted() {
        let doc = Document::new();
        let info = collect_security(&doc, None);
        assert!(!info.encrypted);
    }

    #[test]
    fn security_encrypted_v4() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-3904));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "AES-128");
        assert_eq!(info.version, 4);
        assert_eq!(info.revision, 4);
        assert_eq!(info.key_length, 128);
    }

    #[test]
    fn security_encrypted_v5() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(5));
        encrypt.set("R", Object::Integer(6));
        encrypt.set("Length", Object::Integer(256));
        encrypt.set("P", Object::Integer(-1));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "AES-256");
        assert_eq!(info.version, 5);
    }

    #[test]
    fn security_encrypted_v1() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(1));
        encrypt.set("R", Object::Integer(2));
        encrypt.set("P", Object::Integer(-44));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "RC4, 40-bit");
    }

    #[test]
    fn security_permissions_decode() {
        // 2564 = bit 2 (print) + bit 9 (accessibility) + bit 11 (print hq)
        let perms = decode_permissions(2564);
        assert!(perms["Print"]);
        assert!(!perms["Modify"]);
        assert!(!perms["Copy/extract text"]);
        assert!(perms["Accessibility extract"]);
        assert!(perms["Print high quality"]);
    }

    #[test]
    fn security_permissions_all_allowed() {
        let perms = decode_permissions(-1); // All bits set
        assert!(perms["Print"]);
        assert!(perms["Modify"]);
        assert!(perms["Copy/extract text"]);
        assert!(perms["Annotate"]);
        assert!(perms["Fill forms"]);
        assert!(perms["Accessibility extract"]);
        assert!(perms["Assemble"]);
        assert!(perms["Print high quality"]);
    }

    #[test]
    fn security_print_unencrypted() {
        let doc = Document::new();
        let dummy = std::path::Path::new("nonexistent.pdf");
        let out = output_of(|w| print_security(w, &doc, dummy));
        assert!(out.contains("Encryption: No"));
    }

    #[test]
    fn security_print_encrypted() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-3)); // all permissions
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));
        let dummy = std::path::Path::new("nonexistent.pdf");
        let out = output_of(|w| print_security(w, &doc, dummy));
        assert!(out.contains("Encryption: Yes"));
        assert!(out.contains("AES-128"));
        assert!(out.contains("[YES] Print"));
    }

    #[test]
    fn security_json_output() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-3));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));
        let dummy = std::path::Path::new("nonexistent.pdf");
        let out = output_of(|w| render_json(w, &security_json_value(&doc, dummy)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["encrypted"], true);
        assert_eq!(parsed["algorithm"], "AES-128");
        assert_eq!(parsed["version"], 4);
        assert!(parsed["permissions"].is_object());
    }

    #[test]
    fn extract_int_after_key_basic() {
        let data = b"<</V 4/R 4/Length 128/P -1084>>";
        assert_eq!(extract_int_after_key(data, b"/V"), Some(4));
        assert_eq!(extract_int_after_key(data, b"/R"), Some(4));
        assert_eq!(extract_int_after_key(data, b"/Length"), Some(128));
        assert_eq!(extract_int_after_key(data, b"/P"), Some(-1084));
    }

    #[test]
    fn extract_int_after_key_with_spaces() {
        let data = b"<< /V  2  /R  3  /Length  128  /P  -3904 >>";
        assert_eq!(extract_int_after_key(data, b"/V"), Some(2));
        assert_eq!(extract_int_after_key(data, b"/P"), Some(-3904));
    }

    #[test]
    fn extract_int_after_key_missing() {
        let data = b"<</V 4/R 4>>";
        assert_eq!(extract_int_after_key(data, b"/Length"), None);
    }

    #[test]
    fn extract_int_after_key_negative() {
        let data = b"/P -1";
        assert_eq!(extract_int_after_key(data, b"/P"), Some(-1));
    }

    #[test]
    fn parse_encrypt_from_raw_file_with_tempfile() {
        let content =
            b"%PDF-1.6\n5 0 obj\n<</Filter/Standard/V 2/R 3/Length 128/P -1084>>\nendobj\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pdf");
        std::fs::write(&path, content).unwrap();

        let info = parse_encrypt_from_raw_file(&path, 5).unwrap();
        assert!(info.encrypted);
        assert_eq!(info.version, 2);
        assert_eq!(info.revision, 3);
        assert_eq!(info.key_length, 128);
        assert_eq!(info.permissions_raw, -1084);
        assert_eq!(info.encrypt_object, Some(5));
    }

    #[test]
    fn parse_encrypt_from_raw_file_not_found() {
        let content = b"%PDF-1.6\n1 0 obj\n<</Type/Catalog>>\nendobj\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pdf");
        std::fs::write(&path, content).unwrap();

        let result = parse_encrypt_from_raw_file(&path, 99);
        assert!(result.is_none());
    }

    // --- New tests below ---

    #[test]
    fn algorithm_name_v0_undocumented() {
        assert_eq!(algorithm_name(0, 0), "Undocumented");
    }

    #[test]
    fn algorithm_name_v2_default_length() {
        // When length <= 0, should default to 40
        assert_eq!(algorithm_name(2, 0), "RC4, 40-bit");
        assert_eq!(algorithm_name(2, -1), "RC4, 40-bit");
    }

    #[test]
    fn algorithm_name_v2_custom_length() {
        assert_eq!(algorithm_name(2, 128), "RC4, 128-bit");
        assert_eq!(algorithm_name(2, 56), "RC4, 56-bit");
    }

    #[test]
    fn algorithm_name_v3_unpublished() {
        assert_eq!(algorithm_name(3, 0), "Unpublished");
    }

    #[test]
    fn algorithm_name_unknown_version() {
        assert_eq!(algorithm_name(99, 0), "Unknown (V=99)");
        assert_eq!(algorithm_name(6, 256), "Unknown (V=6)");
    }

    #[test]
    fn decode_permissions_all_denied() {
        let perms = decode_permissions(0);
        assert!(!perms["Print"]);
        assert!(!perms["Modify"]);
        assert!(!perms["Copy/extract text"]);
        assert!(!perms["Annotate"]);
        assert!(!perms["Fill forms"]);
        assert!(!perms["Accessibility extract"]);
        assert!(!perms["Assemble"]);
        assert!(!perms["Print high quality"]);
    }

    #[test]
    fn encrypt_dict_inline_in_trailer() {
        // /Encrypt as an inline dictionary in the trailer, not a reference
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(5));
        encrypt.set("R", Object::Integer(6));
        encrypt.set("Length", Object::Integer(256));
        encrypt.set("P", Object::Integer(-4));
        doc.trailer.set("Encrypt", Object::Dictionary(encrypt));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "AES-256");
        assert_eq!(info.version, 5);
        assert_eq!(info.revision, 6);
        assert_eq!(info.key_length, 256);
        // No reference, so encrypt_object should be None
        assert_eq!(info.encrypt_object, None);
    }

    #[test]
    fn encrypt_dict_via_xref_stream_fallback() {
        use lopdf::Stream;

        // Simulate lopdf's post-decryption state: trailer has no /Encrypt,
        // but an XRef stream object still carries an /Encrypt reference.
        let mut doc = Document::new();

        // Add the encrypt dict as an object
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-3904));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));

        // Create an XRef stream that references the encrypt dict
        let mut xref_dict = Dictionary::new();
        xref_dict.set("Type", Object::Name(b"XRef".to_vec()));
        xref_dict.set("Encrypt", Object::Reference(enc_id));
        let xref_stream = Stream::new(xref_dict, vec![]);
        doc.add_object(Object::Stream(xref_stream));

        // Trailer does NOT have /Encrypt (simulating lopdf stripping it)
        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "AES-128");
        assert_eq!(info.version, 4);
        assert_eq!(info.encrypt_object, Some(enc_id.0));
    }

    #[test]
    fn extract_int_after_key_positive_sign() {
        let data = b"/V +4";
        assert_eq!(extract_int_after_key(data, b"/V"), Some(4));
    }

    #[test]
    fn extract_int_after_key_sign_only_no_digits() {
        let data = b"/P - ";
        assert_eq!(extract_int_after_key(data, b"/P"), None);

        let data2 = b"/P + ";
        assert_eq!(extract_int_after_key(data2, b"/P"), None);
    }

    #[test]
    fn extract_int_after_key_newline_whitespace() {
        let data = b"/V\n4";
        assert_eq!(extract_int_after_key(data, b"/V"), Some(4));

        let data2 = b"/V\r\n  42";
        assert_eq!(extract_int_after_key(data2, b"/V"), Some(42));
    }

    #[test]
    fn collect_security_v2_custom_length() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(2));
        encrypt.set("R", Object::Integer(3));
        encrypt.set("Length", Object::Integer(56));
        encrypt.set("P", Object::Integer(-4));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "RC4, 56-bit");
        assert_eq!(info.version, 2);
        assert_eq!(info.key_length, 56);
    }

    #[test]
    fn security_json_value_unencrypted() {
        let doc = Document::new();
        let dummy = std::path::Path::new("nonexistent.pdf");
        let val = security_json_value(&doc, dummy);

        assert_eq!(val["encrypted"], false);
        assert_eq!(val["algorithm"], "-");
        assert_eq!(val["version"], 0);
        assert_eq!(val["revision"], 0);
        assert_eq!(val["key_length"], 0);
        assert_eq!(val["permissions_raw"], 0);
        assert!(val["permissions"].is_object());
        // Unencrypted: permissions map should be empty
        assert_eq!(val["permissions"].as_object().unwrap().len(), 0);
        assert!(val["encrypt_object"].is_null());
    }

    #[test]
    fn print_security_shows_encrypt_object_number() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-1));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let dummy = std::path::Path::new("nonexistent.pdf");
        let out = output_of(|w| print_security(w, &doc, dummy));
        let expected = format!("Encrypt Object: {}", enc_id.0);
        assert!(
            out.contains(&expected),
            "Expected output to contain '{}', got:\n{}",
            expected,
            out
        );
    }

    #[test]
    fn parse_encrypt_from_raw_file_nonexistent_path() {
        let path = std::path::Path::new("/tmp/totally_nonexistent_file_abc123.pdf");
        let result = parse_encrypt_from_raw_file(path, 1);
        assert!(result.is_none());
    }
}
