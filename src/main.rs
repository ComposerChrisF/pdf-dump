use clap::Parser;
use flate2::read::ZlibDecoder;
use lopdf::{Document, Object, ObjectId};
use std::collections::BTreeSet;
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

    let visited_for_print = BTreeSet::new();

    println!("Trailer:");
    print_object(&Object::Dictionary(doc.trailer.clone()), &doc, &visited_for_print, 1, args.decode_streams, args.truncate_binary_streams);
    
    println!("\n\n================================\n");

    let mut visited_for_traverse = BTreeSet::new();
    if let Ok(root_object) = doc.trailer.get(b"Root") {
        if let Ok(root_id) = root_object.as_reference() {
            dump_object_and_children(root_id, &doc, &mut visited_for_traverse, args.decode_streams, args.truncate_binary_streams);
        } else {
            eprintln!("Warning: /Root object in trailer is not a reference.");
        }
    } else {
        eprintln!("Warning: /Root object not found in trailer. Cannot traverse document structure.");
    }
}

fn dump_object_and_children(obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, decode_streams: bool, truncate_binary_streams: bool) {
    if visited.contains(&obj_id) {
        return;
    }
    visited.insert(obj_id);

    println!("Object {} {}:", obj_id.0, obj_id.1);
    
    match doc.get_object(obj_id) {
        Ok(object) => {
            let visited_for_print = BTreeSet::new(); // Use a fresh set for printing
            print_object(object, doc, &visited_for_print, 1, decode_streams, truncate_binary_streams);
            println!("\n");

            let child_refs = collect_references(object);
            for child_id in child_refs {
                if !visited.contains(&child_id) {
                    println!("--------------------------------\n");
                    dump_object_and_children(child_id, doc, visited, decode_streams, truncate_binary_streams);
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

fn print_object(obj: &Object, doc: &Document, visited: &BTreeSet<ObjectId>, indent: usize, decode_streams: bool, truncate_binary_streams: bool) {
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
                print_object(item, doc, visited, indent + 1, decode_streams, truncate_binary_streams);
                println!();
            }
            print!("{}]", indent_str);
        }
        Object::Stream(stream) => {
            println!("<<");
            for (key, value) in stream.dict.iter() {
                print!("{}/{} ", "  ".repeat(indent + 1), String::from_utf8_lossy(key));
                print_object(value, doc, visited, indent + 1, decode_streams, truncate_binary_streams);
                println!();
            }
            print!("{}>> stream", indent_str);

            if decode_streams {
                let mut decoded_content: Option<Vec<u8>> = None;
                let mut applied_filter: Option<String> = None;

                if let Ok(filter_obj) = stream.dict.get(b"Filter") {
                    let filters: Vec<String> = if let Ok(name_bytes) = filter_obj.as_name() {
                        vec![String::from_utf8_lossy(name_bytes).to_string()]
                    } else if let Ok(arr) = filter_obj.as_array() {
                        arr.iter()
                            .filter_map(|obj| obj.as_name().ok())
                            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                            .collect()
                    } else {
                        vec![]
                    };

                    if filters.iter().any(|f| f == "FlateDecode") {
                        let mut decoder = ZlibDecoder::new(&stream.content[..]);
                        let mut decompressed = Vec::new();
                        if decoder.read_to_end(&mut decompressed).is_ok() {
                            decoded_content = Some(decompressed);
                            applied_filter = Some("FlateDecode".to_string());
                        }
                    }
                }

                if let Some(content) = decoded_content {
                    let full_len = content.len();
                    let content_to_display = if truncate_binary_streams && is_binary_stream(&content) {
                        &content[..full_len.min(100)]
                    } else {
                        &content
                    };
                    let len_str = if truncate_binary_streams && full_len > 100 && is_binary_stream(&content) {
                        format!("{} (truncated to 100)", full_len)
                    } else {
                        full_len.to_string()
                    };

                    println!(
                        "\n{}Stream content (decoded with {}, len {}):\n---\n{}\n---",
                        indent_str,
                        applied_filter.unwrap_or_default(),
                        len_str,
                        String::from_utf8_lossy(content_to_display)
                    );
                } else {
                    let full_len = stream.content.len();
                    let content_to_display = if truncate_binary_streams && is_binary_stream(&stream.content) {
                        &stream.content[..full_len.min(100)]
                    } else {
                        &stream.content
                    };
                    let len_str = if truncate_binary_streams && full_len > 100 && is_binary_stream(&stream.content) {
                        format!("{} (truncated to 100)", full_len)
                    } else {
                        full_len.to_string()
                    };

                    println!(
                        "\n{}Stream content (raw, {} bytes):\n---\n{}\n---",
                        indent_str,
                        len_str,
                        String::from_utf8_lossy(content_to_display)
                    );
                }
            }
        }
        Object::Dictionary(dict) => {
            println!("<<");
            for (key, value) in dict.iter() {
                print!("{}/{} ", "  ".repeat(indent + 1), String::from_utf8_lossy(key));
                print_object(value, doc, visited, indent + 1, decode_streams, truncate_binary_streams);
                println!();
            }
            print!("{}>>", indent_str);
        }
        Object::Reference(id) => {
            print!("{} {} R", id.0, id.1);
            if visited.contains(id) {
                print!(" (visited)");
            }
        }
    }
}

fn collect_references(obj: &Object) -> Vec<ObjectId> {
    let mut refs = Vec::new();
    match obj {
        Object::Reference(id) => refs.push(*id),
        Object::Array(arr) => {
            for item in arr {
                refs.extend(collect_references(item));
            }
        }
        Object::Dictionary(dict) | Object::Stream(lopdf::Stream { dict, .. }) => {
            for (_, value) in dict.iter() {
                refs.extend(collect_references(value));
            }
        }
        _ => {}
    }
    refs
}
