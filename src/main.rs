use clap::Parser;
use flate2::read::ZlibDecoder;
use lopdf::{content::Content, Document, Object, ObjectId};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

/// Dumps the internal structure of a PDF file.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the PDF file
    #[arg(required = true)]
    file: PathBuf,

    /// Decode and print the content of streams
    #[arg(long)]
    decode_streams: bool,

    /// Truncate binary streams to the first 100 bytes
    #[arg(long)]
    truncate_binary_streams: bool,

    /// Object number to extract
    #[arg(long, requires = "output")]
    extract_object: Option<u32>,

    /// Output file for extracted object
    #[arg(long, requires = "extract_object")]
    output: Option<PathBuf>,
}

struct DumpConfig {
    decode_streams: bool,
    truncate_binary_streams: bool,
}

fn main() {
    let args = Args::parse();

    let doc = match Document::load(&args.file) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Error: Failed to load PDF file '{}'.", args.file.display());
            eprintln!("Reason: {}", e);
            std::process::exit(1);
        }
    };

    if let Some(object_id) = args.extract_object {
        let output_path = args.output.as_ref().unwrap();
        let object_id = (object_id, 0);
        match doc.get_object(object_id) {
            Ok(Object::Stream(stream)) => {
                let decoded_content = decode_stream(stream);
                if let Err(e) = fs::write(output_path, &*decoded_content) {
                    eprintln!("Error writing to output file: {}", e);
                    std::process::exit(1);
                }
                println!("Successfully extracted object {} to '{}'.", object_id.0, output_path.display());
            }
            Ok(_) => {
                eprintln!("Error: Object {} is not a stream and cannot be extracted to a file.", object_id.0);
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("Error: Object {} not found in the document.", object_id.0);
                std::process::exit(1);
            }
        }
    } else {
        let config = DumpConfig {
            decode_streams: args.decode_streams,
            truncate_binary_streams: args.truncate_binary_streams,
        };
        let mut out = io::stdout().lock();
        writeln!(out, "Trailer:").unwrap();
        let visited_for_print = BTreeSet::new();
        let mut trailer_refs = BTreeSet::new();
        print_object(
            &mut out,
            &Object::Dictionary(doc.trailer.clone()),
            &doc,
            &visited_for_print,
            1,
            &config,
            false,
            &mut trailer_refs,
        );

        writeln!(out, "\n\n================================\n").unwrap();

        let mut visited_for_traverse = BTreeSet::new();
        if let Some(root_id) = doc.trailer.get(b"Root").ok()
            .and_then(|o| o.as_reference().ok())
        {
            dump_object_and_children(&mut out, root_id, &doc, &mut visited_for_traverse, &config, false);
        } else {
            eprintln!("Warning: /Root not found or not a reference in trailer.");
        }
    }
}

fn dump_object_and_children(writer: &mut impl Write, obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, config: &DumpConfig, is_contents: bool) {
    if visited.contains(&obj_id) {
        return;
    }
    visited.insert(obj_id);

    writeln!(writer, "Object {} {}:", obj_id.0, obj_id.1).unwrap();

    match doc.get_object(obj_id) {
        Ok(object) => {
            let visited_for_print = BTreeSet::new();
            let mut child_refs = BTreeSet::new();
            print_object(writer, object, doc, &visited_for_print, 1, config, is_contents, &mut child_refs);
            writeln!(writer, "\n").unwrap();

            for (is_contents, child_id) in child_refs {
                if !visited.contains(&child_id) {
                    writeln!(writer, "--------------------------------\n").unwrap();
                    dump_object_and_children(writer, child_id, doc, visited, config, is_contents);
                }
            }
        }
        Err(e) => {
            writeln!(writer, "  Error getting object: {}", e).unwrap();
        }
    }
}

fn is_binary_stream(content: &[u8]) -> bool {
    content.iter().any(|&b| !b.is_ascii_alphanumeric() && !b.is_ascii_whitespace() && !b.is_ascii_punctuation())
}

fn decode_stream(stream: &lopdf::Stream) -> Cow<'_, [u8]> {
    let has_flate = stream.dict.get(b"Filter").ok().is_some_and(|filter_obj| {
        if let Ok(name) = filter_obj.as_name() {
            name == b"FlateDecode"
        } else if let Ok(arr) = filter_obj.as_array() {
            arr.iter().any(|obj| obj.as_name().ok().is_some_and(|n| n == b"FlateDecode"))
        } else {
            false
        }
    });

    if has_flate {
        let mut decoder = ZlibDecoder::new(&stream.content[..]);
        let mut decompressed = Vec::new();
        if decoder.read_to_end(&mut decompressed).is_ok() {
            return Cow::Owned(decompressed);
        }
    }

    Cow::Borrowed(&stream.content)
}

fn print_stream_content(writer: &mut impl Write, stream: &lopdf::Stream, indent_str: &str, config: &DumpConfig, is_contents: bool) {
    let decoded_content = decode_stream(stream);
    let description = if let Cow::Owned(_) = &decoded_content {
        "decoded"
    } else {
        "raw"
    };

    print_content_data(writer, &decoded_content, description, indent_str, config, is_contents);
}

fn print_content_data(writer: &mut impl Write, content: &[u8], description: &str, indent_str: &str, config: &DumpConfig, is_contents: bool) {
    if is_contents {
        match Content::decode(content) {
            Ok(content) => {
                writeln!(
                    writer,
                    "\n{}Parsed Content Stream ({} operations):",
                    indent_str,
                    content.operations.len()
                ).unwrap();
                for op in &content.operations {
                    write!(writer, "{}  {:?}", indent_str, op).unwrap();
                    writeln!(writer).unwrap();
                }
                return;
            }
            Err(e) => {
                writeln!(writer, "\n{}[Could not parse content stream: {}. Falling back to raw view.]", indent_str, e).unwrap();
            }
        }
    }

    let full_len = content.len();
    let content_to_display = if config.truncate_binary_streams && is_binary_stream(content) {
        &content[..full_len.min(100)]
    } else {
        content
    };

    let len_str = if config.truncate_binary_streams && full_len > 100 && is_binary_stream(content) {
        format!("{} (truncated to 100)", full_len)
    } else {
        full_len.to_string()
    };

    writeln!(
        writer,
        "\n{}Stream content ({}, {} bytes):\n---\n{}\n---",
        indent_str,
        description,
        len_str,
        String::from_utf8_lossy(content_to_display)
    ).unwrap();
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::only_used_in_recursion)]
fn print_object(writer: &mut impl Write, obj: &Object, doc: &Document, visited: &BTreeSet<ObjectId>, indent: usize, config: &DumpConfig, is_contents: bool, child_refs: &mut BTreeSet<(bool, ObjectId)>) {
    let indent_str = "  ".repeat(indent);
    let child_indent = "  ".repeat(indent + 1);

    match obj {
        Object::Null => write!(writer, "null").unwrap(),
        Object::Boolean(b) => write!(writer, "{}", b).unwrap(),
        Object::Integer(i) => write!(writer, "{}", i).unwrap(),
        Object::Real(r) => write!(writer, "{}", r).unwrap(),
        Object::Name(name) => write!(writer, "/{}", String::from_utf8_lossy(name)).unwrap(),
        Object::String(bytes, _) => write!(writer, "({})", String::from_utf8_lossy(bytes)).unwrap(),
        Object::Array(array) => {
            writeln!(writer, "[").unwrap();
            for item in array {
                write!(writer, "{}", child_indent).unwrap();
                print_object(writer, item, doc, visited, indent + 1, config, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}]", indent_str).unwrap();
        }
        Object::Stream(stream) => {
            writeln!(writer, "<<").unwrap();
            for (key, value) in stream.dict.iter() {
                write!(writer, "{}/{} ", child_indent, String::from_utf8_lossy(key)).unwrap();
                print_object(writer, value, doc, visited, indent + 1, config, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}>> stream", indent_str).unwrap();

            if config.decode_streams {
                print_stream_content(writer, stream, &indent_str, config, is_contents);
            }
        }
        Object::Dictionary(dict) => {
            writeln!(writer, "<<").unwrap();
            for (key, value) in dict.iter() {
                write!(writer, "{}/{} ", child_indent, String::from_utf8_lossy(key)).unwrap();
                let is_contents = key == b"Contents";
                print_object(writer, value, doc, visited, indent + 1, config, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}>>", indent_str).unwrap();
        }
        Object::Reference(id) => {
            child_refs.insert((is_contents, *id));
            write!(writer, "{} {} R", id.0, id.1).unwrap();
            if visited.contains(id) {
                write!(writer, " (visited)").unwrap();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;

    // Helper: capture output from print functions into a String
    fn output_of(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut buf = Vec::new();
        f(&mut buf);
        String::from_utf8(buf).unwrap()
    }

    // Helper: create a minimal Document (needed by print_object / dump_object_and_children)
    fn empty_doc() -> Document {
        Document::new()
    }

    // Helper: default config with no decoding/truncation
    fn default_config() -> DumpConfig {
        DumpConfig {
            decode_streams: false,
            truncate_binary_streams: false,
        }
    }

    // ── is_binary_stream ──────────────────────────────────────────────

    #[test]
    fn is_binary_stream_empty() {
        assert!(!is_binary_stream(b""));
    }

    #[test]
    fn is_binary_stream_pure_ascii() {
        assert!(!is_binary_stream(b"Hello World"));
    }

    #[test]
    fn is_binary_stream_ascii_with_whitespace_and_punctuation() {
        assert!(!is_binary_stream(b"key = value; foo: bar\n\ttab"));
    }

    #[test]
    fn is_binary_stream_control_chars() {
        assert!(is_binary_stream(&[0x00]));
        assert!(is_binary_stream(&[0x01]));
        assert!(is_binary_stream(b"abc\x02def"));
    }

    #[test]
    fn is_binary_stream_high_bit() {
        assert!(is_binary_stream(&[0xFF]));
        assert!(is_binary_stream(&[0x80]));
    }

    #[test]
    fn is_binary_stream_mixed() {
        let mut data = b"Hello world".to_vec();
        data.push(0x80);
        assert!(is_binary_stream(&data));
    }

    // ── decode_stream ─────────────────────────────────────────────────

    fn make_stream(filter: Option<Object>, content: Vec<u8>) -> Stream {
        let mut dict = Dictionary::new();
        if let Some(f) = filter {
            dict.set("Filter", f);
        }
        Stream::new(dict, content)
    }

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut encoder, data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn decode_stream_no_filter() {
        let stream = make_stream(None, b"raw content".to_vec());
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw content");
    }

    #[test]
    fn decode_stream_flatedecode_name() {
        let compressed = zlib_compress(b"hello pdf");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"hello pdf");
    }

    #[test]
    fn decode_stream_flatedecode_in_array() {
        let compressed = zlib_compress(b"array filter");
        let stream = make_stream(
            Some(Object::Array(vec![Object::Name(b"FlateDecode".to_vec())])),
            compressed,
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"array filter");
    }

    #[test]
    fn decode_stream_unknown_filter() {
        let stream = make_stream(
            Some(Object::Name(b"DCTDecode".to_vec())),
            b"jpeg data".to_vec(),
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"jpeg data");
    }

    #[test]
    fn decode_stream_corrupt_flatedecode() {
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"not valid zlib".to_vec(),
        );
        let result = decode_stream(&stream);
        // Falls back to borrowed original content
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"not valid zlib");
    }

    // ── print_object ──────────────────────────────────────────────────

    fn print_obj(obj: &Object) -> (String, BTreeSet<(bool, ObjectId)>) {
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            print_object(w, obj, &doc, &visited, 1, &config, false, &mut child_refs);
        });
        (out, child_refs)
    }

    #[test]
    fn print_object_null() {
        let (out, _) = print_obj(&Object::Null);
        assert_eq!(out, "null");
    }

    #[test]
    fn print_object_boolean() {
        let (out, _) = print_obj(&Object::Boolean(true));
        assert_eq!(out, "true");
        let (out, _) = print_obj(&Object::Boolean(false));
        assert_eq!(out, "false");
    }

    #[test]
    fn print_object_integer() {
        let (out, _) = print_obj(&Object::Integer(42));
        assert_eq!(out, "42");
    }

    #[test]
    fn print_object_real() {
        let (out, _) = print_obj(&Object::Real(2.72));
        assert_eq!(out, "2.72");
    }

    #[test]
    fn print_object_name() {
        let (out, _) = print_obj(&Object::Name(b"Type".to_vec()));
        assert_eq!(out, "/Type");
    }

    #[test]
    fn print_object_string() {
        let (out, _) = print_obj(&Object::String(b"hello".to_vec(), StringFormat::Literal));
        assert_eq!(out, "(hello)");
    }

    #[test]
    fn print_object_array() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let (out, _) = print_obj(&arr);
        assert!(out.contains("["));
        assert!(out.contains("1"));
        assert!(out.contains("2"));
        assert!(out.contains("]"));
    }

    #[test]
    fn print_object_dictionary() {
        let mut dict = Dictionary::new();
        dict.set("Key", Object::Integer(99));
        let (out, _) = print_obj(&Object::Dictionary(dict));
        assert!(out.contains("<<"));
        assert!(out.contains("/Key"));
        assert!(out.contains("99"));
        assert!(out.contains(">>"));
    }

    #[test]
    fn print_object_stream_no_decode() {
        let stream = make_stream(None, b"stream data".to_vec());
        let (out, _) = print_obj(&Object::Stream(stream));
        assert!(out.contains("<<"));
        assert!(out.contains(">> stream"));
        // decode_streams=false, so no stream content printed
        assert!(!out.contains("Stream content"));
    }

    #[test]
    fn print_object_stream_with_decode() {
        let stream = make_stream(None, b"visible data".to_vec());
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false };
        let out = output_of(|w| {
            print_object(w, &Object::Stream(stream), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(out.contains(">> stream"));
        assert!(out.contains("Stream content"));
        assert!(out.contains("visible data"));
    }

    #[test]
    fn print_object_reference_populates_child_refs() {
        let obj = Object::Reference((5, 0));
        let (out, refs) = print_obj(&obj);
        assert_eq!(out, "5 0 R");
        assert!(refs.contains(&(false, (5, 0))));
    }

    #[test]
    fn print_object_reference_visited() {
        let doc = empty_doc();
        let mut visited = BTreeSet::new();
        visited.insert((5, 0));
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            print_object(w, &Object::Reference((5, 0)), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(out.contains("5 0 R (visited)"));
    }

    #[test]
    fn print_object_contents_key_propagates_is_contents() {
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Reference((10, 0)));
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &Object::Dictionary(dict), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        // The reference under /Contents should have is_contents=true
        assert!(child_refs.contains(&(true, (10, 0))));
    }

    // ── print_content_data ────────────────────────────────────────────

    #[test]
    fn print_content_data_ascii_no_truncation() {
        let content = b"Hello PDF stream";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, false);
        });
        assert!(out.contains("Stream content (raw, 16 bytes)"));
        assert!(out.contains("Hello PDF stream"));
    }

    #[test]
    fn print_content_data_binary_truncated() {
        // 200 bytes of binary data (contains 0x80 so is_binary_stream = true)
        let content: Vec<u8> = (0..200).map(|i| (i as u8).wrapping_add(0x80)).collect();
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false);
        });
        assert!(out.contains("200 (truncated to 100)"));
    }

    #[test]
    fn print_content_data_is_contents_parses_operations() {
        // A simple PDF content stream: "BT /F1 12 Tf ET"
        let content = b"BT\n/F1 12 Tf\nET";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "decoded", "  ", &config, true);
        });
        assert!(out.contains("Parsed Content Stream"));
        assert!(out.contains("operations"));
    }

    // ── dump_object_and_children ──────────────────────────────────────

    #[test]
    fn dump_single_object_no_refs() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("42"));
        assert!(visited.contains(&(1, 0)));
    }

    #[test]
    fn dump_object_follows_references() {
        let mut doc = Document::new();
        // Object 1 is a dict with a reference to object 2
        let mut dict = Dictionary::new();
        dict.set("Child", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(99));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("99"));
        assert!(visited.contains(&(1, 0)));
        assert!(visited.contains(&(2, 0)));
    }

    #[test]
    fn dump_object_circular_reference() {
        let mut doc = Document::new();
        // Object 1 references object 2, object 2 references object 1
        let mut dict1 = Dictionary::new();
        dict1.set("Next", Object::Reference((2, 0)));
        let mut dict2 = Dictionary::new();
        dict2.set("Prev", Object::Reference((1, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        let mut visited = BTreeSet::new();
        let config = default_config();
        // This should terminate (not infinite-loop) thanks to the visited set
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(visited.contains(&(1, 0)));
        assert!(visited.contains(&(2, 0)));
    }

    // ── is_binary_stream (additional edge cases) ────────────────────

    #[test]
    fn is_binary_stream_del_char() {
        // DEL (0x7F) is not alphanumeric, not whitespace, not punctuation → binary
        assert!(is_binary_stream(&[0x7F]));
    }

    #[test]
    fn is_binary_stream_punctuation_only() {
        assert!(!is_binary_stream(b"!@#$%^&*(){}[]"));
    }

    #[test]
    fn is_binary_stream_whitespace_only() {
        assert!(!is_binary_stream(b"   \t\n\r"));
    }

    #[test]
    fn is_binary_stream_all_allowed_types_combined() {
        // Alphanumeric + whitespace + punctuation → not binary
        assert!(!is_binary_stream(b"abc 123\n!@#"));
    }

    #[test]
    fn is_binary_stream_single_null_among_ascii() {
        // Even one null byte makes it binary
        assert!(is_binary_stream(b"abc\x00def"));
    }

    // ── decode_stream (additional branches) ─────────────────────────

    #[test]
    fn decode_stream_empty_content_no_filter() {
        let stream = make_stream(None, vec![]);
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"");
    }

    #[test]
    fn decode_stream_filter_is_integer_ignored() {
        // Filter that's neither Name nor Array → treated as no filter
        let stream = make_stream(Some(Object::Integer(42)), b"raw bytes".to_vec());
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw bytes");
    }

    #[test]
    fn decode_stream_multiple_filters_with_flatedecode() {
        // Array with FlateDecode and another filter
        let compressed = zlib_compress(b"multi-filter");
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"ASCIIHexDecode".to_vec()),
            ])),
            compressed,
        );
        let result = decode_stream(&stream);
        // FlateDecode is found → decompresses
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"multi-filter");
    }

    #[test]
    fn decode_stream_array_without_flatedecode() {
        // Array of filters, none of which is FlateDecode
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"DCTDecode".to_vec()),
            ])),
            b"pass through".to_vec(),
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"pass through");
    }

    #[test]
    fn decode_stream_empty_filter_array() {
        let stream = make_stream(
            Some(Object::Array(vec![])),
            b"no filters".to_vec(),
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"no filters");
    }

    #[test]
    fn decode_stream_flatedecode_empty_payload() {
        // Compressed empty content
        let compressed = zlib_compress(b"");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"");
    }

    // ── print_object (additional branches) ──────────────────────────

    #[test]
    fn print_object_empty_array() {
        let arr = Object::Array(vec![]);
        let (out, refs) = print_obj(&arr);
        assert!(out.contains("["));
        assert!(out.contains("]"));
        assert!(refs.is_empty());
    }

    #[test]
    fn print_object_empty_dictionary() {
        let dict = Dictionary::new();
        let (out, refs) = print_obj(&Object::Dictionary(dict));
        assert!(out.contains("<<"));
        assert!(out.contains(">>"));
        assert!(refs.is_empty());
    }

    #[test]
    fn print_object_nested_dictionary() {
        let mut inner = Dictionary::new();
        inner.set("InnerKey", Object::Integer(7));
        let mut outer = Dictionary::new();
        outer.set("Outer", Object::Dictionary(inner));
        let (out, _) = print_obj(&Object::Dictionary(outer));
        assert!(out.contains("/Outer"));
        assert!(out.contains("/InnerKey"));
        assert!(out.contains("7"));
    }

    #[test]
    fn print_object_array_with_references_collects_child_refs() {
        let arr = Object::Array(vec![
            Object::Reference((3, 0)),
            Object::Reference((4, 0)),
        ]);
        let (out, refs) = print_obj(&arr);
        assert!(out.contains("3 0 R"));
        assert!(out.contains("4 0 R"));
        assert!(refs.contains(&(false, (3, 0))));
        assert!(refs.contains(&(false, (4, 0))));
    }

    #[test]
    fn print_object_negative_integer() {
        let (out, _) = print_obj(&Object::Integer(-99));
        assert_eq!(out, "-99");
    }

    #[test]
    fn print_object_zero_real() {
        let (out, _) = print_obj(&Object::Real(0.0));
        assert_eq!(out, "0");
    }

    #[test]
    fn print_object_name_with_special_chars() {
        let (out, _) = print_obj(&Object::Name(b"Font+Name".to_vec()));
        assert_eq!(out, "/Font+Name");
    }

    #[test]
    fn print_object_string_hex_format() {
        let (out, _) = print_obj(&Object::String(b"hex".to_vec(), StringFormat::Hexadecimal));
        assert_eq!(out, "(hex)");
    }

    #[test]
    fn print_object_stream_with_flatedecode_and_decode_flag() {
        let compressed = zlib_compress(b"decompressed text");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false };
        let out = output_of(|w| {
            print_object(w, &Object::Stream(stream), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(out.contains(">> stream"));
        assert!(out.contains("decoded"));
        assert!(out.contains("decompressed text"));
    }

    #[test]
    fn print_object_multiple_refs_in_dict() {
        let mut dict = Dictionary::new();
        dict.set("A", Object::Reference((10, 0)));
        dict.set("B", Object::Reference((20, 0)));
        let (_, refs) = print_obj(&Object::Dictionary(dict));
        assert!(refs.contains(&(false, (10, 0))));
        assert!(refs.contains(&(false, (20, 0))));
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn print_object_is_contents_propagated_to_array_ref() {
        // When print_object is called with is_contents=true, refs in arrays get is_contents=true
        let arr = Object::Array(vec![Object::Reference((7, 0))]);
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &arr, &doc, &visited, 1, &config, true, &mut child_refs);
        });
        assert!(child_refs.contains(&(true, (7, 0))));
    }

    #[test]
    fn print_object_stream_dict_entries_printed() {
        let mut dict = Dictionary::new();
        dict.set("Length", Object::Integer(11));
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, b"stream data".to_vec());
        let (out, _) = print_obj(&Object::Stream(stream));
        assert!(out.contains("/Length"));
        assert!(out.contains("11"));
        assert!(out.contains("/Filter"));
        assert!(out.contains("/FlateDecode"));
    }

    // ── print_content_data (additional branches) ────────────────────

    #[test]
    fn print_content_data_empty_content() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"", "raw", "  ", &config, false);
        });
        assert!(out.contains("Stream content (raw, 0 bytes)"));
    }

    #[test]
    fn print_content_data_binary_no_truncation() {
        // Binary content but truncate_binary_streams=false → full output
        let content: Vec<u8> = (0..200).map(|i| (i as u8).wrapping_add(0x80)).collect();
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false);
        });
        assert!(out.contains("200 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_short_with_truncation_enabled() {
        // Binary content < 100 bytes with truncation enabled → no truncation applied
        let content: Vec<u8> = vec![0x80; 50];
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false);
        });
        assert!(out.contains("50 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_exactly_100_bytes_with_truncation() {
        // Exactly 100 bytes of binary → no truncation (only truncates > 100)
        let content: Vec<u8> = vec![0x80; 100];
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false);
        });
        assert!(out.contains("100 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_101_bytes_with_truncation() {
        // 101 bytes of binary → should truncate
        let content: Vec<u8> = vec![0x80; 101];
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false);
        });
        assert!(out.contains("101 (truncated to 100)"));
    }

    #[test]
    fn print_content_data_is_contents_invalid_stream_falls_back() {
        // Content::decode is lenient, so we verify the fallback path by checking
        // that badly formed streams either parse (with 0 ops) or show the fallback.
        // Use content that Content::decode will reject: unbalanced parens cause a parse error.
        let content = b"( unclosed string";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, true);
        });
        // lopdf's Content::decode may or may not fail on this.
        // If it parses: we see "Parsed Content Stream"; if it fails: we see the fallback.
        let parsed = out.contains("Parsed Content Stream");
        let fallback = out.contains("Could not parse content stream") && out.contains("Stream content");
        assert!(parsed || fallback, "Expected either parsed or fallback output, got: {}", out);
    }

    #[test]
    fn print_content_data_ascii_not_truncated_even_when_flag_set() {
        // ASCII content >100 bytes with truncation flag → no truncation (not binary)
        let content = b"abcdefghij".repeat(20); // 200 bytes of ASCII
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false);
        });
        assert!(out.contains("200 bytes"));
        assert!(!out.contains("truncated"));
    }

    // ── print_stream_content ────────────────────────────────────────

    #[test]
    fn print_stream_content_no_filter() {
        let stream = make_stream(None, b"raw stream bytes".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, false);
        });
        assert!(out.contains("raw"));
        assert!(out.contains("raw stream bytes"));
    }

    #[test]
    fn print_stream_content_flatedecode() {
        let compressed = zlib_compress(b"decoded content");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, false);
        });
        assert!(out.contains("decoded"));
        assert!(out.contains("decoded content"));
    }

    #[test]
    fn print_stream_content_with_truncation() {
        // Large binary stream with truncation enabled
        let content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, content);
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true };
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("truncated to 100"));
    }

    #[test]
    fn print_stream_content_is_contents_parses() {
        let content = b"BT\n/F1 12 Tf\nET";
        let stream = make_stream(None, content.to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, true);
        });
        assert!(out.contains("Parsed Content Stream"));
    }

    // ── dump_object_and_children (additional paths) ─────────────────

    #[test]
    fn dump_object_not_found() {
        let doc = Document::new();
        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (99, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 99 0:"));
        assert!(out.contains("Error getting object"));
        assert!(visited.contains(&(99, 0)));
    }

    #[test]
    fn dump_object_already_visited_produces_no_output() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let mut visited = BTreeSet::new();
        visited.insert((1, 0));
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert_eq!(out, "");
    }

    #[test]
    fn dump_object_deep_chain_three_levels() {
        let mut doc = Document::new();
        let mut dict1 = Dictionary::new();
        dict1.set("Next", Object::Reference((2, 0)));
        let mut dict2 = Dictionary::new();
        dict2.set("Next", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));
        doc.objects.insert((3, 0), Object::Integer(777));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("Object 3 0:"));
        assert!(out.contains("777"));
        assert_eq!(visited.len(), 3);
    }

    #[test]
    fn dump_object_multiple_children_from_parent() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Child1", Object::Reference((2, 0)));
        dict.set("Child2", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(22));
        doc.objects.insert((3, 0), Object::Integer(33));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("Object 3 0:"));
        assert!(out.contains("22"));
        assert!(out.contains("33"));
    }

    #[test]
    fn dump_object_with_stream_and_decode() {
        let mut doc = Document::new();
        let compressed = zlib_compress(b"stream content here");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("stream content here"));
    }

    #[test]
    fn dump_object_is_contents_propagates() {
        let mut doc = Document::new();
        // Object 1 has /Contents referencing object 2
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        // Object 2 is a valid content stream
        let content = b"BT\n/F1 12 Tf\nET";
        let stream = make_stream(None, content.to_vec());
        doc.objects.insert((2, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 2 0:"));
        assert!(out.contains("Parsed Content Stream"));
    }

    #[test]
    fn dump_object_separator_between_siblings() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("A", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(1));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("--------------------------------"));
    }

    // ── print_object: edge-case Object variants ─────────────────────

    #[test]
    fn print_object_integer_zero() {
        let (out, _) = print_obj(&Object::Integer(0));
        assert_eq!(out, "0");
    }

    #[test]
    fn print_object_real_negative() {
        let (out, _) = print_obj(&Object::Real(-2.75));
        assert_eq!(out, "-2.75");
    }

    #[test]
    fn print_object_real_large() {
        let (out, _) = print_obj(&Object::Real(99999.5));
        assert_eq!(out, "99999.5");
    }

    #[test]
    fn print_object_empty_string() {
        let (out, _) = print_obj(&Object::String(b"".to_vec(), StringFormat::Literal));
        assert_eq!(out, "()");
    }

    #[test]
    fn print_object_empty_name() {
        let (out, _) = print_obj(&Object::Name(b"".to_vec()));
        assert_eq!(out, "/");
    }

    #[test]
    fn print_object_string_non_utf8() {
        // Non-UTF8 bytes should be handled by from_utf8_lossy with replacement char
        let (out, _) = print_obj(&Object::String(vec![0xFF, 0xFE], StringFormat::Literal));
        assert!(out.starts_with('('));
        assert!(out.ends_with(')'));
        assert!(out.contains('\u{FFFD}'), "Non-UTF8 bytes should produce replacement chars");
    }

    #[test]
    fn print_object_name_non_utf8() {
        let (out, _) = print_obj(&Object::Name(vec![0x80, 0x81]));
        assert!(out.starts_with('/'));
        assert!(out.contains('\u{FFFD}'), "Non-UTF8 name bytes should produce replacement chars");
    }

    #[test]
    fn print_object_reference_nonzero_generation() {
        let obj = Object::Reference((5, 2));
        let (out, refs) = print_obj(&obj);
        assert_eq!(out, "5 2 R");
        assert!(refs.contains(&(false, (5, 2))));
    }

    #[test]
    fn print_object_array_mixed_types() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Name(b"Foo".to_vec()),
            Object::Boolean(true),
            Object::Null,
            Object::Real(1.5),
        ]);
        let (out, _) = print_obj(&arr);
        assert!(out.contains("1"));
        assert!(out.contains("/Foo"));
        assert!(out.contains("true"));
        assert!(out.contains("null"));
        assert!(out.contains("1.5"));
    }

    #[test]
    fn print_object_array_of_arrays() {
        let inner = Object::Array(vec![Object::Integer(10)]);
        let outer = Object::Array(vec![inner]);
        let (out, _) = print_obj(&outer);
        // Should have nested brackets
        let open_count = out.matches('[').count();
        let close_count = out.matches(']').count();
        assert_eq!(open_count, 2, "Expected 2 opening brackets for nested arrays");
        assert_eq!(close_count, 2, "Expected 2 closing brackets for nested arrays");
        assert!(out.contains("10"));
    }

    #[test]
    fn print_object_dict_in_array() {
        let mut dict = Dictionary::new();
        dict.set("K", Object::Integer(5));
        let arr = Object::Array(vec![Object::Dictionary(dict)]);
        let (out, _) = print_obj(&arr);
        assert!(out.contains("<<"));
        assert!(out.contains("/K"));
        assert!(out.contains("5"));
        assert!(out.contains(">>"));
    }

    #[test]
    fn print_object_stream_dict_with_reference_collects_child_ref() {
        // Stream dict entries that are references should be collected
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((20, 0)));
        let stream = Stream::new(dict, b"data".to_vec());
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &Object::Stream(stream), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(child_refs.contains(&(false, (20, 0))), "Reference in stream dict should be collected");
    }

    #[test]
    fn print_object_contents_key_with_non_reference_value() {
        // /Contents with a non-reference value (e.g., an integer) should not crash
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Integer(42));
        let (out, refs) = print_obj(&Object::Dictionary(dict));
        assert!(out.contains("/Contents"));
        assert!(out.contains("42"));
        assert!(refs.is_empty(), "Non-reference Contents value should not add child refs");
    }

    #[test]
    fn print_object_contents_key_with_array_of_refs() {
        // /Contents pointing to an array of references: each ref should get is_contents=true
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        let doc = empty_doc();
        let visited = BTreeSet::new();
        let mut child_refs = BTreeSet::new();
        let config = default_config();
        output_of(|w| {
            print_object(w, &Object::Dictionary(dict), &doc, &visited, 1, &config, false, &mut child_refs);
        });
        assert!(child_refs.contains(&(true, (10, 0))), "Array ref under /Contents should have is_contents=true");
        assert!(child_refs.contains(&(true, (11, 0))), "Array ref under /Contents should have is_contents=true");
    }

    // ── decode_stream: filter array with mixed types ────────────────

    #[test]
    fn decode_stream_filter_array_with_non_name_elements() {
        // Array with a non-Name object (e.g., Integer) mixed in → filter_map skips it
        let compressed = zlib_compress(b"mixed types");
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Integer(42),  // not a Name, should be skipped
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            compressed,
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"mixed types");
    }

    #[test]
    fn decode_stream_filter_array_all_non_name() {
        // Array where no elements are Names → empty filter list → no decode
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Integer(1),
                Object::Boolean(true),
            ])),
            b"raw data".to_vec(),
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw data");
    }

    #[test]
    fn decode_stream_large_content() {
        // Verify decompression works for larger payloads
        let large: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let compressed = zlib_compress(&large);
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let result = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, &large[..]);
    }

    // ── print_content_data: formatting details ──────────────────────

    #[test]
    fn print_content_data_description_propagated() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"x", "custom-desc", "  ", &config, false);
        });
        assert!(out.contains("custom-desc"), "Description should appear in output");
    }

    #[test]
    fn print_content_data_indent_str_used() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"data", "raw", "    ", &config, false);
        });
        assert!(out.contains("    Stream content"), "Indent string should prefix stream content line");
    }

    #[test]
    fn print_content_data_is_contents_indent_str_used() {
        let content = b"BT\n/F1 12 Tf\nET";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", ">>> ", &config, true);
        });
        assert!(out.contains(">>> Parsed Content Stream"), "Indent string should prefix parsed content header");
    }

    // ── print_stream_content: combined paths ────────────────────────

    #[test]
    fn print_stream_content_flatedecode_is_contents() {
        // Combined path: FlateDecode decompression + content stream parsing
        let content = b"BT\n/F1 12 Tf\n(Hello) Tj\nET";
        let compressed = zlib_compress(content);
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, true);
        });
        assert!(out.contains("Parsed Content Stream"), "Decoded content stream should be parsed");
    }

    #[test]
    fn print_stream_content_corrupt_flatedecode_not_contents() {
        // Corrupt FlateDecode with is_contents=false → falls back to raw borrowed content
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"not valid zlib data at all".to_vec(),
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("raw"), "Corrupt FlateDecode should fall back to 'raw'");
        assert!(out.contains("not valid zlib data"));
    }

    #[test]
    fn print_stream_content_description_shows_decoded_for_flatedecode() {
        let compressed = zlib_compress(b"text");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("decoded"), "Successfully decompressed should show 'decoded'");
    }

    #[test]
    fn print_stream_content_description_shows_raw_for_no_filter() {
        let stream = make_stream(None, b"plain".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("raw"), "No filter should show 'raw'");
    }

    // ── dump_object_and_children: additional paths ──────────────────

    #[test]
    fn dump_object_stream_dict_refs_traversed() {
        // Stream dict contains references → those children should be traversed
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((2, 0)));
        let stream = Stream::new(dict, b"data".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        doc.objects.insert((2, 0), Object::Integer(42));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("Object 1 0:"), "Parent stream should be printed");
        assert!(out.contains("Object 2 0:"), "Referenced object in stream dict should be traversed");
        assert!(out.contains("42"));
    }

    #[test]
    fn dump_object_is_contents_direct_param() {
        // Passing is_contents=true directly to dump_object_and_children
        let mut doc = Document::new();
        let content = b"BT\n/F1 12 Tf\nET";
        let stream = make_stream(None, content.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, true);
        });
        assert!(out.contains("Parsed Content Stream"), "Direct is_contents=true should trigger content parsing");
    }

    #[test]
    fn dump_object_with_decode_and_truncate() {
        // Both decode_streams=true and truncate_binary_streams=true with binary stream
        let mut doc = Document::new();
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: true };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert!(out.contains("truncated to 100"), "Binary stream should be truncated");
    }

    #[test]
    fn dump_object_diamond_dependency() {
        // A → B, A → C, B → D, C → D  (diamond: D visited once)
        let mut doc = Document::new();
        let mut dict_a = Dictionary::new();
        dict_a.set("B", Object::Reference((2, 0)));
        dict_a.set("C", Object::Reference((3, 0)));
        let mut dict_b = Dictionary::new();
        dict_b.set("D", Object::Reference((4, 0)));
        let mut dict_c = Dictionary::new();
        dict_c.set("D", Object::Reference((4, 0)));

        doc.objects.insert((1, 0), Object::Dictionary(dict_a));
        doc.objects.insert((2, 0), Object::Dictionary(dict_b));
        doc.objects.insert((3, 0), Object::Dictionary(dict_c));
        doc.objects.insert((4, 0), Object::Integer(999));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        assert_eq!(visited.len(), 4, "All 4 objects should be visited exactly once");
        // Object 4 should appear exactly once (not duplicated)
        let count = out.matches("Object 4 0:").count();
        assert_eq!(count, 1, "Diamond dependency: object 4 should be dumped only once");
    }

    #[test]
    fn dump_object_self_referencing() {
        // An object that references itself
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Self", Object::Reference((1, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let mut visited = BTreeSet::new();
        let config = default_config();
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false);
        });
        // Should terminate and print the object once
        let count = out.matches("Object 1 0:").count();
        assert_eq!(count, 1, "Self-referencing object should be printed once");
    }

    // ── is_binary_stream: specific byte boundaries ──────────────────

    #[test]
    fn is_binary_stream_unit_separator() {
        // 0x1F is a control char, not alphanumeric/whitespace/punctuation → binary
        assert!(is_binary_stream(&[0x1F]));
    }

    #[test]
    fn is_binary_stream_tilde_is_punctuation() {
        // 0x7E (~) is ASCII punctuation → not binary
        assert!(!is_binary_stream(b"~"));
    }

    #[test]
    fn is_binary_stream_tab_only() {
        // Tab (0x09) is ASCII whitespace
        assert!(!is_binary_stream(b"\t"));
    }

    #[test]
    fn is_binary_stream_escape_char() {
        // 0x1B (ESC) is a control char → binary
        assert!(is_binary_stream(&[0x1B]));
    }
}
