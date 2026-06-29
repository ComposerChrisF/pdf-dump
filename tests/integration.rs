//! Integration tests for pdf-dump binary.
//!
//! These tests exercise the CLI binary end-to-end using synthetic PDF files
//! created via lopdf, verifying both dump and extract modes.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use lopdf::{Dictionary, Document, Object, Stream};

/// Path to the compiled binary under test.
fn binary_path() -> PathBuf {
    // `cargo test` puts the test binary in target/debug/deps;
    // the actual binary is in target/debug/.
    let mut path = std::env::current_exe().unwrap();
    // Walk up from target/debug/deps/<test-binary> to target/debug/
    path.pop(); // remove test binary name
    if path.ends_with("deps") {
        path.pop(); // remove "deps"
    }
    path.push("pdf-dump");
    path
}

/// Create a minimal synthetic PDF with a single page and return the temp file path.
fn create_minimal_pdf() -> tempfile::NamedTempFile {
    let mut doc = Document::new();

    // Page content stream: "BT /F1 12 Tf (Hello) Tj ET"
    let content_bytes = b"BT\n/F1 12 Tf\n(Hello) Tj\nET";
    let content_stream = Stream::new(Dictionary::new(), content_bytes.to_vec());
    let content_id = doc.add_object(Object::Stream(content_stream));

    // Font dictionary
    let mut font_dict = Dictionary::new();
    font_dict.set("Type", Object::Name(b"Font".to_vec()));
    font_dict.set("Subtype", Object::Name(b"Type1".to_vec()));
    font_dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    let font_id = doc.add_object(Object::Dictionary(font_dict));

    // Resources
    let mut f1_dict = Dictionary::new();
    f1_dict.set("F1", Object::Reference(font_id));
    let mut resources = Dictionary::new();
    resources.set("Font", Object::Dictionary(f1_dict));

    // Page
    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set("Contents", Object::Reference(content_id));
    page_dict.set("Resources", Object::Dictionary(resources));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let page_id = doc.add_object(Object::Dictionary(page_dict));

    // Pages
    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages_dict.set("Count", Object::Integer(1));
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));

    // Update page's /Parent
    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    // Catalog
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));

    // Set trailer
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    tmp
}

/// Create a PDF with a FlateDecode stream for extraction testing.
fn create_pdf_with_flatedecode_stream() -> (tempfile::NamedTempFile, u32) {
    let mut doc = Document::new();

    // Create a FlateDecode compressed stream
    let original = b"This is the decompressed stream content for testing.";
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(original).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut stream_dict = Dictionary::new();
    stream_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    let stream = Stream::new(stream_dict, compressed);
    let stream_id = doc.add_object(Object::Stream(stream));

    // Minimal catalog
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    (tmp, stream_id.0)
}

// ── Integration tests ───────────────────────────────────────────────

#[test]
fn extract_stream_object_writes_file() {
    let (pdf, stream_obj_num) = create_pdf_with_flatedecode_stream();
    let output_file = tempfile::NamedTempFile::new().unwrap();
    let output_path = output_file.path().to_path_buf();

    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--extract-stream")
        .arg(stream_obj_num.to_string())
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "extract failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let extracted = fs::read(&output_path).unwrap();
    assert_eq!(
        extracted,
        b"This is the decompressed stream content for testing."
    );
}

#[test]
fn extract_non_stream_object_fails() {
    let mut doc = Document::new();
    let int_id = doc.add_object(Object::Integer(42));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output_file = tempfile::NamedTempFile::new().unwrap();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--extract-stream")
        .arg(int_id.0.to_string())
        .arg("--output")
        .arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a stream"),
        "Should report non-stream error: {}",
        stderr
    );
}

#[test]
fn extract_nonexistent_object_fails() {
    let pdf = create_minimal_pdf();
    let output_file = tempfile::NamedTempFile::new().unwrap();

    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--extract-stream")
        .arg("9999")
        .arg("--output")
        .arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "Should report object not found: {}",
        stderr
    );
}

#[test]
fn nonexistent_pdf_file_fails_gracefully() {
    let output = Command::new(binary_path())
        .arg("/tmp/nonexistent_pdf_file_that_does_not_exist.pdf")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error") || stderr.contains("error"),
        "Should show error for missing file: {}",
        stderr
    );
}

#[test]
fn no_arguments_shows_error() {
    let output = Command::new(binary_path())
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // clap should report missing required argument
    assert!(
        stderr.contains("required") || stderr.contains("Usage"),
        "Should show usage/required error: {}",
        stderr
    );
}

// ── CLI argument validation ─────────────────────────────────────────

#[test]
fn output_without_extract_object_fails() {
    // --output requires --extract-stream (clap `requires` constraint)
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--output")
        .arg("/tmp/out.bin")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("extract") || stderr.contains("required"),
        "Should indicate --output requires --extract-stream: {}",
        stderr
    );
}

#[test]
fn help_flag_prints_usage() {
    let output = Command::new(binary_path())
        .arg("--help")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage") || stdout.contains("usage"),
        "Should print usage info: {}",
        stdout
    );
    assert!(stdout.contains("--decode"), "Should list --decode flag");
    assert!(
        stdout.contains("--extract-stream"),
        "Should list --extract-stream flag"
    );
}

#[test]
fn version_flag_prints_version() {
    let output = Command::new(binary_path())
        .arg("--version")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0.5.0") || stdout.contains("pdf-dump"),
        "Should print version info: {}",
        stdout
    );
}

#[test]
fn corrupt_pdf_file_fails_gracefully() {
    // Write garbage data to a file and try to load it as PDF
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(b"this is not a pdf file at all").unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error") || stderr.contains("error"),
        "Should show error for corrupt PDF: {}",
        stderr
    );
}

// ── Extract mode edge cases ─────────────────────────────────────────

#[test]
fn extract_uncompressed_stream() {
    // Extract a stream with no FlateDecode filter
    let mut doc = Document::new();
    let raw_bytes = b"raw uncompressed stream data for extraction test";
    let stream = Stream::new(Dictionary::new(), raw_bytes.to_vec());
    let stream_id = doc.add_object(Object::Stream(stream));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output_file = tempfile::NamedTempFile::new().unwrap();
    let output_path = output_file.path().to_path_buf();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--extract-stream")
        .arg(stream_id.0.to_string())
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "Extract failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let extracted = fs::read(&output_path).unwrap();
    assert_eq!(
        extracted, raw_bytes,
        "Uncompressed stream should be extracted as-is"
    );
}

#[test]
fn extract_object_prints_success_message() {
    let (pdf, stream_obj_num) = create_pdf_with_flatedecode_stream();
    let output_file = tempfile::NamedTempFile::new().unwrap();

    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--extract-stream")
        .arg(stream_obj_num.to_string())
        .arg("--output")
        .arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Successfully extracted"),
        "Should print success message: {}",
        stdout
    );
}

// ── New mode integration tests ───────────────────────────────────────

#[test]
fn object_flag_prints_single_object() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg("1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "--object should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Object 1 0 ("),
        "Should show object header with type label"
    );
    // Should NOT show trailer or full dump
    assert!(
        !stdout.contains("Trailer:"),
        "Should not show trailer in --object mode"
    );
}

#[test]
fn object_flag_short_form() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("-o")
        .arg("1")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "-o should work as short form: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Object 1 0 ("));
}

#[test]
fn object_flag_nonexistent_object_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg("9999")
        .output()
        .expect("failed to execute binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "Should report object not found: {}",
        stderr
    );
}

#[test]
fn list_flag_shows_object_table() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--list")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "--list should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("PDF"), "Should show PDF version");
    assert!(stdout.contains("objects"), "Should show object count");
    assert!(stdout.contains("Obj#"), "Should show table header");
    assert!(
        stdout.contains("Dictionary") || stdout.contains("Stream"),
        "Should show object kinds"
    );
}

#[test]
fn list_flag_short_form() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("-s")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "-s should work as short form");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Obj#"));
}

#[test]
fn page_flag_shows_page_content() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "--page should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Page 1"), "Should show page header");
    assert!(stdout.contains("MediaBox"), "Should show page properties");
}

#[test]
fn page_flag_with_decode_streams_still_works() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("1")
        .arg("--decode")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "--page with --decode should still succeed"
    );
    assert!(
        stdout.contains("Page 1"),
        "Should show page info even with --decode"
    );
}

#[test]
fn page_flag_nonexistent_page_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("999")
        .output()
        .expect("failed to execute binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "Should report page not found: {}",
        stderr
    );
}

#[test]
fn object_and_page_flags_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg("1")
        .arg("--page")
        .arg("1")
        .output()
        .expect("failed to execute binary");

    // --page is now always a modifier, so --object --page succeeds
    assert!(
        output.status.success(),
        "object + page should succeed (page is a modifier)"
    );
}

// ── JSON mode integration tests ──────────────────────────────────────

#[test]
fn json_object_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg("1")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert_eq!(parsed["object_number"], 1);
}

#[test]
fn json_list_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--list")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed.get("version").is_some());
    assert!(parsed.get("objects").is_some());
}

#[test]
fn json_page_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("1")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert_eq!(parsed["pages"][0]["page_number"], 1);
}

// ── Search mode integration tests ────────────────────────────────────

#[test]
fn search_finds_fonts() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Font")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "search should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Found"), "Should show match count");
    assert!(
        stdout.contains("matching objects"),
        "Should show match summary"
    );
}

#[test]
fn search_no_matches() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Nonexistent")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Found 0 matching objects."));
}

#[test]
fn search_with_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Font")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed.get("query").is_some());
    assert!(parsed.get("match_count").is_some());
    assert!(parsed.get("matches").is_some());
}

#[test]
fn search_with_list() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Font")
        .arg("--list")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Obj#"), "Should show list table header");
}

#[test]
fn search_bad_expression_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("badexpr")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid") || stderr.contains("error"),
        "Should report bad expression: {}",
        stderr
    );
}

#[test]
fn search_has_key() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("key=MediaBox")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Found"),
        "Should find objects with MediaBox"
    );
}

// ── Text mode integration tests ──────────────────────────────────────

#[test]
fn text_extracts_hello() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("--- Page 1 ---"), "Should show page header");
    assert!(stdout.contains("Hello"), "Should extract Hello text");
}

#[test]
fn text_with_page_filter() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--page")
        .arg("1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("--- Page 1 ---"));
    assert!(stdout.contains("Hello"));
}

#[test]
fn text_nonexistent_page_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--page")
        .arg("999")
        .output()
        .expect("failed to execute binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "Should report page not found: {}",
        stderr
    );
}

#[test]
fn text_with_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed["pages"].is_array());
    let page = &parsed["pages"][0];
    assert_eq!(page["page_number"], 1);
    assert!(page["text"].as_str().unwrap().contains("Hello"));
}

#[test]
fn text_mutually_exclusive_with_list() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--list")
        .output()
        .expect("failed to execute binary");

    // Document-level modes are now combinable
    assert!(
        output.status.success(),
        "text + list should succeed (combinable modes)"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("=== Text ==="), "should show Text header");
    assert!(stdout.contains("=== List ==="), "should show List header");
}

// ── Cross-mode tests ─────────────────────────────────────────────────

#[test]
fn search_with_decode_streams() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Font")
        .arg("--decode")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "--search with --decode should succeed"
    );
}

/// Create a two-page PDF for multi-page tests.
fn create_two_page_pdf() -> tempfile::NamedTempFile {
    let mut doc = Document::new();

    let c1 = Stream::new(
        Dictionary::new(),
        b"BT\n/F1 12 Tf\n(Page1Text) Tj\nET".to_vec(),
    );
    let c1_id = doc.add_object(Object::Stream(c1));
    let c2 = Stream::new(
        Dictionary::new(),
        b"BT\n/F1 12 Tf\n(Page2Text) Tj\nET".to_vec(),
    );
    let c2_id = doc.add_object(Object::Stream(c2));

    let mut font_dict = Dictionary::new();
    font_dict.set("Type", Object::Name(b"Font".to_vec()));
    font_dict.set("Subtype", Object::Name(b"Type1".to_vec()));
    font_dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    let font_id = doc.add_object(Object::Dictionary(font_dict));

    let mut f1_dict = Dictionary::new();
    f1_dict.set("F1", Object::Reference(font_id));
    let mut resources = Dictionary::new();
    resources.set("Font", Object::Dictionary(f1_dict));

    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Count", Object::Integer(2));
    pages_dict.set("Kids", Object::Array(vec![])); // placeholder
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));

    let mut page1 = Dictionary::new();
    page1.set("Type", Object::Name(b"Page".to_vec()));
    page1.set("Parent", Object::Reference(pages_id));
    page1.set("Contents", Object::Reference(c1_id));
    page1.set("Resources", Object::Dictionary(resources.clone()));
    page1.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let p1_id = doc.add_object(Object::Dictionary(page1));

    let mut page2 = Dictionary::new();
    page2.set("Type", Object::Name(b"Page".to_vec()));
    page2.set("Parent", Object::Reference(pages_id));
    page2.set("Contents", Object::Reference(c2_id));
    page2.set("Resources", Object::Dictionary(resources));
    page2.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let p2_id = doc.add_object(Object::Dictionary(page2));

    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
        d.set(
            "Kids",
            Object::Array(vec![Object::Reference(p1_id), Object::Reference(p2_id)]),
        );
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    tmp
}

#[test]
fn search_multiple_and_conditions() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Font,Subtype=Type1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "search should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Found"), "Should show match count");
    // Our minimal PDF has a Helvetica Type1 font, so it should match
    assert!(stdout.contains("matching objects"));
}

#[test]
fn search_value_expression() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("value=Helvetica")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "search should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Found"), "Should show match count");
}

#[test]
fn page_zero_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("0")
        .output()
        .expect("failed to execute binary");

    assert!(
        !output.status.success(),
        "Page 0 should fail (pages are 1-based)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("Error"),
        "Should report page not found: {}",
        stderr
    );
}

#[test]
fn text_extracts_from_multiple_pages() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("--- Page 1 ---"));
    assert!(stdout.contains("--- Page 2 ---"));
    assert!(stdout.contains("Page1Text"));
    assert!(stdout.contains("Page2Text"));
}

#[test]
fn text_page_2_only() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--page")
        .arg("2")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("--- Page 2 ---"));
    assert!(stdout.contains("Page2Text"));
    assert!(!stdout.contains("Page1Text"), "Should not show page 1 text");
}

#[test]
fn page_2_dump_shows_page_2() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("2")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Page 2 (Object"));
}

#[test]
fn search_multiple_conditions_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Font,key=BaseFont")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(
        parsed["match_count"].as_u64().unwrap() >= 1,
        "Should find font with both conditions"
    );
}

#[test]
fn object_mode_with_decode_streams() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg("1")
        .arg("--decode")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "--object with --decode should work"
    );
}

#[test]
fn search_with_list_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search")
        .arg("Type=Font")
        .arg("--list")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // --list with --search and --json: should still produce valid JSON
    let _parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
}

#[test]
fn json_text_with_page_filter() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--page")
        .arg("1")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    let pages = parsed["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1, "Should have exactly one page");
    assert_eq!(pages[0]["page_number"], 1);
    assert!(pages[0]["text"].as_str().unwrap().contains("Page1Text"));
}

#[test]
fn list_with_two_page_pdf() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--list")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Obj#"), "Should show table header");
    // Should list all objects
    assert!(stdout.contains("Stream") || stdout.contains("Dictionary"));
}

// ── P1: --hex integration tests ─────────────────────────────────────

fn create_pdf_with_binary_stream() -> (tempfile::NamedTempFile, u32) {
    let mut doc = Document::new();

    // Binary stream (not text)
    let binary_content: Vec<u8> = (0..64).collect();
    let stream = Stream::new(Dictionary::new(), binary_content);
    let stream_id = doc.add_object(Object::Stream(stream));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    (tmp, stream_id.0)
}

#[test]
fn hex_flag_with_decode_streams() {
    let (pdf, obj_num) = create_pdf_with_binary_stream();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg(obj_num.to_string())
        .arg("--decode")
        .arg("--hex")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    // Hex dump should show offset
    assert!(stdout.contains("00000000"), "Should show hex dump offsets");
}

#[test]
fn hex_flag_json_mode() {
    let (pdf, obj_num) = create_pdf_with_flatedecode_stream();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg(obj_num.to_string())
        .arg("--decode")
        .arg("--hex")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // Text stream won't have content_hex; depends on content, but JSON should parse
    assert!(parsed["object"].is_object());
}

// ── P1: --fonts integration tests ──────────────────────────────────

#[test]
fn fonts_lists_fonts() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--fonts")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("fonts found"));
    assert!(stdout.contains("Helvetica"));
}

#[test]
fn fonts_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--fonts")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["font_count"].is_number());
    assert!(parsed["fonts"].is_array());
}

#[test]
fn fonts_mutually_exclusive() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--fonts")
        .arg("--list")
        .output()
        .expect("failed to execute binary");

    // Document-level modes are now combinable
    assert!(
        output.status.success(),
        "fonts + list should succeed (combinable modes)"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("=== Fonts ==="), "should show Fonts header");
    assert!(stdout.contains("=== List ==="), "should show List header");
}

// ── P1: --images integration tests ─────────────────────────────────

fn create_pdf_with_image() -> tempfile::NamedTempFile {
    let mut doc = Document::new();

    // Image stream
    let mut img_dict = Dictionary::new();
    img_dict.set("Type", Object::Name(b"XObject".to_vec()));
    img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
    img_dict.set("Width", Object::Integer(100));
    img_dict.set("Height", Object::Integer(100));
    img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
    img_dict.set("BitsPerComponent", Object::Integer(8));
    let image_stream = Stream::new(img_dict, vec![0u8; 300]);
    let _image_id = doc.add_object(Object::Stream(image_stream));

    // Minimal catalog
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    tmp
}

#[test]
fn images_lists_images() {
    let pdf = create_pdf_with_image();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--images")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("1 images found"));
    assert!(stdout.contains("100"));
    assert!(stdout.contains("DeviceRGB"));
}

#[test]
fn images_json() {
    let pdf = create_pdf_with_image();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--images")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["image_count"], 1);
    assert_eq!(parsed["images"][0]["width"], 100);
}

#[test]
fn images_no_images() {
    let (pdf, _) = create_pdf_with_flatedecode_stream();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--images")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("0 images found"));
}

// ── P1: --validate integration tests ───────────────────────────────

#[test]
fn validate_minimal_pdf() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--validate")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    // A well-formed minimal PDF should pass validation or have only warnings
    assert!(stdout.contains("[OK]") || stdout.contains("Summary:"));
}

#[test]
fn validate_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--validate")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["error_count"].is_number());
    assert!(parsed["warning_count"].is_number());
    assert!(parsed["issues"].is_array());
}

#[test]
fn validate_mutually_exclusive() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--validate")
        .arg("--fonts")
        .output()
        .expect("failed to execute binary");

    // Document-level modes are now combinable
    assert!(
        output.status.success(),
        "validate + fonts should succeed (combinable modes)"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("=== Validate ==="),
        "should show Validate header"
    );
    assert!(stdout.contains("=== Fonts ==="), "should show Fonts header");
}

// ── --tree integration tests ─────────────────────────────────────────

#[test]
fn tree_shows_reference_tree() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Reference Tree:"),
        "Should have tree header: {}",
        stdout
    );
    assert!(
        stdout.contains("Trailer"),
        "Should show Trailer root: {}",
        stdout
    );
    assert!(
        stdout.contains("/Root ->"),
        "Should show /Root reference: {}",
        stdout
    );
    assert!(
        stdout.contains("Catalog"),
        "Should identify Catalog: {}",
        stdout
    );
}

#[test]
fn tree_json_valid_structure() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--json")
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["tree"]["node"], "Trailer");
    assert!(parsed["tree"]["children"].is_array());
}

#[test]
fn tree_with_depth_limit() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--depth")
        .arg("1")
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Reference Tree:"));
    assert!(
        stdout.contains("depth limit reached"),
        "Should show depth limit: {}",
        stdout
    );
}

#[test]
fn tree_mutually_exclusive() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--list")
        .output()
        .expect("failed to execute binary");
    // Document-level modes are now combinable
    assert!(
        output.status.success(),
        "tree + list should succeed (combinable modes)"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("=== Tree ==="), "should show Tree header");
    assert!(stdout.contains("=== List ==="), "should show List header");
}

// ── P2 gap: filter pipeline integration tests ───────────────────────

fn create_pdf_with_asciihex_flate_pipeline() -> (tempfile::NamedTempFile, u32) {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    let mut doc = Document::new();

    // Original content, compressed with FlateDecode, then ASCIIHex-encoded
    let original = b"Pipeline integration test content";
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(original).unwrap();
    let compressed = encoder.finish().unwrap();
    let hex_encoded: String = compressed.iter().map(|b| format!("{:02x}", b)).collect();
    let hex_bytes = format!("{}>", hex_encoded).into_bytes();

    let mut stream_dict = Dictionary::new();
    stream_dict.set(
        "Filter",
        Object::Array(vec![
            Object::Name(b"ASCIIHexDecode".to_vec()),
            Object::Name(b"FlateDecode".to_vec()),
        ]),
    );
    let stream = Stream::new(stream_dict, hex_bytes);
    let stream_id = doc.add_object(Object::Stream(stream));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Data", Object::Reference(stream_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    (tmp, stream_id.0)
}

#[test]
fn pipeline_asciihex_flate_decode_integration() {
    let (pdf, obj_num) = create_pdf_with_asciihex_flate_pipeline();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg(obj_num.to_string())
        .arg("--decode")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("decoded"),
        "Should show decoded description: {}",
        stdout
    );
    assert!(
        stdout.contains("Pipeline integration test content"),
        "Should decode pipeline: {}",
        stdout
    );
    assert!(
        !stdout.contains("WARNING"),
        "Successful pipeline should have no warning"
    );
}

#[test]
fn pipeline_asciihex_flate_json() {
    let (pdf, obj_num) = create_pdf_with_asciihex_flate_pipeline();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg(obj_num.to_string())
        .arg("--decode")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        parsed["object"]["content"].is_string(),
        "JSON should have decoded content"
    );
    assert!(
        parsed["object"].get("decode_warning").is_none(),
        "No warning expected for valid pipeline"
    );
}

// ── P2 gap: truncate with --hex ─────────────────────────────────────

#[test]
fn truncate_with_hex_flag() {
    let (pdf, obj_num) = create_pdf_with_binary_stream();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg(obj_num.to_string())
        .arg("--decode")
        .arg("--hex")
        .arg("--truncate")
        .arg("16")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("truncated to 16"),
        "Should show truncation: {}",
        stdout
    );
    assert!(stdout.contains("00000000"), "Should show hex dump");
    // With 16 bytes truncation, should have exactly 1 hex line (no second offset)
    assert!(
        !stdout.contains("00000010"),
        "Should not have second hex line with 16-byte truncation: {}",
        stdout
    );
}

// ── P2 gap: tree with missing objects (integration) ─────────────────

#[test]
fn tree_with_broken_reference() {
    let mut doc = Document::new();
    // Catalog references a nonexistent object
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference((999, 0)));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("(missing)"),
        "Missing object should show (missing) in tree: {}",
        stdout
    );
}

#[test]
fn tree_json_with_broken_reference() {
    let mut doc = Document::new();
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference((999, 0)));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let tree_str = serde_json::to_string(&parsed).unwrap();
    assert!(
        tree_str.contains("\"missing\""),
        "JSON tree should contain missing status: {}",
        tree_str
    );
}

// ── P2 gap: depth with --page mode ──────────────────────────────────

#[test]
fn depth_with_page_still_works() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--page")
        .arg("1")
        .arg("--depth")
        .arg("0")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "--page with --depth should still succeed"
    );
    assert!(
        stdout.contains("Page 1"),
        "Should show page info: {}",
        stdout
    );
}

#[test]
fn depth_with_page_unlimited() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--page")
        .arg("1")
        .arg("--depth")
        .arg("100")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Page 1"),
        "Should show page info: {}",
        stdout
    );
}

// ── P2 gap: depth=1 with --tree (verify children are limited) ───────

#[test]
fn tree_depth_one_shows_root_children_only() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--depth")
        .arg("2")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Reference Tree:"));
    assert!(stdout.contains("Catalog"), "Should show Catalog at depth 2");
}

// ── P2 gap: tree --json --depth combined ────────────────────────────

#[test]
fn tree_json_depth_zero() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--json")
        .arg("--depth")
        .arg("0")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["tree"]["node"], "Trailer");
    let tree_str = serde_json::to_string(&parsed).unwrap();
    assert!(
        tree_str.contains("depth_limit_reached"),
        "Depth 0 should limit all children: {}",
        tree_str
    );
}

// ── P2 gap: extract with decode warning ─────────────────────────────

#[test]
fn extract_corrupt_stream_shows_warning_on_stderr() {
    let mut doc = Document::new();
    let mut stream_dict = Dictionary::new();
    stream_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    let stream = Stream::new(stream_dict, b"not valid zlib data".to_vec());
    let stream_id = doc.add_object(Object::Stream(stream));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output_file = tempfile::NamedTempFile::new().unwrap();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--extract-stream")
        .arg(stream_id.0.to_string())
        .arg("--output")
        .arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Warning"),
        "Extract should show warning on stderr: {}",
        stderr
    );
}

// ── P2 gap: tree with stream objects ────────────────────────────────

#[test]
fn tree_with_stream_object() {
    let mut doc = Document::new();

    let stream = Stream::new(Dictionary::new(), vec![1, 2, 3, 4, 5]);
    let stream_id = doc.add_object(Object::Stream(stream));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Data", Object::Reference(stream_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Stream"),
        "Tree should show Stream label: {}",
        stdout
    );
    assert!(
        stdout.contains("bytes"),
        "Tree should show byte count for streams: {}",
        stdout
    );
}

// ── --bookmarks integration tests ───────────────────────────────────

#[test]
fn bookmarks_on_pdf_with_outlines() {
    let mut doc = Document::new();

    // Bookmark
    let mut bm = Dictionary::new();
    bm.set(
        "Title",
        Object::String(b"Introduction".to_vec(), lopdf::StringFormat::Literal),
    );
    let bm_id = doc.add_object(Object::Dictionary(bm));

    let mut outlines = Dictionary::new();
    outlines.set("Type", Object::Name(b"Outlines".to_vec()));
    outlines.set("First", Object::Reference(bm_id));
    let outlines_id = doc.add_object(Object::Dictionary(outlines));

    // Page + Pages for catalog
    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let page_id = doc.add_object(Object::Dictionary(page_dict));
    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages_dict.set("Count", Object::Integer(1));
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));
    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    catalog.set("Outlines", Object::Reference(outlines_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--bookmarks")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Introduction"),
        "Should show bookmark title: {}",
        stdout
    );
    assert!(
        stdout.contains("1 bookmarks"),
        "Should show count: {}",
        stdout
    );
}

#[test]
fn bookmarks_no_outlines_says_no_bookmarks() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--bookmarks")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("No bookmarks"),
        "Should indicate no bookmarks: {}",
        stdout
    );
}

// ── --annotations integration tests ─────────────────────────────────

#[test]
fn annotations_on_pdf_with_link() {
    let mut doc = Document::new();

    let mut annot = Dictionary::new();
    annot.set("Type", Object::Name(b"Annot".to_vec()));
    annot.set("Subtype", Object::Name(b"Link".to_vec()));
    annot.set(
        "Rect",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(100),
            Object::Integer(50),
        ]),
    );
    annot.set(
        "Contents",
        Object::String(b"Test link".to_vec(), lopdf::StringFormat::Literal),
    );
    let annot_id = doc.add_object(Object::Dictionary(annot));

    let content_stream = Stream::new(Dictionary::new(), b"BT ET".to_vec());
    let content_id = doc.add_object(Object::Stream(content_stream));

    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set("Contents", Object::Reference(content_id));
    page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let page_id = doc.add_object(Object::Dictionary(page_dict));

    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages_dict.set("Count", Object::Integer(1));
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));

    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--annotations")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("1 annotations found"),
        "Should show count: {}",
        stdout
    );
    assert!(stdout.contains("Link"), "Should show subtype: {}", stdout);
    assert!(
        stdout.contains("Test link"),
        "Should show contents: {}",
        stdout
    );
}

#[test]
fn annotations_with_page_filter() {
    let mut doc = Document::new();

    let mut annot = Dictionary::new();
    annot.set("Type", Object::Name(b"Annot".to_vec()));
    annot.set("Subtype", Object::Name(b"Text".to_vec()));
    annot.set(
        "Rect",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(50),
            Object::Integer(50),
        ]),
    );
    let annot_id = doc.add_object(Object::Dictionary(annot));

    let content_stream = Stream::new(Dictionary::new(), b"BT ET".to_vec());
    let content_id = doc.add_object(Object::Stream(content_stream));

    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set("Contents", Object::Reference(content_id));
    page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let page_id = doc.add_object(Object::Dictionary(page_dict));

    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages_dict.set("Count", Object::Integer(1));
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));

    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    // --annotations --page 1 should work
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--annotations")
        .arg("--page")
        .arg("1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("1 annotations found"),
        "Should find annotation on page 1: {}",
        stdout
    );
}

// ── --tree --dot integration tests ──────────────────────────────────

#[test]
fn tree_dot_produces_valid_dot() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--tree")
        .arg("--dot")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("digraph pdf {"),
        "Should contain digraph: {}",
        stdout
    );
    assert!(stdout.contains("->"), "Should contain edges: {}", stdout);
    assert!(
        stdout.trim_end().ends_with("}"),
        "Should end with }}: {}",
        stdout
    );
}

#[test]
fn tree_dot_with_depth_limit() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--tree")
        .arg("--dot")
        .arg("--depth")
        .arg("1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("digraph pdf {"),
        "Should contain digraph: {}",
        stdout
    );
}

// ── Mode mutual exclusivity for new modes ───────────────────────────

#[test]
fn bookmarks_and_fonts_mutual_exclusion() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--bookmarks")
        .arg("--fonts")
        .output()
        .expect("failed to execute binary");
    // Document-level modes are now combinable
    assert!(
        output.status.success(),
        "bookmarks + fonts should succeed (combinable modes)"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("=== Bookmarks ==="),
        "should show Bookmarks header"
    );
    assert!(stdout.contains("=== Fonts ==="), "should show Fonts header");
}

// ── --page range integration tests ──────────────────────────────────

/// Create a PDF with two pages for range testing.
fn create_two_page_pdf_for_range() -> tempfile::NamedTempFile {
    let mut doc = Document::new();

    // Page 1
    let c1 = Stream::new(Dictionary::new(), b"BT (Page1) Tj ET".to_vec());
    let c1_id = doc.add_object(Object::Stream(c1));
    let mut p1 = Dictionary::new();
    p1.set("Type", Object::Name(b"Page".to_vec()));
    p1.set("Contents", Object::Reference(c1_id));
    p1.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let p1_id = doc.add_object(Object::Dictionary(p1));

    // Page 2
    let c2 = Stream::new(Dictionary::new(), b"BT (Page2) Tj ET".to_vec());
    let c2_id = doc.add_object(Object::Stream(c2));
    let mut p2 = Dictionary::new();
    p2.set("Type", Object::Name(b"Page".to_vec()));
    p2.set("Contents", Object::Reference(c2_id));
    p2.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let p2_id = doc.add_object(Object::Dictionary(p2));

    // Pages
    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set(
        "Kids",
        Object::Array(vec![Object::Reference(p1_id), Object::Reference(p2_id)]),
    );
    pages_dict.set("Count", Object::Integer(2));
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));

    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(p1_id) {
        d.set("Parent", Object::Reference(pages_id));
    }
    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(p2_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    tmp
}

#[test]
fn page_range_dump() {
    let pdf = create_two_page_pdf_for_range();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("1-2")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Page 1"),
        "Should contain Page 1: {}",
        stdout
    );
    assert!(
        stdout.contains("Page 2"),
        "Should contain Page 2: {}",
        stdout
    );
}

#[test]
fn page_range_text() {
    let pdf = create_two_page_pdf_for_range();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--page")
        .arg("1-2")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("--- Page 1 ---"),
        "Should contain Page 1 header: {}",
        stdout
    );
    assert!(
        stdout.contains("--- Page 2 ---"),
        "Should contain Page 2 header: {}",
        stdout
    );
}

#[test]
fn page_zero_rejected() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("0")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success(), "Page 0 should be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must be >= 1") || stderr.contains("Error"),
        "Should show error: {}",
        stderr
    );
}

#[test]
fn page_invalid_range_rejected() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("5-3")
        .output()
        .expect("failed to execute binary");

    assert!(
        !output.status.success(),
        "Reversed range should be rejected"
    );
}

// ── exit code 3: input had problems ────────────────────────────────

#[test]
fn page_out_of_range_exits_3() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("99")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(3),
        "--page out of range should exit 3, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn page_out_of_range_json_exits_3() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("99")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(3),
        "--page out of range with --json should exit 3"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        parsed["error"].is_string(),
        "JSON should carry an error field"
    );
}

// ── --page open-range (e.g. "2-") ──────────────────────────────────

#[test]
fn page_open_range_dumps_from_start_to_last() {
    let pdf = create_two_page_pdf_for_range();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("2-")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "open range should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Page 1"),
        "should NOT contain Page 1: {}",
        stdout
    );
    assert!(
        stdout.contains("Page 2"),
        "should contain Page 2: {}",
        stdout
    );
}

#[test]
fn page_open_range_above_last_exits_3() {
    let pdf = create_two_page_pdf_for_range();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("99-")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(3),
        "open range above last page should exit 3"
    );
}

// ── --text font-aware decoding + reliability detection ─────────────

/// Build a single-page PDF whose page uses one font `/F1` (optionally with a
/// ToUnicode CMap stream) and the given content stream.
fn create_pdf_with_font(
    mut font_dict: Dictionary,
    tounicode: Option<&[u8]>,
    content: &[u8],
) -> tempfile::NamedTempFile {
    let mut doc = Document::new();

    let content_stream = Stream::new(Dictionary::new(), content.to_vec());
    let content_id = doc.add_object(Object::Stream(content_stream));

    if let Some(tu) = tounicode {
        let tu_stream = Stream::new(Dictionary::new(), tu.to_vec());
        let tu_id = doc.add_object(Object::Stream(tu_stream));
        font_dict.set("ToUnicode", Object::Reference(tu_id));
    }
    let font_id = doc.add_object(Object::Dictionary(font_dict));

    let mut f1_dict = Dictionary::new();
    f1_dict.set("F1", Object::Reference(font_id));
    let mut resources = Dictionary::new();
    resources.set("Font", Object::Dictionary(f1_dict));

    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set("Contents", Object::Reference(content_id));
    page_dict.set("Resources", Object::Dictionary(resources));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let page_id = doc.add_object(Object::Dictionary(page_dict));

    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages_dict.set("Count", Object::Integer(1));
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));
    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    tmp
}

fn type0_font_dict(base_font: &[u8]) -> Dictionary {
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type0".to_vec()));
    font.set("BaseFont", Object::Name(base_font.to_vec()));
    font
}

#[test]
fn text_type0_with_tounicode_decodes_and_exits_0() {
    // Codes 0x0041 -> 'H', 0x0042 -> 'i'.
    let cmap = b"begincodespacerange <0000> <FFFF> endcodespacerange \
                 beginbfchar <0041> <0048> <0042> <0069> endbfchar";
    let pdf = create_pdf_with_font(
        type0_font_dict(b"ABCDEF+Custom"),
        Some(cmap),
        b"BT /F1 12 Tf <00410042> Tj ET",
    );
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "reliable ToUnicode decode should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Hi"),
        "should decode 2-byte codes via ToUnicode, got: {}",
        stdout
    );
}

#[test]
fn text_cid_without_tounicode_exits_3_with_banner() {
    let pdf = create_pdf_with_font(
        type0_font_dict(b"ABCDEF+NoMap"),
        None,
        b"BT /F1 12 Tf <0041> Tj ET",
    );
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(3),
        "CID font without ToUnicode should exit 3, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("UNRELIABLE") && stderr.contains("TEXT EXTRACTION RELIABILITY"),
        "should print the loud reliability banner, stderr: {}",
        stderr
    );
}

#[test]
fn text_reliable_font_exits_0_no_banner() {
    // create_minimal_pdf uses standard-14 Helvetica.
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "reliable text should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Hello"));
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("RELIABILITY"),
        "no banner on the happy path"
    );
}

#[test]
fn text_cid_without_tounicode_json_reliability_object() {
    let pdf = create_pdf_with_font(
        type0_font_dict(b"ABCDEF+NoMap"),
        None,
        b"BT /F1 12 Tf <0041> Tj ET",
    );
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(3),
        "unreliable --text --json should exit 3"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["reliability"]["verdict"], "unreliable");
    assert!(parsed["reliability"]["fonts"].is_array());
    assert!(parsed["pages"].is_array());
}

// ── Lenient /Length recovery + --strict (recovery surfacing) ─────────

/// Build a single-page PDF whose content stream declares a deliberately wrong
/// `/Length` (so a strict reader drops its body and the page text vanishes),
/// while preserving byte offsets so lopdf's xref stays valid. Returns the temp
/// file and the content stream's object number. The page draws the literal
/// token `HelloRecovery`, present only when the stream body is recovered.
fn create_malformed_length_pdf() -> (tempfile::NamedTempFile, u32) {
    let mut doc = Document::new();

    let content_bytes = b"BT\n/F1 24 Tf\n72 700 Td\n(HelloRecovery) Tj\nET";
    let content_stream = Stream::new(Dictionary::new(), content_bytes.to_vec());
    let content_id = doc.add_object(Object::Stream(content_stream));

    let mut font_dict = Dictionary::new();
    font_dict.set("Type", Object::Name(b"Font".to_vec()));
    font_dict.set("Subtype", Object::Name(b"Type1".to_vec()));
    font_dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    let font_id = doc.add_object(Object::Dictionary(font_dict));

    let mut f1_dict = Dictionary::new();
    f1_dict.set("F1", Object::Reference(font_id));
    let mut resources = Dictionary::new();
    resources.set("Font", Object::Dictionary(f1_dict));

    let mut page_dict = Dictionary::new();
    page_dict.set("Type", Object::Name(b"Page".to_vec()));
    page_dict.set("Contents", Object::Reference(content_id));
    page_dict.set("Resources", Object::Dictionary(resources));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let page_id = doc.add_object(Object::Dictionary(page_dict));

    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages_dict.set("Count", Object::Integer(1));
    let pages_id = doc.add_object(Object::Dictionary(pages_dict));

    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    // Serialize, then corrupt the content stream's /Length in place. Keeping the
    // same digit width leaves every xref offset valid, so lopdf still locates
    // the object (and then drops its body, which recovery puts back).
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    corrupt_first_length(&mut buf);

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(&buf).unwrap();
    tmp.flush().unwrap();
    (tmp, content_id.0)
}

/// Replace the first `/Length <n>` value with a same-width wrong value (all 9s,
/// which over-reads past the real body so no `endstream` is found where the
/// length claims) so the declared length no longer matches the stream body.
fn corrupt_first_length(bytes: &mut [u8]) {
    let needle = b"/Length ";
    let pos = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("content stream should have a /Length");
    let start = pos + needle.len();
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    assert!(end > start, "expected digits after /Length");
    let replacement = if bytes[start..end].iter().all(|&d| d == b'9') {
        b'1'
    } else {
        b'9'
    };
    for b in &mut bytes[start..end] {
        *b = replacement;
    }
}

#[test]
fn malformed_length_recovered_in_text_json() {
    let (pdf, _content) = create_malformed_length_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "default (tolerant) mode should exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MALFORMED PDF") && stderr.contains("recovered"),
        "loud recovery banner expected on stderr: {}",
        stderr
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["recovery"]["repaired"], serde_json::json!(true));
    assert_eq!(parsed["recovery"]["strict"], serde_json::json!(false));
    assert!(parsed["recovery"]["count"].as_u64().unwrap() >= 1);
    assert!(
        stdout.contains("HelloRecovery"),
        "recovered page text should appear: {}",
        stdout
    );
}

#[test]
fn malformed_length_recovery_key_in_object_json() {
    let (pdf, content) = create_malformed_length_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object")
        .arg(content.to_string())
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    // The standalone --object path carries the same recovery diagnostic.
    assert_eq!(parsed["recovery"]["repaired"], serde_json::json!(true));
    assert_eq!(
        parsed["recovery"]["streams"][0]["object"],
        serde_json::json!(content)
    );
}

#[test]
fn wellformed_pdf_has_no_recovery_key() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(
        parsed.get("recovery").is_none(),
        "a clean PDF must not add a recovery key: {}",
        stdout
    );
}

#[test]
fn strict_refuses_repair_and_exits_3() {
    let (pdf, _content) = create_malformed_length_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--strict")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(3),
        "strict mode must exit 3 on malformed input"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--strict") && stderr.contains("did NOT repair"),
        "strict banner expected: {}",
        stderr
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("HelloRecovery"),
        "strict mode must NOT recover the dropped text: {}",
        stdout
    );
}

#[test]
fn strict_json_marks_unrepaired() {
    let (pdf, _content) = create_malformed_length_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--json")
        .arg("--strict")
        .output()
        .expect("failed to execute binary");

    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["recovery"]["repaired"], serde_json::json!(false));
    assert_eq!(parsed["recovery"]["strict"], serde_json::json!(true));
    assert!(parsed["recovery"]["count"].as_u64().unwrap() >= 1);
}

#[test]
fn strict_on_clean_pdf_exits_0() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--strict")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "strict on a clean PDF should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Encrypted PDFs: overview correctness + --password ────────────────

/// Build a single-page PDF encrypted with V1 (RC4) and a NON-empty user
/// password, written to a temp file. Opened without the password it reproduces
/// the degraded-load bug; with `--password` it should read fully.
fn create_encrypted_pdf(user_password: &str) -> tempfile::NamedTempFile {
    use lopdf::{EncryptionState, EncryptionVersion, Permissions, StringFormat};

    let mut doc = Document::new();

    let content = Stream::new(
        Dictionary::new(),
        b"BT\n/F1 12 Tf\n(Secret) Tj\nET".to_vec(),
    );
    let content_id = doc.add_object(Object::Stream(content));

    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    let font_id = doc.add_object(Object::Dictionary(font));

    let mut f1 = Dictionary::new();
    f1.set("F1", Object::Reference(font_id));
    let mut resources = Dictionary::new();
    resources.set("Font", Object::Dictionary(f1));

    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Contents", Object::Reference(content_id));
    page.set("Resources", Object::Dictionary(resources));
    page.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    let page_id = doc.add_object(Object::Dictionary(page));

    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    let pages_id = doc.add_object(Object::Dictionary(pages));

    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
        d.set("Parent", Object::Reference(pages_id));
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    // File ID is required for encryption.
    doc.trailer.set(
        "ID",
        Object::Array(vec![
            Object::String(vec![1u8; 16], StringFormat::Hexadecimal),
            Object::String(vec![1u8; 16], StringFormat::Hexadecimal),
        ]),
    );

    let state = EncryptionState::try_from(EncryptionVersion::V1 {
        document: &doc,
        owner_password: "owner",
        user_password,
        permissions: Permissions::all(),
    })
    .expect("failed to build encryption state");
    doc.encrypt(&state).expect("failed to encrypt document");

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();
    tmp
}

#[test]
fn encrypted_pdf_overview_reports_encrypted_and_exits_3() {
    let pdf = create_encrypted_pdf("secret");
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(3),
        "encrypted-but-undecrypted overview must exit 3"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ENCRYPTED PDF") && stderr.contains("NOT DECRYPTED"),
        "loud encryption banner expected on stderr: {}",
        stderr
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("valid json");
    assert_eq!(parsed["encrypted"], serde_json::json!(true));
    assert_eq!(parsed["decrypted"], serde_json::json!(false));
    assert!(parsed["validation"]["warning_count"].as_u64().unwrap() >= 1);
}

#[test]
fn encrypted_overview_agrees_with_detail_security() {
    // Regression tie (bug report test plan #2): json.encrypted must equal what
    // --detail security reports — the two paths must never contradict.
    let pdf = create_encrypted_pdf("secret");

    let json_out = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--json")
        .output()
        .expect("failed to execute binary");
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&json_out.stdout)).expect("valid json");

    let sec_out = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--detail")
        .arg("security")
        .output()
        .expect("failed to execute binary");
    let security_says_encrypted =
        String::from_utf8_lossy(&sec_out.stdout).contains("Encryption: Yes");

    assert_eq!(
        parsed["encrypted"],
        serde_json::json!(security_says_encrypted)
    );
    assert!(
        security_says_encrypted,
        "security detail should report this fixture as encrypted"
    );
}

#[test]
fn encrypted_pdf_with_correct_password_reads_fully() {
    let pdf = create_encrypted_pdf("secret");
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--password")
        .arg("secret")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "the correct --password should exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("valid json");
    assert_eq!(parsed["encrypted"], serde_json::json!(true));
    assert_eq!(parsed["decrypted"], serde_json::json!(true));
    assert!(
        parsed["object_count"].as_u64().unwrap() > 1,
        "full object set expected after decrypt"
    );
    assert_eq!(parsed["page_count"], serde_json::json!(1));
}

#[test]
fn encrypted_pdf_with_wrong_password_exits_1() {
    let pdf = create_encrypted_pdf("secret");
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--password")
        .arg("nope")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a wrong --password must exit 1, not silently degrade"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("password"),
        "the error should mention the password: {}",
        stderr
    );
}

#[test]
fn plain_pdf_overview_has_no_decrypted_key() {
    // Control: a non-encrypted file is unchanged — encrypted:false, no decrypted
    // key, exit 0.
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("valid json");
    assert_eq!(parsed["encrypted"], serde_json::json!(false));
    assert!(
        parsed.get("decrypted").is_none(),
        "a non-encrypted PDF must not add a decrypted key"
    );
}
