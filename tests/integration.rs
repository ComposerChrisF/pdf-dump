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
        stdout.contains("0.1.1") || stdout.contains("pdf_dump"),
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
