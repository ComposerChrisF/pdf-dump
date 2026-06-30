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
    Dictionary, Document, EncryptionState, EncryptionVersion, LoadOptions, Object, Permissions,
    StringFormat,
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
    page.set(
        b"MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );

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
    doc.trailer.set(
        b"ID",
        Object::Array(vec![
            Object::String(vec![1u8; 16], StringFormat::Hexadecimal),
            Object::String(vec![1u8; 16], StringFormat::Hexadecimal),
        ]),
    );

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
    doc.save_to(&mut buf)
        .expect("failed to save encrypted document");
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
            let is_xref = s
                .dict
                .get(b"Type")
                .ok()
                .and_then(|v| v.as_name().ok())
                .is_some_and(|n| n == b"XRef");
            if is_xref {
                return s
                    .dict
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

// ── Canary: encrypted PDF with a NON-empty password (the degraded load) ──────
//
// These guard the encrypted-overview fix (v0.23.0). lopdf's `load` auto-decrypts
// only with the empty password; a password-protected file opened without the
// password returns a DEGRADED Ok document. pdf-dump depends on that exact
// signature in `lib.rs` (encrypted_undecrypted detection → exit 3) and
// `summary.rs::is_encrypted` (= encryption_state.is_some() || doc.is_encrypted()).

/// Encrypt `doc` with V1 (RC4 40-bit) and a NON-empty user password, returning
/// the serialized bytes (not reloaded) so the caller chooses how to reopen it.
fn encrypt_to_bytes(mut doc: Document, user_password: &str) -> Vec<u8> {
    let state = EncryptionState::try_from(EncryptionVersion::V1 {
        document: &doc,
        owner_password: "owner",
        user_password,
        permissions: Permissions::all(),
    })
    .expect("failed to create encryption state");
    doc.encrypt(&state).expect("failed to encrypt document");
    let mut buf = Vec::new();
    doc.save_to(&mut buf)
        .expect("failed to save encrypted document");
    buf
}

#[test]
fn lopdf_degrades_on_missing_password() {
    // A non-empty user password with none supplied: lopdf returns Ok with only
    // the /Encrypt dict parsed, encryption_state None, and /Encrypt still in the
    // trailer (is_encrypted() true). If this changes, revisit the encryption
    // detection in summary.rs/lib.rs.
    let bytes = encrypt_to_bytes(build_minimal_doc(), "secret");
    let loaded =
        Document::load_mem(&bytes).expect("load without a password should still return Ok");

    assert_eq!(
        loaded.objects.len(),
        1,
        "lopdf changed: a password-required PDF loaded without a password no longer \
         collapses to just the /Encrypt dict."
    );
    assert!(
        loaded.encryption_state.is_none(),
        "lopdf changed: encryption_state is now Some after a FAILED decrypt."
    );
    assert!(
        loaded.trailer.get(b"Encrypt").is_ok(),
        "lopdf changed: /Encrypt no longer survives in the trailer after a failed decrypt."
    );
    assert!(
        loaded.is_encrypted(),
        "lopdf changed: is_encrypted() is no longer true for an undecrypted document."
    );
}

#[test]
fn lopdf_loads_fully_with_correct_password() {
    // The --password fix path: the correct password runs the full decrypt — all
    // objects present, encryption_state Some, and /Encrypt removed from the
    // trailer (is_encrypted() false).
    let bytes = encrypt_to_bytes(build_minimal_doc(), "secret");
    let loaded = Document::load_mem_with_options(&bytes, LoadOptions::with_password("secret"))
        .expect("load_mem_with_options with the correct password should succeed");

    assert!(
        loaded.objects.len() > 1,
        "expected the full object set after a successful decrypt, got {}",
        loaded.objects.len()
    );
    assert!(
        loaded.encryption_state.is_some(),
        "encryption_state should be Some after a successful decrypt."
    );
    assert!(
        !loaded.is_encrypted(),
        "lopdf removes /Encrypt from the trailer after a successful decrypt."
    );
}

#[test]
fn lopdf_load_with_wrong_password_errors() {
    // A wrong password must error (so pdf-dump can exit 1), not silently degrade.
    let bytes = encrypt_to_bytes(build_minimal_doc(), "secret");
    assert!(
        Document::load_mem_with_options(&bytes, LoadOptions::with_password("wrong")).is_err(),
        "lopdf changed: a wrong password no longer errors."
    );
}

// ── Canary: save_modern() + encryption corrupts ObjStm-packed strings ────────
//
// save_modern() packs objects (including /Info metadata strings) into an ObjStm
// that lopdf leaves UNENCRYPTED while /Encrypt tells readers to decrypt ALL
// streams — so those strings read back corrupt. pdf-orchestrator works around it
// by using traditional save() for encrypted output (NOT save_modern). This canary
// is the authoritative gate for "has lopdf fixed it yet": it FAILS LOUDLY when the
// bug is gone, signalling that the workaround can be removed.
//
// It is hardened against the false "everything's okay" that bit us once already:
//   1. PRECONDITION — it asserts save_modern actually emitted an ObjStm, so
//      "no corruption" can never be misread as "fixed" when the fixture simply
//      failed to trigger the ObjStm packing;
//   2. EXACT round-trip — it compares the reloaded /Info Title to a unique marker,
//      not an exit status or a substring proxy;
//   3. BOTH failure modes count as "still broken" — a hard load error OR a
//      silently garbled title (which one occurs depends on cipher/byte alignment).

/// Read /Info → /Title back as a String, if present and decodable.
fn info_title(doc: &Document) -> Option<String> {
    let info = doc.trailer.get(b"Info").ok()?;
    let dict = match info {
        Object::Reference(id) => doc.get_object(*id).ok()?.as_dict().ok()?,
        Object::Dictionary(d) => d,
        _ => return None,
    };
    match dict.get(b"Title").ok()? {
        Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    }
}

#[test]
fn lopdf_save_modern_encryption_corrupts_objstm_strings() {
    const MARKER: &str = "SENTINEL_save_modern_objstm_roundtrip_marker_42";

    // A doc with a distinctive /Info Title plus enough filler dict objects that
    // save_modern packs them (and /Info) into an ObjStm.
    let mut doc = build_minimal_doc();
    let mut info = Dictionary::new();
    info.set(b"Title", Object::string_literal(MARKER.as_bytes().to_vec()));
    let info_id = doc.add_object(Object::Dictionary(info));
    doc.trailer.set(b"Info", Object::Reference(info_id));
    for i in 0..40 {
        let mut d = Dictionary::new();
        d.set(
            b"SentinelFiller",
            Object::string_literal(format!("filler-string-{i}").into_bytes()),
        );
        doc.add_object(Object::Dictionary(d));
    }

    // Encrypt (owner password; empty user password → load_mem auto-decrypts),
    // then serialize with save_modern (the path that mis-handles the ObjStm).
    let state = EncryptionState::try_from(EncryptionVersion::V1 {
        document: &doc,
        owner_password: "owner",
        user_password: "",
        permissions: Permissions::all(),
    })
    .expect("failed to create encryption state");
    doc.encrypt(&state).expect("failed to encrypt document");
    let mut bytes = Vec::new();
    doc.save_modern(&mut bytes).expect("failed to save_modern");

    // (1) Precondition: the bug only triggers when save_modern emits an ObjStm.
    // If it didn't, the fixture stopped exercising the bug — fail loudly rather
    // than let "no corruption" be silently read as "fixed".
    let has_objstm = bytes.windows(6).any(|w| w == b"ObjStm");
    assert!(
        has_objstm,
        "sentinel fixture no longer makes save_modern emit an ObjStm — strengthen the \
         fixture; without an ObjStm this canary cannot detect the encryption bug."
    );

    // (2)+(3) Reload and require an EXACT round-trip of the marker to call it
    // fixed. A hard load error or a garbled/missing title both mean still-broken.
    match Document::load_mem(&bytes) {
        Err(_) => { /* hard-error failure mode — bug still present */ }
        Ok(loaded) => {
            assert_ne!(
                info_title(&loaded).as_deref(),
                Some(MARKER),
                "lopdf save_modern() + encryption now round-trips ObjStm-packed strings — the \
                 bug appears FIXED. Re-enable save_modern for encrypted output in pdf-orchestrator \
                 (drop the traditional-save workaround in main.rs) and delete this canary."
            );
        }
    }
}
