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
    #[arg(long)]
    extract_object: Option<u32>,

    /// Output file for extracted object
    #[arg(long, requires = "extract_object")]
    output: Option<PathBuf>,
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

    if let (Some(object_id), Some(output_path)) = (args.extract_object, &args.output) {
        let object_id = (object_id, 0);
        match doc.get_object(object_id) {
            Ok(Object::Stream(stream)) => {
                let decoded_content = decode_stream(&stream);
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
            args.decode_streams,
            args.truncate_binary_streams,
            false,
            &mut trailer_refs,
        );

        writeln!(out, "\n\n================================\n").unwrap();

        let mut visited_for_traverse = BTreeSet::new();
        if let Ok(root_object) = doc.trailer.get(b"Root") {
            if let Ok(root_id) = root_object.as_reference() {
                dump_object_and_children(&mut out, root_id, &doc, &mut visited_for_traverse, args.decode_streams, args.truncate_binary_streams, false);
            } else {
                eprintln!("Warning: /Root object in trailer is not a reference.");
            }
        } else {
            eprintln!("Warning: /Root object not found in trailer. Cannot traverse document structure.");
        }
    }
}

fn dump_object_and_children(writer: &mut impl Write, obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, decode_streams: bool, truncate_binary_streams: bool, is_contents: bool) {
    if visited.contains(&obj_id) {
        return;
    }
    visited.insert(obj_id);

    writeln!(writer, "Object {} {}:", obj_id.0, obj_id.1).unwrap();

    match doc.get_object(obj_id) {
        Ok(object) => {
            let visited_for_print = BTreeSet::new();
            let mut child_refs = BTreeSet::new();
            print_object(writer, object, doc, &visited_for_print, 1, decode_streams, truncate_binary_streams, is_contents, &mut child_refs);
            writeln!(writer, "\n").unwrap();

            for (is_contents, child_id) in child_refs {
                if !visited.contains(&child_id) {
                    writeln!(writer, "--------------------------------\n").unwrap();
                    dump_object_and_children(writer, child_id, doc, visited, decode_streams, truncate_binary_streams, is_contents);
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
    let filters = stream.dict.get(b"Filter").ok().and_then(|filter_obj| {
        if let Ok(name_bytes) = filter_obj.as_name() {
            Some(vec![String::from_utf8_lossy(name_bytes).to_string()])
        } else if let Ok(arr) = filter_obj.as_array() {
            Some(
                arr.iter()
                    .filter_map(|obj| obj.as_name().ok())
                    .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                    .collect()
            )
        } else {
            None
        }
    }).unwrap_or_default();

    if filters.iter().any(|f| f == "FlateDecode") {
        let mut decoder = ZlibDecoder::new(&stream.content[..]);
        let mut decompressed = Vec::new();
        if decoder.read_to_end(&mut decompressed).is_ok() {
            return Cow::Owned(decompressed);
        }
    }

    Cow::Borrowed(&stream.content)
}

fn print_stream_content(writer: &mut impl Write, stream: &lopdf::Stream, indent_str: &str, truncate_binary_streams: bool, is_contents: bool) {
    let decoded_content = decode_stream(stream);
    let description = if let Cow::Owned(_) = &decoded_content {
        "decoded"
    } else {
        "raw"
    };

    print_content_data(writer, &decoded_content, description, indent_str, truncate_binary_streams, is_contents);
}

fn print_content_data(writer: &mut impl Write, content: &[u8], description: &str, indent_str: &str, truncate_binary_streams: bool, is_contents: bool) {
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
    let content_to_display = if truncate_binary_streams && is_binary_stream(content) {
        &content[..full_len.min(100)]
    } else {
        content
    };

    let len_str = if truncate_binary_streams && full_len > 100 && is_binary_stream(content) {
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

fn print_object(writer: &mut impl Write, obj: &Object, doc: &Document, visited: &BTreeSet<ObjectId>, indent: usize, decode_streams: bool, truncate_binary_streams: bool, is_contents: bool, child_refs: &mut BTreeSet<(bool, ObjectId)>) {
    let indent_str = "  ".repeat(indent);

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
                write!(writer, "{}", "  ".repeat(indent + 1)).unwrap();
                print_object(writer, item, doc, visited, indent + 1, decode_streams, truncate_binary_streams, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}]", indent_str).unwrap();
        }
        Object::Stream(stream) => {
            writeln!(writer, "<<").unwrap();
            for (key, value) in stream.dict.iter() {
                write!(writer, "{}/{} ", "  ".repeat(indent + 1), String::from_utf8_lossy(key)).unwrap();
                print_object(writer, value, doc, visited, indent + 1, decode_streams, truncate_binary_streams, is_contents, child_refs);
                writeln!(writer).unwrap();
            }
            write!(writer, "{}>> stream", indent_str).unwrap();

            if decode_streams {
                print_stream_content(writer, stream, &indent_str, truncate_binary_streams, is_contents);
            }
        }
        Object::Dictionary(dict) => {
            writeln!(writer, "<<").unwrap();
            for (key, value) in dict.iter() {
                write!(writer, "{}/{} ", "  ".repeat(indent + 1), String::from_utf8_lossy(key)).unwrap();
                let is_contents = key == b"Contents";
                print_object(writer, value, doc, visited, indent + 1, decode_streams, truncate_binary_streams, is_contents, child_refs);
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
        let out = output_of(|w| {
            print_object(w, obj, &doc, &visited, 1, false, false, false, &mut child_refs);
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
        let out = output_of(|w| {
            print_object(w, &Object::Stream(stream), &doc, &visited, 1, true, false, false, &mut child_refs);
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
        let out = output_of(|w| {
            print_object(w, &Object::Reference((5, 0)), &doc, &visited, 1, false, false, false, &mut child_refs);
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
        output_of(|w| {
            print_object(w, &Object::Dictionary(dict), &doc, &visited, 1, false, false, false, &mut child_refs);
        });
        // The reference under /Contents should have is_contents=true
        assert!(child_refs.contains(&(true, (10, 0))));
    }

    // ── print_content_data ────────────────────────────────────────────

    #[test]
    fn print_content_data_ascii_no_truncation() {
        let content = b"Hello PDF stream";
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", false, false);
        });
        assert!(out.contains("Stream content (raw, 16 bytes)"));
        assert!(out.contains("Hello PDF stream"));
    }

    #[test]
    fn print_content_data_binary_truncated() {
        // 200 bytes of binary data (contains 0x80 so is_binary_stream = true)
        let content: Vec<u8> = (0..200).map(|i| (i as u8).wrapping_add(0x80)).collect();
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", true, false);
        });
        assert!(out.contains("200 (truncated to 100)"));
    }

    #[test]
    fn print_content_data_is_contents_parses_operations() {
        // A simple PDF content stream: "BT /F1 12 Tf ET"
        let content = b"BT\n/F1 12 Tf\nET";
        let out = output_of(|w| {
            print_content_data(w, content, "decoded", "  ", false, true);
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
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, false, false, false);
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
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, false, false, false);
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
        // This should terminate (not infinite-loop) thanks to the visited set
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, false, false, false);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
        assert!(visited.contains(&(1, 0)));
        assert!(visited.contains(&(2, 0)));
    }
}
