use clap::Parser;
use flate2::read::ZlibDecoder;
use lopdf::{content::Content, Document, Object, ObjectId};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
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
        println!("Trailer:");
        let visited_for_print = BTreeSet::new();
        let mut trailer_refs = BTreeSet::new();
        print_object(
            &Object::Dictionary(doc.trailer.clone()),
            &doc,
            &visited_for_print,
            1,
            args.decode_streams,
            args.truncate_binary_streams,
            false,
            &mut trailer_refs,
        );
        
        println!("\n\n================================\n");

        let mut visited_for_traverse = BTreeSet::new();
        if let Ok(root_object) = doc.trailer.get(b"Root") {
            if let Ok(root_id) = root_object.as_reference() {
                dump_object_and_children(root_id, &doc, &mut visited_for_traverse, args.decode_streams, args.truncate_binary_streams, false);
            } else {
                eprintln!("Warning: /Root object in trailer is not a reference.");
            }
        } else {
            eprintln!("Warning: /Root object not found in trailer. Cannot traverse document structure.");
        }
    }
}

fn dump_object_and_children(obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, decode_streams: bool, truncate_binary_streams: bool, is_contents: bool) {
    if visited.contains(&obj_id) {
        return;
    }
    visited.insert(obj_id);

    println!("Object {} {}:", obj_id.0, obj_id.1);
    
    match doc.get_object(obj_id) {
        Ok(object) => {
            let visited_for_print = BTreeSet::new(); // Use a fresh set for printing
            let mut child_refs = BTreeSet::new();
            print_object(object, doc, &visited_for_print, 1, decode_streams, truncate_binary_streams, is_contents, &mut child_refs);
            println!("\n");

            for (is_contents, child_id) in child_refs {
                if !visited.contains(&child_id) {
                    println!("--------------------------------\n");
                    dump_object_and_children(child_id, doc, visited, decode_streams, truncate_binary_streams, is_contents);
                }
            }
        }
        Err(e) => {
            println!("  Error getting object: {}", e);
        }
    }
}

fn is_binary_stream(content: &[u8]) -> bool {
    content.iter().any(|&b| !b.is_ascii_alphanumeric() && !b.is_ascii_whitespace() && !b.is_ascii_punctuation())
}

fn decode_stream(stream: &lopdf::Stream) -> Cow<[u8]> {
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

fn print_stream_content(stream: &lopdf::Stream, indent_str: &str, truncate_binary_streams: bool, is_contents: bool) {
    let decoded_content = decode_stream(stream);
    let description = if let Cow::Owned(_) = &decoded_content {
        "decoded"
    } else {
        "raw"
    };

    print_content_data(&decoded_content, description, indent_str, truncate_binary_streams, is_contents);
}

fn print_content_data(content: &[u8], description: &str, indent_str: &str, truncate_binary_streams: bool,  is_contents: bool) {
    if is_contents {
        match Content::decode(content) {
            Ok(content) => {
                println!(
                    "\n{}Parsed Content Stream ({} operations):",
                    indent_str,
                    content.operations.len()
                );
                for op in &content.operations {
                    print!("{}  {:?}", indent_str, op);
                    println!();
                }
                return; // Successfully parsed and printed, so we are done.
            }
            Err(e) => {
                println!("\n{}[Could not parse content stream: {}. Falling back to raw view.]", indent_str, e);
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

    println!(
        "\n{}Stream content ({}, {} bytes):\n---\n{}\n---",
        indent_str,
        description,
        len_str,
        String::from_utf8_lossy(content_to_display)
    );
}

fn print_object(obj: &Object, doc: &Document, visited: &BTreeSet<ObjectId>, indent: usize, decode_streams: bool, truncate_binary_streams: bool, is_contents: bool, child_refs: &mut BTreeSet<(bool, ObjectId)>) {
    let indent_str = "  ".repeat(indent);

    match obj {
        Object::Null => print!("null"),
        Object::Boolean(b) => print!("{}", b),
        Object::Integer(i) => print!("{}", i),
        Object::Real(r) => print!("{}", r),
        Object::Name(name) => print!("/{}", String::from_utf8_lossy(name)),
        Object::String(bytes, _) => print!("({})", String::from_utf8_lossy(bytes)),
        Object::Array(array) => {
            println!("[");
            for item in array {
                print!("{}", "  ".repeat(indent + 1));
                print_object(item, doc, visited, indent + 1, decode_streams, truncate_binary_streams, is_contents, child_refs);
                println!();
            }
            print!("{}]", indent_str);
        }
        Object::Stream(stream) => {
            println!("<<");
            for (key, value) in stream.dict.iter() {
                print!("{}/{} ", "  ".repeat(indent + 1), String::from_utf8_lossy(key));
                print_object(value, doc, visited, indent + 1, decode_streams, truncate_binary_streams, is_contents, child_refs);
                println!();
            }
            print!("{}>> stream", indent_str);

            if decode_streams {
                print_stream_content(stream, &indent_str, truncate_binary_streams, is_contents);
            }
        }
        Object::Dictionary(dict) => {
            println!("<<");
            for (key, value) in dict.iter() {
                print!("{}/{} ", "  ".repeat(indent + 1), String::from_utf8_lossy(key));
                let is_contents = key == b"Contents";
                print_object(value, doc, visited, indent + 1, decode_streams, truncate_binary_streams, is_contents, child_refs);
                println!();
            }
            print!("{}>>", indent_str);
        }
        Object::Reference(id) => {
            child_refs.insert((is_contents, *id));
            print!("{} {} R", id.0, id.1);
            if visited.contains(id) {
                print!(" (visited)");
            }
        }
    }
}


