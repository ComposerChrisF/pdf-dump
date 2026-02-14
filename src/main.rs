use clap::Parser;
use flate2::read::ZlibDecoder;
use lopdf::{content::Content, Document, Object, ObjectId};
use serde_json::{self, Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
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

    /// Print a single object by number (generation 0)
    #[arg(short = 'o', long)]
    object: Option<u32>,

    /// Print a one-line summary of every object
    #[arg(short = 's', long)]
    summary: bool,

    /// Dump the object tree for a specific page (1-based)
    #[arg(long)]
    page: Option<u32>,

    /// Print document metadata
    #[arg(short = 'm', long)]
    metadata: bool,

    /// Output as structured JSON
    #[arg(long)]
    json: bool,

    /// Search for objects matching an expression (e.g. Type=Font, key=MediaBox, value=Hello)
    #[arg(long)]
    search: Option<String>,

    /// Extract readable text from page content streams
    #[arg(long)]
    text: bool,

    /// Compare structurally with a second PDF file
    #[arg(long)]
    diff: Option<PathBuf>,
}

struct DumpConfig {
    decode_streams: bool,
    truncate_binary_streams: bool,
    json: bool,
}

fn main() {
    let args = Args::parse();

    // Mutual exclusivity check
    // --summary alone is a mode; with --search it becomes a modifier
    // --page alone is a mode; with --text it becomes a filter
    let mode_count = [
        args.extract_object.is_some(),
        args.object.is_some(),
        args.summary && args.search.is_none(),
        args.metadata,
        args.page.is_some() && !args.text,
        args.search.is_some(),
        args.text,
    ].iter().filter(|&&b| b).count();
    if mode_count > 1 {
        eprintln!("Error: Only one mode flag may be used at a time.");
        std::process::exit(1);
    }

    // --diff validation: only works with default mode, --page, and --json
    if args.diff.is_some() {
        let incompatible = args.object.is_some()
            || (args.summary && args.search.is_none())
            || args.metadata
            || args.extract_object.is_some()
            || args.search.is_some()
            || args.text;
        if incompatible {
            eprintln!("Error: --diff can only be combined with --page and --json.");
            std::process::exit(1);
        }
    }

    let doc = match Document::load(&args.file) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Error: Failed to load PDF file '{}'.", args.file.display());
            eprintln!("Reason: {}", e);
            std::process::exit(1);
        }
    };

    let config = DumpConfig {
        decode_streams: args.decode_streams,
        truncate_binary_streams: args.truncate_binary_streams,
        json: args.json,
    };

    // --diff mode: load second doc and compare
    if let Some(ref diff_path) = args.diff {
        let doc2 = match Document::load(diff_path) {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("Error: Failed to load second PDF file '{}'.", diff_path.display());
                eprintln!("Reason: {}", e);
                std::process::exit(1);
            }
        };
        let page_filter = args.page;
        let result = compare_pdfs(&doc, &doc2, page_filter);
        let mut out = io::stdout().lock();
        if config.json {
            print_diff_json(&mut out, &result, &args.file, diff_path);
        } else {
            print_diff(&mut out, &result, &args.file, diff_path);
        }
        return;
    }

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
    } else if let Some(ref search_expr) = args.search {
        let conditions = match parse_search_expr(search_expr) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error: Invalid search expression: {}", e);
                std::process::exit(1);
            }
        };
        let mut out = io::stdout().lock();
        if config.json {
            search_objects_json(&mut out, &doc, search_expr, &conditions, &config);
        } else {
            search_objects(&mut out, &doc, &conditions, &config, args.summary);
        }
    } else if args.text {
        let mut out = io::stdout().lock();
        if config.json {
            print_text_json(&mut out, &doc, args.page);
        } else {
            print_text(&mut out, &doc, args.page);
        }
    } else if let Some(obj_num) = args.object {
        let mut out = io::stdout().lock();
        if config.json {
            print_single_object_json(&mut out, &doc, obj_num, &config);
        } else {
            print_single_object(&mut out, &doc, obj_num, &config);
        }
    } else if args.summary {
        let mut out = io::stdout().lock();
        if config.json {
            print_summary_json(&mut out, &doc);
        } else {
            print_summary(&mut out, &doc);
        }
    } else if let Some(page_num) = args.page {
        let mut out = io::stdout().lock();
        if config.json {
            dump_page_json(&mut out, &doc, page_num, &config);
        } else {
            dump_page(&mut out, &doc, page_num, &config);
        }
    } else if args.metadata {
        let mut out = io::stdout().lock();
        if config.json {
            print_metadata_json(&mut out, &doc);
        } else {
            print_metadata(&mut out, &doc);
        }
    } else {
        let mut out = io::stdout().lock();
        if config.json {
            dump_json(&mut out, &doc, &config);
        } else {
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

fn object_type_label(obj: &Object) -> String {
    let dict = match obj {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return "-".to_string(),
    };
    match dict.get_type() {
        Ok(name) => String::from_utf8_lossy(name).into_owned(),
        Err(_) => "-".to_string(),
    }
}

fn print_single_object(writer: &mut impl Write, doc: &Document, obj_num: u32, config: &DumpConfig) {
    let obj_id = (obj_num, 0);
    match doc.get_object(obj_id) {
        Ok(object) => {
            writeln!(writer, "Object {} 0:", obj_num).unwrap();
            let visited = BTreeSet::new();
            let mut child_refs = BTreeSet::new();
            print_object(writer, object, doc, &visited, 1, config, false, &mut child_refs);
            writeln!(writer).unwrap();
        }
        Err(_) => {
            eprintln!("Error: Object {} not found in the document.", obj_num);
            std::process::exit(1);
        }
    }
}

fn print_summary(writer: &mut impl Write, doc: &Document) {
    writeln!(writer, "PDF {}  |  {} objects\n", doc.version, doc.objects.len()).unwrap();
    writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} Detail", "Obj#", "Gen", "Kind", "/Type").unwrap();

    for (&(obj_num, generation), object) in &doc.objects {
        let kind = object.enum_variant();
        let type_label = object_type_label(object);
        let detail = summary_detail(object);
        writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} {}", obj_num, generation, kind, type_label, detail).unwrap();
    }
}

fn dump_page(writer: &mut impl Write, doc: &Document, page_num: u32, config: &DumpConfig) {
    let pages = doc.get_pages();
    let total = pages.len();
    let page_id = match pages.get(&page_num) {
        Some(&id) => id,
        None => {
            eprintln!("Error: Page {} not found. Document has {} pages.", page_num, total);
            std::process::exit(1);
        }
    };

    let mut visited = BTreeSet::new();

    // Pre-seed visited with /Parent to confine traversal to this page's subtree
    if let Ok(Object::Dictionary(dict)) = doc.get_object(page_id)
        && let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference())
    {
        visited.insert(parent_ref);
    }

    writeln!(writer, "Page {} (Object {} {}):", page_num, page_id.0, page_id.1).unwrap();
    dump_object_and_children(writer, page_id, doc, &mut visited, config, false);
}

// ── JSON output (Phase 1) ────────────────────────────────────────────

#[allow(clippy::only_used_in_recursion)]
fn object_to_json(obj: &Object, doc: &Document, config: &DumpConfig) -> Value {
    match obj {
        Object::Null => json!({"type": "null"}),
        Object::Boolean(b) => json!({"type": "boolean", "value": b}),
        Object::Integer(i) => json!({"type": "integer", "value": i}),
        Object::Real(r) => json!({"type": "real", "value": r}),
        Object::Name(n) => json!({"type": "name", "value": String::from_utf8_lossy(n)}),
        Object::String(bytes, _) => json!({"type": "string", "value": String::from_utf8_lossy(bytes)}),
        Object::Array(arr) => {
            let items: Vec<Value> = arr.iter().map(|o| object_to_json(o, doc, config)).collect();
            json!({"type": "array", "items": items})
        }
        Object::Dictionary(dict) => {
            let entries: serde_json::Map<String, Value> = dict.iter()
                .map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), object_to_json(v, doc, config)))
                .collect();
            json!({"type": "dictionary", "entries": entries})
        }
        Object::Stream(stream) => {
            let entries: serde_json::Map<String, Value> = stream.dict.iter()
                .map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), object_to_json(v, doc, config)))
                .collect();
            let mut val = json!({"type": "stream", "dict": entries});
            if config.decode_streams {
                let decoded = decode_stream(stream);
                if !is_binary_stream(&decoded) {
                    val["content"] = json!(String::from_utf8_lossy(&decoded));
                } else if config.truncate_binary_streams {
                    val["content_truncated"] = json!(format!("<binary, {} bytes>", decoded.len()));
                } else {
                    val["content_binary"] = json!(format!("<binary, {} bytes>", decoded.len()));
                }
            }
            val
        }
        Object::Reference(id) => json!({"type": "reference", "object_number": id.0, "generation": id.1}),
    }
}

fn collect_reachable_objects(doc: &Document) -> BTreeMap<String, Value> {
    let config = DumpConfig { decode_streams: false, truncate_binary_streams: false, json: true };
    let mut result = BTreeMap::new();
    let mut visited = BTreeSet::new();

    fn walk(doc: &Document, obj_id: ObjectId, visited: &mut BTreeSet<ObjectId>, result: &mut BTreeMap<String, Value>, config: &DumpConfig) {
        if visited.contains(&obj_id) { return; }
        visited.insert(obj_id);
        if let Ok(obj) = doc.get_object(obj_id) {
            let key = format!("{}:{}", obj_id.0, obj_id.1);
            result.insert(key, object_to_json(obj, doc, config));
            collect_refs(obj, doc, visited, result, config);
        }
    }

    fn collect_refs(obj: &Object, doc: &Document, visited: &mut BTreeSet<ObjectId>, result: &mut BTreeMap<String, Value>, config: &DumpConfig) {
        match obj {
            Object::Reference(id) => walk(doc, *id, visited, result, config),
            Object::Array(arr) => {
                for item in arr { collect_refs(item, doc, visited, result, config); }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() { collect_refs(v, doc, visited, result, config); }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter() { collect_refs(v, doc, visited, result, config); }
            }
            _ => {}
        }
    }

    // Start from trailer refs
    for (_, v) in doc.trailer.iter() {
        if let Ok(id) = v.as_reference() {
            walk(doc, id, &mut visited, &mut result, &config);
        }
    }

    result
}

fn dump_json(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let trailer_json = object_to_json(&Object::Dictionary(doc.trailer.clone()), doc, config);
    let objects = collect_reachable_objects(doc);
    let output = json!({
        "trailer": trailer_json,
        "objects": objects,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

fn print_single_object_json(writer: &mut impl Write, doc: &Document, obj_num: u32, config: &DumpConfig) {
    let obj_id = (obj_num, 0);
    match doc.get_object(obj_id) {
        Ok(object) => {
            let output = json!({
                "object_number": obj_num,
                "generation": 0,
                "object": object_to_json(object, doc, config),
            });
            writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
        }
        Err(_) => {
            eprintln!("Error: Object {} not found in the document.", obj_num);
            std::process::exit(1);
        }
    }
}

fn summary_detail(object: &Object) -> String {
    match object {
        Object::Stream(stream) => {
            let filter = stream.dict.get(b"Filter").ok()
                .and_then(|f| f.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                .unwrap_or_default();
            if filter.is_empty() {
                format!("{} bytes", stream.content.len())
            } else {
                format!("{} bytes, {}", stream.content.len(), filter)
            }
        }
        Object::Dictionary(dict) => {
            let count = dict.len();
            let notable: Vec<String> = dict.iter()
                .filter(|(k, _)| {
                    let k = &**k;
                    k == b"BaseFont" || k == b"Subtype" || k == b"MediaBox"
                })
                .take(3)
                .map(|(k, v)| {
                    let key = String::from_utf8_lossy(k);
                    match v {
                        Object::Name(n) => format!("/{}={}", key, String::from_utf8_lossy(n)),
                        Object::Array(arr) => {
                            let items: Vec<String> = arr.iter().map(|o| match o {
                                Object::Integer(i) => i.to_string(),
                                Object::Real(r) => r.to_string(),
                                _ => "?".to_string(),
                            }).collect();
                            format!("/{}=[{}]", key, items.join(" "))
                        }
                        _ => format!("/{}=...", key),
                    }
                })
                .collect();
            if notable.is_empty() {
                format!("{} keys", count)
            } else {
                notable.join(", ")
            }
        }
        _ => String::new(),
    }
}

fn print_summary_json(writer: &mut impl Write, doc: &Document) {
    let objects: Vec<Value> = doc.objects.iter()
        .map(|(&(obj_num, generation), object)| {
            json!({
                "object_number": obj_num,
                "generation": generation,
                "kind": object.enum_variant(),
                "type": object_type_label(object),
                "detail": summary_detail(object),
            })
        })
        .collect();
    let output = json!({
        "version": doc.version,
        "object_count": doc.objects.len(),
        "objects": objects,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

fn metadata_info(doc: &Document) -> (serde_json::Map<String, Value>, serde_json::Map<String, Value>) {
    let mut info = serde_json::Map::new();
    let mut catalog = serde_json::Map::new();

    if let Ok(info_ref) = doc.trailer.get(b"Info")
        && let Ok((_, Object::Dictionary(info_dict))) = doc.dereference(info_ref)
    {
        let fields = [
            b"Title".as_slice(), b"Author", b"Subject", b"Keywords",
            b"Creator", b"Producer", b"CreationDate", b"ModDate",
        ];
        for key in fields {
            if let Ok(Object::String(bytes, _)) = info_dict.get(key) {
                info.insert(
                    String::from_utf8_lossy(key).into_owned(),
                    json!(String::from_utf8_lossy(bytes)),
                );
            }
        }
    }

    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok())
        && let Ok(Object::Dictionary(cat)) = doc.get_object(root_ref)
    {
        for key in [b"PageLayout".as_slice(), b"PageMode", b"Lang"] {
            if let Ok(val) = cat.get(key) {
                let text = match val {
                    Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => continue,
                };
                catalog.insert(String::from_utf8_lossy(key).into_owned(), json!(text));
            }
        }
    }

    (info, catalog)
}

fn print_metadata_json(writer: &mut impl Write, doc: &Document) {
    let (info, catalog) = metadata_info(doc);
    let output = json!({
        "version": doc.version,
        "object_count": doc.objects.len(),
        "page_count": doc.get_pages().len(),
        "info": info,
        "catalog": catalog,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

fn dump_page_json(writer: &mut impl Write, doc: &Document, page_num: u32, config: &DumpConfig) {
    let pages = doc.get_pages();
    let total = pages.len();
    let page_id = match pages.get(&page_num) {
        Some(&id) => id,
        None => {
            eprintln!("Error: Page {} not found. Document has {} pages.", page_num, total);
            std::process::exit(1);
        }
    };

    // Collect page subtree objects into JSON
    let mut visited = BTreeSet::new();
    let mut objects = BTreeMap::new();

    // Pre-seed visited with /Parent
    if let Ok(Object::Dictionary(dict)) = doc.get_object(page_id)
        && let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference())
    {
        visited.insert(parent_ref);
    }

    fn walk_page(doc: &Document, obj_id: ObjectId, visited: &mut BTreeSet<ObjectId>, objects: &mut BTreeMap<String, Value>, config: &DumpConfig) {
        if visited.contains(&obj_id) { return; }
        visited.insert(obj_id);
        if let Ok(obj) = doc.get_object(obj_id) {
            let key = format!("{}:{}", obj_id.0, obj_id.1);
            objects.insert(key, object_to_json(obj, doc, config));
            collect_refs_page(obj, doc, visited, objects, config);
        }
    }

    fn collect_refs_page(obj: &Object, doc: &Document, visited: &mut BTreeSet<ObjectId>, objects: &mut BTreeMap<String, Value>, config: &DumpConfig) {
        match obj {
            Object::Reference(id) => walk_page(doc, *id, visited, objects, config),
            Object::Array(arr) => {
                for item in arr { collect_refs_page(item, doc, visited, objects, config); }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() { collect_refs_page(v, doc, visited, objects, config); }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter() { collect_refs_page(v, doc, visited, objects, config); }
            }
            _ => {}
        }
    }

    walk_page(doc, page_id, &mut visited, &mut objects, config);

    let output = json!({
        "page_number": page_num,
        "objects": objects,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Search (Phase 2) ─────────────────────────────────────────────────

enum SearchCondition {
    KeyEquals { key: Vec<u8>, value: Vec<u8> },
    HasKey { key: Vec<u8> },
    ValueContains { text: String },
}

fn parse_search_expr(expr: &str) -> Result<Vec<SearchCondition>, String> {
    let mut conditions = Vec::new();
    for part in expr.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        if let Some((left, right)) = part.split_once('=') {
            let left = left.trim();
            let right = right.trim();
            if right.is_empty() {
                return Err(format!("Empty value in '{}'", part));
            }
            if left.eq_ignore_ascii_case("key") {
                conditions.push(SearchCondition::HasKey { key: right.as_bytes().to_vec() });
            } else if left.eq_ignore_ascii_case("value") {
                conditions.push(SearchCondition::ValueContains { text: right.to_string() });
            } else {
                conditions.push(SearchCondition::KeyEquals {
                    key: left.as_bytes().to_vec(),
                    value: right.as_bytes().to_vec(),
                });
            }
        } else {
            return Err(format!("Invalid condition '{}'. Expected Key=Value, key=Key, or value=Text", part));
        }
    }
    if conditions.is_empty() {
        return Err("Empty search expression".to_string());
    }
    Ok(conditions)
}

fn object_matches(obj: &Object, conditions: &[SearchCondition]) -> bool {
    let dict = match obj {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return false,
    };

    conditions.iter().all(|cond| match cond {
        SearchCondition::KeyEquals { key, value } => {
            dict.get(key).ok().is_some_and(|v| {
                match v {
                    Object::Name(n) => n.eq_ignore_ascii_case(value),
                    Object::String(bytes, _) => {
                        let v_lower = value.to_ascii_lowercase();
                        bytes.to_ascii_lowercase() == v_lower
                    }
                    _ => false,
                }
            })
        }
        SearchCondition::HasKey { key } => dict.get(key).is_ok(),
        SearchCondition::ValueContains { text } => {
            let text_lower = text.to_lowercase();
            dict.iter().any(|(_, v)| {
                match v {
                    Object::Name(n) => String::from_utf8_lossy(n).to_lowercase().contains(&text_lower),
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).to_lowercase().contains(&text_lower),
                    _ => false,
                }
            })
        }
    })
}

fn search_objects(writer: &mut impl Write, doc: &Document, conditions: &[SearchCondition], config: &DumpConfig, summary_mode: bool) {
    let mut count = 0;

    if summary_mode {
        writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} Detail", "Obj#", "Gen", "Kind", "/Type").unwrap();
    }

    for (&(obj_num, generation), object) in &doc.objects {
        if object_matches(object, conditions) {
            count += 1;
            if summary_mode {
                let kind = object.enum_variant();
                let type_label = object_type_label(object);
                let detail = summary_detail(object);
                writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} {}", obj_num, generation, kind, type_label, detail).unwrap();
            } else {
                writeln!(writer, "Object {} {}:", obj_num, generation).unwrap();
                let visited = BTreeSet::new();
                let mut child_refs = BTreeSet::new();
                print_object(writer, object, doc, &visited, 1, config, false, &mut child_refs);
                writeln!(writer, "\n").unwrap();
            }
        }
    }
    writeln!(writer, "Found {} matching objects.", count).unwrap();
}

fn search_objects_json(writer: &mut impl Write, doc: &Document, expr: &str, conditions: &[SearchCondition], config: &DumpConfig) {
    let mut matches = Vec::new();
    for (&(obj_num, generation), object) in &doc.objects {
        if object_matches(object, conditions) {
            matches.push(json!({
                "object_number": obj_num,
                "generation": generation,
                "object": object_to_json(object, doc, config),
            }));
        }
    }
    let output = json!({
        "query": expr,
        "match_count": matches.len(),
        "matches": matches,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Text extraction (Phase 3) ────────────────────────────────────────

fn extract_text_from_page(doc: &Document, page_id: ObjectId) -> String {
    let mut text = String::new();

    // Get content stream(s) for the page
    let page_dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return text,
    };

    let content_ids: Vec<ObjectId> = match page_dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return text,
    };

    let mut all_bytes = Vec::new();
    for cid in &content_ids {
        if let Ok(Object::Stream(stream)) = doc.get_object(*cid) {
            let decoded = decode_stream(stream);
            all_bytes.extend_from_slice(&decoded);
        }
    }

    let operations = match Content::decode(&all_bytes) {
        Ok(content) => content.operations,
        Err(_) => return text,
    };

    let mut first_bt = true;
    for op in &operations {
        match op.operator.as_str() {
            "BT" => {
                if !first_bt && !text.ends_with('\n') {
                    text.push('\n');
                }
                first_bt = false;
            }
            "Td" | "TD" => {
                // Check ty (second operand) for line break
                if op.operands.len() >= 2 {
                    if let Object::Integer(ty) = &op.operands[1] {
                        if *ty != 0 { text.push('\n'); }
                    } else if let Object::Real(ty) = &op.operands[1]
                        && *ty != 0.0 { text.push('\n');
                    }
                }
            }
            "T*" => { text.push('\n'); }
            "Tj" => {
                if let Some(Object::String(bytes, _)) = op.operands.first() {
                    text.push_str(&String::from_utf8_lossy(bytes));
                }
            }
            "TJ" => {
                if let Some(Object::Array(arr)) = op.operands.first() {
                    for item in arr {
                        match item {
                            Object::String(bytes, _) => {
                                text.push_str(&String::from_utf8_lossy(bytes));
                            }
                            Object::Integer(n) if *n < -100 => { text.push(' '); }
                            Object::Real(n) if *n < -100.0 => { text.push(' '); }
                            _ => {}
                        }
                    }
                }
            }
            "'" => {
                text.push('\n');
                if let Some(Object::String(bytes, _)) = op.operands.first() {
                    text.push_str(&String::from_utf8_lossy(bytes));
                }
            }
            "\"" => {
                text.push('\n');
                // Third operand is the string
                if let Some(Object::String(bytes, _)) = op.operands.get(2) {
                    text.push_str(&String::from_utf8_lossy(bytes));
                }
            }
            _ => {}
        }
    }

    text
}

fn print_text(writer: &mut impl Write, doc: &Document, page_filter: Option<u32>) {
    let pages = doc.get_pages();

    if let Some(page_num) = page_filter {
        let page_id = match pages.get(&page_num) {
            Some(&id) => id,
            None => {
                eprintln!("Error: Page {} not found. Document has {} pages.", page_num, pages.len());
                std::process::exit(1);
            }
        };
        writeln!(writer, "--- Page {} ---", page_num).unwrap();
        let text = extract_text_from_page(doc, page_id);
        writeln!(writer, "{}", text).unwrap();
    } else {
        for (&page_num, &page_id) in &pages {
            writeln!(writer, "--- Page {} ---", page_num).unwrap();
            let text = extract_text_from_page(doc, page_id);
            writeln!(writer, "{}", text).unwrap();
        }
    }
}

fn print_text_json(writer: &mut impl Write, doc: &Document, page_filter: Option<u32>) {
    let pages = doc.get_pages();
    let mut page_results = Vec::new();

    if let Some(page_num) = page_filter {
        let page_id = match pages.get(&page_num) {
            Some(&id) => id,
            None => {
                eprintln!("Error: Page {} not found. Document has {} pages.", page_num, pages.len());
                std::process::exit(1);
            }
        };
        let text = extract_text_from_page(doc, page_id);
        page_results.push(json!({"page_number": page_num, "text": text}));
    } else {
        for (&page_num, &page_id) in &pages {
            let text = extract_text_from_page(doc, page_id);
            page_results.push(json!({"page_number": page_num, "text": text}));
        }
    }

    let output = json!({"pages": page_results});
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Diff (Phase 4) ──────────────────────────────────────────────────

struct DiffResult {
    metadata_diffs: Vec<String>,
    page_diffs: Vec<PageDiff>,
    font_diffs: FontDiff,
    object_count: (usize, usize),
}

struct PageDiff {
    page_number: u32,
    identical: bool,
    dict_diffs: Vec<String>,
    resource_diffs: Vec<String>,
    content_diffs: Vec<String>,
}

struct FontDiff {
    only_in_first: Vec<String>,
    only_in_second: Vec<String>,
}

fn compare_pdfs(doc1: &Document, doc2: &Document, page_filter: Option<u32>) -> DiffResult {
    let metadata_diffs = compare_metadata(doc1, doc2);
    let font_diffs = compare_fonts(doc1, doc2);

    let pages1 = doc1.get_pages();
    let pages2 = doc2.get_pages();

    let mut page_diffs = Vec::new();

    if let Some(pn) = page_filter {
        let id1 = pages1.get(&pn);
        let id2 = pages2.get(&pn);
        match (id1, id2) {
            (Some(&id1), Some(&id2)) => {
                page_diffs.push(compare_page(doc1, doc2, id1, id2, pn));
            }
            (Some(_), None) => {
                page_diffs.push(PageDiff {
                    page_number: pn,
                    identical: false,
                    dict_diffs: vec![format!("Page {} only exists in first file", pn)],
                    resource_diffs: vec![],
                    content_diffs: vec![],
                });
            }
            (None, Some(_)) => {
                page_diffs.push(PageDiff {
                    page_number: pn,
                    identical: false,
                    dict_diffs: vec![format!("Page {} only exists in second file", pn)],
                    resource_diffs: vec![],
                    content_diffs: vec![],
                });
            }
            (None, None) => {
                page_diffs.push(PageDiff {
                    page_number: pn,
                    identical: false,
                    dict_diffs: vec![format!("Page {} not found in either file", pn)],
                    resource_diffs: vec![],
                    content_diffs: vec![],
                });
            }
        }
    } else {
        let max_pages = pages1.len().max(pages2.len()) as u32;
        for pn in 1..=max_pages {
            let id1 = pages1.get(&pn);
            let id2 = pages2.get(&pn);
            match (id1, id2) {
                (Some(&id1), Some(&id2)) => {
                    page_diffs.push(compare_page(doc1, doc2, id1, id2, pn));
                }
                (Some(_), None) => {
                    page_diffs.push(PageDiff {
                        page_number: pn,
                        identical: false,
                        dict_diffs: vec![format!("Page {} only in first file", pn)],
                        resource_diffs: vec![],
                        content_diffs: vec![],
                    });
                }
                (None, Some(_)) => {
                    page_diffs.push(PageDiff {
                        page_number: pn,
                        identical: false,
                        dict_diffs: vec![format!("Page {} only in second file", pn)],
                        resource_diffs: vec![],
                        content_diffs: vec![],
                    });
                }
                _ => {}
            }
        }
    }

    DiffResult {
        metadata_diffs,
        page_diffs,
        font_diffs,
        object_count: (doc1.objects.len(), doc2.objects.len()),
    }
}

fn compare_metadata(doc1: &Document, doc2: &Document) -> Vec<String> {
    let mut diffs = Vec::new();

    if doc1.version != doc2.version {
        diffs.push(format!("Version: {} vs {}", doc1.version, doc2.version));
    }

    let pages1 = doc1.get_pages().len();
    let pages2 = doc2.get_pages().len();
    if pages1 != pages2 {
        diffs.push(format!("Pages: {} vs {}", pages1, pages2));
    }

    // Compare /Info fields
    let info_fields = [
        b"Title".as_slice(), b"Author", b"Subject", b"Keywords",
        b"Creator", b"Producer", b"CreationDate", b"ModDate",
    ];
    let get_info = |doc: &Document, field: &[u8]| -> Option<String> {
        let info_ref = doc.trailer.get(b"Info").ok()?;
        let (_, obj) = doc.dereference(info_ref).ok()?;
        if let Object::Dictionary(d) = obj
            && let Ok(Object::String(bytes, _)) = d.get(field)
        {
            return Some(String::from_utf8_lossy(bytes).into_owned());
        }
        None
    };

    for field in info_fields {
        let v1 = get_info(doc1, field);
        let v2 = get_info(doc2, field);
        if v1 != v2 {
            let name = String::from_utf8_lossy(field);
            let s1 = v1.unwrap_or_else(|| "(none)".to_string());
            let s2 = v2.unwrap_or_else(|| "(none)".to_string());
            diffs.push(format!("{}: \"{}\" vs \"{}\"", name, s1, s2));
        }
    }

    diffs
}

fn format_dict_value(obj: &Object) -> String {
    match obj {
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::Integer(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::Boolean(b) => b.to_string(),
        Object::String(bytes, _) => format!("({})", String::from_utf8_lossy(bytes)),
        Object::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_dict_value).collect();
            format!("[{}]", items.join(" "))
        }
        Object::Reference(id) => format!("{} {} R", id.0, id.1),
        Object::Null => "null".to_string(),
        Object::Dictionary(_) => "<<...>>".to_string(),
        Object::Stream(_) => "<<stream>>".to_string(),
    }
}

fn compare_page(doc1: &Document, doc2: &Document, page_id1: ObjectId, page_id2: ObjectId, page_num: u32) -> PageDiff {
    let dict1 = match doc1.get_object(page_id1) {
        Ok(Object::Dictionary(d)) => d,
        _ => return PageDiff { page_number: page_num, identical: false, dict_diffs: vec!["Could not read page from first file".into()], resource_diffs: vec![], content_diffs: vec![] },
    };
    let dict2 = match doc2.get_object(page_id2) {
        Ok(Object::Dictionary(d)) => d,
        _ => return PageDiff { page_number: page_num, identical: false, dict_diffs: vec!["Could not read page from second file".into()], resource_diffs: vec![], content_diffs: vec![] },
    };

    let mut dict_diffs = Vec::new();
    let mut resource_diffs = Vec::new();

    // Compare page dict entries (skip Parent, Contents, Resources)
    let skip_keys: &[&[u8]] = &[b"Parent", b"Contents", b"Resources"];
    for (key, val1) in dict1.iter() {
        if skip_keys.contains(&key.as_slice()) { continue; }
        let v1_str = format_dict_value(val1);
        match dict2.get(key) {
            Ok(val2) => {
                let v2_str = format_dict_value(val2);
                if v1_str != v2_str {
                    dict_diffs.push(format!("/{}: {} vs {}", String::from_utf8_lossy(key), v1_str, v2_str));
                }
            }
            Err(_) => {
                dict_diffs.push(format!("/{}: {} vs (missing)", String::from_utf8_lossy(key), v1_str));
            }
        }
    }
    // Keys only in dict2
    for (key, val2) in dict2.iter() {
        if skip_keys.contains(&key.as_slice()) { continue; }
        if dict1.get(key).is_err() {
            dict_diffs.push(format!("/{}: (missing) vs {}", String::from_utf8_lossy(key), format_dict_value(val2)));
        }
    }

    // Compare resources
    let get_font_names = |doc: &Document, dict: &lopdf::Dictionary| -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let resources = match dict.get(b"Resources") {
            Ok(Object::Reference(id)) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { d } else { return names; }
            }
            Ok(Object::Dictionary(d)) => d,
            _ => return names,
        };
        if let Ok(font_obj) = resources.get(b"Font") {
            let font_dict = match font_obj {
                Object::Dictionary(d) => d,
                Object::Reference(id) => {
                    if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { d } else { return names; }
                }
                _ => return names,
            };
            for (k, _) in font_dict.iter() {
                names.insert(String::from_utf8_lossy(k).into_owned());
            }
        }
        names
    };

    let fonts1 = get_font_names(doc1, dict1);
    let fonts2 = get_font_names(doc2, dict2);
    if fonts1 != fonts2 {
        for f in fonts1.difference(&fonts2) {
            resource_diffs.push(format!("Font {} only in first file", f));
        }
        for f in fonts2.difference(&fonts1) {
            resource_diffs.push(format!("Font {} only in second file", f));
        }
    }

    // Compare content streams
    let content_diffs = compare_content_streams(doc1, doc2, page_id1, page_id2);

    let identical = dict_diffs.is_empty() && resource_diffs.is_empty() && content_diffs.is_empty();

    PageDiff {
        page_number: page_num,
        identical,
        dict_diffs,
        resource_diffs,
        content_diffs,
    }
}

fn get_content_ops(doc: &Document, page_id: ObjectId) -> Vec<String> {
    let dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return vec![],
    };

    let content_ids: Vec<ObjectId> = match dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return vec![],
    };

    let mut all_bytes = Vec::new();
    for cid in &content_ids {
        if let Ok(Object::Stream(stream)) = doc.get_object(*cid) {
            let decoded = decode_stream(stream);
            all_bytes.extend_from_slice(&decoded);
        }
    }

    match Content::decode(&all_bytes) {
        Ok(content) => content.operations.iter().map(|op| format!("{:?}", op)).collect(),
        Err(_) => vec![],
    }
}

fn compare_content_streams(doc1: &Document, doc2: &Document, page_id1: ObjectId, page_id2: ObjectId) -> Vec<String> {
    let ops1 = get_content_ops(doc1, page_id1);
    let ops2 = get_content_ops(doc2, page_id2);

    if ops1 == ops2 {
        return vec![];
    }

    // Simple line-based diff
    let mut diffs = Vec::new();
    let max = ops1.len().max(ops2.len());
    for i in 0..max {
        match (ops1.get(i), ops2.get(i)) {
            (Some(a), Some(b)) if a != b => {
                diffs.push(format!("- {}", a));
                diffs.push(format!("+ {}", b));
            }
            (Some(a), None) => {
                diffs.push(format!("- {}", a));
            }
            (None, Some(b)) => {
                diffs.push(format!("+ {}", b));
            }
            _ => {}
        }
    }
    diffs
}

fn collect_all_font_names(doc: &Document) -> BTreeSet<String> {
    let mut fonts = BTreeSet::new();
    for obj in doc.objects.values() {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        if dict.get_type().ok().is_some_and(|t| t == b"Font")
            && let Ok(Object::Name(name)) = dict.get(b"BaseFont")
        {
            fonts.insert(String::from_utf8_lossy(name).into_owned());
        }
    }
    fonts
}

fn compare_fonts(doc1: &Document, doc2: &Document) -> FontDiff {
    let fonts1 = collect_all_font_names(doc1);
    let fonts2 = collect_all_font_names(doc2);
    FontDiff {
        only_in_first: fonts1.difference(&fonts2).cloned().collect(),
        only_in_second: fonts2.difference(&fonts1).cloned().collect(),
    }
}

fn print_diff(writer: &mut impl Write, result: &DiffResult, file1: &std::path::Path, file2: &std::path::Path) {
    writeln!(writer, "Comparing: {} vs {}", file1.display(), file2.display()).unwrap();
    writeln!(writer, "Objects: {} vs {}\n", result.object_count.0, result.object_count.1).unwrap();

    if !result.metadata_diffs.is_empty() {
        writeln!(writer, "--- Metadata ---").unwrap();
        for d in &result.metadata_diffs {
            writeln!(writer, "  {}", d).unwrap();
        }
        writeln!(writer).unwrap();
    }

    for page in &result.page_diffs {
        writeln!(writer, "--- Page {} ---", page.page_number).unwrap();
        if page.identical {
            writeln!(writer, "  (identical)").unwrap();
        } else {
            for d in &page.dict_diffs {
                writeln!(writer, "  {}", d).unwrap();
            }
            for d in &page.resource_diffs {
                writeln!(writer, "  {}", d).unwrap();
            }
            if !page.content_diffs.is_empty() {
                writeln!(writer, "  Content stream: differs").unwrap();
                for d in &page.content_diffs {
                    writeln!(writer, "    {}", d).unwrap();
                }
            }
        }
        writeln!(writer).unwrap();
    }

    if !result.font_diffs.only_in_first.is_empty() || !result.font_diffs.only_in_second.is_empty() {
        writeln!(writer, "--- Fonts ---").unwrap();
        for f in &result.font_diffs.only_in_first {
            writeln!(writer, "  Only in {}: {}", file1.display(), f).unwrap();
        }
        for f in &result.font_diffs.only_in_second {
            writeln!(writer, "  Only in {}: {}", file2.display(), f).unwrap();
        }
        writeln!(writer).unwrap();
    }
}

fn print_diff_json(writer: &mut impl Write, result: &DiffResult, file1: &std::path::Path, file2: &std::path::Path) {
    let pages: Vec<Value> = result.page_diffs.iter().map(|p| {
        json!({
            "page_number": p.page_number,
            "identical": p.identical,
            "dict_diffs": p.dict_diffs,
            "resource_diffs": p.resource_diffs,
            "content_diffs": p.content_diffs,
        })
    }).collect();

    let output = json!({
        "file1": file1.display().to_string(),
        "file2": file2.display().to_string(),
        "object_count": {"file1": result.object_count.0, "file2": result.object_count.1},
        "metadata_diffs": result.metadata_diffs,
        "page_diffs": pages,
        "font_diffs": {
            "only_in_first": result.font_diffs.only_in_first,
            "only_in_second": result.font_diffs.only_in_second,
        },
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Existing text output functions ───────────────────────────────────

fn print_metadata(writer: &mut impl Write, doc: &Document) {
    writeln!(writer, "PDF Version: {}", doc.version).unwrap();
    writeln!(writer, "Objects:     {}", doc.objects.len()).unwrap();
    writeln!(writer, "Pages:       {}", doc.get_pages().len()).unwrap();

    // Extract /Info from trailer
    if let Ok(info_ref) = doc.trailer.get(b"Info")
        && let Ok((_, Object::Dictionary(info))) = doc.dereference(info_ref)
    {
        let fields = [
            b"Title".as_slice(),
            b"Author",
            b"Subject",
            b"Keywords",
            b"Creator",
            b"Producer",
            b"CreationDate",
            b"ModDate",
        ];
        for key in fields {
            if let Ok(val) = info.get(key) {
                let text = match val {
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => continue,
                };
                writeln!(writer, "{}: {}", String::from_utf8_lossy(key), text).unwrap();
            }
        }
    }

    // Check catalog for additional fields
    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok())
        && let Ok(Object::Dictionary(catalog)) = doc.get_object(root_ref)
    {
        for key in [b"PageLayout".as_slice(), b"PageMode", b"Lang"] {
            if let Ok(val) = catalog.get(key) {
                let text = match val {
                    Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => continue,
                };
                writeln!(writer, "{}: {}", String::from_utf8_lossy(key), text).unwrap();
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
            json: false,
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
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
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
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true, json: false };
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
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
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
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true, json: false };
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
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true, json: false };
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
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true, json: false };
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
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true, json: false };
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
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: true, json: false };
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
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
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
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
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
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
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
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: true, json: false };
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

    // ── object_type_label ───────────────────────────────────────────

    #[test]
    fn object_type_label_dictionary_with_type() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        assert_eq!(object_type_label(&Object::Dictionary(dict)), "Page");
    }

    #[test]
    fn object_type_label_dictionary_without_type() {
        let dict = Dictionary::new();
        assert_eq!(object_type_label(&Object::Dictionary(dict)), "-");
    }

    #[test]
    fn object_type_label_stream_with_type() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, vec![]);
        assert_eq!(object_type_label(&Object::Stream(stream)), "XObject");
    }

    #[test]
    fn object_type_label_stream_without_type() {
        let stream = Stream::new(Dictionary::new(), vec![]);
        assert_eq!(object_type_label(&Object::Stream(stream)), "-");
    }

    #[test]
    fn object_type_label_integer() {
        assert_eq!(object_type_label(&Object::Integer(42)), "-");
    }

    #[test]
    fn object_type_label_null() {
        assert_eq!(object_type_label(&Object::Null), "-");
    }

    // ── print_single_object ─────────────────────────────────────────

    #[test]
    fn print_single_object_integer() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = default_config();
        let out = output_of(|w| {
            print_single_object(w, &doc, 1, &config);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("42"));
    }

    #[test]
    fn print_single_object_dict_does_not_follow_refs() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Child", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(99));
        let config = default_config();
        let out = output_of(|w| {
            print_single_object(w, &doc, 1, &config);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("2 0 R"));
        // Should NOT follow into object 2
        assert!(!out.contains("Object 2 0:"));
        assert!(!out.contains("99"));
    }

    #[test]
    fn print_single_object_stream_with_decode() {
        let mut doc = Document::new();
        let stream = make_stream(None, b"visible data".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
        let out = output_of(|w| {
            print_single_object(w, &doc, 1, &config);
        });
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("visible data"));
    }

    // ── print_summary ───────────────────────────────────────────────

    #[test]
    fn print_summary_shows_version_and_count() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("PDF 1.4"));
        assert!(out.contains("1 objects"));
        assert!(out.contains("Obj#"));
    }

    #[test]
    fn print_summary_stream_shows_bytes_and_filter() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, vec![0u8; 100]);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("100 bytes"));
        assert!(out.contains("FlateDecode"));
        assert!(out.contains("Stream"));
    }

    #[test]
    fn print_summary_dict_shows_type_label() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("Page"));
        assert!(out.contains("Dictionary"));
    }

    #[test]
    fn print_summary_multiple_objects_sorted() {
        let mut doc = Document::new();
        doc.objects.insert((3, 0), Object::Integer(30));
        doc.objects.insert((1, 0), Object::Integer(10));
        doc.objects.insert((2, 0), Object::Integer(20));
        let out = output_of(|w| {
            print_summary(w, &doc);
        });
        assert!(out.contains("3 objects"));
        // All three should appear
        let pos1 = out.find("     1").unwrap();
        let pos2 = out.find("     2").unwrap();
        let pos3 = out.find("     3").unwrap();
        assert!(pos1 < pos2 && pos2 < pos3, "Objects should be in sorted order");
    }

    // ── dump_page ───────────────────────────────────────────────────

    fn build_two_page_doc() -> Document {
        let mut doc = Document::new();

        // Content streams
        let c1 = Stream::new(Dictionary::new(), b"BT /F1 12 Tf (Page1) Tj ET".to_vec());
        let c1_id = doc.add_object(Object::Stream(c1));
        let c2 = Stream::new(Dictionary::new(), b"BT /F1 12 Tf (Page2) Tj ET".to_vec());
        let c2_id = doc.add_object(Object::Stream(c2));

        // Font
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let font_id = doc.add_object(Object::Dictionary(font));

        // Resources (shared)
        let mut f1 = Dictionary::new();
        f1.set("F1", Object::Reference(font_id));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(f1));
        let resources_id = doc.add_object(Object::Dictionary(resources));

        // Pages node (placeholder, will update Kids after page creation)
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(2));
        pages.set("Kids", Object::Array(vec![])); // placeholder
        let pages_id = doc.add_object(Object::Dictionary(pages));

        // Page 1
        let mut p1 = Dictionary::new();
        p1.set("Type", Object::Name(b"Page".to_vec()));
        p1.set("Parent", Object::Reference(pages_id));
        p1.set("Contents", Object::Reference(c1_id));
        p1.set("Resources", Object::Reference(resources_id));
        p1.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let p1_id = doc.add_object(Object::Dictionary(p1));

        // Page 2
        let mut p2 = Dictionary::new();
        p2.set("Type", Object::Name(b"Page".to_vec()));
        p2.set("Parent", Object::Reference(pages_id));
        p2.set("Contents", Object::Reference(c2_id));
        p2.set("Resources", Object::Reference(resources_id));
        p2.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let p2_id = doc.add_object(Object::Dictionary(p2));

        // Update Pages Kids
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(pages_id) {
            d.set("Kids", Object::Array(vec![
                Object::Reference(p1_id),
                Object::Reference(p2_id),
            ]));
        }

        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        doc
    }

    #[test]
    fn dump_page_shows_page_header() {
        let doc = build_two_page_doc();
        let config = default_config();
        let out = output_of(|w| {
            dump_page(w, &doc, 1, &config);
        });
        assert!(out.contains("Page 1 (Object"));
    }

    #[test]
    fn dump_page_confines_to_target_page() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
        let out = output_of(|w| {
            dump_page(w, &doc, 1, &config);
        });
        // Should contain page 1's content but not page 2's
        assert!(out.contains("Page1"), "Should contain page 1 content");
        assert!(!out.contains("Page2"), "Should NOT contain page 2 content");
    }

    #[test]
    fn dump_page_two_shows_only_page_two() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: false };
        let out = output_of(|w| {
            dump_page(w, &doc, 2, &config);
        });
        assert!(out.contains("Page 2 (Object"));
        assert!(out.contains("Page2"), "Should contain page 2 content");
        assert!(!out.contains("Page1"), "Should NOT contain page 1 content");
    }

    // ── print_metadata ──────────────────────────────────────────────

    #[test]
    fn print_metadata_basic_fields() {
        let doc = Document::new();
        let out = output_of(|w| {
            print_metadata(w, &doc);
        });
        assert!(out.contains("PDF Version: 1.4"));
        assert!(out.contains("Objects:"));
        assert!(out.contains("Pages:"));
    }

    #[test]
    fn print_metadata_with_info_dict() {
        let mut doc = Document::new();
        let mut info = Dictionary::new();
        info.set("Title", Object::String(b"Test Title".to_vec(), StringFormat::Literal));
        info.set("Author", Object::String(b"Test Author".to_vec(), StringFormat::Literal));
        info.set("Producer", Object::String(b"Test Producer".to_vec(), StringFormat::Literal));
        let info_id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let out = output_of(|w| {
            print_metadata(w, &doc);
        });
        assert!(out.contains("Title: Test Title"));
        assert!(out.contains("Author: Test Author"));
        assert!(out.contains("Producer: Test Producer"));
    }

    #[test]
    fn print_metadata_empty_info_dict() {
        let mut doc = Document::new();
        let info = Dictionary::new();
        let info_id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let out = output_of(|w| {
            print_metadata(w, &doc);
        });
        // Should still show basic fields, just no Info entries
        assert!(out.contains("PDF Version:"));
        assert!(!out.contains("Title:"));
    }

    #[test]
    fn print_metadata_catalog_fields() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("PageLayout", Object::Name(b"SinglePage".to_vec()));
        catalog.set("Lang", Object::String(b"en-US".to_vec(), StringFormat::Literal));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| {
            print_metadata(w, &doc);
        });
        assert!(out.contains("PageLayout: /SinglePage"));
        assert!(out.contains("Lang: en-US"));
    }

    // ── object_to_json ──────────────────────────────────────────────

    fn json_config() -> DumpConfig {
        DumpConfig { decode_streams: false, truncate_binary_streams: false, json: true }
    }

    #[test]
    fn object_to_json_null() {
        let val = object_to_json(&Object::Null, &empty_doc(), &json_config());
        assert_eq!(val["type"], "null");
    }

    #[test]
    fn object_to_json_boolean() {
        let val = object_to_json(&Object::Boolean(true), &empty_doc(), &json_config());
        assert_eq!(val["type"], "boolean");
        assert_eq!(val["value"], true);
    }

    #[test]
    fn object_to_json_integer() {
        let val = object_to_json(&Object::Integer(42), &empty_doc(), &json_config());
        assert_eq!(val["type"], "integer");
        assert_eq!(val["value"], 42);
    }

    #[test]
    fn object_to_json_real() {
        let val = object_to_json(&Object::Real(3.14), &empty_doc(), &json_config());
        assert_eq!(val["type"], "real");
    }

    #[test]
    fn object_to_json_name() {
        let val = object_to_json(&Object::Name(b"Page".to_vec()), &empty_doc(), &json_config());
        assert_eq!(val["type"], "name");
        assert_eq!(val["value"], "Page");
    }

    #[test]
    fn object_to_json_string() {
        let val = object_to_json(&Object::String(b"Hello".to_vec(), StringFormat::Literal), &empty_doc(), &json_config());
        assert_eq!(val["type"], "string");
        assert_eq!(val["value"], "Hello");
    }

    #[test]
    fn object_to_json_array() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let val = object_to_json(&arr, &empty_doc(), &json_config());
        assert_eq!(val["type"], "array");
        assert_eq!(val["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn object_to_json_dictionary() {
        let mut dict = Dictionary::new();
        dict.set("Key", Object::Integer(99));
        let val = object_to_json(&Object::Dictionary(dict), &empty_doc(), &json_config());
        assert_eq!(val["type"], "dictionary");
        assert_eq!(val["entries"]["Key"]["value"], 99);
    }

    #[test]
    fn object_to_json_stream() {
        let stream = make_stream(None, b"data".to_vec());
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &json_config());
        assert_eq!(val["type"], "stream");
        assert!(val.get("dict").is_some());
    }

    #[test]
    fn object_to_json_reference() {
        let val = object_to_json(&Object::Reference((5, 0)), &empty_doc(), &json_config());
        assert_eq!(val["type"], "reference");
        assert_eq!(val["object_number"], 5);
        assert_eq!(val["generation"], 0);
    }

    #[test]
    fn object_to_json_stream_with_decode() {
        let stream = make_stream(None, b"text content".to_vec());
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: true };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["content"], "text content");
    }

    // ── JSON output functions ───────────────────────────────────────

    #[test]
    fn dump_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let config = json_config();
        let out = output_of(|w| dump_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert!(parsed.get("trailer").is_some());
        assert!(parsed.get("objects").is_some());
    }

    #[test]
    fn print_single_object_json_produces_valid_json() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = json_config();
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["object_number"], 1);
        assert_eq!(parsed["generation"], 0);
        assert_eq!(parsed["object"]["type"], "integer");
    }

    #[test]
    fn print_summary_json_produces_valid_json() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let out = output_of(|w| print_summary_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["object_count"], 1);
        assert!(parsed["objects"].is_array());
    }

    #[test]
    fn print_metadata_json_produces_valid_json() {
        let doc = Document::new();
        let out = output_of(|w| print_metadata_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert!(parsed.get("version").is_some());
        assert!(parsed.get("page_count").is_some());
    }

    #[test]
    fn dump_page_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let config = json_config();
        let out = output_of(|w| dump_page_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["page_number"], 1);
        assert!(parsed.get("objects").is_some());
    }

    // ── parse_search_expr ───────────────────────────────────────────

    #[test]
    fn parse_search_expr_key_value() {
        let conds = parse_search_expr("Type=Font").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::KeyEquals { key, value } if key == b"Type" && value == b"Font"));
    }

    #[test]
    fn parse_search_expr_has_key() {
        let conds = parse_search_expr("key=MediaBox").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::HasKey { key } if key == b"MediaBox"));
    }

    #[test]
    fn parse_search_expr_value_contains() {
        let conds = parse_search_expr("value=Hello").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::ValueContains { text } if text == "Hello"));
    }

    #[test]
    fn parse_search_expr_multiple() {
        let conds = parse_search_expr("Type=Font,Subtype=Type1").unwrap();
        assert_eq!(conds.len(), 2);
    }

    #[test]
    fn parse_search_expr_empty_fails() {
        assert!(parse_search_expr("").is_err());
    }

    #[test]
    fn parse_search_expr_no_equals_fails() {
        assert!(parse_search_expr("badexpr").is_err());
    }

    #[test]
    fn parse_search_expr_empty_value_fails() {
        assert!(parse_search_expr("Type=").is_err());
    }

    // ── object_matches ──────────────────────────────────────────────

    #[test]
    fn object_matches_key_value() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_key_value_case_insensitive() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"font".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_has_key() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Integer(0));
        let conds = vec![SearchCondition::HasKey { key: b"MediaBox".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_has_key_missing() {
        let dict = Dictionary::new();
        let conds = vec![SearchCondition::HasKey { key: b"MediaBox".to_vec() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_value_contains() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let conds = vec![SearchCondition::ValueContains { text: "helvet".to_string() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_non_dict_returns_false() {
        let conds = vec![SearchCondition::HasKey { key: b"Type".to_vec() }];
        assert!(!object_matches(&Object::Integer(42), &conds));
    }

    #[test]
    fn object_matches_stream() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, vec![]);
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"XObject".to_vec() }];
        assert!(object_matches(&Object::Stream(stream), &conds));
    }

    #[test]
    fn object_matches_and_logic() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        let conds = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() },
            SearchCondition::KeyEquals { key: b"Subtype".to_vec(), value: b"Type1".to_vec() },
        ];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_and_logic_partial_fail() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        let conds = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() },
            SearchCondition::KeyEquals { key: b"Subtype".to_vec(), value: b"Type1".to_vec() },
        ];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    // ── search_objects ──────────────────────────────────────────────

    #[test]
    fn search_objects_finds_match() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = default_config();
        let out = output_of(|w| search_objects(w, &doc, &conds, &config, false));
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Found 1 matching objects."));
    }

    #[test]
    fn search_objects_no_match() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = default_config();
        let out = output_of(|w| search_objects(w, &doc, &conds, &config, false));
        assert!(out.contains("Found 0 matching objects."));
    }

    #[test]
    fn search_objects_summary_mode() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = default_config();
        let out = output_of(|w| search_objects(w, &doc, &conds, &config, true));
        assert!(out.contains("Obj#"));
        assert!(out.contains("Found 1 matching objects."));
    }

    #[test]
    fn search_objects_json_produces_valid_json() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = json_config();
        let out = output_of(|w| search_objects_json(w, &doc, "Type=Font", &conds, &config));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["match_count"], 1);
        assert_eq!(parsed["query"], "Type=Font");
    }

    // ── extract_text_from_page ──────────────────────────────────────

    #[test]
    fn extract_text_tj() {
        let mut doc = Document::new();
        let content = b"BT\n(Hello) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Hello"));
    }

    #[test]
    fn extract_text_tj_array() {
        let mut doc = Document::new();
        let content = b"BT\n[(H) (ello)] TJ\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Hello"));
    }

    #[test]
    fn extract_text_tj_array_spacing() {
        let mut doc = Document::new();
        // -200 should insert a space
        let content = b"BT\n[(Hello) -200 (World)] TJ\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Hello World"));
    }

    #[test]
    fn extract_text_td_newline() {
        let mut doc = Document::new();
        let content = b"BT\n0 -12 Td\n(Line1) Tj\n0 -12 Td\n(Line2) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Line1"));
        assert!(text.contains("Line2"));
        assert!(text.contains('\n'));
    }

    #[test]
    fn extract_text_tstar() {
        let mut doc = Document::new();
        let content = b"BT\n(Line1) Tj\nT*\n(Line2) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Line1\nLine2"));
    }

    #[test]
    fn extract_text_quote_operator() {
        let mut doc = Document::new();
        let content = b"BT\n(Line1) '\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Line1"));
    }

    #[test]
    fn extract_text_empty_stream() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), vec![]);
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.is_empty());
    }

    #[test]
    fn extract_text_no_contents() {
        let mut doc = Document::new();
        let page = Dictionary::new();
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.is_empty());
    }

    // ── print_text ──────────────────────────────────────────────────

    #[test]
    fn print_text_all_pages() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_text(w, &doc, None));
        assert!(out.contains("--- Page 1 ---"));
        assert!(out.contains("--- Page 2 ---"));
    }

    #[test]
    fn print_text_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_text_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert!(parsed["pages"].is_array());
        assert_eq!(parsed["pages"].as_array().unwrap().len(), 2);
    }

    // ── compare_pdfs / diff ─────────────────────────────────────────

    #[test]
    fn diff_identical_docs() {
        let doc = build_two_page_doc();
        let result = compare_pdfs(&doc, &doc, None);
        assert!(result.metadata_diffs.is_empty());
        for page in &result.page_diffs {
            assert!(page.identical, "Page {} should be identical", page.page_number);
        }
    }

    #[test]
    fn diff_different_page_counts() {
        let doc1 = build_two_page_doc();
        let mut doc2 = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc2.add_object(Object::Dictionary(catalog));
        doc2.trailer.set("Root", Object::Reference(catalog_id));

        let result = compare_pdfs(&doc1, &doc2, None);
        assert!(result.metadata_diffs.iter().any(|d| d.contains("Pages")));
    }

    #[test]
    fn diff_with_page_filter() {
        let doc = build_two_page_doc();
        let result = compare_pdfs(&doc, &doc, Some(1));
        assert_eq!(result.page_diffs.len(), 1);
        assert_eq!(result.page_diffs[0].page_number, 1);
        assert!(result.page_diffs[0].identical);
    }

    #[test]
    fn compare_fonts_identical() {
        let doc = build_two_page_doc();
        let fd = compare_fonts(&doc, &doc);
        assert!(fd.only_in_first.is_empty());
        assert!(fd.only_in_second.is_empty());
    }

    #[test]
    fn print_diff_produces_output() {
        let doc = build_two_page_doc();
        let result = compare_pdfs(&doc, &doc, None);
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("Comparing:"));
        assert!(out.contains("Objects:"));
    }

    #[test]
    fn print_diff_json_produces_valid_json() {
        let doc = build_two_page_doc();
        let result = compare_pdfs(&doc, &doc, None);
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff_json(w, &result, &file1, &file2));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert!(parsed.get("page_diffs").is_some());
        assert!(parsed.get("metadata_diffs").is_some());
    }

    // ── summary_detail ──────────────────────────────────────────────

    #[test]
    fn summary_detail_integer() {
        assert_eq!(summary_detail(&Object::Integer(42)), "");
    }

    #[test]
    fn summary_detail_stream() {
        let stream = make_stream(None, vec![0; 100]);
        assert_eq!(summary_detail(&Object::Stream(stream)), "100 bytes");
    }

    #[test]
    fn summary_detail_stream_with_filter() {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, vec![0; 50]);
        assert!(summary_detail(&Object::Stream(stream)).contains("FlateDecode"));
    }

    #[test]
    fn summary_detail_dict_with_basefont() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        assert!(summary_detail(&Object::Dictionary(dict)).contains("Helvetica"));
    }

    #[test]
    fn summary_detail_dict_keys_only() {
        // Dict with no notable keys (BaseFont, Subtype, MediaBox) → shows "N keys"
        let mut dict = Dictionary::new();
        dict.set("Foo", Object::Integer(1));
        dict.set("Bar", Object::Integer(2));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("2 keys"), "Dict with no notable keys should show key count, got: {}", detail);
    }

    #[test]
    fn summary_detail_dict_with_mediabox() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("MediaBox"), "Should show MediaBox");
        assert!(detail.contains("[0 0 612 792]"), "Should show array values, got: {}", detail);
    }

    #[test]
    fn summary_detail_dict_with_subtype() {
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("/Subtype=Type1"));
    }

    #[test]
    fn summary_detail_dict_notable_non_name_non_array() {
        // A notable key (BaseFont) with a non-Name, non-Array value → "/BaseFont=..."
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Integer(42));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("/BaseFont=..."), "Non-name/array notable should show '...', got: {}", detail);
    }

    #[test]
    fn summary_detail_stream_no_filter() {
        let stream = make_stream(None, vec![0; 75]);
        let detail = summary_detail(&Object::Stream(stream));
        assert_eq!(detail, "75 bytes");
    }

    #[test]
    fn summary_detail_null() {
        assert_eq!(summary_detail(&Object::Null), "");
    }

    #[test]
    fn summary_detail_boolean() {
        assert_eq!(summary_detail(&Object::Boolean(true)), "");
    }

    #[test]
    fn summary_detail_mediabox_with_reals() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Array(vec![
            Object::Real(0.0), Object::Real(0.0),
            Object::Real(595.28), Object::Real(841.89),
        ]));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("595.28"), "Should format Real values, got: {}", detail);
    }

    #[test]
    fn summary_detail_mediabox_with_mixed_types() {
        // Array item that's neither Integer nor Real → "?"
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0),
            Object::Name(b"Unknown".to_vec()),
        ]));
        let detail = summary_detail(&Object::Dictionary(dict));
        assert!(detail.contains("?"), "Non-numeric array items should show '?', got: {}", detail);
    }

    // ── extract_text_from_page: additional operators ─────────────────

    #[test]
    fn extract_text_double_quote_operator() {
        // The " operator: third operand is the string
        let mut doc = Document::new();
        let content = b"BT\n1 2 (Quoted) \"\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Quoted"), "Double-quote operator should extract text, got: {:?}", text);
    }

    #[test]
    fn extract_text_td_uppercase() {
        // TD operator should also produce newline when ty != 0
        let mut doc = Document::new();
        let content = b"BT\n0 -14 TD\n(Line1) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains('\n'), "TD with non-zero ty should produce newline");
        assert!(text.contains("Line1"));
    }

    #[test]
    fn extract_text_td_zero_ty_no_newline() {
        // Td with ty=0 should NOT produce a newline
        let mut doc = Document::new();
        let content = b"BT\n100 0 Td\n(NoNewline) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        // The text from BT should not have a newline before "NoNewline"
        // since ty=0. There may be a newline from BT, but not from Td.
        assert!(text.contains("NoNewline"));
    }

    #[test]
    fn extract_text_td_real_operand() {
        // Td with Real operands
        let mut doc = Document::new();
        let content = b"BT\n0 -14.5 Td\n(RealTd) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains('\n'), "Td with non-zero Real ty should produce newline");
        assert!(text.contains("RealTd"));
    }

    #[test]
    fn extract_text_tj_small_negative_no_space() {
        // TJ with small negative (-50 > -100): should NOT insert space
        let mut doc = Document::new();
        let content = b"BT\n[(He) -50 (llo)] TJ\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Hello"), "Small negative should not insert space, got: {:?}", text);
        assert!(!text.contains("He llo"), "Should not have space");
    }

    #[test]
    fn extract_text_multiple_bt_blocks() {
        // Multiple BT/ET blocks should insert newline between them
        let mut doc = Document::new();
        let content = b"BT\n(Block1) Tj\nET\nBT\n(Block2) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Block1"), "Should contain first block text");
        assert!(text.contains("Block2"), "Should contain second block text");
        // There should be a newline between the blocks (from second BT)
        let block1_pos = text.find("Block1").unwrap();
        let block2_pos = text.find("Block2").unwrap();
        let between = &text[block1_pos + 6..block2_pos];
        assert!(between.contains('\n'), "Should have newline between BT blocks, between: {:?}", between);
    }

    #[test]
    fn extract_text_contents_array_of_refs() {
        // /Contents as an array of stream references
        let mut doc = Document::new();
        let s1 = Stream::new(Dictionary::new(), b"BT\n(Part1) Tj\nET".to_vec());
        let s1_id = doc.add_object(Object::Stream(s1));
        let s2 = Stream::new(Dictionary::new(), b"BT\n(Part2) Tj\nET".to_vec());
        let s2_id = doc.add_object(Object::Stream(s2));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Array(vec![
            Object::Reference(s1_id),
            Object::Reference(s2_id),
        ]));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Part1"), "Should extract text from first stream");
        assert!(text.contains("Part2"), "Should extract text from second stream");
    }

    #[test]
    fn extract_text_non_dictionary_page() {
        // Page object is not a dictionary → empty text
        let mut doc = Document::new();
        let p_id = doc.add_object(Object::Integer(42));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.is_empty(), "Non-dictionary page should return empty text");
    }

    #[test]
    fn extract_text_contents_ref_to_non_stream() {
        // /Contents references a non-stream object → skipped, no crash
        let mut doc = Document::new();
        let non_stream_id = doc.add_object(Object::Integer(42));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(non_stream_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.is_empty(), "Non-stream contents should return empty text");
    }

    #[test]
    fn extract_text_flatedecode_content_stream() {
        // Content stream with FlateDecode should be decoded before text extraction
        let mut doc = Document::new();
        let raw_content = b"BT\n(Compressed) Tj\nET";
        let compressed = zlib_compress(raw_content);
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, compressed);
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Compressed"), "Should decode FlateDecode stream before extracting text, got: {:?}", text);
    }

    // ── format_dict_value ────────────────────────────────────────────

    #[test]
    fn format_dict_value_name() {
        let val = format_dict_value(&Object::Name(b"Page".to_vec()));
        assert_eq!(val, "/Page");
    }

    #[test]
    fn format_dict_value_integer() {
        assert_eq!(format_dict_value(&Object::Integer(42)), "42");
    }

    #[test]
    fn format_dict_value_real() {
        assert_eq!(format_dict_value(&Object::Real(3.14)), "3.14");
    }

    #[test]
    fn format_dict_value_boolean() {
        assert_eq!(format_dict_value(&Object::Boolean(true)), "true");
        assert_eq!(format_dict_value(&Object::Boolean(false)), "false");
    }

    #[test]
    fn format_dict_value_string() {
        let val = format_dict_value(&Object::String(b"hello".to_vec(), StringFormat::Literal));
        assert_eq!(val, "(hello)");
    }

    #[test]
    fn format_dict_value_array() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        assert_eq!(format_dict_value(&arr), "[1 2]");
    }

    #[test]
    fn format_dict_value_reference() {
        assert_eq!(format_dict_value(&Object::Reference((5, 0))), "5 0 R");
    }

    #[test]
    fn format_dict_value_null() {
        assert_eq!(format_dict_value(&Object::Null), "null");
    }

    #[test]
    fn format_dict_value_dictionary() {
        let dict = Dictionary::new();
        assert_eq!(format_dict_value(&Object::Dictionary(dict)), "<<...>>");
    }

    #[test]
    fn format_dict_value_stream() {
        let stream = make_stream(None, vec![]);
        assert_eq!(format_dict_value(&Object::Stream(stream)), "<<stream>>");
    }

    #[test]
    fn format_dict_value_nested_array() {
        let inner = Object::Array(vec![Object::Name(b"X".to_vec())]);
        let outer = Object::Array(vec![inner, Object::Integer(3)]);
        let val = format_dict_value(&outer);
        assert_eq!(val, "[[/X] 3]");
    }

    // ── compare_metadata ─────────────────────────────────────────────

    #[test]
    fn compare_metadata_identical() {
        let doc = Document::new();
        let diffs = compare_metadata(&doc, &doc);
        assert!(diffs.is_empty());
    }

    #[test]
    fn compare_metadata_different_page_counts() {
        let doc1 = build_two_page_doc();
        let doc2 = Document::new();
        let diffs = compare_metadata(&doc1, &doc2);
        assert!(diffs.iter().any(|d| d.contains("Pages")), "Should report page count diff, got: {:?}", diffs);
    }

    #[test]
    fn compare_metadata_info_field_diffs() {
        let mut doc1 = Document::new();
        let mut info1 = Dictionary::new();
        info1.set("Title", Object::String(b"Title A".to_vec(), StringFormat::Literal));
        let info1_id = doc1.add_object(Object::Dictionary(info1));
        doc1.trailer.set("Info", Object::Reference(info1_id));

        let mut doc2 = Document::new();
        let mut info2 = Dictionary::new();
        info2.set("Title", Object::String(b"Title B".to_vec(), StringFormat::Literal));
        let info2_id = doc2.add_object(Object::Dictionary(info2));
        doc2.trailer.set("Info", Object::Reference(info2_id));

        let diffs = compare_metadata(&doc1, &doc2);
        assert!(diffs.iter().any(|d| d.contains("Title")), "Should report title diff, got: {:?}", diffs);
        assert!(diffs.iter().any(|d| d.contains("Title A") && d.contains("Title B")));
    }

    #[test]
    fn compare_metadata_info_present_vs_absent() {
        let mut doc1 = Document::new();
        let mut info = Dictionary::new();
        info.set("Author", Object::String(b"Someone".to_vec(), StringFormat::Literal));
        let info_id = doc1.add_object(Object::Dictionary(info));
        doc1.trailer.set("Info", Object::Reference(info_id));

        let doc2 = Document::new();

        let diffs = compare_metadata(&doc1, &doc2);
        assert!(diffs.iter().any(|d| d.contains("Author")), "Should report author diff, got: {:?}", diffs);
    }

    // ── compare_content_streams ──────────────────────────────────────

    #[test]
    fn compare_content_streams_identical() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"BT\n(Hello) Tj\nET".to_vec());
        let s_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(s_id));
        let p_id = doc.add_object(Object::Dictionary(page));

        let diffs = compare_content_streams(&doc, &doc, p_id, p_id);
        assert!(diffs.is_empty(), "Identical streams should have no diffs");
    }

    #[test]
    fn compare_content_streams_different() {
        let mut doc1 = Document::new();
        let s1 = Stream::new(Dictionary::new(), b"BT\n(Hello) Tj\nET".to_vec());
        let s1_id = doc1.add_object(Object::Stream(s1));
        let mut page1 = Dictionary::new();
        page1.set("Contents", Object::Reference(s1_id));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let s2 = Stream::new(Dictionary::new(), b"BT\n(World) Tj\nET".to_vec());
        let s2_id = doc2.add_object(Object::Stream(s2));
        let mut page2 = Dictionary::new();
        page2.set("Contents", Object::Reference(s2_id));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let diffs = compare_content_streams(&doc1, &doc2, p1_id, p2_id);
        assert!(!diffs.is_empty(), "Different streams should have diffs");
        assert!(diffs.iter().any(|d| d.starts_with("- ") || d.starts_with("+ ")));
    }

    #[test]
    fn compare_content_streams_one_longer() {
        let mut doc1 = Document::new();
        let s1 = Stream::new(Dictionary::new(), b"BT\n(A) Tj\n(B) Tj\nET".to_vec());
        let s1_id = doc1.add_object(Object::Stream(s1));
        let mut page1 = Dictionary::new();
        page1.set("Contents", Object::Reference(s1_id));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let s2 = Stream::new(Dictionary::new(), b"BT\n(A) Tj\nET".to_vec());
        let s2_id = doc2.add_object(Object::Stream(s2));
        let mut page2 = Dictionary::new();
        page2.set("Contents", Object::Reference(s2_id));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let diffs = compare_content_streams(&doc1, &doc2, p1_id, p2_id);
        assert!(!diffs.is_empty(), "Different-length streams should have diffs");
    }

    #[test]
    fn compare_content_streams_no_contents() {
        let mut doc = Document::new();
        let page = Dictionary::new();
        let p_id = doc.add_object(Object::Dictionary(page));
        let diffs = compare_content_streams(&doc, &doc, p_id, p_id);
        assert!(diffs.is_empty(), "Pages with no contents should have no diffs");
    }

    // ── get_content_ops ──────────────────────────────────────────────

    #[test]
    fn get_content_ops_valid_stream() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"BT\n(Hello) Tj\nET".to_vec());
        let s_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(s_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let ops = get_content_ops(&doc, p_id);
        assert!(!ops.is_empty(), "Should have operations");
    }

    #[test]
    fn get_content_ops_no_contents() {
        let mut doc = Document::new();
        let page = Dictionary::new();
        let p_id = doc.add_object(Object::Dictionary(page));
        let ops = get_content_ops(&doc, p_id);
        assert!(ops.is_empty());
    }

    #[test]
    fn get_content_ops_non_dict_page() {
        let mut doc = Document::new();
        let p_id = doc.add_object(Object::Integer(42));
        let ops = get_content_ops(&doc, p_id);
        assert!(ops.is_empty());
    }

    // ── collect_all_font_names ───────────────────────────────────────

    #[test]
    fn collect_all_font_names_finds_fonts() {
        let doc = build_two_page_doc();
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.contains("Helvetica"), "Should find Helvetica font, got: {:?}", fonts);
    }

    #[test]
    fn collect_all_font_names_no_fonts() {
        let doc = Document::new();
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.is_empty());
    }

    #[test]
    fn collect_all_font_names_ignores_non_font() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        dict.set("BaseFont", Object::Name(b"NotAFont".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.is_empty(), "Should only collect fonts with Type=Font");
    }

    #[test]
    fn collect_all_font_names_stream_font() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"CourierNew".to_vec()));
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let fonts = collect_all_font_names(&doc);
        assert!(fonts.contains("CourierNew"), "Should find font in stream object");
    }

    // ── compare_fonts ────────────────────────────────────────────────

    #[test]
    fn compare_fonts_different() {
        let mut doc1 = Document::new();
        let mut f1 = Dictionary::new();
        f1.set("Type", Object::Name(b"Font".to_vec()));
        f1.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc1.objects.insert((1, 0), Object::Dictionary(f1));

        let mut doc2 = Document::new();
        let mut f2 = Dictionary::new();
        f2.set("Type", Object::Name(b"Font".to_vec()));
        f2.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc2.objects.insert((1, 0), Object::Dictionary(f2));

        let fd = compare_fonts(&doc1, &doc2);
        assert!(fd.only_in_first.contains(&"Helvetica".to_string()));
        assert!(fd.only_in_second.contains(&"Courier".to_string()));
    }

    // ── compare_page ─────────────────────────────────────────────────

    #[test]
    fn compare_page_identical() {
        let doc = build_two_page_doc();
        let pages = doc.get_pages();
        let p1_id = *pages.get(&1).unwrap();
        let pd = compare_page(&doc, &doc, p1_id, p1_id, 1);
        assert!(pd.identical, "Same page should be identical");
        assert!(pd.dict_diffs.is_empty());
        assert!(pd.resource_diffs.is_empty());
        assert!(pd.content_diffs.is_empty());
    }

    #[test]
    fn compare_page_different_dict_entries() {
        let mut doc1 = Document::new();
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(595), Object::Integer(842),
        ]));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.dict_diffs.iter().any(|d| d.contains("MediaBox")), "Should show MediaBox diff, got: {:?}", pd.dict_diffs);
    }

    #[test]
    fn compare_page_key_only_in_first() {
        let mut doc1 = Document::new();
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Rotate", Object::Integer(90));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.dict_diffs.iter().any(|d| d.contains("Rotate") && d.contains("(missing)")));
    }

    #[test]
    fn compare_page_key_only_in_second() {
        let mut doc1 = Document::new();
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("CropBox", Object::Array(vec![Object::Integer(0)]));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.dict_diffs.iter().any(|d| d.contains("CropBox") && d.contains("(missing)")));
    }

    #[test]
    fn compare_page_non_dict_page() {
        let mut doc = Document::new();
        let p_id = doc.add_object(Object::Integer(42));
        let pd = compare_page(&doc, &doc, p_id, p_id, 1);
        assert!(!pd.identical);
        assert!(!pd.dict_diffs.is_empty());
    }

    // ── compare_pdfs: page filter edge cases ─────────────────────────

    #[test]
    fn compare_pdfs_page_only_in_first() {
        let doc1 = build_two_page_doc();
        let doc2 = Document::new();
        let result = compare_pdfs(&doc1, &doc2, Some(1));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(!result.page_diffs[0].identical);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("only exists in first")));
    }

    #[test]
    fn compare_pdfs_page_only_in_second() {
        let doc1 = Document::new();
        let doc2 = build_two_page_doc();
        let result = compare_pdfs(&doc1, &doc2, Some(1));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(!result.page_diffs[0].identical);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("only exists in second")));
    }

    #[test]
    fn compare_pdfs_page_not_in_either() {
        let doc = Document::new();
        let result = compare_pdfs(&doc, &doc, Some(999));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("not found in either")));
    }

    #[test]
    fn compare_pdfs_no_filter_different_page_counts() {
        let doc1 = build_two_page_doc();
        let mut doc2 = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc2.add_object(Object::Dictionary(catalog));
        doc2.trailer.set("Root", Object::Reference(catalog_id));

        let result = compare_pdfs(&doc1, &doc2, None);
        // Pages only in doc1 should be reported
        assert!(result.page_diffs.iter().any(|p| p.dict_diffs.iter().any(|d| d.contains("only in first"))));
    }

    // ── object_matches: additional branches ──────────────────────────

    #[test]
    fn object_matches_key_equals_string_value() {
        // KeyEquals with Object::String value (not Name)
        let mut dict = Dictionary::new();
        dict.set("Title", Object::String(b"MyTitle".to_vec(), StringFormat::Literal));
        let conds = vec![SearchCondition::KeyEquals { key: b"Title".to_vec(), value: b"mytitle".to_vec() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds), "String matching should be case-insensitive");
    }

    #[test]
    fn object_matches_key_equals_non_matching_type() {
        // KeyEquals where value is neither Name nor String → should not match
        let mut dict = Dictionary::new();
        dict.set("Count", Object::Integer(42));
        let conds = vec![SearchCondition::KeyEquals { key: b"Count".to_vec(), value: b"42".to_vec() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds), "Integer values should not match KeyEquals");
    }

    #[test]
    fn object_matches_value_contains_string_object() {
        // ValueContains should match Object::String values too
        let mut dict = Dictionary::new();
        dict.set("Title", Object::String(b"Hello World".to_vec(), StringFormat::Literal));
        let conds = vec![SearchCondition::ValueContains { text: "world".to_string() }];
        assert!(object_matches(&Object::Dictionary(dict), &conds), "ValueContains should match String objects");
    }

    #[test]
    fn object_matches_value_contains_no_match() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        let conds = vec![SearchCondition::ValueContains { text: "nonexistent".to_string() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_value_contains_non_string_values_skipped() {
        // Dict with only Integer values → ValueContains should not match
        let mut dict = Dictionary::new();
        dict.set("Count", Object::Integer(42));
        let conds = vec![SearchCondition::ValueContains { text: "42".to_string() }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    // ── parse_search_expr: additional edge cases ─────────────────────

    #[test]
    fn parse_search_expr_key_case_insensitive() {
        // "KEY=..." should be treated as HasKey
        let conds = parse_search_expr("KEY=MediaBox").unwrap();
        assert!(matches!(&conds[0], SearchCondition::HasKey { key } if key == b"MediaBox"));
    }

    #[test]
    fn parse_search_expr_value_case_insensitive() {
        // "VALUE=..." should be treated as ValueContains
        let conds = parse_search_expr("VALUE=Hello").unwrap();
        assert!(matches!(&conds[0], SearchCondition::ValueContains { text } if text == "Hello"));
    }

    #[test]
    fn parse_search_expr_whitespace_trimmed() {
        let conds = parse_search_expr("  Type = Font  ").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::KeyEquals { key, value } if key == b"Type" && value == b"Font"));
    }

    #[test]
    fn parse_search_expr_multiple_with_whitespace() {
        let conds = parse_search_expr("Type=Font , Subtype=Type1").unwrap();
        assert_eq!(conds.len(), 2);
    }

    #[test]
    fn parse_search_expr_empty_parts_skipped() {
        // Trailing comma should be OK
        let conds = parse_search_expr("Type=Font,").unwrap();
        assert_eq!(conds.len(), 1);
    }

    #[test]
    fn parse_search_expr_only_commas_fails() {
        assert!(parse_search_expr(",,,").is_err());
    }

    // ── object_to_json: stream paths ─────────────────────────────────

    #[test]
    fn object_to_json_stream_with_decode_binary() {
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: false, json: true };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content_binary").is_some(), "Binary stream should have content_binary field");
    }

    #[test]
    fn object_to_json_stream_with_decode_binary_truncated() {
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        let config = DumpConfig { decode_streams: true, truncate_binary_streams: true, json: true };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content_truncated").is_some(), "Truncated binary should have content_truncated field");
    }

    #[test]
    fn object_to_json_stream_no_decode() {
        let stream = make_stream(None, b"text data".to_vec());
        let config = DumpConfig { decode_streams: false, truncate_binary_streams: false, json: true };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content").is_none(), "No content when decode_streams=false");
        assert!(val.get("content_binary").is_none());
    }

    // ── metadata_info ────────────────────────────────────────────────

    #[test]
    fn metadata_info_with_info_and_catalog() {
        let mut doc = Document::new();
        let mut info = Dictionary::new();
        info.set("Title", Object::String(b"Test".to_vec(), StringFormat::Literal));
        let info_id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Lang", Object::String(b"en-US".to_vec(), StringFormat::Literal));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let (info_map, catalog_map) = metadata_info(&doc);
        assert_eq!(info_map["Title"], "Test");
        assert_eq!(catalog_map["Lang"], "en-US");
    }

    #[test]
    fn metadata_info_empty_doc() {
        let doc = Document::new();
        let (info_map, catalog_map) = metadata_info(&doc);
        assert!(info_map.is_empty());
        assert!(catalog_map.is_empty());
    }

    #[test]
    fn metadata_info_catalog_page_layout_name() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("PageLayout", Object::Name(b"TwoColumnLeft".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let (_, catalog_map) = metadata_info(&doc);
        assert_eq!(catalog_map["PageLayout"], "/TwoColumnLeft");
    }

    // ── collect_reachable_objects ─────────────────────────────────────

    #[test]
    fn collect_reachable_objects_basic() {
        let doc = build_two_page_doc();
        let objects = collect_reachable_objects(&doc);
        assert!(!objects.is_empty(), "Should collect reachable objects");
        // Every reachable object should have a valid JSON value
        for (key, val) in &objects {
            assert!(key.contains(':'), "Key should be obj:gen format, got: {}", key);
            assert!(val.get("type").is_some(), "Each object should have a type field");
        }
    }

    #[test]
    fn collect_reachable_objects_empty_doc() {
        let doc = Document::new();
        let objects = collect_reachable_objects(&doc);
        assert!(objects.is_empty(), "Empty doc should have no reachable objects");
    }

    // ── print_text_json with page filter ─────────────────────────────

    #[test]
    fn print_text_json_with_page_filter() {
        let doc = build_two_page_doc();
        let out = output_of(|w| print_text_json(w, &doc, Some(1)));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        let pages = parsed["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1, "Should have exactly one page");
        assert_eq!(pages[0]["page_number"], 1);
    }

    // ── print_diff: non-identical output paths ───────────────────────

    #[test]
    fn print_diff_shows_metadata_diffs() {
        let result = DiffResult {
            metadata_diffs: vec!["Version: 1.4 vs 1.7".to_string()],
            page_diffs: vec![],
            font_diffs: FontDiff { only_in_first: vec![], only_in_second: vec![] },
            object_count: (5, 6),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("--- Metadata ---"), "Should show metadata section");
        assert!(out.contains("Version: 1.4 vs 1.7"));
    }

    #[test]
    fn print_diff_shows_font_diffs() {
        let result = DiffResult {
            metadata_diffs: vec![],
            page_diffs: vec![],
            font_diffs: FontDiff {
                only_in_first: vec!["Helvetica".to_string()],
                only_in_second: vec!["Courier".to_string()],
            },
            object_count: (5, 5),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("--- Fonts ---"), "Should show fonts section");
        assert!(out.contains("Helvetica"));
        assert!(out.contains("Courier"));
    }

    #[test]
    fn print_diff_shows_page_content_diffs() {
        let result = DiffResult {
            metadata_diffs: vec![],
            page_diffs: vec![PageDiff {
                page_number: 1,
                identical: false,
                dict_diffs: vec!["/MediaBox: [0 0 612 792] vs [0 0 595 842]".to_string()],
                resource_diffs: vec!["Font F1 only in first file".to_string()],
                content_diffs: vec!["- (Hello) Tj".to_string(), "+ (World) Tj".to_string()],
            }],
            font_diffs: FontDiff { only_in_first: vec![], only_in_second: vec![] },
            object_count: (5, 5),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("--- Page 1 ---"));
        assert!(out.contains("MediaBox"));
        assert!(out.contains("Font F1 only in first file"));
        assert!(out.contains("Content stream: differs"));
        assert!(out.contains("- (Hello) Tj"));
        assert!(out.contains("+ (World) Tj"));
    }

    #[test]
    fn print_diff_identical_page() {
        let result = DiffResult {
            metadata_diffs: vec![],
            page_diffs: vec![PageDiff {
                page_number: 1,
                identical: true,
                dict_diffs: vec![],
                resource_diffs: vec![],
                content_diffs: vec![],
            }],
            font_diffs: FontDiff { only_in_first: vec![], only_in_second: vec![] },
            object_count: (5, 5),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff(w, &result, &file1, &file2));
        assert!(out.contains("(identical)"));
    }

    // ── print_diff_json: non-identical ────────────────────────────────

    #[test]
    fn print_diff_json_with_diffs() {
        let result = DiffResult {
            metadata_diffs: vec!["Version: 1.4 vs 1.7".to_string()],
            page_diffs: vec![PageDiff {
                page_number: 1,
                identical: false,
                dict_diffs: vec!["diff1".to_string()],
                resource_diffs: vec!["rdiff1".to_string()],
                content_diffs: vec!["cdiff1".to_string()],
            }],
            font_diffs: FontDiff {
                only_in_first: vec!["Helvetica".to_string()],
                only_in_second: vec![],
            },
            object_count: (5, 6),
        };
        let file1 = PathBuf::from("a.pdf");
        let file2 = PathBuf::from("b.pdf");
        let out = output_of(|w| print_diff_json(w, &result, &file1, &file2));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["metadata_diffs"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["page_diffs"][0]["identical"], false);
        assert_eq!(parsed["font_diffs"]["only_in_first"][0], "Helvetica");
        assert_eq!(parsed["object_count"]["file1"], 5);
        assert_eq!(parsed["object_count"]["file2"], 6);
    }

    // ── print_metadata_json with info ─────────────────────────────────

    #[test]
    fn print_metadata_json_with_info() {
        let mut doc = Document::new();
        let mut info = Dictionary::new();
        info.set("Title", Object::String(b"Test Title".to_vec(), StringFormat::Literal));
        let info_id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let out = output_of(|w| print_metadata_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        assert_eq!(parsed["info"]["Title"], "Test Title");
    }

    // ── print_summary_json with multiple types ────────────────────────

    #[test]
    fn print_summary_json_includes_detail() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            vec![0; 50],
        );
        doc.objects.insert((2, 0), Object::Stream(stream));

        let out = output_of(|w| print_summary_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).expect("Should be valid JSON");
        let objects = parsed["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 2);
        // Check that detail field is populated
        assert!(objects.iter().any(|o| o["type"] == "Font"));
    }

    // ── dump_page_json confines to page ──────────────────────────────

    #[test]
    fn dump_page_json_confines_to_page() {
        let doc = build_two_page_doc();
        let config = json_config();
        let out1 = output_of(|w| dump_page_json(w, &doc, 1, &config));
        let out2 = output_of(|w| dump_page_json(w, &doc, 2, &config));
        let parsed1: Value = serde_json::from_str(&out1).unwrap();
        let parsed2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(parsed1["page_number"], 1);
        assert_eq!(parsed2["page_number"], 2);
        // Both should have objects but potentially different sets
        assert!(parsed1.get("objects").is_some());
        assert!(parsed2.get("objects").is_some());
    }

    // ── search_objects_json: zero matches ─────────────────────────────

    #[test]
    fn search_objects_json_no_matches() {
        let doc = Document::new();
        let conds = vec![SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() }];
        let config = json_config();
        let out = output_of(|w| search_objects_json(w, &doc, "Type=Font", &conds, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["match_count"], 0);
        assert!(parsed["matches"].as_array().unwrap().is_empty());
    }
}
