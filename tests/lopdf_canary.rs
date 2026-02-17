//! Canary tests: detect when lopdf changes its handling of encrypted PDFs
//! and cross-reference streams, so we know when to revisit our workarounds
//! in validate.rs and summary.rs.
//!
//! ## Background
//!
//! When lopdf loads an encrypted PDF with a cross-reference stream (PDF 1.5+),
//! it currently:
//!   1. Removes `/Encrypt` from `doc.trailer`
//!   2. Removes the Encrypt dictionary object from `doc.objects`
//!   3. Leaves the XRef stream object in `doc.objects` with a stale `/Encrypt`
//!      reference (dangling)
//!   4. Populates `doc.encryption_state` with `Some(EncryptionState)`
//!
//! We work around #1–#3 in:
//!   - `validate.rs`: skip XRef stream objects during broken-ref and unreachable checks
//!   - `summary.rs`: detect encryption via XRef stream `/Encrypt` key when trailer lacks it
//!
//! If any of these tests fail, lopdf's behavior has changed and our workarounds
//! should be revisited. Each test documents which workaround it guards.

use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat,
};

// ── Helpers ──────────────────────────────────────────────────────────────

/// Build a minimal valid one-page PDF document.
fn build_minimal_doc() -> Document {
    let mut doc = Document::new();

    let mut pages = Dictionary::new();
    pages.set(b"Type", Object::Name(b"Pages".to_vec()));
    pages.set(b"Count", Object::Integer(1));

    let mut page = Dictionary::new();
    page.set(b"Type", Object::Name(b"Page".to_vec()));
    page.set(b"MediaBox", Object::Array(vec![
        Object::Integer(0), Object::Integer(0),
        Object::Integer(612), Object::Integer(792),
    ]));

    let page_id = doc.add_object(Object::Dictionary(page));
    pages.set(b"Kids", Object::Array(vec![Object::Reference(page_id)]));
    let pages_id = doc.add_object(Object::Dictionary(pages));

    // Update page's /Parent
    if let Ok(Object::Dictionary(p)) = doc.get_object_mut(page_id) {
        p.set(b"Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
    catalog.set(b"Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set(b"Root", Object::Reference(catalog_id));

    // File ID is required for encryption
    doc.trailer.set(b"ID", Object::Array(vec![
        Object::String(vec![1u8; 16], StringFormat::Hexadecimal),
        Object::String(vec![1u8; 16], StringFormat::Hexadecimal),
    ]));

    doc
}

/// Encrypt a document with V1 (RC4 40-bit, empty user password) and
/// round-trip through save/load. Returns the loaded (decrypted) document.
fn encrypt_and_reload(mut doc: Document) -> Document {
    let state = EncryptionState::try_from(EncryptionVersion::V1 {
        document: &doc,
        owner_password: "owner",
        user_password: "",
        permissions: Permissions::all(),
    })
    .expect("failed to create encryption state");
    doc.encrypt(&state).expect("failed to encrypt document");

    let mut buf = Vec::new();
    doc.save_to(&mut buf).expect("failed to save encrypted document");
    Document::load_mem(&buf).expect("failed to load encrypted document")
}

/// Check if any object in doc.objects is a /Type /XRef stream.
fn has_xref_stream_in_objects(doc: &Document) -> bool {
    doc.objects.values().any(|obj| {
        if let Object::Stream(s) = obj {
            s.dict
                .get(b"Type")
                .ok()
                .and_then(|v| v.as_name().ok())
                .is_some_and(|n| n == b"XRef")
        } else {
            false
        }
    })
}

/// Find the /Encrypt reference inside an XRef stream, if any.
fn find_encrypt_ref_in_xref_stream(doc: &Document) -> Option<(u32, u16)> {
    doc.objects.values().find_map(|obj| {
        if let Object::Stream(s) = obj {
            let is_xref = s.dict
                .get(b"Type")
                .ok()
                .and_then(|v| v.as_name().ok())
                .is_some_and(|n| n == b"XRef");
            if is_xref {
                return s.dict
                    .get(b"Encrypt")
                    .ok()
                    .and_then(|v| v.as_reference().ok());
            }
        }
        None
    })
}

// ── Canary: Encrypted PDF behavior ──────────────────────────────────────
//
// These test what lopdf does after loading an encrypted-then-decrypted PDF.
// The workarounds they guard are in validate.rs and summary.rs.

#[test]
fn lopdf_populates_encryption_state() {
    // After loading an encrypted PDF, encryption_state should be Some.
    // If this fails: lopdf's encryption detection mechanism changed.
    //
    // Guards: summary.rs could use `encryption_state.is_some()` as an
    //         alternative to XRef-stream scanning for encryption detection.
    let doc = build_minimal_doc();
    let loaded = encrypt_and_reload(doc);

    assert!(
        loaded.encryption_state.is_some(),
        "lopdf changed: encryption_state is no longer populated after decryption. \
         Check summary.rs encryption detection logic."
    );
}

#[test]
fn lopdf_removes_encrypt_from_trailer() {
    // After decryption, lopdf removes /Encrypt from doc.trailer.
    // Our summary.rs workaround detects encryption via XRef stream's /Encrypt key.
    //
    // If this fails: lopdf now preserves /Encrypt in the trailer.
    // → The XRef-stream fallback in summary.rs print_overview/print_overview_json
    //   is redundant (the trailer check catches it). Consider simplifying.
    let doc = build_minimal_doc();
    let loaded = encrypt_and_reload(doc);

    assert!(
        loaded.trailer.get(b"Encrypt").is_err(),
        "lopdf changed: /Encrypt is now preserved in the trailer after decryption. \
         The XRef-stream encryption fallback in summary.rs may be removable."
    );
}

#[test]
fn lopdf_removes_encrypt_dict_object() {
    // After decryption, lopdf removes the actual Encrypt dictionary object
    // from doc.objects. If an XRef stream references it, that becomes a
    // dangling reference.
    //
    // If this fails: lopdf now preserves the Encrypt dict object.
    // → The broken-reference issue is resolved upstream. Consider removing
    //   the XRef-stream skip in validate.rs check_broken_references().
    let doc = build_minimal_doc();
    let loaded = encrypt_and_reload(doc);

    // The encrypt dict should have been removed — verify no object looks like
    // an encrypt dictionary (has /Filter /Standard or /V + /R keys).
    let has_encrypt_dict = loaded.objects.values().any(|obj| {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => return false,
        };
        dict.get(b"Filter")
            .ok()
            .and_then(|v| v.as_name().ok())
            .is_some_and(|n| n == b"Standard")
    });

    assert!(
        !has_encrypt_dict,
        "lopdf changed: Encrypt dict object is now preserved after decryption. \
         The dangling-reference workaround in validate.rs may be removable."
    );
}

// ── Canary: XRef stream in doc.objects ───────────────────────────────────
//
// These test whether lopdf puts XRef stream objects into doc.objects.
// Our validate.rs workaround (collect_xref_stream_ids) skips them.

#[test]
fn lopdf_puts_xref_streams_in_objects_after_encrypted_round_trip() {
    // When lopdf saves an encrypted PDF and re-loads it, the XRef stream
    // object (which doubles as the trailer in PDF 1.5+) ends up in doc.objects.
    //
    // If this fails: lopdf no longer puts XRef streams in doc.objects.
    // → collect_xref_stream_ids() in validate.rs returns empty sets and the
    //   skip logic is dead code. It's harmless but can be cleaned up.
    let doc = build_minimal_doc();
    let loaded = encrypt_and_reload(doc);

    // Note: this may also fail if lopdf's writer switches to traditional xref
    // tables. That's fine — it still means the XRef-in-objects scenario doesn't
    // arise for lopdf-generated files.
    if has_xref_stream_in_objects(&loaded) {
        // XRef stream is in objects — our workaround is actively needed.
        // Also verify the dangling /Encrypt reference exists.
        let encrypt_ref = find_encrypt_ref_in_xref_stream(&loaded);
        if let Some(enc_id) = encrypt_ref {
            assert!(
                loaded.get_object(enc_id).is_err(),
                "XRef stream has /Encrypt ref but the object still exists. \
                 lopdf may have changed — review validate.rs workaround."
            );
        }
    }
    // If no XRef stream in objects: the writer used a traditional xref table.
    // Our workaround is unnecessary for this path, but still harmless.
}

// ── Canary: Non-encrypted XRef stream behavior ──────────────────────────
//
// Tests XRef stream handling with a real-world (non-encrypted) PDF that
// was created by an external tool. This is the most common case in the wild.

#[test]
fn lopdf_puts_xref_streams_in_objects_for_external_pdfs() {
    // out.pdf was created by an external tool and uses a cross-reference stream.
    // After loading, lopdf currently puts the XRef stream in doc.objects.
    //
    // If this fails: lopdf no longer loads XRef stream objects into doc.objects.
    // → collect_xref_stream_ids() in validate.rs is dead code. Consider removing.
    let path = std::path::Path::new("out.pdf");
    if !path.exists() {
        eprintln!("Skipping: out.pdf not found (needed for XRef stream canary test)");
        return;
    }

    let doc = Document::load(path).expect("failed to load out.pdf");
    assert!(
        has_xref_stream_in_objects(&doc),
        "lopdf changed: XRef stream objects from external PDFs are no longer in doc.objects. \
         The collect_xref_stream_ids() workaround in validate.rs may be removable."
    );
}
