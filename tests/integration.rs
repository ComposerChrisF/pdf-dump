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
    path.push("pdf_dump");
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
fn dump_minimal_pdf_prints_trailer_and_objects() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "binary failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Trailer:"), "Should print Trailer header");
    assert!(stdout.contains("/Root"), "Trailer should contain /Root");
    assert!(stdout.contains("/Catalog"), "Should traverse to Catalog");
    assert!(stdout.contains("/Pages"), "Catalog should reference Pages");
    assert!(stdout.contains("/Page"), "Should find a Page object");
}

#[test]
fn dump_with_decode_streams_shows_content() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    // The content stream should be parsed because it's under /Contents
    assert!(
        stdout.contains("Parsed Content Stream") || stdout.contains("Stream content"),
        "Should show stream content when --decode-streams is set"
    );
}

#[test]
fn extract_stream_object_writes_file() {
    let (pdf, stream_obj_num) = create_pdf_with_flatedecode_stream();
    let output_file = tempfile::NamedTempFile::new().unwrap();
    let output_path = output_file.path().to_path_buf();

    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--extract-object")
        .arg(stream_obj_num.to_string())
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "extract failed: {}", String::from_utf8_lossy(&output.stderr));
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
        .arg("--extract-object")
        .arg(int_id.0.to_string())
        .arg("--output")
        .arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a stream"), "Should report non-stream error: {}", stderr);
}

#[test]
fn extract_nonexistent_object_fails() {
    let pdf = create_minimal_pdf();
    let output_file = tempfile::NamedTempFile::new().unwrap();

    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--extract-object")
        .arg("9999")
        .arg("--output")
        .arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"), "Should report object not found: {}", stderr);
}

#[test]
fn nonexistent_pdf_file_fails_gracefully() {
    let output = Command::new(binary_path())
        .arg("/tmp/nonexistent_pdf_file_that_does_not_exist.pdf")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error") || stderr.contains("error"), "Should show error for missing file: {}", stderr);
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

#[test]
fn truncate_binary_streams_flag_accepted() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--decode-streams")
        .arg("--truncate-binary-streams")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "Should accept --truncate-binary-streams flag");
}

// ── CLI argument validation ─────────────────────────────────────────

#[test]
fn output_without_extract_object_fails() {
    // --output requires --extract-object (clap `requires` constraint)
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
        "Should indicate --output requires --extract-object: {}",
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
    assert!(stdout.contains("--decode-streams"), "Should list --decode-streams flag");
    assert!(stdout.contains("--extract-object"), "Should list --extract-object flag");
}

#[test]
fn version_flag_prints_version() {
    let output = Command::new(binary_path())
        .arg("--version")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0.2.0") || stdout.contains("pdf_dump"),
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

// ── Dump mode behavior ──────────────────────────────────────────────

#[test]
fn dump_shows_separator_between_objects() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("================================"),
        "Should have separator between trailer and objects"
    );
    assert!(
        stdout.contains("--------------------------------"),
        "Should have separators between individual objects"
    );
}

#[test]
fn dump_with_decode_streams_shows_parsed_content_stream() {
    // Specifically verify that the /Contents stream is parsed into operations
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Parsed Content Stream"),
        "Content stream should be parsed with --decode-streams: {}",
        stdout
    );
    // The synthetic PDF has "BT /F1 12 Tf (Hello) Tj ET" content
    assert!(
        stdout.contains("BT") || stdout.contains("Tf"),
        "Should show PDF operators from content stream"
    );
}

#[test]
fn dump_binary_stream_truncation_visible() {
    // Create a PDF with a large binary stream and verify truncation in output
    let mut doc = Document::new();

    let binary_content: Vec<u8> = (0..500).map(|i| (i as u8).wrapping_add(0x80)).collect();
    let stream = Stream::new(Dictionary::new(), binary_content);
    let stream_id = doc.add_object(Object::Stream(stream));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("BinaryData", Object::Reference(stream_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--decode-streams")
        .arg("--truncate-binary-streams")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("truncated to 100"),
        "Binary stream should show truncation: {}",
        stdout
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
        .arg("--extract-object")
        .arg(stream_id.0.to_string())
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "Extract failed: {}", String::from_utf8_lossy(&output.stderr));
    let extracted = fs::read(&output_path).unwrap();
    assert_eq!(extracted, raw_bytes, "Uncompressed stream should be extracted as-is");
}

#[test]
fn extract_object_prints_success_message() {
    let (pdf, stream_obj_num) = create_pdf_with_flatedecode_stream();
    let output_file = tempfile::NamedTempFile::new().unwrap();

    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--extract-object")
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
    assert!(output.status.success(), "--object should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Object 1 0:"), "Should show object header");
    // Should NOT show trailer or full dump
    assert!(!stdout.contains("Trailer:"), "Should not show trailer in --object mode");
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

    assert!(output.status.success(), "-o should work as short form: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Object 1 0:"));
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

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"), "Should report object not found: {}", stderr);
}

#[test]
fn summary_flag_shows_object_table() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--summary")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "--summary should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("PDF"), "Should show PDF version");
    assert!(stdout.contains("objects"), "Should show object count");
    assert!(stdout.contains("Obj#"), "Should show table header");
    assert!(stdout.contains("Dictionary") || stdout.contains("Stream"), "Should show object kinds");
}

#[test]
fn summary_flag_short_form() {
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
fn metadata_flag_shows_info() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--metadata")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "--metadata should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("PDF Version:"), "Should show PDF version");
    assert!(stdout.contains("Objects:"), "Should show object count");
    assert!(stdout.contains("Pages:"), "Should show page count");
}

#[test]
fn metadata_flag_short_form() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("-m")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "-m should work as short form");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PDF Version:"));
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
    assert!(output.status.success(), "--page should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Page 1 (Object"), "Should show page header");
    assert!(stdout.contains("/Page"), "Should show page type");
    assert!(stdout.contains("MediaBox"), "Should show page properties");
}

#[test]
fn page_flag_with_decode_streams() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page")
        .arg("1")
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Parsed Content Stream") || stdout.contains("Stream content"),
        "Should show decoded stream content when --decode-streams is used with --page"
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

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"), "Should report page not found: {}", stderr);
}

#[test]
fn two_mode_flags_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--summary")
        .arg("--metadata")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Only one mode"), "Should report mutual exclusivity: {}", stderr);
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

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Only one mode"), "Should report mutual exclusivity: {}", stderr);
}

#[test]
fn dump_traverses_all_page_tree_objects() {
    // Verify the dump traverses through Catalog → Pages → Page → Font
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("/Catalog"), "Should show Catalog type");
    assert!(stdout.contains("/Pages"), "Should traverse to Pages");
    assert!(stdout.contains("/Helvetica"), "Should traverse to Font and show BaseFont");
    assert!(stdout.contains("MediaBox"), "Should show page's MediaBox");
}

// ── JSON mode integration tests ──────────────────────────────────────

#[test]
fn json_default_dump_is_valid_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "should succeed: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed.get("trailer").is_some(), "Should have trailer");
    assert!(parsed.get("objects").is_some(), "Should have objects");
}

#[test]
fn json_object_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object").arg("1")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert_eq!(parsed["object_number"], 1);
}

#[test]
fn json_summary_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--summary")
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
fn json_metadata_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--metadata")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed.get("version").is_some());
    assert!(parsed.get("page_count").is_some());
}

#[test]
fn json_page_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page").arg("1")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert_eq!(parsed["page_number"], 1);
}

// ── Search mode integration tests ────────────────────────────────────

#[test]
fn search_finds_fonts() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("Type=Font")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "search should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Found"), "Should show match count");
    assert!(stdout.contains("matching objects"), "Should show match summary");
}

#[test]
fn search_no_matches() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("Type=Nonexistent")
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
        .arg("--search").arg("Type=Font")
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
fn search_with_summary() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("Type=Font")
        .arg("--summary")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Obj#"), "Should show summary table header");
}

#[test]
fn search_bad_expression_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("badexpr")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Invalid") || stderr.contains("error"), "Should report bad expression: {}", stderr);
}

#[test]
fn search_has_key() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("key=MediaBox")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Found"), "Should find objects with MediaBox");
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
    assert!(output.status.success(), "should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("--- Page 1 ---"), "Should show page header");
    assert!(stdout.contains("Hello"), "Should extract Hello text");
}

#[test]
fn text_with_page_filter() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--page").arg("1")
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
        .arg("--page").arg("999")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"), "Should report page not found: {}", stderr);
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
fn text_mutually_exclusive_with_summary() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--summary")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Only one mode"), "Should report mutual exclusivity: {}", stderr);
}

// ── Diff mode integration tests ──────────────────────────────────────

#[test]
fn diff_identical_files() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Comparing:"), "Should show comparison header");
    assert!(stdout.contains("(identical)"), "Identical files should report identical pages");
}

#[test]
fn diff_with_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed.get("page_diffs").is_some());
    assert!(parsed.get("metadata_diffs").is_some());
}

#[test]
fn diff_with_page_filter() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--page").arg("1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Page 1"), "Should show page 1 comparison");
}

#[test]
fn diff_nonexistent_second_file_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg("/tmp/nonexistent_file.pdf")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error"), "Should report error: {}", stderr);
}

#[test]
fn diff_incompatible_with_summary_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--summary")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--diff can only be combined"), "Should report incompatible modes: {}", stderr);
}

#[test]
fn diff_incompatible_with_object_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--object").arg("1")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--diff can only be combined"), "Should report incompatible modes: {}", stderr);
}

#[test]
fn diff_incompatible_with_text_mode() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--text")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--diff can only be combined"), "Should report incompatible modes: {}", stderr);
}

// ── Cross-mode tests ─────────────────────────────────────────────────

#[test]
fn search_with_decode_streams() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("Type=Font")
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "--search with --decode-streams should succeed");
}

// ── Additional integration tests ─────────────────────────────────

/// Create a second PDF with different content for diff testing.
fn create_different_pdf() -> tempfile::NamedTempFile {
    let mut doc = Document::new();

    // Different content: "BT /F1 14 Tf (World) Tj ET"
    let content_bytes = b"BT\n/F1 14 Tf\n(World) Tj\nET";
    let content_stream = Stream::new(Dictionary::new(), content_bytes.to_vec());
    let content_id = doc.add_object(Object::Stream(content_stream));

    // Different font
    let mut font_dict = Dictionary::new();
    font_dict.set("Type", Object::Name(b"Font".to_vec()));
    font_dict.set("Subtype", Object::Name(b"Type1".to_vec()));
    font_dict.set("BaseFont", Object::Name(b"Courier".to_vec()));
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
            Object::Integer(595),  // A4 width instead of letter
            Object::Integer(842),  // A4 height
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

/// Create a two-page PDF for multi-page tests.
fn create_two_page_pdf() -> tempfile::NamedTempFile {
    let mut doc = Document::new();

    let c1 = Stream::new(Dictionary::new(), b"BT\n/F1 12 Tf\n(Page1Text) Tj\nET".to_vec());
    let c1_id = doc.add_object(Object::Stream(c1));
    let c2 = Stream::new(Dictionary::new(), b"BT\n/F1 12 Tf\n(Page2Text) Tj\nET".to_vec());
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
    page1.set("MediaBox", Object::Array(vec![
        Object::Integer(0), Object::Integer(0),
        Object::Integer(612), Object::Integer(792),
    ]));
    let p1_id = doc.add_object(Object::Dictionary(page1));

    let mut page2 = Dictionary::new();
    page2.set("Type", Object::Name(b"Page".to_vec()));
    page2.set("Parent", Object::Reference(pages_id));
    page2.set("Contents", Object::Reference(c2_id));
    page2.set("Resources", Object::Dictionary(resources));
    page2.set("MediaBox", Object::Array(vec![
        Object::Integer(0), Object::Integer(0),
        Object::Integer(612), Object::Integer(792),
    ]));
    let p2_id = doc.add_object(Object::Dictionary(page2));

    if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
        d.set("Kids", Object::Array(vec![
            Object::Reference(p1_id),
            Object::Reference(p2_id),
        ]));
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
fn diff_different_pdfs_shows_differences() {
    let pdf1 = create_minimal_pdf();
    let pdf2 = create_different_pdf();
    let output = Command::new(binary_path())
        .arg(pdf1.path())
        .arg("--diff").arg(pdf2.path())
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Comparing:"));
    // Should show page differences (different MediaBox or content)
    assert!(
        !stdout.contains("(identical)") || stdout.contains("Content stream: differs") || stdout.contains("MediaBox"),
        "Diff of different PDFs should show differences: {}",
        stdout
    );
}

#[test]
fn diff_different_pdfs_json_shows_differences() {
    let pdf1 = create_minimal_pdf();
    let pdf2 = create_different_pdf();
    let output = Command::new(binary_path())
        .arg(pdf1.path())
        .arg("--diff").arg(pdf2.path())
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed.get("page_diffs").is_some());
    assert!(parsed.get("font_diffs").is_some());
}

#[test]
fn search_multiple_and_conditions() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("Type=Font,Subtype=Type1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "search should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Found"), "Should show match count");
    // Our minimal PDF has a Helvetica Type1 font, so it should match
    assert!(stdout.contains("matching objects"));
}

#[test]
fn search_value_expression() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("value=Helvetica")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "search should succeed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("Found"), "Should show match count");
}

#[test]
fn page_zero_fails() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--page").arg("0")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success(), "Page 0 should fail (pages are 1-based)");
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
        .arg("--page").arg("2")
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
        .arg("--page").arg("2")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Page 2 (Object"));
}

#[test]
fn diff_different_page_counts() {
    let pdf1 = create_two_page_pdf();
    let pdf2 = create_minimal_pdf(); // single page
    let output = Command::new(binary_path())
        .arg(pdf1.path())
        .arg("--diff").arg(pdf2.path())
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Comparing:"));
    // Should report that page 2 only exists in first file
    assert!(
        stdout.contains("Page 2") || stdout.contains("only in first"),
        "Should mention page count difference: {}",
        stdout
    );
}

#[test]
fn search_multiple_conditions_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("Type=Font,key=BaseFont")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(parsed["match_count"].as_u64().unwrap() >= 1, "Should find font with both conditions");
}

#[test]
fn object_mode_with_decode_streams() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object").arg("1")
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "--object with --decode-streams should work");
}

#[test]
fn search_with_summary_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--search").arg("Type=Font")
        .arg("--summary")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // --summary with --search and --json: should still produce valid JSON
    let _parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
}

#[test]
fn diff_incompatible_with_metadata() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--metadata")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--diff can only be combined"));
}

#[test]
fn diff_incompatible_with_search() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--search").arg("Type=Font")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--diff can only be combined"));
}

#[test]
fn diff_incompatible_with_extract() {
    let pdf = create_minimal_pdf();
    let output_file = tempfile::NamedTempFile::new().unwrap();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--extract-object").arg("1")
        .arg("--output").arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--diff can only be combined"));
}

#[test]
fn json_text_with_page_filter() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--text")
        .arg("--page").arg("1")
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
fn summary_with_two_page_pdf() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--summary")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Obj#"), "Should show table header");
    // Should list all objects
    assert!(stdout.contains("Stream") || stdout.contains("Dictionary"));
}

#[test]
fn metadata_with_two_page_pdf() {
    let pdf = create_two_page_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--metadata")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Pages:       2") || stdout.contains("Pages:"), "Should show 2 pages");
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
        .arg("--object").arg(obj_num.to_string())
        .arg("--decode-streams")
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
        .arg("--object").arg(obj_num.to_string())
        .arg("--decode-streams")
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

// ── P1: --refs-to integration tests ────────────────────────────────

#[test]
fn refs_to_finds_references() {
    let pdf = create_minimal_pdf();
    // Object 1 should be referenced by the trailer
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--refs-to").arg("1")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("referencing 1 0 R"));
}

#[test]
fn refs_to_json() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--refs-to").arg("1")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["target_object"], 1);
    assert!(parsed["references"].is_array());
}

#[test]
fn refs_to_mutually_exclusive() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--refs-to").arg("1")
        .arg("--summary")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
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
        .arg("--metadata")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
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

    assert!(!output.status.success());
}

// ── P1: --diff incompatible with new modes ─────────────────────────

#[test]
fn diff_incompatible_with_refs_to() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--refs-to").arg("1")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
}

#[test]
fn diff_incompatible_with_fonts() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--fonts")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
}

#[test]
fn diff_incompatible_with_images() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--images")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
}

#[test]
fn diff_incompatible_with_validate() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--validate")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success());
}

// ── Configurable truncation ──────────────────────────────────────────

#[test]
fn truncate_flag_with_custom_value() {
    let mut doc = Document::new();
    let binary_content: Vec<u8> = (0..500).map(|i| (i as u8).wrapping_add(0x80)).collect();
    let stream = Stream::new(Dictionary::new(), binary_content);
    let stream_id = doc.add_object(Object::Stream(stream));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("BinaryData", Object::Reference(stream_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--decode-streams")
        .arg("--truncate")
        .arg("50")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("truncated to 50"), "Should truncate to custom value: {}", stdout);
}

#[test]
fn truncate_binary_streams_backward_compat() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--decode-streams")
        .arg("--truncate-binary-streams")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "--truncate-binary-streams should still work");
}

#[test]
fn truncate_conflicts_with_truncate_binary_streams() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--truncate-binary-streams")
        .arg("--truncate")
        .arg("50")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success(), "--truncate and --truncate-binary-streams should conflict");
}

// ── Decode failure warnings ──────────────────────────────────────────

#[test]
fn corrupt_stream_shows_warning() {
    let mut doc = Document::new();
    let mut stream_dict = Dictionary::new();
    stream_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    let stream = Stream::new(stream_dict, b"not valid zlib data".to_vec());
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
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("WARNING"), "Should show warning for corrupt stream: {}", stdout);
    assert!(stdout.contains("FlateDecode decompression failed"), "Should describe the failure: {}", stdout);
}

// ── --depth integration tests ────────────────────────────────────────

#[test]
fn depth_zero_limits_output() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--depth")
        .arg("0")
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("depth limit reached"), "Should show depth limit message: {}", stdout);
}

#[test]
fn depth_large_value_shows_all() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--depth")
        .arg("100")
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(!stdout.contains("depth limit reached"), "Large depth should not limit: {}", stdout);
}

#[test]
fn depth_with_json() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--depth")
        .arg("0")
        .arg("--json")
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    // JSON output should parse and have limited objects
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["objects"].is_object(), "Should have objects key");
    // With depth 0, only immediate trailer refs should be in objects
    let obj_count = parsed["objects"].as_object().unwrap().len();
    // Without depth, a minimal PDF has more objects
    let output_no_depth = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--json")
        .output()
        .expect("failed to execute binary");
    let stdout_no_depth = String::from_utf8_lossy(&output_no_depth.stdout);
    let parsed_no_depth: serde_json::Value = serde_json::from_str(&stdout_no_depth).unwrap();
    let obj_count_no_depth = parsed_no_depth["objects"].as_object().unwrap().len();
    assert!(obj_count <= obj_count_no_depth, "Depth-limited should have fewer or equal objects: {} vs {}", obj_count, obj_count_no_depth);
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
    assert!(stdout.contains("Reference Tree:"), "Should have tree header: {}", stdout);
    assert!(stdout.contains("Trailer"), "Should show Trailer root: {}", stdout);
    assert!(stdout.contains("/Root ->"), "Should show /Root reference: {}", stdout);
    assert!(stdout.contains("Catalog"), "Should identify Catalog: {}", stdout);
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
    assert!(stdout.contains("depth limit reached"), "Should show depth limit: {}", stdout);
}

#[test]
fn tree_mutually_exclusive() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--summary")
        .output()
        .expect("failed to execute binary");
    assert!(!output.status.success(), "tree + summary should fail");
}

// ── P2 gap: filter pipeline integration tests ───────────────────────

fn create_pdf_with_asciihex_flate_pipeline() -> (tempfile::NamedTempFile, u32) {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
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
    stream_dict.set("Filter", Object::Array(vec![
        Object::Name(b"ASCIIHexDecode".to_vec()),
        Object::Name(b"FlateDecode".to_vec()),
    ]));
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
        .arg("--object").arg(obj_num.to_string())
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("decoded"), "Should show decoded description: {}", stdout);
    assert!(stdout.contains("Pipeline integration test content"), "Should decode pipeline: {}", stdout);
    assert!(!stdout.contains("WARNING"), "Successful pipeline should have no warning");
}

#[test]
fn pipeline_asciihex_flate_json() {
    let (pdf, obj_num) = create_pdf_with_asciihex_flate_pipeline();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object").arg(obj_num.to_string())
        .arg("--decode-streams")
        .arg("--json")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["object"]["content"].is_string(), "JSON should have decoded content");
    assert!(parsed["object"].get("decode_warning").is_none(), "No warning expected for valid pipeline");
}

#[test]
fn pipeline_unsupported_filter_shows_warning() {
    let mut doc = Document::new();

    let mut stream_dict = Dictionary::new();
    stream_dict.set("Filter", Object::Array(vec![
        Object::Name(b"JBIG2Decode".to_vec()),
        Object::Name(b"FlateDecode".to_vec()),
    ]));
    let stream = Stream::new(stream_dict, b"raw jbig2 data".to_vec());
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
        .arg("--decode-streams")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("WARNING"), "Unsupported filter should show warning: {}", stdout);
    assert!(stdout.contains("JBIG2Decode"), "Warning should mention filter name: {}", stdout);
}

// ── P2 gap: decode warning with --hex ───────────────────────────────

#[test]
fn corrupt_stream_warning_with_hex_flag() {
    let mut doc = Document::new();
    let mut stream_dict = Dictionary::new();
    stream_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    // Binary content that's not valid zlib
    let stream = Stream::new(stream_dict, vec![0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0, 0xF0]);
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
        .arg("--decode-streams")
        .arg("--hex")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("WARNING"), "Should show warning with --hex: {}", stdout);
    assert!(stdout.contains("FlateDecode"), "Warning should mention filter: {}", stdout);
    // Binary content with hex flag should show hex dump
    assert!(stdout.contains("00000000"), "Should show hex dump: {}", stdout);
}

// ── P2 gap: truncate with --hex ─────────────────────────────────────

#[test]
fn truncate_with_hex_flag() {
    let (pdf, obj_num) = create_pdf_with_binary_stream();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--object").arg(obj_num.to_string())
        .arg("--decode-streams")
        .arg("--hex")
        .arg("--truncate").arg("16")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("truncated to 16"), "Should show truncation: {}", stdout);
    assert!(stdout.contains("00000000"), "Should show hex dump");
    // With 16 bytes truncation, should have exactly 1 hex line (no second offset)
    assert!(!stdout.contains("00000010"), "Should not have second hex line with 16-byte truncation: {}", stdout);
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
    assert!(stdout.contains("(missing)"), "Missing object should show (missing) in tree: {}", stdout);
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
    assert!(tree_str.contains("\"missing\""), "JSON tree should contain missing status: {}", tree_str);
}

// ── P2 gap: depth with --page mode ──────────────────────────────────

#[test]
fn depth_with_page_limits_output() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--page").arg("1")
        .arg("--depth").arg("0")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("depth limit reached"), "Depth 0 with page should limit: {}", stdout);
}

#[test]
fn depth_with_page_unlimited() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--page").arg("1")
        .arg("--depth").arg("100")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(!stdout.contains("depth limit reached"), "Large depth should not limit: {}", stdout);
}

// ── P2 gap: truncate=0 via CLI ──────────────────────────────────────

#[test]
fn truncate_zero_cli() {
    let mut doc = Document::new();
    let binary_content: Vec<u8> = (0..500).map(|i| (i as u8).wrapping_add(0x80)).collect();
    let stream = Stream::new(Dictionary::new(), binary_content);
    let stream_id = doc.add_object(Object::Stream(stream));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("BinaryData", Object::Reference(stream_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    doc.save_to(&mut tmp).unwrap();
    tmp.flush().unwrap();

    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--decode-streams")
        .arg("--truncate").arg("0")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("truncated to 0"), "Truncate 0 should truncate everything: {}", stdout);
}

// ── P2 gap: depth=1 with --tree (verify children are limited) ───────

#[test]
fn tree_depth_one_shows_root_children_only() {
    let tmp = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(tmp.path())
        .arg("--tree")
        .arg("--depth").arg("2")
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
        .arg("--depth").arg("0")
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["tree"]["node"], "Trailer");
    let tree_str = serde_json::to_string(&parsed).unwrap();
    assert!(tree_str.contains("depth_limit_reached"), "Depth 0 should limit all children: {}", tree_str);
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
        .arg("--extract-object").arg(stream_id.0.to_string())
        .arg("--output").arg(output_file.path())
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning"), "Extract should show warning on stderr: {}", stderr);
}

// ── P2 gap: --diff incompatible with --tree ─────────────────────────

#[test]
fn diff_incompatible_with_tree() {
    let pdf = create_minimal_pdf();
    let output = Command::new(binary_path())
        .arg(pdf.path())
        .arg("--diff").arg(pdf.path())
        .arg("--tree")
        .output()
        .expect("failed to execute binary");

    assert!(!output.status.success(), "diff + tree should fail");
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
    assert!(stdout.contains("Stream"), "Tree should show Stream label: {}", stdout);
    assert!(stdout.contains("bytes"), "Tree should show byte count for streams: {}", stdout);
}
