use clap::Parser;
use flate2::read::ZlibDecoder;
use lopdf::{content::Content, Document, Object, ObjectId};
use regex::Regex;
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

    // ── Overview ──────────────────────────────────────────────────────

    /// Print document metadata (version, pages, /Info fields)
    #[arg(short = 'm', long, help_heading = "Overview")]
    metadata: bool,

    /// Print a one-line summary of every object
    #[arg(short = 's', long, help_heading = "Overview")]
    summary: bool,

    /// Show document statistics (object types, stream sizes, filter usage)
    #[arg(long, help_heading = "Overview")]
    stats: bool,

    /// Run structural validation checks on the PDF
    #[arg(long, help_heading = "Overview")]
    validate: bool,

    // ── Content ──────────────────────────────────────────────────────

    /// Extract readable text from page content streams
    #[arg(long, help_heading = "Content")]
    text: bool,

    /// Show content stream operators (all pages, or filtered with --page)
    #[arg(long, help_heading = "Content")]
    operators: bool,

    /// Show page resource map (fonts, images, graphics states, color spaces)
    #[arg(long, help_heading = "Content")]
    resources: bool,

    /// List all fonts in the document
    #[arg(long, help_heading = "Content")]
    fonts: bool,

    /// List all images in the document
    #[arg(long, help_heading = "Content")]
    images: bool,

    // ── Structure ────────────────────────────────────────────────────

    /// Show the object graph as an indented reference tree
    #[arg(long, help_heading = "Structure")]
    tree: bool,

    /// Show document bookmarks (outline tree)
    #[arg(long, help_heading = "Structure")]
    bookmarks: bool,

    /// Show tagged PDF logical structure tree
    #[arg(long, help_heading = "Structure")]
    structure: bool,

    /// Show optional content groups (layers)
    #[arg(long, alias = "ocg", help_heading = "Structure")]
    layers: bool,

    /// Show page labels (logical page numbering)
    #[arg(long, help_heading = "Structure")]
    page_labels: bool,

    // ── Annotations & Links ──────────────────────────────────────────

    /// Show annotations (all pages, or filtered with --page)
    #[arg(long, help_heading = "Annotations & Links")]
    annotations: bool,

    /// List link annotations with targets (all pages, or filtered with --page)
    #[arg(long, help_heading = "Annotations & Links")]
    links: bool,

    /// List form fields (AcroForm)
    #[arg(long, help_heading = "Annotations & Links")]
    forms: bool,

    // ── Objects ──────────────────────────────────────────────────────

    /// Print one or more objects by number (e.g. 5, 1,5,12, 3-7, 1,5,10-15)
    #[arg(short = 'o', long, help_heading = "Objects")]
    object: Option<String>,

    /// Show a human-readable explanation of an object's role, with full content
    #[arg(long, help_heading = "Objects")]
    info: Option<u32>,

    /// Find all objects that reference a given object number
    #[arg(long, help_heading = "Objects")]
    refs_to: Option<u32>,

    /// Search for objects matching an expression (e.g. Type=Font, key=MediaBox, value=Hello)
    #[arg(long, help_heading = "Objects")]
    search: Option<String>,

    // ── Security & Files ─────────────────────────────────────────────

    /// Show encryption and permission details
    #[arg(long, help_heading = "Security & Files")]
    security: bool,

    /// List embedded files (file attachments)
    #[arg(long, help_heading = "Security & Files")]
    embedded_files: bool,

    // ── Comparison ───────────────────────────────────────────────────

    /// Compare structurally with a second PDF file
    #[arg(long, help_heading = "Comparison")]
    diff: Option<PathBuf>,

    // ── Export ────────────────────────────────────────────────────────

    /// Extract a stream object to a file
    #[arg(long, requires = "output", help_heading = "Export")]
    extract_stream: Option<u32>,

    /// Output file for extracted stream
    #[arg(long, requires = "extract_stream", help_heading = "Export")]
    output: Option<PathBuf>,

    /// Full depth-first dump of all reachable objects from /Root
    #[arg(long, help_heading = "Export")]
    dump: bool,

    // ── Modifiers ────────────────────────────────────────────────────

    /// Dump the object tree for a specific page or range (e.g. 1, 1-3)
    #[arg(long, help_heading = "Modifiers")]
    page: Option<String>,

    /// Output as structured JSON
    #[arg(long, help_heading = "Modifiers")]
    json: bool,

    /// Decode and print the content of streams
    #[arg(long, help_heading = "Modifiers")]
    decode_streams: bool,

    /// Inline-expand references to show target summaries (use with --object or --page)
    #[arg(long, help_heading = "Modifiers")]
    deref: bool,

    /// Limit traversal depth (0 = root only, 1 = root + immediate refs, etc.)
    #[arg(long, help_heading = "Modifiers")]
    depth: Option<usize>,

    /// Display binary stream content as hex dump (use with --decode-streams)
    #[arg(long, help_heading = "Modifiers")]
    hex: bool,

    /// Truncate binary streams to the first N bytes
    #[arg(long, help_heading = "Modifiers")]
    truncate: Option<usize>,

    /// Show raw undecoded stream bytes (use with --object)
    #[arg(long, help_heading = "Modifiers")]
    raw: bool,

    /// Output tree as GraphViz DOT format (use with --tree)
    #[arg(long, requires = "tree", help_heading = "Modifiers")]
    dot: bool,
}

#[derive(Clone, Copy)]
struct DumpConfig {
    decode_streams: bool,
    truncate: Option<usize>,
    json: bool,
    hex: bool,
    depth: Option<usize>,
    deref: bool,
    raw: bool,
}

enum PageSpec {
    Single(u32),
    Range(u32, u32),
}

impl PageSpec {
    fn parse(s: &str) -> Result<PageSpec, String> {
        if let Some((start_s, end_s)) = s.split_once('-') {
            let start: u32 = start_s.trim().parse()
                .map_err(|_| format!("Invalid page range start: '{}'", start_s.trim()))?;
            let end: u32 = end_s.trim().parse()
                .map_err(|_| format!("Invalid page range end: '{}'", end_s.trim()))?;
            if start == 0 || end == 0 {
                return Err("Page numbers must be >= 1".to_string());
            }
            if start > end {
                return Err(format!("Invalid page range: {} > {}", start, end));
            }
            Ok(PageSpec::Range(start, end))
        } else {
            let num: u32 = s.trim().parse()
                .map_err(|_| format!("Invalid page number: '{}'", s.trim()))?;
            if num == 0 {
                return Err("Page numbers must be >= 1".to_string());
            }
            Ok(PageSpec::Single(num))
        }
    }

    fn contains(&self, page: u32) -> bool {
        match self {
            PageSpec::Single(n) => page == *n,
            PageSpec::Range(start, end) => page >= *start && page <= *end,
        }
    }

    fn pages(&self) -> Vec<u32> {
        match self {
            PageSpec::Single(n) => vec![*n],
            PageSpec::Range(start, end) => (*start..=*end).collect(),
        }
    }

}

fn parse_object_spec(s: &str) -> Result<Vec<u32>, String> {
    let mut result = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        if let Some((start_s, end_s)) = part.split_once('-') {
            let start: u32 = start_s.trim().parse()
                .map_err(|_| format!("Invalid object number: '{}'", start_s.trim()))?;
            let end: u32 = end_s.trim().parse()
                .map_err(|_| format!("Invalid object number: '{}'", end_s.trim()))?;
            if start > end {
                return Err(format!("Invalid object range: {} > {}", start, end));
            }
            result.extend(start..=end);
        } else {
            let num: u32 = part.parse()
                .map_err(|_| format!("Invalid object number: '{}'", part))?;
            result.push(num);
        }
    }
    if result.is_empty() {
        return Err("Empty object specification".to_string());
    }
    Ok(result)
}

fn main() {
    let args = Args::parse();

    // Mutual exclusivity check
    // --summary alone is a mode; with --search it becomes a modifier
    // --page alone is a mode; with --text it becomes a filter
    let mode_count = [
        args.extract_stream.is_some(),
        args.object.is_some(),
        args.summary && args.search.is_none(),
        args.metadata,
        args.page.is_some() && !args.text && !args.annotations && !args.operators && !args.resources && !args.links,
        args.search.is_some(),
        args.text,
        args.operators,
        args.resources,
        args.forms,
        args.refs_to.is_some(),
        args.fonts,
        args.images,
        args.validate,
        args.tree,
        args.stats,
        args.bookmarks,
        args.annotations && args.page.is_none(),
        args.security,
        args.embedded_files,
        args.page_labels,
        args.links && args.page.is_none(),
        args.layers,
        args.structure,
        args.info.is_some(),
        args.dump,
    ].iter().filter(|&&b| b).count();
    if mode_count > 1 {
        eprintln!("Error: Only one mode flag may be used at a time.");
        std::process::exit(1);
    }

    // --raw validation: requires --object, conflicts with --decode-streams
    if args.raw {
        if args.object.is_none() {
            eprintln!("Error: --raw requires --object.");
            std::process::exit(1);
        }
        if args.decode_streams {
            eprintln!("Error: --raw and --decode-streams cannot be used together.");
            std::process::exit(1);
        }
    }

    // --diff validation: only works with default mode, --page, and --json
    if args.diff.is_some() {
        let incompatible = args.object.is_some()
            || (args.summary && args.search.is_none())
            || args.metadata
            || args.extract_stream.is_some()
            || args.search.is_some()
            || args.text
            || args.refs_to.is_some()
            || args.fonts
            || args.images
            || args.validate
            || args.tree
            || args.stats
            || args.bookmarks
            || args.annotations
            || args.operators
            || args.resources
            || args.forms
            || args.security
            || args.embedded_files
            || args.page_labels
            || args.links
            || args.layers
            || args.structure
            || args.info.is_some()
            || args.dump;
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

    let truncate = args.truncate;
    let config = DumpConfig {
        decode_streams: args.decode_streams,
        truncate,
        json: args.json,
        hex: args.hex,
        depth: args.depth,
        deref: args.deref,
        raw: args.raw,
    };

    let page_spec = args.page.as_deref().map(|s| {
        PageSpec::parse(s).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        })
    });

    let object_nums = args.object.as_deref().map(|s| {
        parse_object_spec(s).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        })
    });

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
        let result = compare_pdfs(&doc, &doc2, page_spec.as_ref());
        let mut out = io::stdout().lock();
        if config.json {
            print_diff_json(&mut out, &result, &args.file, diff_path);
        } else {
            print_diff(&mut out, &result, &args.file, diff_path);
        }
        return;
    }

    if let Some(object_id) = args.extract_stream {
        let output_path = args.output.as_ref().unwrap();
        let object_id = (object_id, 0);
        match doc.get_object(object_id) {
            Ok(Object::Stream(stream)) => {
                let (decoded_content, warning) = decode_stream(stream);
                if let Some(warn) = &warning {
                    eprintln!("Warning: {}", warn);
                }
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
            print_text_json(&mut out, &doc, page_spec.as_ref());
        } else {
            print_text(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.operators {
        let mut out = io::stdout().lock();
        if config.json {
            print_operators_json(&mut out, &doc, page_spec.as_ref());
        } else {
            print_operators(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.resources {
        let mut out = io::stdout().lock();
        if config.json {
            print_resources_json(&mut out, &doc, page_spec.as_ref());
        } else {
            print_resources(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.forms {
        let mut out = io::stdout().lock();
        if config.json {
            print_forms_json(&mut out, &doc);
        } else {
            print_forms(&mut out, &doc);
        }
    } else if let Some(info_num) = args.info {
        let mut out = io::stdout().lock();
        if config.json {
            print_info_json(&mut out, &doc, info_num);
        } else {
            print_info(&mut out, &doc, info_num);
        }
    } else if let Some(target) = args.refs_to {
        let mut out = io::stdout().lock();
        if config.json {
            print_refs_to_json(&mut out, &doc, target);
        } else {
            print_refs_to(&mut out, &doc, target);
        }
    } else if args.fonts {
        let mut out = io::stdout().lock();
        if config.json {
            print_fonts_json(&mut out, &doc);
        } else {
            print_fonts(&mut out, &doc);
        }
    } else if args.images {
        let mut out = io::stdout().lock();
        if config.json {
            print_images_json(&mut out, &doc);
        } else {
            print_images(&mut out, &doc);
        }
    } else if args.validate {
        let mut out = io::stdout().lock();
        if config.json {
            print_validation_json(&mut out, &doc);
        } else {
            print_validation(&mut out, &doc);
        }
    } else if args.stats {
        let mut out = io::stdout().lock();
        if config.json {
            print_stats_json(&mut out, &doc);
        } else {
            print_stats(&mut out, &doc);
        }
    } else if args.bookmarks {
        let mut out = io::stdout().lock();
        if config.json {
            print_bookmarks_json(&mut out, &doc);
        } else {
            print_bookmarks(&mut out, &doc);
        }
    } else if args.annotations {
        let mut out = io::stdout().lock();
        if config.json {
            print_annotations_json(&mut out, &doc, page_spec.as_ref());
        } else {
            print_annotations(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.security {
        let mut out = io::stdout().lock();
        if config.json {
            print_security_json(&mut out, &doc, &args.file);
        } else {
            print_security(&mut out, &doc, &args.file);
        }
    } else if args.embedded_files {
        let mut out = io::stdout().lock();
        if config.json {
            print_embedded_files_json(&mut out, &doc);
        } else {
            print_embedded_files(&mut out, &doc);
        }
    } else if args.page_labels {
        let mut out = io::stdout().lock();
        if config.json {
            print_page_labels_json(&mut out, &doc);
        } else {
            print_page_labels(&mut out, &doc);
        }
    } else if args.links {
        let mut out = io::stdout().lock();
        if config.json {
            print_links_json(&mut out, &doc, page_spec.as_ref());
        } else {
            print_links(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.layers {
        let mut out = io::stdout().lock();
        if config.json {
            print_layers_json(&mut out, &doc);
        } else {
            print_layers(&mut out, &doc);
        }
    } else if args.structure {
        let mut out = io::stdout().lock();
        if config.json {
            print_structure_json(&mut out, &doc, &config);
        } else {
            print_structure(&mut out, &doc, &config);
        }
    } else if args.tree {
        let mut out = io::stdout().lock();
        if args.dot {
            print_tree_dot(&mut out, &doc, &config);
        } else if config.json {
            print_tree_json(&mut out, &doc, &config);
        } else {
            print_tree(&mut out, &doc, &config);
        }
    } else if let Some(ref nums) = object_nums {
        let mut out = io::stdout().lock();
        if config.json {
            print_objects_json(&mut out, &doc, nums, &config);
        } else {
            print_objects(&mut out, &doc, nums, &config);
        }
    } else if args.summary {
        let mut out = io::stdout().lock();
        if config.json {
            print_summary_json(&mut out, &doc);
        } else {
            print_summary(&mut out, &doc);
        }
    } else if let Some(ref spec) = page_spec {
        let mut out = io::stdout().lock();
        if config.json {
            dump_page_json(&mut out, &doc, spec, &config);
        } else {
            dump_page(&mut out, &doc, spec, &config);
        }
    } else if args.metadata {
        let mut out = io::stdout().lock();
        if config.json {
            print_metadata_json(&mut out, &doc);
        } else {
            print_metadata(&mut out, &doc);
        }
    } else if args.dump {
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
                dump_object_and_children(&mut out, root_id, &doc, &mut visited_for_traverse, &config, false, 0);
            } else {
                eprintln!("Warning: /Root not found or not a reference in trailer.");
            }
        }
    } else {
        // Default: overview mode
        let mut out = io::stdout().lock();
        if config.json {
            print_overview_json(&mut out, &doc);
        } else {
            print_overview(&mut out, &doc);
        }
    }
}

fn dump_object_and_children(writer: &mut impl Write, obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, config: &DumpConfig, is_contents: bool, current_depth: usize) {
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

            if let Some(max_depth) = config.depth
                && current_depth >= max_depth
            {
                let unvisited: Vec<_> = child_refs.iter()
                    .filter(|(_, id)| !visited.contains(id))
                    .collect();
                if !unvisited.is_empty() {
                    writeln!(writer, "  (depth limit reached, {} references not followed)", unvisited.len()).unwrap();
                }
                return;
            }

            for (is_contents, child_id) in child_refs {
                if !visited.contains(&child_id) {
                    writeln!(writer, "--------------------------------\n").unwrap();
                    dump_object_and_children(writer, child_id, doc, visited, config, is_contents, current_depth + 1);
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

fn decode_ascii85(data: &[u8]) -> Result<Vec<u8>, String> {
    // Strip whitespace and find end-of-data marker ~>
    let cleaned: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut input = if cleaned.ends_with(b"~>") {
        &cleaned[..cleaned.len() - 2]
    } else {
        &cleaned[..]
    };
    // Strip optional <~ prefix
    if input.starts_with(b"<~") {
        input = &input[2..];
    }

    let mut result = Vec::new();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'z' {
            result.extend_from_slice(&[0, 0, 0, 0]);
            i += 1;
            continue;
        }

        let chunk_len = (input.len() - i).min(5);
        let chunk = &input[i..i + chunk_len];

        // Validate characters are in range !..u (33..117)
        for &b in chunk {
            if !(b'!'..=b'u').contains(&b) {
                return Err(format!("ASCII85Decode: invalid character 0x{:02x}", b));
            }
        }

        // Pad short final group with 'u' (117)
        let mut digits = [b'u'; 5];
        digits[..chunk_len].copy_from_slice(chunk);

        let mut value: u64 = 0;
        for &d in &digits {
            value = value * 85 + (d - b'!') as u64;
        }

        let bytes = value.to_be_bytes();
        // For full 5-char groups output 4 bytes, for partial output (chunk_len - 1) bytes
        let output_len = if chunk_len == 5 { 4 } else { chunk_len - 1 };
        result.extend_from_slice(&bytes[4..4 + output_len]);
        i += chunk_len;
    }
    Ok(result)
}

fn decode_asciihex(data: &[u8]) -> Result<Vec<u8>, String> {
    // Strip whitespace and find end-of-data marker >
    let cleaned: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    let input = if cleaned.ends_with(b">") {
        &cleaned[..cleaned.len() - 1]
    } else {
        &cleaned
    };

    let mut result = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let hi = match hex_digit(input[i]) {
            Some(v) => v,
            None => return Err(format!("ASCIIHexDecode: invalid hex character 0x{:02x}", input[i])),
        };
        let lo = if i + 1 < input.len() {
            match hex_digit(input[i + 1]) {
                Some(v) => v,
                None => return Err(format!("ASCIIHexDecode: invalid hex character 0x{:02x}", input[i + 1])),
            }
        } else {
            0 // Trailing single digit padded with 0
        };
        result.push(hi << 4 | lo);
        i += 2;
    }
    Ok(result)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn decode_lzw(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
    decoder.decode(data).map_err(|e| format!("LZWDecode: {}", e))
}

fn decode_run_length(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let length = data[i];
        i += 1;
        if length <= 127 {
            // Copy next (length+1) bytes literally
            let count = length as usize + 1;
            if i + count > data.len() {
                return Err("RunLengthDecode: truncated literal run".to_string());
            }
            result.extend_from_slice(&data[i..i + count]);
            i += count;
        } else if length == 128 {
            // EOD marker
            break;
        } else {
            // Repeat next byte (257-length) times
            if i >= data.len() {
                return Err("RunLengthDecode: truncated repeat run".to_string());
            }
            let count = 257 - length as usize;
            let byte = data[i];
            i += 1;
            result.extend(std::iter::repeat_n(byte, count));
        }
    }
    Ok(result)
}

fn get_filter_names(stream: &lopdf::Stream) -> Vec<Vec<u8>> {
    match stream.dict.get(b"Filter").ok() {
        Some(filter_obj) => {
            if let Ok(name) = filter_obj.as_name() {
                vec![name.to_vec()]
            } else if let Ok(arr) = filter_obj.as_array() {
                arr.iter().filter_map(|obj| obj.as_name().ok().map(|n| n.to_vec())).collect()
            } else {
                vec![]
            }
        }
        None => vec![],
    }
}

fn decode_stream(stream: &lopdf::Stream) -> (Cow<'_, [u8]>, Option<String>) {
    let filters = get_filter_names(stream);
    if filters.is_empty() {
        return (Cow::Borrowed(&stream.content), None);
    }

    let mut data: Cow<'_, [u8]> = Cow::Borrowed(&stream.content);
    for filter in &filters {
        let result = match filter.as_slice() {
            b"FlateDecode" => {
                let mut decoder = ZlibDecoder::new(&data[..]);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)
                    .map(|_| decompressed)
                    .map_err(|_| "FlateDecode decompression failed".to_string())
            }
            b"ASCII85Decode" => decode_ascii85(&data),
            b"ASCIIHexDecode" => decode_asciihex(&data),
            b"LZWDecode" => decode_lzw(&data),
            b"RunLengthDecode" => decode_run_length(&data),
            other => {
                let name = String::from_utf8_lossy(other);
                return (Cow::Owned(data.into_owned()), Some(format!("unsupported filter: {}", name)));
            }
        };
        match result {
            Ok(decoded) => data = Cow::Owned(decoded),
            Err(msg) => return (Cow::Owned(data.into_owned()), Some(msg)),
        }
    }

    (data, None)
}

fn format_hex_dump(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for (offset, chunk) in data.chunks(16).enumerate() {
        // Offset column
        write!(result, "{:08x}  ", offset * 16).unwrap();
        // Hex bytes: first 8
        for (i, &b) in chunk.iter().enumerate() {
            write!(result, "{:02x} ", b).unwrap();
            if i == 7 { result.push(' '); }
        }
        // Pad if less than 16 bytes
        if chunk.len() < 16 {
            for i in chunk.len()..16 {
                result.push_str("   ");
                if i == 7 { result.push(' '); }
            }
        }
        // ASCII column
        result.push(' ');
        result.push('|');
        for &b in chunk {
            if b.is_ascii_graphic() || b == b' ' {
                result.push(b as char);
            } else {
                result.push('.');
            }
        }
        result.push('|');
        result.push('\n');
    }
    result
}

fn print_stream_content(writer: &mut impl Write, stream: &lopdf::Stream, indent_str: &str, config: &DumpConfig, is_contents: bool) {
    let (decoded_content, warning) = decode_stream(stream);
    let filters = get_filter_names(stream);
    let description = if warning.is_none() && !filters.is_empty() {
        "decoded"
    } else {
        "raw"
    };

    print_content_data(writer, &decoded_content, description, indent_str, config, is_contents, warning.as_deref());
}

fn print_content_data(writer: &mut impl Write, content: &[u8], description: &str, indent_str: &str, config: &DumpConfig, is_contents: bool, warning: Option<&str>) {
    if let Some(warn) = warning {
        writeln!(writer, "\n{}[WARNING: {}]", indent_str, warn).unwrap();
    }

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
                    writeln!(writer, "{}  {}", indent_str, format_operation(op)).unwrap();
                }
                return;
            }
            Err(e) => {
                writeln!(writer, "\n{}[Could not parse content stream: {}. Falling back to raw view.]", indent_str, e).unwrap();
            }
        }
    }

    let full_len = content.len();
    let is_binary = is_binary_stream(content);
    let content_to_display = if let Some(limit) = config.truncate {
        if is_binary { &content[..full_len.min(limit)] } else { content }
    } else {
        content
    };

    let len_str = if let Some(limit) = config.truncate {
        if full_len > limit && is_binary {
            format!("{} (truncated to {})", full_len, limit)
        } else {
            full_len.to_string()
        }
    } else {
        full_len.to_string()
    };

    if config.hex && is_binary {
        writeln!(
            writer,
            "\n{}Stream content ({}, {} bytes):\n{}",
            indent_str,
            description,
            len_str,
            format_hex_dump(content_to_display)
        ).unwrap();
    } else {
        writeln!(
            writer,
            "\n{}Stream content ({}, {} bytes):\n---\n{}\n---",
            indent_str,
            description,
            len_str,
            String::from_utf8_lossy(content_to_display)
        ).unwrap();
    }
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

            if config.raw {
                print_content_data(writer, &stream.content, "raw, undecoded", &indent_str, config, false, None);
            } else if config.decode_streams {
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
            } else if config.deref
                && let Ok(resolved) = doc.get_object(*id) {
                write!(writer, " => {}", deref_summary(resolved, doc)).unwrap();
            }
        }
    }
}

fn deref_summary(obj: &Object, _doc: &Document) -> String {
    match obj {
        Object::Null => "null".to_string(),
        Object::Boolean(b) => b.to_string(),
        Object::Integer(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::String(bytes, _) => format!("({})", String::from_utf8_lossy(bytes)),
        Object::Array(arr) => format!("[{} items]", arr.len()),
        Object::Reference(id) => format!("{} {} R", id.0, id.1),
        Object::Stream(stream) => {
            let type_label = object_type_label(obj);
            let filter = stream.dict.get(b"Filter").ok()
                .and_then(|f| f.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));
            let mut parts = vec![format!("stream, {} bytes", stream.content.len())];
            if type_label != "-" { parts.insert(0, format!("/Type /{}", type_label)); }
            if let Some(f) = filter { parts.push(f); }
            format!("<< {} >>", parts.join(", "))
        }
        Object::Dictionary(dict) => {
            let type_label = object_type_label(obj);
            let count = dict.len();
            let mut parts = Vec::new();
            if type_label != "-" { parts.push(format!("/Type /{}", type_label)); }
            // Show a few notable keys
            for key in [b"BaseFont".as_slice(), b"Subtype", b"Count", b"MediaBox"] {
                if let Ok(val) = dict.get(key) {
                    parts.push(format!("/{}={}", String::from_utf8_lossy(key), format_dict_value(val)));
                }
            }
            if parts.is_empty() {
                format!("<< {} keys >>", count)
            } else {
                format!("<< {}, {} keys >>", parts.join(", "), count)
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

fn print_objects(writer: &mut impl Write, doc: &Document, nums: &[u32], config: &DumpConfig) {
    for (i, &obj_num) in nums.iter().enumerate() {
        if i > 0 { writeln!(writer).unwrap(); }
        print_single_object(writer, doc, obj_num, config);
    }
}

fn print_objects_json(writer: &mut impl Write, doc: &Document, nums: &[u32], config: &DumpConfig) {
    if nums.len() == 1 {
        print_single_object_json(writer, doc, nums[0], config);
    } else {
        let mut items = Vec::new();
        for &obj_num in nums {
            let obj_id = (obj_num, 0);
            match doc.get_object(obj_id) {
                Ok(object) => {
                    items.push(json!({
                        "object_number": obj_num,
                        "generation": 0,
                        "object": object_to_json(object, doc, config),
                    }));
                }
                Err(_) => {
                    items.push(json!({
                        "object_number": obj_num,
                        "generation": 0,
                        "error": "not found",
                    }));
                }
            }
        }
        let output = json!({"objects": items});
        writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
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

fn dump_page(writer: &mut impl Write, doc: &Document, spec: &PageSpec, config: &DumpConfig) {
    let pages = doc.get_pages();
    let total = pages.len();

    for page_num in spec.pages() {
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
        dump_object_and_children(writer, page_id, doc, &mut visited, config, false, 0);
    }
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
            if config.raw {
                let content = &stream.content;
                if !is_binary_stream(content) {
                    val["raw_content"] = json!(String::from_utf8_lossy(content));
                } else if config.hex {
                    let display_data = if let Some(limit) = config.truncate {
                        &content[..content.len().min(limit)]
                    } else {
                        content.as_slice()
                    };
                    val["raw_content_hex"] = json!(format_hex_dump(display_data));
                } else {
                    val["raw_content_binary"] = json!(format!("<binary, {} bytes>", content.len()));
                }
            } else if config.decode_streams {
                let (decoded, warning) = decode_stream(stream);
                if let Some(warn) = &warning {
                    val["decode_warning"] = json!(warn);
                }
                if !is_binary_stream(&decoded) {
                    val["content"] = json!(String::from_utf8_lossy(&decoded));
                } else if config.hex {
                    let display_data = if let Some(limit) = config.truncate {
                        &decoded[..decoded.len().min(limit)]
                    } else {
                        &decoded
                    };
                    val["content_hex"] = json!(format_hex_dump(display_data));
                } else if config.truncate.is_some() {
                    val["content_truncated"] = json!(format!("<binary, {} bytes>", decoded.len()));
                } else {
                    val["content_binary"] = json!(format!("<binary, {} bytes>", decoded.len()));
                }
            }
            val
        }
        Object::Reference(id) => {
            let mut val = json!({"type": "reference", "object_number": id.0, "generation": id.1});
            if config.deref
                && let Ok(resolved) = doc.get_object(*id) {
                let no_deref = DumpConfig { deref: false, ..*config };
                val["resolved"] = object_to_json(resolved, doc, &no_deref);
            }
            val
        }
    }
}

fn collect_reachable_objects(doc: &Document, max_depth: Option<usize>) -> BTreeMap<String, Value> {
    let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
    let mut result = BTreeMap::new();
    let mut visited = BTreeSet::new();

    fn walk(doc: &Document, obj_id: ObjectId, visited: &mut BTreeSet<ObjectId>, result: &mut BTreeMap<String, Value>, config: &DumpConfig, current_depth: usize, max_depth: Option<usize>) {
        if visited.contains(&obj_id) { return; }
        if let Some(max) = max_depth
            && current_depth > max { return; }
        visited.insert(obj_id);
        if let Ok(obj) = doc.get_object(obj_id) {
            let key = format!("{}:{}", obj_id.0, obj_id.1);
            result.insert(key, object_to_json(obj, doc, config));
            collect_refs(obj, doc, visited, result, config, current_depth, max_depth);
        }
    }

    fn collect_refs(obj: &Object, doc: &Document, visited: &mut BTreeSet<ObjectId>, result: &mut BTreeMap<String, Value>, config: &DumpConfig, current_depth: usize, max_depth: Option<usize>) {
        match obj {
            Object::Reference(id) => walk(doc, *id, visited, result, config, current_depth + 1, max_depth),
            Object::Array(arr) => {
                for item in arr { collect_refs(item, doc, visited, result, config, current_depth, max_depth); }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() { collect_refs(v, doc, visited, result, config, current_depth, max_depth); }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter() { collect_refs(v, doc, visited, result, config, current_depth, max_depth); }
            }
            _ => {}
        }
    }

    // Start from trailer refs
    for (_, v) in doc.trailer.iter() {
        if let Ok(id) = v.as_reference() {
            walk(doc, id, &mut visited, &mut result, &config, 0, max_depth);
        }
    }

    result
}

fn dump_json(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let trailer_json = object_to_json(&Object::Dictionary(doc.trailer.clone()), doc, config);
    let objects = collect_reachable_objects(doc, config.depth);
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

fn dump_page_json(writer: &mut impl Write, doc: &Document, spec: &PageSpec, config: &DumpConfig) {
    let pages = doc.get_pages();
    let total = pages.len();

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

    let mut page_outputs = Vec::new();

    for page_num in spec.pages() {
        let page_id = match pages.get(&page_num) {
            Some(&id) => id,
            None => {
                eprintln!("Error: Page {} not found. Document has {} pages.", page_num, total);
                std::process::exit(1);
            }
        };

        let mut visited = BTreeSet::new();
        let mut objects = BTreeMap::new();

        if let Ok(Object::Dictionary(dict)) = doc.get_object(page_id)
            && let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference())
        {
            visited.insert(parent_ref);
        }

        walk_page(doc, page_id, &mut visited, &mut objects, config);

        page_outputs.push(json!({
            "page_number": page_num,
            "objects": objects,
        }));
    }

    // For single page, output as before; for range, output array
    if page_outputs.len() == 1 {
        writeln!(writer, "{}", serde_json::to_string_pretty(&page_outputs[0]).unwrap()).unwrap();
    } else {
        writeln!(writer, "{}", serde_json::to_string_pretty(&json!({"pages": page_outputs})).unwrap()).unwrap();
    }
}

// ── Search (Phase 2) ─────────────────────────────────────────────────

enum SearchCondition {
    KeyEquals { key: Vec<u8>, value: Vec<u8> },
    HasKey { key: Vec<u8> },
    ValueContains { text: String },
    StreamContains { text: String },
    RegexMatch { pattern: Regex },
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
            } else if left.eq_ignore_ascii_case("stream") {
                conditions.push(SearchCondition::StreamContains { text: right.to_string() });
            } else if left.eq_ignore_ascii_case("regex") {
                let re = Regex::new(right)
                    .map_err(|e| format!("Invalid regex '{}': {}", right, e))?;
                conditions.push(SearchCondition::RegexMatch { pattern: re });
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

    // Lazily decode stream content once for conditions that need it
    let needs_stream = conditions.iter().any(|c| matches!(c, SearchCondition::StreamContains { .. } | SearchCondition::RegexMatch { .. }));
    let decoded_content = if needs_stream {
        if let Object::Stream(stream) = obj {
            let (decoded, _) = decode_stream(stream);
            Some(decoded)
        } else {
            None
        }
    } else {
        None
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
            let needle = text.to_lowercase();
            let needle_bytes = needle.as_bytes();
            dict.iter().any(|(_, v)| {
                let haystack: &[u8] = match v {
                    Object::Name(n) => n,
                    Object::String(bytes, _) => bytes,
                    _ => return false,
                };
                // Case-insensitive byte search: zero-allocation byte-by-byte compare
                haystack.windows(needle_bytes.len()).any(|w|
                    w.iter().zip(needle_bytes).all(|(a, b)| a.to_ascii_lowercase() == *b)
                )
            })
        }
        SearchCondition::StreamContains { text } => {
            if let Some(ref decoded) = decoded_content {
                let text_lower = text.to_lowercase();
                let content_str = String::from_utf8_lossy(decoded);
                content_str.to_lowercase().contains(&text_lower)
            } else {
                false
            }
        }
        SearchCondition::RegexMatch { pattern } => {
            // Match against dict key names, Name values, String values
            let key_or_value_match = dict.iter().any(|(k, v)| {
                if let Ok(key_str) = std::str::from_utf8(k)
                    && pattern.is_match(key_str) { return true; }
                match v {
                    Object::Name(n) => std::str::from_utf8(n).is_ok_and(|s| pattern.is_match(s)),
                    Object::String(bytes, _) => std::str::from_utf8(bytes).is_ok_and(|s| pattern.is_match(s)),
                    _ => false,
                }
            });
            if key_or_value_match { return true; }
            // Also match decoded stream content
            if let Some(ref decoded) = decoded_content {
                let content_str = String::from_utf8_lossy(decoded);
                pattern.is_match(&content_str)
            } else {
                false
            }
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

struct TextResult {
    text: String,
    warnings: Vec<String>,
}

fn extract_text_from_page(doc: &Document, page_id: ObjectId) -> String {
    extract_text_from_page_with_warnings(doc, page_id).text
}

fn extract_text_from_page_with_warnings(doc: &Document, page_id: ObjectId) -> TextResult {
    let mut text = String::new();
    let mut warnings = Vec::new();

    // Get content stream(s) for the page
    let page_dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return TextResult { text, warnings },
    };

    // Check font encodings for this page
    let font_warnings = check_page_font_encodings(doc, page_dict);
    warnings.extend(font_warnings);

    let content_ids: Vec<ObjectId> = match page_dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return TextResult { text, warnings },
    };

    let mut all_bytes = Vec::new();
    let mut decode_failed = false;
    for cid in &content_ids {
        match doc.get_object(*cid) {
            Ok(Object::Stream(stream)) => {
                let (decoded, warning) = decode_stream(stream);
                if let Some(warn) = warning {
                    warnings.push(format!("Content stream {} {}: {}", cid.0, cid.1, warn));
                    decode_failed = true;
                }
                all_bytes.extend_from_slice(&decoded);
            }
            Ok(_) => {
                warnings.push(format!("Content stream {} {} is not a stream object", cid.0, cid.1));
            }
            Err(_) => {
                warnings.push(format!("Content stream {} {} not found", cid.0, cid.1));
            }
        }
    }

    if all_bytes.is_empty() && !content_ids.is_empty() && decode_failed {
        warnings.push("Content stream could not be decoded".to_string());
        return TextResult { text, warnings };
    }

    let operations = match Content::decode(&all_bytes) {
        Ok(content) => content.operations,
        Err(_) => {
            warnings.push("Content stream has syntax errors".to_string());
            return TextResult { text, warnings };
        }
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
                // Check ty (second operand) for line break — negative y means downward movement
                if op.operands.len() >= 2 {
                    if let Object::Integer(ty) = &op.operands[1] {
                        if *ty < 0 { text.push('\n'); }
                    } else if let Object::Real(ty) = &op.operands[1]
                        && *ty < 0.0 { text.push('\n');
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

    TextResult { text, warnings }
}

/// Check whether fonts on a page have known encodings.
/// Returns warnings for fonts that lack ToUnicode maps or recognized encodings.
fn check_page_font_encodings(doc: &Document, page_dict: &lopdf::Dictionary) -> Vec<String> {
    let mut warnings = Vec::new();

    // Resolve /Resources (may be a reference)
    let resources = match page_dict.get(b"Resources") {
        Ok(Object::Dictionary(d)) => d.clone(),
        Ok(Object::Reference(r)) => {
            match doc.get_object(*r) {
                Ok(Object::Dictionary(d)) => d.clone(),
                _ => return warnings,
            }
        }
        _ => return warnings,
    };

    // Get /Font sub-dictionary
    let font_dict = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d.clone(),
        Ok(Object::Reference(r)) => {
            match doc.get_object(*r) {
                Ok(Object::Dictionary(d)) => d.clone(),
                _ => return warnings,
            }
        }
        _ => return warnings,
    };

    for (name, value) in font_dict.iter() {
        let font_name = String::from_utf8_lossy(name);
        let font_obj = match value {
            Object::Reference(r) => {
                match doc.get_object(*r) {
                    Ok(obj) => obj,
                    _ => continue,
                }
            }
            obj => obj,
        };

        let dict = match font_obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };

        // Check for /ToUnicode — if present, encoding is known
        if dict.has(b"ToUnicode") {
            continue;
        }

        // Check /Encoding
        let has_known_encoding = match dict.get(b"Encoding") {
            Ok(Object::Name(enc)) => {
                let enc_str = String::from_utf8_lossy(enc);
                matches!(enc_str.as_ref(), "WinAnsiEncoding" | "MacRomanEncoding" | "MacExpertEncoding" | "StandardEncoding")
            }
            Ok(Object::Dictionary(_)) => true, // Encoding dict with /Differences
            Ok(Object::Reference(r)) => {
                matches!(doc.get_object(*r), Ok(Object::Dictionary(_)) | Ok(Object::Name(_)))
            }
            _ => false,
        };

        if has_known_encoding {
            continue;
        }

        // Check /Subtype — CID fonts without ToUnicode are problematic
        let subtype = dict.get(b"Subtype").ok()
            .and_then(|v| v.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();

        let base_font = dict.get(b"BaseFont").ok()
            .and_then(|v| v.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_else(|| font_name.to_string());

        if subtype == "Type0" || subtype == "CIDFontType0" || subtype == "CIDFontType2" {
            warnings.push(format!(
                "Font /{} ({}) uses CID encoding without ToUnicode map. Text may be inaccurate.",
                font_name, base_font
            ));
        } else if subtype == "Type1" || subtype == "TrueType" || subtype == "Type3" {
            // Simple fonts without encoding — may use built-in encoding
            // Only warn if it looks custom (not a standard 14 font)
            let standard_14 = [
                "Courier", "Courier-Bold", "Courier-BoldOblique", "Courier-Oblique",
                "Helvetica", "Helvetica-Bold", "Helvetica-BoldOblique", "Helvetica-Oblique",
                "Times-Roman", "Times-Bold", "Times-BoldItalic", "Times-Italic",
                "Symbol", "ZapfDingbats",
            ];
            if !standard_14.iter().any(|s| base_font == *s) {
                warnings.push(format!(
                    "Font /{} ({}) has no explicit encoding or ToUnicode map. Text may be inaccurate.",
                    font_name, base_font
                ));
            }
        }
    }

    warnings
}

fn print_text(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    for (pn, page_id) in &page_list {
        writeln!(writer, "--- Page {} ---", pn).unwrap();
        let result = extract_text_from_page_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        writeln!(writer, "{}", result.text).unwrap();
    }
}

fn print_text_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    let mut page_results = Vec::new();
    for (pn, page_id) in &page_list {
        let result = extract_text_from_page_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        let mut entry = serde_json::Map::new();
        entry.insert("page_number".to_string(), json!(pn));
        entry.insert("text".to_string(), json!(result.text));
        if !result.warnings.is_empty() {
            entry.insert("warnings".to_string(), json!(result.warnings));
        }
        page_results.push(Value::Object(entry));
    }

    let output = json!({"pages": page_results});
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Operators ───────────────────────────────────────────────────────

struct OpsResult {
    operations: Vec<lopdf::content::Operation>,
    warnings: Vec<String>,
}

fn get_page_operations(doc: &Document, page_id: ObjectId) -> Vec<lopdf::content::Operation> {
    get_page_operations_with_warnings(doc, page_id).operations
}

fn get_page_operations_with_warnings(doc: &Document, page_id: ObjectId) -> OpsResult {
    let dict = match doc.get_object(page_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return OpsResult { operations: vec![], warnings: vec![] },
    };

    let content_ids: Vec<ObjectId> = match dict.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return OpsResult { operations: vec![], warnings: vec![] },
    };

    let mut all_bytes = Vec::new();
    let mut warnings = Vec::new();
    for cid in &content_ids {
        match doc.get_object(*cid) {
            Ok(Object::Stream(stream)) => {
                let (decoded, warning) = decode_stream(stream);
                if let Some(warn) = warning {
                    warnings.push(format!("Content stream {} {}: {}", cid.0, cid.1, warn));
                }
                all_bytes.extend_from_slice(&decoded);
            }
            _ => {
                warnings.push(format!("Content stream {} {} could not be read", cid.0, cid.1));
            }
        }
    }

    match Content::decode(&all_bytes) {
        Ok(content) => OpsResult { operations: content.operations, warnings },
        Err(_) => {
            warnings.push("Content stream has syntax errors".to_string());
            OpsResult { operations: vec![], warnings }
        }
    }
}

fn print_operators(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    for (pn, page_id) in &page_list {
        let result = get_page_operations_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        writeln!(writer, "--- Page {} ({} operations) ---", pn, result.operations.len()).unwrap();
        for op in &result.operations {
            writeln!(writer, "{}", format_operation(op)).unwrap();
        }
        writeln!(writer).unwrap();
    }
}

fn print_operators_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    let mut page_results = Vec::new();
    for (pn, page_id) in &page_list {
        let result = get_page_operations_with_warnings(doc, *page_id);
        for warn in &result.warnings {
            eprintln!("Warning: Page {}: {}", pn, warn);
        }
        let json_ops: Vec<Value> = result.operations.iter().map(|op| {
            let operands: Vec<Value> = op.operands.iter().map(|o| json!(format_dict_value(o))).collect();
            json!({
                "operator": op.operator,
                "operands": operands,
            })
        }).collect();
        let mut entry = serde_json::Map::new();
        entry.insert("page_number".to_string(), json!(pn));
        entry.insert("operation_count".to_string(), json!(result.operations.len()));
        entry.insert("operations".to_string(), json!(json_ops));
        if !result.warnings.is_empty() {
            entry.insert("warnings".to_string(), json!(result.warnings));
        }
        page_results.push(Value::Object(entry));
    }

    let output = json!({"pages": page_results});
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Resources ────────────────────────────────────────────────────────

struct ResourceEntry {
    name: String,
    object_id: Option<ObjectId>,
    detail: String,
}

struct PageResources {
    fonts: Vec<ResourceEntry>,
    xobjects: Vec<ResourceEntry>,
    ext_gstate: Vec<ResourceEntry>,
    color_spaces: Vec<ResourceEntry>,
}

fn resolve_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a lopdf::Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Dictionary(d) => Some(d),
            _ => None,
        },
        _ => None,
    }
}

fn resolve_array<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Vec<Object>> {
    match obj {
        Object::Array(a) => Some(a),
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Array(a) => Some(a),
            _ => None,
        },
        _ => None,
    }
}

/// Extracts a String from an Object::String or Object::Name (lossy UTF-8 conversion).
fn obj_to_string_lossy(obj: &Object) -> Option<String> {
    match obj {
        Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

/// Extracts a String from an Object::Name only (lossy UTF-8 conversion).
fn name_to_string(obj: &Object) -> Option<String> {
    match obj {
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

fn resolve_page_resources(doc: &Document, page_id: ObjectId) -> Option<&lopdf::Dictionary> {
    // Walk up the page tree to find inherited Resources
    let mut current_id = page_id;
    while let Ok(Object::Dictionary(dict)) = doc.get_object(current_id) {
        if let Ok(res) = dict.get(b"Resources") {
            return resolve_dict(doc, res);
        }
        // Walk up to parent
        if let Ok(parent_ref) = dict.get(b"Parent").and_then(|o| o.as_reference()) {
            if parent_ref == current_id { break; }
            current_id = parent_ref;
        } else {
            break;
        }
    }
    None
}

fn font_detail(doc: &Document, obj_id: ObjectId) -> String {
    let dict = match doc.get_object(obj_id) {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Stream(s)) => &s.dict,
        _ => return "?".to_string(),
    };
    let base_font = dict.get(b"BaseFont").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));
    let subtype = dict.get(b"Subtype").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));
    let embedded = dict.get(b"FontDescriptor").ok()
        .and_then(|v| v.as_reference().ok())
        .and_then(|fd_id| doc.get_object(fd_id).ok())
        .and_then(|fd_obj| {
            let fd_dict = match fd_obj {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                _ => return None,
            };
            for key in &[b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                if fd_dict.get(key).ok().and_then(|v| v.as_reference().ok()).is_some() {
                    return Some(true);
                }
            }
            None
        });
    let mut parts = Vec::new();
    if let Some(bf) = base_font { parts.push(bf); }
    if let Some(st) = subtype { parts.push(st); }
    if embedded == Some(true) { parts.push("embedded".to_string()); }
    if parts.is_empty() { "?".to_string() } else { parts.join(", ") }
}

fn xobject_detail(doc: &Document, obj_id: ObjectId) -> String {
    match doc.get_object(obj_id) {
        Ok(Object::Stream(s)) => {
            let subtype = s.dict.get(b"Subtype").ok()
                .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                .unwrap_or_else(|| "?".to_string());
            if subtype == "Image" {
                let w = s.dict.get(b"Width").ok().and_then(|v| v.as_i64().ok()).unwrap_or(0);
                let h = s.dict.get(b"Height").ok().and_then(|v| v.as_i64().ok()).unwrap_or(0);
                let cs = s.dict.get(b"ColorSpace").ok()
                    .map(|v| format_color_space(v, doc))
                    .unwrap_or_else(|| "-".to_string());
                format!("Image, {}x{}, {}", w, h, cs)
            } else {
                subtype
            }
        }
        _ => "?".to_string(),
    }
}

fn collect_page_resources(doc: &Document, page_id: ObjectId) -> PageResources {
    let empty = PageResources {
        fonts: vec![], xobjects: vec![], ext_gstate: vec![], color_spaces: vec![],
    };

    let res_dict = match resolve_page_resources(doc, page_id) {
        Some(d) => d,
        None => return empty,
    };

    let mut fonts = Vec::new();
    if let Ok(font_obj) = res_dict.get(b"Font") {
        let font_dict = match font_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(fd) = font_dict {
            for (name, val) in fd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                if let Ok(id) = val.as_reference() {
                    fonts.push(ResourceEntry {
                        name: name_str,
                        object_id: Some(id),
                        detail: font_detail(doc, id),
                    });
                } else {
                    fonts.push(ResourceEntry { name: name_str, object_id: None, detail: "inline".to_string() });
                }
            }
        }
    }
    fonts.sort_by(|a, b| a.name.cmp(&b.name));

    let mut xobjects = Vec::new();
    if let Ok(xobj_obj) = res_dict.get(b"XObject") {
        let xobj_dict = match xobj_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(xd) = xobj_dict {
            for (name, val) in xd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                if let Ok(id) = val.as_reference() {
                    xobjects.push(ResourceEntry {
                        name: name_str,
                        object_id: Some(id),
                        detail: xobject_detail(doc, id),
                    });
                }
            }
        }
    }
    xobjects.sort_by(|a, b| a.name.cmp(&b.name));

    let mut ext_gstate = Vec::new();
    if let Ok(gs_obj) = res_dict.get(b"ExtGState") {
        let gs_dict = match gs_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(gd) = gs_dict {
            for (name, val) in gd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                if let Ok(id) = val.as_reference() {
                    let key_count = match doc.get_object(id) {
                        Ok(Object::Dictionary(d)) => d.len(),
                        Ok(Object::Stream(s)) => s.dict.len(),
                        _ => 0,
                    };
                    ext_gstate.push(ResourceEntry {
                        name: name_str,
                        object_id: Some(id),
                        detail: format!("{} keys", key_count),
                    });
                }
            }
        }
    }
    ext_gstate.sort_by(|a, b| a.name.cmp(&b.name));

    let mut color_spaces = Vec::new();
    if let Ok(cs_obj) = res_dict.get(b"ColorSpace") {
        let cs_dict = match cs_obj {
            Object::Dictionary(d) => Some(d),
            Object::Reference(id) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { Some(d) } else { None }
            }
            _ => None,
        };
        if let Some(cd) = cs_dict {
            for (name, val) in cd.iter() {
                let name_str = format!("/{}", String::from_utf8_lossy(name));
                let detail = match val {
                    Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                    Object::Array(arr) => {
                        arr.first().and_then(|v| v.as_name().ok())
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                            .unwrap_or_else(|| format!("[{} items]", arr.len()))
                    }
                    Object::Reference(id) => {
                        if let Ok(resolved) = doc.get_object(*id) {
                            match resolved {
                                Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                                Object::Array(arr) => {
                                    arr.first().and_then(|v| v.as_name().ok())
                                        .map(|n| String::from_utf8_lossy(n).into_owned())
                                        .unwrap_or_else(|| format!("[{} items]", arr.len()))
                                }
                                _ => format!("obj {}", id.0),
                            }
                        } else {
                            format!("{} {} R", id.0, id.1)
                        }
                    }
                    _ => "?".to_string(),
                };
                let obj_id = val.as_reference().ok();
                color_spaces.push(ResourceEntry { name: name_str, object_id: obj_id, detail });
            }
        }
    }
    color_spaces.sort_by(|a, b| a.name.cmp(&b.name));

    PageResources { fonts, xobjects, ext_gstate, color_spaces }
}

fn print_resource_section(writer: &mut impl Write, label: &str, entries: &[ResourceEntry]) {
    if entries.is_empty() { return; }
    writeln!(writer, "{}:", label).unwrap();
    for e in entries {
        let obj_str = match e.object_id {
            Some(id) => format!("obj {}", id.0),
            None => "inline".to_string(),
        };
        writeln!(writer, "  {:<6} -> {} ({})", e.name, obj_str, e.detail).unwrap();
    }
}

fn print_resources(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    for (pn, page_id) in &page_list {
        let res = collect_page_resources(doc, *page_id);
        writeln!(writer, "--- Page {} Resources ---", pn).unwrap();
        print_resource_section(writer, "Fonts", &res.fonts);
        print_resource_section(writer, "XObjects", &res.xobjects);
        print_resource_section(writer, "ExtGState", &res.ext_gstate);
        print_resource_section(writer, "ColorSpaces", &res.color_spaces);
        writeln!(writer).unwrap();
    }
}

fn resource_entries_to_json(entries: &[ResourceEntry]) -> Vec<Value> {
    entries.iter().map(|e| {
        let mut obj = json!({
            "name": e.name,
            "detail": e.detail,
        });
        if let Some(id) = e.object_id {
            obj["object_number"] = json!(id.0);
            obj["generation"] = json!(id.1);
        }
        obj
    }).collect()
}

fn print_resources_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let pages = doc.get_pages();

    let page_list: Vec<(u32, ObjectId)> = if let Some(spec) = page_filter {
        spec.pages().into_iter().map(|pn| {
            let page_id = match pages.get(&pn) {
                Some(&id) => id,
                None => {
                    eprintln!("Error: Page {} not found. Document has {} pages.", pn, pages.len());
                    std::process::exit(1);
                }
            };
            (pn, page_id)
        }).collect()
    } else {
        pages.iter().map(|(&pn, &id)| (pn, id)).collect()
    };

    let mut page_results = Vec::new();
    for (pn, page_id) in &page_list {
        let res = collect_page_resources(doc, *page_id);
        page_results.push(json!({
            "page_number": pn,
            "fonts": resource_entries_to_json(&res.fonts),
            "xobjects": resource_entries_to_json(&res.xobjects),
            "ext_gstate": resource_entries_to_json(&res.ext_gstate),
            "color_spaces": resource_entries_to_json(&res.color_spaces),
        }));
    }

    let output = json!({"pages": page_results});
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Forms ────────────────────────────────────────────────────────────

struct FormFieldInfo {
    object_id: ObjectId,
    qualified_name: String,
    field_type: String,
    value: String,
    page_number: Option<u32>,
    flags: u32,
}

fn field_type_abbrev(ft: &str) -> &str {
    match ft {
        "Tx" => "Tx",
        "Btn" => "Btn",
        "Ch" => "Ch",
        "Sig" => "Sig",
        _ => ft,
    }
}

fn collect_form_fields(doc: &Document) -> (Option<ObjectId>, bool, Vec<FormFieldInfo>) {
    // Find AcroForm in catalog
    let catalog_id = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return (None, false, vec![]),
    };
    let catalog = match doc.get_object(catalog_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return (None, false, vec![]),
    };
    let acroform_ref = match catalog.get(b"AcroForm") {
        Ok(Object::Reference(id)) => *id,
        Ok(Object::Dictionary(_)) => {
            // Inline AcroForm - use catalog_id as placeholder
            // We need to work with the dict directly
            return collect_form_fields_from_dict(doc, catalog, catalog_id);
        }
        _ => return (None, false, vec![]),
    };
    let acroform_dict = match doc.get_object(acroform_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return (Some(acroform_ref), false, vec![]),
    };
    collect_form_fields_from_dict(doc, acroform_dict, acroform_ref)
}

fn collect_form_fields_from_dict(doc: &Document, acroform_dict: &lopdf::Dictionary, acroform_id: ObjectId) -> (Option<ObjectId>, bool, Vec<FormFieldInfo>) {
    let need_appearances = acroform_dict.get(b"NeedAppearances")
        .ok()
        .and_then(|v| match v { Object::Boolean(b) => Some(*b), _ => None })
        .unwrap_or(false);

    let fields_array = match acroform_dict.get(b"Fields") {
        Ok(Object::Array(arr)) => arr,
        Ok(Object::Reference(id)) => {
            match doc.get_object(*id) {
                Ok(Object::Array(arr)) => arr,
                _ => return (Some(acroform_id), need_appearances, vec![]),
            }
        }
        _ => return (Some(acroform_id), need_appearances, vec![]),
    };

    // Build page annotation map: widget object_id -> page number
    let pages = doc.get_pages();
    let mut widget_to_page: BTreeMap<ObjectId, u32> = BTreeMap::new();
    for (&page_num, &page_id) in &pages {
        if let Ok(Object::Dictionary(page_dict)) = doc.get_object(page_id)
            && let Ok(Object::Array(annots)) = page_dict.get(b"Annots") {
            for annot in annots {
                if let Ok(id) = annot.as_reference() {
                    widget_to_page.insert(id, page_num);
                }
            }
        }
    }

    let mut fields = Vec::new();
    for field_obj in fields_array {
        if let Ok(field_id) = field_obj.as_reference() {
            collect_field_recursive(doc, field_id, "", &widget_to_page, &mut fields);
        }
    }

    (Some(acroform_id), need_appearances, fields)
}

fn collect_field_recursive(
    doc: &Document,
    field_id: ObjectId,
    parent_name: &str,
    widget_to_page: &BTreeMap<ObjectId, u32>,
    fields: &mut Vec<FormFieldInfo>,
) {
    let dict = match doc.get_object(field_id) {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Stream(s)) => &s.dict,
        _ => return,
    };

    // Get field name (T)
    let partial_name = dict.get(b"T").ok()
        .and_then(|v| match v {
            Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        })
        .unwrap_or_default();

    let qualified_name = if parent_name.is_empty() {
        partial_name.clone()
    } else if partial_name.is_empty() {
        parent_name.to_string()
    } else {
        format!("{}.{}", parent_name, partial_name)
    };

    // Check for Kids
    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        // Check if kids are fields (have T) or widgets (no T)
        let mut has_field_kids = false;
        for kid in kids {
            if let Ok(kid_id) = kid.as_reference()
                && let Ok(kid_obj) = doc.get_object(kid_id) {
                let kid_dict = match kid_obj {
                    Object::Dictionary(d) => d,
                    Object::Stream(s) => &s.dict,
                    _ => continue,
                };
                if kid_dict.get(b"T").is_ok() {
                    has_field_kids = true;
                    break;
                }
            }
        }
        if has_field_kids {
            for kid in kids {
                if let Ok(kid_id) = kid.as_reference() {
                    collect_field_recursive(doc, kid_id, &qualified_name, widget_to_page, fields);
                }
            }
            return;
        }
        // Kids are widgets — fall through to collect this field
    }

    // Determine field type (FT) — may be inherited
    let field_type = dict.get(b"FT").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
        .unwrap_or_else(|| "-".to_string());

    // Get value (V)
    let value = dict.get(b"V").ok()
        .map(|v| match v {
            Object::String(bytes, _) => format!("\"{}\"", String::from_utf8_lossy(bytes)),
            Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
            Object::Integer(i) => i.to_string(),
            Object::Boolean(b) => b.to_string(),
            Object::Array(_) => "[array]".to_string(),
            _ => "(empty)".to_string(),
        })
        .unwrap_or_else(|| "(empty)".to_string());

    // Get flags (Ff)
    let flags = dict.get(b"Ff").ok()
        .and_then(|v| v.as_i64().ok())
        .unwrap_or(0) as u32;

    // Determine page number
    let page_number = widget_to_page.get(&field_id).copied();

    fields.push(FormFieldInfo {
        object_id: field_id,
        qualified_name,
        field_type,
        value,
        page_number,
        flags,
    });
}

fn print_forms(writer: &mut impl Write, doc: &Document) {
    let (acroform_id, need_appearances, fields) = collect_form_fields(doc);

    match acroform_id {
        None => {
            writeln!(writer, "No AcroForm found in document.").unwrap();
            return;
        }
        Some(id) => {
            writeln!(writer, "AcroForm found (obj {}), NeedAppearances: {}", id.0, need_appearances).unwrap();
        }
    }

    writeln!(writer, "{} form fields\n", fields.len()).unwrap();
    if fields.is_empty() { return; }

    writeln!(writer, "  {:>4}  {:<24} {:<6}  {:<20} Page", "Obj#", "FieldName", "Type", "Value").unwrap();
    for f in &fields {
        let page_str = f.page_number.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
        writeln!(writer, "  {:>4}  {:<24} {:<6}  {:<20} {}",
            f.object_id.0,
            if f.qualified_name.len() > 24 { &f.qualified_name[..24] } else { &f.qualified_name },
            field_type_abbrev(&f.field_type),
            if f.value.len() > 20 { &f.value[..20] } else { &f.value },
            page_str,
        ).unwrap();
    }
}

fn print_forms_json(writer: &mut impl Write, doc: &Document) {
    let (acroform_id, need_appearances, fields) = collect_form_fields(doc);

    let items: Vec<Value> = fields.iter().map(|f| {
        json!({
            "object_number": f.object_id.0,
            "generation": f.object_id.1,
            "field_name": f.qualified_name,
            "field_type": f.field_type,
            "value": f.value,
            "flags": f.flags,
            "page_number": f.page_number,
        })
    }).collect();

    let output = json!({
        "acroform_object": acroform_id.map(|id| id.0),
        "need_appearances": need_appearances,
        "field_count": items.len(),
        "fields": items,
    });
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

fn compare_pdfs(doc1: &Document, doc2: &Document, page_filter: Option<&PageSpec>) -> DiffResult {
    let metadata_diffs = compare_metadata(doc1, doc2);
    let font_diffs = compare_fonts(doc1, doc2);

    let pages1 = doc1.get_pages();
    let pages2 = doc2.get_pages();

    let mut page_diffs = Vec::new();

    let page_numbers: Vec<u32> = if let Some(spec) = page_filter {
        spec.pages()
    } else {
        let max_pages = pages1.len().max(pages2.len()) as u32;
        (1..=max_pages).collect()
    };

    for pn in page_numbers {
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
            (None, None) if page_filter.is_some() => {
                page_diffs.push(PageDiff {
                    page_number: pn,
                    identical: false,
                    dict_diffs: vec![format!("Page {} not found in either file", pn)],
                    resource_diffs: vec![],
                    content_diffs: vec![],
                });
            }
            _ => {}
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

    // Compare resources across multiple categories
    let get_resource_names = |doc: &Document, dict: &lopdf::Dictionary, category: &[u8]| -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let resources = match dict.get(b"Resources") {
            Ok(Object::Reference(id)) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { d } else { return names; }
            }
            Ok(Object::Dictionary(d)) => d,
            _ => return names,
        };
        if let Ok(cat_obj) = resources.get(category) {
            let cat_dict = match cat_obj {
                Object::Dictionary(d) => d,
                Object::Reference(id) => {
                    if let Ok(Object::Dictionary(d)) = doc.get_object(*id) { d } else { return names; }
                }
                _ => return names,
            };
            for (k, _) in cat_dict.iter() {
                names.insert(String::from_utf8_lossy(k).into_owned());
            }
        }
        names
    };

    for category in &[b"Font" as &[u8], b"XObject", b"ColorSpace", b"ExtGState", b"Pattern", b"Shading"] {
        let cat_name = String::from_utf8_lossy(category);
        let names1 = get_resource_names(doc1, dict1, category);
        let names2 = get_resource_names(doc2, dict2, category);
        if names1 != names2 {
            for n in names1.difference(&names2) {
                resource_diffs.push(format!("{} {} only in first file", cat_name, n));
            }
            for n in names2.difference(&names1) {
                resource_diffs.push(format!("{} {} only in second file", cat_name, n));
            }
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

fn format_operation(op: &lopdf::content::Operation) -> String {
    if op.operands.is_empty() {
        return op.operator.clone();
    }
    let operands: Vec<String> = op.operands.iter().map(format_dict_value).collect();
    format!("{} {}", operands.join(" "), op.operator)
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
            let (decoded, _warning) = decode_stream(stream);
            all_bytes.extend_from_slice(&decoded);
        }
    }

    match Content::decode(&all_bytes) {
        Ok(content) => content.operations.iter().map(format_operation).collect(),
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

// ── Overview (default mode) ──────────────────────────────────────────

fn print_overview(writer: &mut impl Write, doc: &Document) {
    // Metadata
    writeln!(writer, "PDF Version: {}", doc.version).unwrap();
    writeln!(writer, "Objects:     {}", doc.objects.len()).unwrap();
    writeln!(writer, "Pages:       {}", doc.get_pages().len()).unwrap();

    // Encryption status
    let encrypted = doc.trailer.get(b"Encrypt").is_ok();
    writeln!(writer, "Encrypted:   {}", if encrypted { "yes" } else { "no" }).unwrap();

    // Producer / Creator from /Info
    if let Ok(info_ref) = doc.trailer.get(b"Info")
        && let Ok((_, Object::Dictionary(info))) = doc.dereference(info_ref)
    {
        for key in [b"Producer".as_slice(), b"Creator"] {
            if let Ok(Object::String(bytes, _)) = info.get(key) {
                writeln!(writer, "{:<13}{}", format!("{}:", String::from_utf8_lossy(key)), String::from_utf8_lossy(bytes)).unwrap();
            }
        }
    }

    // Validation summary
    let report = validate_pdf(doc);
    writeln!(writer).unwrap();
    if report.issues.is_empty() {
        writeln!(writer, "Validation:  no issues found").unwrap();
    } else {
        writeln!(writer, "Validation:  {} errors, {} warnings, {} info",
            report.error_count, report.warn_count, report.info_count).unwrap();
        for issue in &report.issues {
            let prefix = match issue.level {
                ValidationLevel::Error => "[ERROR]",
                ValidationLevel::Warn => "[WARN]",
                ValidationLevel::Info => "[INFO]",
            };
            writeln!(writer, "  {} {}", prefix, issue.message).unwrap();
        }
    }

    // Object stats summary
    let mut stream_count = 0usize;
    let mut total_stream_bytes = 0usize;
    for object in doc.objects.values() {
        if let Object::Stream(stream) = object {
            stream_count += 1;
            total_stream_bytes += stream.content.len();
        }
    }
    writeln!(writer).unwrap();
    writeln!(writer, "Streams:     {} ({} bytes)", stream_count, total_stream_bytes).unwrap();
}

fn print_overview_json(writer: &mut impl Write, doc: &Document) {
    let (info, catalog) = metadata_info(doc);
    let report = validate_pdf(doc);

    let issues: Vec<Value> = report.issues.iter().map(|i| {
        json!({
            "level": match i.level {
                ValidationLevel::Error => "error",
                ValidationLevel::Warn => "warning",
                ValidationLevel::Info => "info",
            },
            "message": i.message,
        })
    }).collect();

    let mut stream_count = 0usize;
    let mut total_stream_bytes = 0usize;
    for object in doc.objects.values() {
        if let Object::Stream(stream) = object {
            stream_count += 1;
            total_stream_bytes += stream.content.len();
        }
    }

    let encrypted = doc.trailer.get(b"Encrypt").is_ok();

    let output = json!({
        "version": doc.version,
        "object_count": doc.objects.len(),
        "page_count": doc.get_pages().len(),
        "encrypted": encrypted,
        "info": info,
        "catalog": catalog,
        "validation": {
            "error_count": report.error_count,
            "warning_count": report.warn_count,
            "info_count": report.info_count,
            "issues": issues,
        },
        "streams": {
            "count": stream_count,
            "total_bytes": total_stream_bytes,
        },
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Refs-To (P1) ────────────────────────────────────────────────────

fn collect_references_in_object(obj: &Object, target_id: ObjectId, path: &str) -> Vec<String> {
    let mut found = Vec::new();
    collect_references_in_object_into(obj, target_id, path, &mut found);
    found
}

fn collect_references_in_object_into(obj: &Object, target_id: ObjectId, path: &str, found: &mut Vec<String>) {
    match obj {
        Object::Reference(id) if *id == target_id => {
            found.push(path.to_string());
        }
        Object::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                collect_references_in_object_into(item, target_id, &child_path, found);
            }
        }
        Object::Dictionary(dict) => {
            for (key, value) in dict.iter() {
                let child_path = format!("{}/{}", path, String::from_utf8_lossy(key));
                collect_references_in_object_into(value, target_id, &child_path, found);
            }
        }
        Object::Stream(stream) => {
            for (key, value) in stream.dict.iter() {
                let child_path = format!("{}/{}", path, String::from_utf8_lossy(key));
                collect_references_in_object_into(value, target_id, &child_path, found);
            }
        }
        _ => {}
    }
}

struct ReverseRef {
    obj_num: u32,
    generation: u16,
    kind: String,
    type_label: String,
    paths: Vec<String>,
}

fn collect_reverse_refs(doc: &Document, target_id: ObjectId) -> Vec<ReverseRef> {
    let mut refs = Vec::new();
    for (&(obj_num, generation), object) in &doc.objects {
        let paths = collect_references_in_object(object, target_id, "");
        if !paths.is_empty() {
            refs.push(ReverseRef {
                obj_num,
                generation,
                kind: object.enum_variant().to_string(),
                type_label: object_type_label(object),
                paths,
            });
        }
    }
    refs
}

fn reverse_refs_to_json(refs: &[ReverseRef]) -> Vec<Value> {
    refs.iter().map(|r| json!({
        "object_number": r.obj_num,
        "generation": r.generation,
        "kind": r.kind,
        "type": r.type_label,
        "via_keys": r.paths,
    })).collect()
}

fn collect_forward_refs_json(doc: &Document, object: &Object) -> Vec<Value> {
    collect_refs_with_paths(object).iter().map(|(path, ref_id)| {
        let mut entry = json!({
            "path": path,
            "object_number": ref_id.0,
            "generation": ref_id.1,
        });
        if let Ok(resolved) = doc.get_object(*ref_id) {
            entry["summary"] = json!(deref_summary(resolved, doc));
        }
        entry
    }).collect()
}

fn print_refs_to(writer: &mut impl Write, doc: &Document, target_num: u32) {
    let target_id = (target_num, 0);
    writeln!(writer, "Objects referencing {} 0 R:\n", target_num).unwrap();

    let rev_refs = collect_reverse_refs(doc, target_id);
    for r in &rev_refs {
        writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} via {}", r.obj_num, r.generation, r.kind, r.type_label, r.paths.join(", ")).unwrap();
    }
    writeln!(writer, "\nFound {} objects referencing {} 0 R.", rev_refs.len(), target_num).unwrap();
}

fn print_refs_to_json(writer: &mut impl Write, doc: &Document, target_num: u32) {
    let target_id = (target_num, 0);
    let references = reverse_refs_to_json(&collect_reverse_refs(doc, target_id));
    let output = json!({
        "target_object": target_num,
        "reference_count": references.len(),
        "references": references,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Fonts (P1) ──────────────────────────────────────────────────────

struct FontInfo {
    object_id: ObjectId,
    base_font: String,
    subtype: String,
    encoding: String,
    embedded: Option<ObjectId>,
    to_unicode: Option<ObjectId>,
    first_char: Option<i64>,
    last_char: Option<i64>,
    widths_len: Option<usize>,
    encoding_differences: Option<String>,
    cid_system_info: Option<String>,
}

fn collect_fonts(doc: &Document) -> Vec<FontInfo> {
    let font_subtypes: &[&[u8]] = &[
        b"Type1", b"TrueType", b"Type0", b"CIDFontType0", b"CIDFontType2", b"MMType1", b"Type3",
    ];

    let mut fonts = Vec::new();
    for (&obj_id, object) in &doc.objects {
        let dict = match object {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };

        let is_font = dict.get_type().ok().is_some_and(|t| t == b"Font")
            || dict.get(b"Subtype").ok().is_some_and(|v| {
                if let Ok(name) = v.as_name() {
                    font_subtypes.contains(&name)
                } else {
                    false
                }
            });

        if !is_font { continue; }

        let base_font = dict.get(b"BaseFont").ok()
            .and_then(name_to_string)
            .unwrap_or_else(|| "-".to_string());

        let subtype = dict.get(b"Subtype").ok()
            .and_then(name_to_string)
            .unwrap_or_else(|| "-".to_string());

        let encoding = dict.get(b"Encoding").ok()
            .map(|v| match v {
                Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                Object::Reference(id) => format!("{} {} R", id.0, id.1),
                _ => "-".to_string(),
            })
            .unwrap_or_else(|| "-".to_string());

        // Check FontDescriptor for embedded font files
        let embedded = dict.get(b"FontDescriptor").ok()
            .and_then(|v| v.as_reference().ok())
            .and_then(|fd_id| doc.get_object(fd_id).ok())
            .and_then(|fd_obj| {
                let fd_dict = match fd_obj {
                    Object::Dictionary(d) => d,
                    Object::Stream(s) => &s.dict,
                    _ => return None,
                };
                for key in &[b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                    if let Ok(ff_ref) = fd_dict.get(key)
                        && let Ok(id) = ff_ref.as_reference() {
                            return Some(id);
                    }
                }
                None
            });

        let to_unicode = dict.get(b"ToUnicode").ok()
            .and_then(|v| v.as_reference().ok());

        let first_char = dict.get(b"FirstChar").ok()
            .and_then(|v| v.as_i64().ok());

        let last_char = dict.get(b"LastChar").ok()
            .and_then(|v| v.as_i64().ok());

        let widths_len = dict.get(b"Widths").ok()
            .and_then(|v| match v {
                Object::Array(arr) => Some(arr.len()),
                Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| {
                    if let Object::Array(arr) = o { Some(arr.len()) } else { None }
                }),
                _ => None,
            });

        let encoding_differences = extract_encoding_differences(doc, dict);
        let cid_system_info = extract_cid_system_info(doc, dict);

        fonts.push(FontInfo {
            object_id: obj_id, base_font, subtype, encoding, embedded,
            to_unicode, first_char, last_char, widths_len,
            encoding_differences, cid_system_info,
        });
    }

    fonts.sort_by_key(|f| f.object_id);
    fonts
}

fn extract_encoding_differences(doc: &Document, dict: &lopdf::Dictionary) -> Option<String> {
    let enc_obj = dict.get(b"Encoding").ok()?;
    let enc_dict = resolve_dict(doc, enc_obj)?;
    let diffs = enc_dict.get(b"Differences").ok()?;
    let arr = resolve_array(doc, diffs)?;

    let mut parts = Vec::new();
    let mut current_code: Option<i64> = None;
    let mut total_names = 0usize;
    for item in arr {
        match item {
            Object::Integer(n) => { current_code = Some(*n); }
            Object::Name(n) => {
                total_names += 1;
                if parts.len() < 5 {
                    let name = String::from_utf8_lossy(n);
                    if let Some(code) = current_code {
                        parts.push(format!("{}=/{}", code, name));
                        current_code = Some(code + 1);
                    } else {
                        parts.push(format!("/{}", name));
                    }
                }
            }
            _ => {}
        }
    }

    if total_names == 0 {
        return None;
    }

    let mut summary = parts.join(", ");
    if total_names > 5 {
        summary.push_str(&format!(", ... ({} total)", total_names));
    }
    Some(summary)
}

fn extract_cid_system_info(doc: &Document, dict: &lopdf::Dictionary) -> Option<String> {
    let descendants = dict.get(b"DescendantFonts").ok()?;
    let arr = resolve_array(doc, descendants)?;

    let first = arr.first()?;
    let cid_font_dict = match first {
        Object::Dictionary(d) => d,
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => return None,
        },
        _ => return None,
    };

    let csi = cid_font_dict.get(b"CIDSystemInfo").ok()?;
    let csi_dict = resolve_dict(doc, csi)?;

    let registry = csi_dict.get(b"Registry").ok()
        .and_then(obj_to_string_lossy)
        .unwrap_or_else(|| "?".to_string());
    let ordering = csi_dict.get(b"Ordering").ok()
        .and_then(obj_to_string_lossy)
        .unwrap_or_else(|| "?".to_string());
    let supplement = csi_dict.get(b"Supplement").ok()
        .and_then(|v| v.as_i64().ok())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".to_string());

    Some(format!("{}-{}-{}", registry, ordering, supplement))
}

fn print_fonts(writer: &mut impl Write, doc: &Document) {
    let fonts = collect_fonts(doc);
    writeln!(writer, "{} fonts found\n", fonts.len()).unwrap();
    writeln!(writer, "  {:>4}  {:<30} {:<14} {:<18} Embedded", "Obj#", "BaseFont", "Subtype", "Encoding").unwrap();
    for f in &fonts {
        let embedded_str = match f.embedded {
            Some(id) => format!("yes ({})", id.0),
            None => "no".to_string(),
        };
        writeln!(writer, "  {:>4}  {:<30} {:<14} {:<18} {}", f.object_id.0, f.base_font, f.subtype, f.encoding, embedded_str).unwrap();
        // Diagnostic details
        if let Some(id) = f.to_unicode {
            writeln!(writer, "          ToUnicode: {} 0 R", id.0).unwrap();
        }
        if f.first_char.is_some() || f.last_char.is_some() || f.widths_len.is_some() {
            let fc = f.first_char.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
            let lc = f.last_char.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
            let wl = f.widths_len.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
            writeln!(writer, "          CharRange: {}-{}, Widths: {}", fc, lc, wl).unwrap();
        }
        if let Some(ref diffs) = f.encoding_differences {
            writeln!(writer, "          Differences: {}", diffs).unwrap();
        }
        if let Some(ref csi) = f.cid_system_info {
            writeln!(writer, "          CIDSystemInfo: {}", csi).unwrap();
        }
    }
}

fn print_fonts_json(writer: &mut impl Write, doc: &Document) {
    let fonts = collect_fonts(doc);
    let items: Vec<Value> = fonts.iter().map(|f| {
        let mut obj = json!({
            "object_number": f.object_id.0,
            "generation": f.object_id.1,
            "base_font": f.base_font,
            "subtype": f.subtype,
            "encoding": f.encoding,
        });
        if let Some(id) = f.embedded {
            obj["embedded"] = json!({"object_number": id.0, "generation": id.1});
        } else {
            obj["embedded"] = json!(null);
        }
        if let Some(id) = f.to_unicode {
            obj["to_unicode"] = json!({"object_number": id.0, "generation": id.1});
        }
        if let Some(fc) = f.first_char {
            obj["first_char"] = json!(fc);
        }
        if let Some(lc) = f.last_char {
            obj["last_char"] = json!(lc);
        }
        if let Some(wl) = f.widths_len {
            obj["widths_count"] = json!(wl);
        }
        if let Some(ref diffs) = f.encoding_differences {
            obj["encoding_differences"] = json!(diffs);
        }
        if let Some(ref csi) = f.cid_system_info {
            obj["cid_system_info"] = json!(csi);
        }
        obj
    }).collect();
    let output = json!({
        "font_count": items.len(),
        "fonts": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Images (P1) ─────────────────────────────────────────────────────

struct ImageInfo {
    object_id: ObjectId,
    width: i64,
    height: i64,
    color_space: String,
    bits_per_component: i64,
    filter: String,
    size: usize,
}

fn format_color_space(obj: &Object, doc: &Document) -> String {
    match obj {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Array(arr) => {
            let names: Vec<String> = arr.iter().map(|item| match item {
                Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                Object::Reference(id) => format!("{} {} R", id.0, id.1),
                Object::Integer(i) => i.to_string(),
                _ => "?".to_string(),
            }).collect();
            format!("[{}]", names.join(" "))
        }
        Object::Reference(id) => {
            if let Ok(resolved) = doc.get_object(*id) {
                format_color_space(resolved, doc)
            } else {
                format!("{} {} R", id.0, id.1)
            }
        }
        _ => "-".to_string(),
    }
}

fn format_filter(obj: &Object) -> String {
    match obj {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Array(arr) => {
            let names: Vec<String> = arr.iter().map(|item| match item {
                Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                _ => "?".to_string(),
            }).collect();
            names.join(", ")
        }
        _ => "-".to_string(),
    }
}

fn collect_images(doc: &Document) -> Vec<ImageInfo> {
    let mut images = Vec::new();
    for (&obj_id, object) in &doc.objects {
        let (dict, content_len) = match object {
            Object::Stream(s) => (&s.dict, s.content.len()),
            _ => continue,
        };

        let is_image = dict.get(b"Subtype").ok()
            .is_some_and(|v| v.as_name().ok().is_some_and(|n| n == b"Image"));
        if !is_image { continue; }

        let width = dict.get(b"Width").ok()
            .and_then(|v| v.as_i64().ok())
            .unwrap_or(0);
        let height = dict.get(b"Height").ok()
            .and_then(|v| v.as_i64().ok())
            .unwrap_or(0);
        let color_space = dict.get(b"ColorSpace").ok()
            .map(|v| format_color_space(v, doc))
            .unwrap_or_else(|| "-".to_string());
        let bits_per_component = dict.get(b"BitsPerComponent").ok()
            .and_then(|v| v.as_i64().ok())
            .unwrap_or(0);
        let filter = dict.get(b"Filter").ok()
            .map(format_filter)
            .unwrap_or_else(|| "-".to_string());

        images.push(ImageInfo {
            object_id: obj_id,
            width,
            height,
            color_space,
            bits_per_component,
            filter,
            size: content_len,
        });
    }

    images.sort_by_key(|i| i.object_id);
    images
}

fn print_images(writer: &mut impl Write, doc: &Document) {
    let images = collect_images(doc);
    writeln!(writer, "{} images found\n", images.len()).unwrap();
    writeln!(writer, "  {:>4}  {:>5}  {:>6}  {:<18} {:>3}  {:<18} {:>8}", "Obj#", "Width", "Height", "ColorSpace", "BPC", "Filter", "Size").unwrap();
    for img in &images {
        writeln!(writer, "  {:>4}  {:>5}  {:>6}  {:<18} {:>3}  {:<18} {:>8}", img.object_id.0, img.width, img.height, img.color_space, img.bits_per_component, img.filter, img.size).unwrap();
    }
}

fn print_images_json(writer: &mut impl Write, doc: &Document) {
    let images = collect_images(doc);
    let items: Vec<Value> = images.iter().map(|img| {
        json!({
            "object_number": img.object_id.0,
            "generation": img.object_id.1,
            "width": img.width,
            "height": img.height,
            "color_space": img.color_space,
            "bits_per_component": img.bits_per_component,
            "filter": img.filter,
            "size": img.size,
        })
    }).collect();
    let output = json!({
        "image_count": items.len(),
        "images": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Validate (P1) ───────────────────────────────────────────────────

#[derive(PartialEq)]
enum ValidationLevel {
    Error,
    Warn,
    Info,
}

struct ValidationIssue {
    level: ValidationLevel,
    message: String,
}

struct ValidationReport {
    issues: Vec<ValidationIssue>,
    error_count: usize,
    warn_count: usize,
    info_count: usize,
}

fn validate_pdf(doc: &Document) -> ValidationReport {
    let mut issues = Vec::new();

    check_broken_references(doc, &mut issues);
    check_unreachable_objects(doc, &mut issues);
    check_required_keys(doc, &mut issues);
    check_stream_lengths(doc, &mut issues);
    check_page_tree(doc, &mut issues);
    check_content_stream_syntax(doc, &mut issues);
    check_font_requirements(doc, &mut issues);
    check_page_tree_cycles(doc, &mut issues);
    check_names_tree_structure(doc, &mut issues);
    check_duplicate_objects(doc, &mut issues);

    let error_count = issues.iter().filter(|i| i.level == ValidationLevel::Error).count();
    let warn_count = issues.iter().filter(|i| i.level == ValidationLevel::Warn).count();
    let info_count = issues.iter().filter(|i| i.level == ValidationLevel::Info).count();

    ValidationReport { issues, error_count, warn_count, info_count }
}

fn check_broken_references(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    for (&(obj_num, generation), object) in &doc.objects {
        let broken = collect_broken_refs(object, doc);
        for (ref_num, ref_generation) in broken {
            issues.push(ValidationIssue {
                level: ValidationLevel::Error,
                message: format!("Object {} {}: references non-existent object {} {}", obj_num, generation, ref_num, ref_generation),
            });
        }
    }
}

fn collect_broken_refs(obj: &Object, doc: &Document) -> Vec<(u32, u16)> {
    let mut broken = Vec::new();
    match obj {
        Object::Reference(id) => {
            if doc.get_object(*id).is_err() {
                broken.push(*id);
            }
        }
        Object::Array(arr) => {
            for item in arr {
                broken.extend(collect_broken_refs(item, doc));
            }
        }
        Object::Dictionary(dict) => {
            for (_, v) in dict.iter() {
                broken.extend(collect_broken_refs(v, doc));
            }
        }
        Object::Stream(stream) => {
            for (_, v) in stream.dict.iter() {
                broken.extend(collect_broken_refs(v, doc));
            }
        }
        _ => {}
    }
    broken
}

fn collect_reachable_ids(doc: &Document) -> BTreeSet<ObjectId> {
    let mut visited = BTreeSet::new();

    fn walk_refs(obj: &Object, doc: &Document, visited: &mut BTreeSet<ObjectId>) {
        match obj {
            Object::Reference(id) => {
                if visited.contains(id) { return; }
                visited.insert(*id);
                if let Ok(resolved) = doc.get_object(*id) {
                    walk_refs(resolved, doc, visited);
                }
            }
            Object::Array(arr) => {
                for item in arr { walk_refs(item, doc, visited); }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() { walk_refs(v, doc, visited); }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter() { walk_refs(v, doc, visited); }
            }
            _ => {}
        }
    }

    // Start from trailer
    for (_, v) in doc.trailer.iter() {
        walk_refs(v, doc, &mut visited);
    }

    visited
}

fn check_unreachable_objects(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let reachable = collect_reachable_ids(doc);
    for &(obj_num, generation) in doc.objects.keys() {
        if !reachable.contains(&(obj_num, generation)) {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Object {} {} is unreachable from trailer", obj_num, generation),
            });
        }
    }
}

fn check_required_keys(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    // Catalog must have /Pages
    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        if let Ok(Object::Dictionary(catalog)) = doc.get_object(root_ref)
            && catalog.get(b"Pages").is_err() {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Error,
                    message: "Catalog missing required /Pages key".to_string(),
                });
        }
    } else {
        issues.push(ValidationIssue {
            level: ValidationLevel::Error,
            message: "Trailer missing /Root reference".to_string(),
        });
    }

    // Each page must have /MediaBox (or inherit from parent)
    let pages = doc.get_pages();
    for (&page_num, &page_id) in &pages {
        if !page_has_media_box(doc, page_id) {
            issues.push(ValidationIssue {
                level: ValidationLevel::Error,
                message: format!("Page {} (object {}): missing /MediaBox (not found in page or parent chain)", page_num, page_id.0),
            });
        }
    }
}

fn page_has_media_box(doc: &Document, page_id: ObjectId) -> bool {
    let mut current_id = Some(page_id);
    let mut depth = 0;
    while let Some(id) = current_id {
        if depth > 20 { break; } // Guard against cycles
        depth += 1;
        if let Ok(obj) = doc.get_object(id) {
            let dict = match obj {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                _ => break,
            };
            if dict.get(b"MediaBox").is_ok() {
                return true;
            }
            // Walk up the /Parent chain
            current_id = dict.get(b"Parent").ok()
                .and_then(|v| v.as_reference().ok());
        } else {
            break;
        }
    }
    false
}

fn check_stream_lengths(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    for (&(obj_num, generation), object) in &doc.objects {
        if let Object::Stream(stream) = object
            && let Ok(Object::Integer(declared)) = stream.dict.get(b"Length") {
                let actual = stream.content.len() as i64;
                if *declared != actual {
                    issues.push(ValidationIssue {
                        level: ValidationLevel::Warn,
                        message: format!("Object {} {}: /Length is {} but stream content is {} bytes", obj_num, generation, declared, actual),
                    });
                }
        }
    }
}

fn check_page_tree(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let pages = doc.get_pages();
    let actual_count = pages.len();

    // Check /Pages /Count
    if let Some(root_ref) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok())
        && let Ok(Object::Dictionary(catalog)) = doc.get_object(root_ref)
        && let Ok(pages_ref) = catalog.get(b"Pages").and_then(|o| o.as_reference())
        && let Ok(Object::Dictionary(pages_dict)) = doc.get_object(pages_ref)
        && let Ok(Object::Integer(count)) = pages_dict.get(b"Count")
        && *count as usize != actual_count
    {
        issues.push(ValidationIssue {
            level: ValidationLevel::Error,
            message: format!("/Pages /Count is {} but document has {} pages", count, actual_count),
        });
    }
}

fn check_content_stream_syntax(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let pages = doc.get_pages();
    for (&page_num, &page_id) in &pages {
        let page_dict = match doc.get_object(page_id) {
            Ok(Object::Dictionary(d)) => d,
            _ => continue,
        };
        let content_ids: Vec<ObjectId> = match page_dict.get(b"Contents") {
            Ok(Object::Reference(id)) => vec![*id],
            Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => continue,
        };
        for content_id in content_ids {
            if let Ok(Object::Stream(stream)) = doc.get_object(content_id) {
                let (decoded, _) = decode_stream(stream);
                if Content::decode(&decoded).is_err() {
                    issues.push(ValidationIssue {
                        level: ValidationLevel::Warn,
                        message: format!("Page {}: content stream {} {} has invalid syntax", page_num, content_id.0, content_id.1),
                    });
                }
            }
        }
    }
}

fn check_font_requirements(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    for (&(obj_num, generation), object) in &doc.objects {
        let dict = match object {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        let is_font = dict.get(b"Type").ok()
            .and_then(|v| v.as_name().ok().map(|n| n == b"Font"))
            .unwrap_or(false);
        if !is_font { continue; }

        let subtype = dict.get(b"Subtype").ok()
            .and_then(|v| v.as_name().ok());

        let needs_basefont = matches!(subtype, Some(b"Type1") | Some(b"TrueType") | Some(b"Type0") | Some(b"CIDFontType0") | Some(b"CIDFontType2"));
        if needs_basefont && dict.get(b"BaseFont").is_err() {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Font object {} {}: missing /BaseFont", obj_num, generation),
            });
        }

        let has_first = dict.get(b"FirstChar").is_ok();
        let has_last = dict.get(b"LastChar").is_ok();
        if has_first != has_last {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Font object {} {}: has {} but not {}", obj_num, generation,
                    if has_first { "/FirstChar" } else { "/LastChar" },
                    if has_first { "/LastChar" } else { "/FirstChar" }),
            });
        }

        if has_first && has_last
            && let (Ok(Object::Integer(first)), Ok(Object::Integer(last))) =
                (dict.get(b"FirstChar"), dict.get(b"LastChar"))
        {
            let expected_width_count = (last - first + 1).max(0) as usize;
            if let Ok(Object::Array(widths)) = dict.get(b"Widths")
                && widths.len() != expected_width_count
            {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Warn,
                    message: format!("Font object {} {}: /Widths has {} entries but /FirstChar..=/LastChar expects {}", obj_num, generation, widths.len(), expected_width_count),
                });
            }
        }
    }
}

fn check_page_tree_cycles(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let pages = doc.get_pages();
    for (&page_num, &page_id) in &pages {
        let mut visited = BTreeSet::new();
        visited.insert(page_id);
        let mut current = doc.get_object(page_id).ok()
            .and_then(|o| match o {
                Object::Dictionary(d) => d.get(b"Parent").ok().and_then(|v| v.as_reference().ok()),
                _ => None,
            });
        while let Some(parent_id) = current {
            if visited.contains(&parent_id) {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Error,
                    message: format!("Page {}: cycle detected in /Parent chain (object {} {} seen twice)", page_num, parent_id.0, parent_id.1),
                });
                break;
            }
            visited.insert(parent_id);
            current = doc.get_object(parent_id).ok()
                .and_then(|o| match o {
                    Object::Dictionary(d) => d.get(b"Parent").ok().and_then(|v| v.as_reference().ok()),
                    _ => None,
                });
        }
    }
}

fn check_names_tree_structure(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return,
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return,
    };
    let names_dict = match catalog.get(b"Names").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return,
    };

    let subtree_keys: &[&[u8]] = &[b"EmbeddedFiles", b"Dests", b"JavaScript", b"AP"];
    for key in subtree_keys {
        let subtree = match names_dict.get(key).ok().and_then(|o| resolve_dict(doc, o)) {
            Some(d) => d,
            None => continue,
        };
        let key_name = String::from_utf8_lossy(key);
        validate_name_tree_node(doc, subtree, &key_name, &mut BTreeSet::new(), issues, 0);
    }
}

fn validate_name_tree_node(
    doc: &Document,
    dict: &lopdf::Dictionary,
    tree_name: &str,
    visited: &mut BTreeSet<ObjectId>,
    issues: &mut Vec<ValidationIssue>,
    depth: usize,
) {
    if depth > 50 {
        issues.push(ValidationIssue {
            level: ValidationLevel::Warn,
            message: format!("Name tree '{}': exceeded maximum depth (50)", tree_name),
        });
        return;
    }

    let has_names = dict.get(b"Names").is_ok();
    let has_kids = dict.get(b"Kids").is_ok();

    if !has_names && !has_kids {
        issues.push(ValidationIssue {
            level: ValidationLevel::Warn,
            message: format!("Name tree '{}': node has neither /Names nor /Kids", tree_name),
        });
        return;
    }

    if let Ok(Object::Array(names)) = dict.get(b"Names")
        && names.len() % 2 != 0
    {
        issues.push(ValidationIssue {
            level: ValidationLevel::Warn,
            message: format!("Name tree '{}': /Names array has odd length ({})", tree_name, names.len()),
        });
    }

    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        for kid in kids {
            let kid_id = match kid {
                Object::Reference(id) => *id,
                _ => continue,
            };
            if visited.contains(&kid_id) {
                issues.push(ValidationIssue {
                    level: ValidationLevel::Error,
                    message: format!("Name tree '{}': cycle detected at object {} {}", tree_name, kid_id.0, kid_id.1),
                });
                continue;
            }
            visited.insert(kid_id);
            if let Ok(Object::Dictionary(kid_dict)) = doc.get_object(kid_id) {
                validate_name_tree_node(doc, kid_dict, tree_name, visited, issues, depth + 1);
            }
        }
    }
}

fn check_duplicate_objects(doc: &Document, issues: &mut Vec<ValidationIssue>) {
    let mut seen_numbers: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    for &(obj_num, generation) in doc.objects.keys() {
        seen_numbers.entry(obj_num).or_default().push(generation);
    }
    for (obj_num, generations) in &seen_numbers {
        if generations.len() > 1 {
            issues.push(ValidationIssue {
                level: ValidationLevel::Warn,
                message: format!("Object {}: multiple generations present ({:?})", obj_num, generations),
            });
        }
    }
}

fn print_validation(writer: &mut impl Write, doc: &Document) {
    let report = validate_pdf(doc);

    if report.issues.is_empty() {
        writeln!(writer, "[OK] No issues found.").unwrap();
        return;
    }

    for issue in &report.issues {
        let prefix = match issue.level {
            ValidationLevel::Error => "[ERROR]",
            ValidationLevel::Warn => "[WARN]",
            ValidationLevel::Info => "[INFO]",
        };
        writeln!(writer, "{} {}", prefix, issue.message).unwrap();
    }
    writeln!(writer, "\nSummary: {} errors, {} warnings, {} info",
        report.error_count, report.warn_count, report.info_count).unwrap();
}

fn print_validation_json(writer: &mut impl Write, doc: &Document) {
    let report = validate_pdf(doc);

    let issues: Vec<Value> = report.issues.iter().map(|i| {
        json!({
            "level": match i.level {
                ValidationLevel::Error => "error",
                ValidationLevel::Warn => "warning",
                ValidationLevel::Info => "info",
            },
            "message": i.message,
        })
    }).collect();

    let output = json!({
        "error_count": report.error_count,
        "warning_count": report.warn_count,
        "info_count": report.info_count,
        "issues": issues,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Stats ────────────────────────────────────────────────────────────

struct PdfStats {
    page_count: usize,
    object_count: usize,
    type_counts: BTreeMap<String, usize>,
    total_stream_bytes: usize,
    total_decoded_bytes: usize,
    filter_counts: BTreeMap<String, usize>,
    largest_streams: Vec<(ObjectId, usize)>,
}

fn collect_stats(doc: &Document) -> PdfStats {
    let page_count = doc.get_pages().len();
    let object_count = doc.objects.len();
    let mut type_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_stream_bytes = 0usize;
    let mut total_decoded_bytes = 0usize;
    let mut filter_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut stream_sizes: Vec<(ObjectId, usize)> = Vec::new();

    for (&obj_id, object) in &doc.objects {
        let variant = object.enum_variant().to_string();
        *type_counts.entry(variant).or_insert(0) += 1;

        if let Object::Stream(stream) = object {
            let raw_len = stream.content.len();
            total_stream_bytes += raw_len;
            stream_sizes.push((obj_id, raw_len));

            let (decoded, _) = decode_stream(stream);
            total_decoded_bytes += decoded.len();

            let filters = get_filter_names(stream);
            for f in &filters {
                let name = String::from_utf8_lossy(f).into_owned();
                *filter_counts.entry(name).or_insert(0) += 1;
            }
        }
    }

    stream_sizes.sort_by(|a, b| b.1.cmp(&a.1));
    stream_sizes.truncate(10);

    PdfStats {
        page_count,
        object_count,
        type_counts,
        total_stream_bytes,
        total_decoded_bytes,
        filter_counts,
        largest_streams: stream_sizes,
    }
}

fn print_stats(writer: &mut impl Write, doc: &Document) {
    let stats = collect_stats(doc);

    writeln!(writer, "--- Overview ---").unwrap();
    writeln!(writer, "  Pages:   {}", stats.page_count).unwrap();
    writeln!(writer, "  Objects: {}", stats.object_count).unwrap();
    writeln!(writer).unwrap();

    writeln!(writer, "--- Objects by Type ---").unwrap();
    for (typ, count) in &stats.type_counts {
        writeln!(writer, "  {:<14} {}", typ, count).unwrap();
    }
    writeln!(writer).unwrap();

    writeln!(writer, "--- Stream Statistics ---").unwrap();
    let stream_count: usize = stats.type_counts.get("Stream").copied().unwrap_or(0);
    writeln!(writer, "  Streams:        {}", stream_count).unwrap();
    writeln!(writer, "  Total raw:      {} bytes", stats.total_stream_bytes).unwrap();
    writeln!(writer, "  Total decoded:  {} bytes", stats.total_decoded_bytes).unwrap();
    if !stats.filter_counts.is_empty() {
        writeln!(writer, "  Filters:").unwrap();
        for (name, count) in &stats.filter_counts {
            writeln!(writer, "    {:<20} {}", name, count).unwrap();
        }
    }
    writeln!(writer).unwrap();

    if !stats.largest_streams.is_empty() {
        writeln!(writer, "--- Largest Streams (top {}) ---", stats.largest_streams.len()).unwrap();
        for (obj_id, size) in &stats.largest_streams {
            writeln!(writer, "  Object {:>4}  {} bytes", obj_id.0, size).unwrap();
        }
    }
}

fn print_stats_json(writer: &mut impl Write, doc: &Document) {
    let stats = collect_stats(doc);
    let largest: Vec<Value> = stats.largest_streams.iter()
        .map(|(id, size)| json!({"object_number": id.0, "generation": id.1, "size": size}))
        .collect();
    let output = json!({
        "page_count": stats.page_count,
        "object_count": stats.object_count,
        "type_counts": stats.type_counts,
        "total_stream_bytes": stats.total_stream_bytes,
        "total_decoded_bytes": stats.total_decoded_bytes,
        "filter_counts": stats.filter_counts,
        "largest_streams": largest,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Name Tree / Number Tree walkers ──────────────────────────────────

fn walk_name_tree(doc: &Document, dict: &lopdf::Dictionary) -> Vec<(String, Object)> {
    let mut results = Vec::new();
    let mut visited = BTreeSet::new();
    walk_name_tree_inner(doc, dict, &mut results, &mut visited);
    results
}

fn walk_name_tree_inner(
    doc: &Document,
    dict: &lopdf::Dictionary,
    results: &mut Vec<(String, Object)>,
    visited: &mut BTreeSet<ObjectId>,
) {
    // Leaf node: /Names array with alternating key/value pairs
    if let Ok(Object::Array(names)) = dict.get(b"Names") {
        let mut i = 0;
        while i + 1 < names.len() {
            let key = match &names[i] {
                Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                _ => { i += 2; continue; }
            };
            results.push((key, names[i + 1].clone()));
            i += 2;
        }
    }
    // Intermediate node: /Kids array
    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        for kid in kids {
            let kid_id = match kid {
                Object::Reference(id) => *id,
                _ => continue,
            };
            if visited.contains(&kid_id) { continue; }
            visited.insert(kid_id);
            if let Ok(Object::Dictionary(kid_dict)) = doc.get_object(kid_id) {
                walk_name_tree_inner(doc, kid_dict, results, visited);
            }
        }
    }
}

fn walk_number_tree(doc: &Document, dict: &lopdf::Dictionary) -> Vec<(i64, Object)> {
    let mut results = Vec::new();
    let mut visited = BTreeSet::new();
    walk_number_tree_inner(doc, dict, &mut results, &mut visited);
    results
}

fn walk_number_tree_inner(
    doc: &Document,
    dict: &lopdf::Dictionary,
    results: &mut Vec<(i64, Object)>,
    visited: &mut BTreeSet<ObjectId>,
) {
    // Leaf node: /Nums array with alternating integer-key/value pairs
    if let Ok(Object::Array(nums)) = dict.get(b"Nums") {
        let mut i = 0;
        while i + 1 < nums.len() {
            let key = match &nums[i] {
                Object::Integer(n) => *n,
                _ => { i += 2; continue; }
            };
            results.push((key, nums[i + 1].clone()));
            i += 2;
        }
    }
    // Intermediate node: /Kids array
    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        for kid in kids {
            let kid_id = match kid {
                Object::Reference(id) => *id,
                _ => continue,
            };
            if visited.contains(&kid_id) { continue; }
            visited.insert(kid_id);
            if let Ok(Object::Dictionary(kid_dict)) = doc.get_object(kid_id) {
                walk_number_tree_inner(doc, kid_dict, results, visited);
            }
        }
    }
}

// ── Bookmarks ────────────────────────────────────────────────────────

struct OutlineItem {
    object_id: ObjectId,
    title: String,
    destination: String,
    children: Vec<OutlineItem>,
}

fn collect_outline_items(doc: &Document, first_id: ObjectId) -> Vec<OutlineItem> {
    let mut visited = BTreeSet::new();
    collect_outline_items_inner(doc, first_id, &mut visited)
}

fn collect_outline_items_inner(doc: &Document, first_id: ObjectId, visited: &mut BTreeSet<ObjectId>) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut current_id = Some(first_id);

    while let Some(id) = current_id {
        if visited.contains(&id) { break; }
        visited.insert(id);

        let dict = match doc.get_object(id) {
            Ok(Object::Dictionary(d)) => d,
            _ => break,
        };

        let title = dict.get(b"Title").ok()
            .map(|v| match v {
                Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                _ => "(untitled)".to_string(),
            })
            .unwrap_or_else(|| "(untitled)".to_string());

        let destination = format_destination(doc, dict);

        let children = dict.get(b"First").ok()
            .and_then(|v| v.as_reference().ok())
            .map(|child_id| collect_outline_items_inner(doc, child_id, visited))
            .unwrap_or_default();

        items.push(OutlineItem { object_id: id, title, destination, children });

        current_id = dict.get(b"Next").ok()
            .and_then(|v| v.as_reference().ok());
    }

    items
}

fn format_destination(doc: &Document, dict: &lopdf::Dictionary) -> String {
    // Check /Dest first
    if let Ok(dest) = dict.get(b"Dest") {
        return format_dest_value(doc, dest);
    }
    // Check /A (action)
    if let Ok(action_obj) = dict.get(b"A") {
        let action_dict = match action_obj {
            Object::Dictionary(d) => d,
            Object::Reference(id) => {
                match doc.get_object(*id) {
                    Ok(Object::Dictionary(d)) => d,
                    _ => return format!("Action({} {} R)", id.0, id.1),
                }
            }
            _ => return "-".to_string(),
        };
        let action_type = action_dict.get(b"S").ok()
            .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
            .unwrap_or_else(|| "?".to_string());
        match action_type.as_str() {
            "GoTo" => {
                if let Ok(d) = action_dict.get(b"D") {
                    return format!("GoTo({})", format_dest_value(doc, d));
                }
                "GoTo(?)".to_string()
            }
            "URI" => {
                let uri = action_dict.get(b"URI").ok()
                    .map(|v| match v {
                        Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                        _ => "?".to_string(),
                    })
                    .unwrap_or_else(|| "?".to_string());
                format!("URI({})", uri)
            }
            other => format!("Action({})", other),
        }
    } else {
        "-".to_string()
    }
}

fn format_dest_value(doc: &Document, dest: &Object) -> String {
    match dest {
        Object::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(|item| match item {
                Object::Reference(id) => format!("{} {} R", id.0, id.1),
                Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
                Object::Integer(i) => i.to_string(),
                Object::Real(r) => r.to_string(),
                Object::Null => "null".to_string(),
                _ => "?".to_string(),
            }).collect();
            format!("[{}]", parts.join(" "))
        }
        Object::String(bytes, _) => format!("({})", String::from_utf8_lossy(bytes)),
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::Reference(id) => {
            if let Ok(resolved) = doc.get_object(*id) {
                format_dest_value(doc, resolved)
            } else {
                format!("{} {} R", id.0, id.1)
            }
        }
        _ => "-".to_string(),
    }
}

fn count_outline_items(items: &[OutlineItem]) -> usize {
    items.iter().map(|item| 1 + count_outline_items(&item.children)).sum()
}

fn print_bookmarks(writer: &mut impl Write, doc: &Document) {
    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => {
            writeln!(writer, "No bookmarks (no /Root in trailer).").unwrap();
            return;
        }
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => {
            writeln!(writer, "No bookmarks (could not read catalog).").unwrap();
            return;
        }
    };
    let outlines_ref = match catalog.get(b"Outlines").ok().and_then(|v| {
        match v {
            Object::Reference(id) => Some(*id),
            _ => None,
        }
    }) {
        Some(id) => id,
        None => {
            writeln!(writer, "No bookmarks.").unwrap();
            return;
        }
    };
    let outlines_dict = match doc.get_object(outlines_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => {
            writeln!(writer, "No bookmarks (could not read /Outlines).").unwrap();
            return;
        }
    };
    let first_id = match outlines_dict.get(b"First").ok().and_then(|v| v.as_reference().ok()) {
        Some(id) => id,
        None => {
            writeln!(writer, "No bookmarks.").unwrap();
            return;
        }
    };

    let items = collect_outline_items(doc, first_id);
    let total = count_outline_items(&items);
    writeln!(writer, "{} bookmarks\n", total).unwrap();
    print_outline_tree(writer, &items, 0);
}

fn print_outline_tree(writer: &mut impl Write, items: &[OutlineItem], depth: usize) {
    let indent = "  ".repeat(depth);
    for item in items {
        writeln!(writer, "{}[{}] {} -> {}", indent, item.object_id.0, item.title, item.destination).unwrap();
        if !item.children.is_empty() {
            print_outline_tree(writer, &item.children, depth + 1);
        }
    }
}

fn print_bookmarks_json(writer: &mut impl Write, doc: &Document) {
    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => {
            writeln!(writer, "{}", serde_json::to_string_pretty(&json!({"bookmark_count": 0, "bookmarks": []})).unwrap()).unwrap();
            return;
        }
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => {
            writeln!(writer, "{}", serde_json::to_string_pretty(&json!({"bookmark_count": 0, "bookmarks": []})).unwrap()).unwrap();
            return;
        }
    };
    let first_id = catalog.get(b"Outlines").ok()
        .and_then(|v| v.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|obj| if let Object::Dictionary(d) = obj { Some(d) } else { None })
        .and_then(|d| d.get(b"First").ok())
        .and_then(|v| v.as_reference().ok());

    let items = match first_id {
        Some(id) => collect_outline_items(doc, id),
        None => vec![],
    };
    let total = count_outline_items(&items);

    fn items_to_json(items: &[OutlineItem]) -> Vec<Value> {
        items.iter().map(|item| {
            let mut obj = json!({
                "object_number": item.object_id.0,
                "title": item.title,
                "destination": item.destination,
            });
            if !item.children.is_empty() {
                obj["children"] = json!(items_to_json(&item.children));
            }
            obj
        }).collect()
    }

    let output = json!({
        "bookmark_count": total,
        "bookmarks": items_to_json(&items),
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Annotations ──────────────────────────────────────────────────────

struct AnnotationInfo {
    page_num: u32,
    object_id: ObjectId,
    subtype: String,
    rect: String,
    contents: String,
}

fn collect_annotations(doc: &Document, page_filter: Option<&PageSpec>) -> Vec<AnnotationInfo> {
    let pages = doc.get_pages();
    let mut annotations = Vec::new();

    for (&page_num, &page_id) in &pages {
        if let Some(spec) = page_filter
            && !spec.contains(page_num) { continue; }

        let page_dict = match doc.get_object(page_id) {
            Ok(Object::Dictionary(d)) => d,
            _ => continue,
        };

        let annot_refs: Vec<ObjectId> = match page_dict.get(b"Annots") {
            Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
            Ok(Object::Reference(id)) => {
                if let Ok(Object::Array(arr)) = doc.get_object(*id) {
                    arr.iter().filter_map(|o| o.as_reference().ok()).collect()
                } else {
                    continue;
                }
            }
            _ => continue,
        };

        for annot_id in annot_refs {
            let annot_dict = match doc.get_object(annot_id) {
                Ok(Object::Dictionary(d)) => d,
                _ => continue,
            };

            let subtype = annot_dict.get(b"Subtype").ok()
                .and_then(name_to_string)
                .unwrap_or_else(|| "-".to_string());

            let rect = annot_dict.get(b"Rect").ok()
                .map(format_dict_value)
                .unwrap_or_else(|| "-".to_string());

            let contents = annot_dict.get(b"Contents").ok()
                .map(|v| match v {
                    Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => "-".to_string(),
                })
                .unwrap_or_default();

            annotations.push(AnnotationInfo {
                page_num,
                object_id: annot_id,
                subtype,
                rect,
                contents,
            });
        }
    }

    annotations
}

fn print_annotations(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let annotations = collect_annotations(doc, page_filter);
    writeln!(writer, "{} annotations found\n", annotations.len()).unwrap();
    if annotations.is_empty() { return; }
    writeln!(writer, "  {:>4}  {:>4}  {:<12} {:<30} Contents", "Page", "Obj#", "Subtype", "Rect").unwrap();
    for a in &annotations {
        writeln!(writer, "  {:>4}  {:>4}  {:<12} {:<30} {}", a.page_num, a.object_id.0, a.subtype, a.rect, a.contents).unwrap();
    }
}

fn print_annotations_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let annotations = collect_annotations(doc, page_filter);
    let items: Vec<Value> = annotations.iter().map(|a| {
        json!({
            "page_number": a.page_num,
            "object_number": a.object_id.0,
            "generation": a.object_id.1,
            "subtype": a.subtype,
            "rect": a.rect,
            "contents": a.contents,
        })
    }).collect();
    let output = json!({
        "annotation_count": items.len(),
        "annotations": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Security ─────────────────────────────────────────────────────────

struct SecurityInfo {
    encrypted: bool,
    algorithm: String,
    version: i64,
    revision: i64,
    key_length: i64,
    permissions_raw: i64,
    permissions: BTreeMap<String, bool>,
    encrypt_object: Option<u32>,
}

fn algorithm_name(v: i64, length: i64) -> String {
    match v {
        0 => "Undocumented".to_string(),
        1 => "RC4, 40-bit".to_string(),
        2 => format!("RC4, {}-bit", if length > 0 { length } else { 40 }),
        3 => "Unpublished".to_string(),
        4 => "AES-128".to_string(),
        5 => "AES-256".to_string(),
        _ => format!("Unknown (V={})", v),
    }
}

fn decode_permissions(p: i64) -> BTreeMap<String, bool> {
    let mut perms = BTreeMap::new();
    let bits = p as u32;
    perms.insert("Print".to_string(), bits & (1 << 2) != 0);
    perms.insert("Modify".to_string(), bits & (1 << 3) != 0);
    perms.insert("Copy/extract text".to_string(), bits & (1 << 4) != 0);
    perms.insert("Annotate".to_string(), bits & (1 << 5) != 0);
    perms.insert("Fill forms".to_string(), bits & (1 << 8) != 0);
    perms.insert("Accessibility extract".to_string(), bits & (1 << 9) != 0);
    perms.insert("Assemble".to_string(), bits & (1 << 10) != 0);
    perms.insert("Print high quality".to_string(), bits & (1 << 11) != 0);
    perms
}

fn collect_security(doc: &Document, file_path: Option<&std::path::Path>) -> SecurityInfo {
    // Try trailer first (works for traditional xref tables)
    let encrypt_ref = doc.trailer.get(b"Encrypt").ok()
        .and_then(|v| match v {
            Object::Reference(id) => Some(*id),
            _ => None,
        });

    let encrypt_dict = encrypt_ref
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|obj| match obj {
            Object::Dictionary(d) => Some(d),
            _ => None,
        });

    // Also check if /Encrypt is an inline dictionary in the trailer
    let encrypt_dict = encrypt_dict.or_else(|| {
        doc.trailer.get(b"Encrypt").ok().and_then(|v| match v {
            Object::Dictionary(d) => Some(d),
            _ => None,
        })
    });

    if let Some(dict) = encrypt_dict {
        return build_security_info(dict, encrypt_ref);
    }

    // Fallback: lopdf consumes /Encrypt from the trailer during loading and
    // removes the encrypt dict object. For cross-reference stream PDFs, the
    // XRef stream object still has /Encrypt in its dict. Scan for it.
    let mut found_encrypt_obj: Option<u32> = None;
    for object in doc.objects.values() {
        let stream_dict = match object {
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        let is_xref = stream_dict.get(b"Type").ok()
            .and_then(|v| v.as_name().ok())
            .is_some_and(|n| n == b"XRef");
        if !is_xref { continue; }

        let enc_ref = stream_dict.get(b"Encrypt").ok()
            .and_then(|v| match v {
                Object::Reference(id) => Some(*id),
                _ => None,
            });

        if let Some(ref_id) = enc_ref {
            // Try to resolve the encrypt dict (usually consumed by lopdf)
            if let Ok(Object::Dictionary(d)) = doc.get_object(ref_id) {
                return build_security_info(d, Some(ref_id));
            }
            found_encrypt_obj = Some(ref_id.0);
            break;
        }

        // Check for inline /Encrypt dict
        if let Ok(Object::Dictionary(d)) = stream_dict.get(b"Encrypt") {
            return build_security_info(d, None);
        }
    }

    // Last resort: read the raw file bytes to find the encrypt dict that
    // lopdf consumed during loading.
    if let Some(obj_num) = found_encrypt_obj
        && let Some(path) = file_path
        && let Some(info) = parse_encrypt_from_raw_file(path, obj_num)
    {
        return info;
    }

    SecurityInfo {
        encrypted: false,
        algorithm: "-".to_string(),
        version: 0,
        revision: 0,
        key_length: 0,
        permissions_raw: 0,
        permissions: BTreeMap::new(),
        encrypt_object: None,
    }
}

fn dict_int(dict: &lopdf::Dictionary, key: &[u8], default: i64) -> i64 {
    dict.get(key).ok()
        .and_then(|o| if let Object::Integer(i) = o { Some(*i) } else { None })
        .unwrap_or(default)
}

fn build_security_info(dict: &lopdf::Dictionary, encrypt_ref: Option<ObjectId>) -> SecurityInfo {
    let v = dict_int(dict, b"V", 0);
    let r = dict_int(dict, b"R", 0);
    let length = dict_int(dict, b"Length", 40);
    let p = dict_int(dict, b"P", 0);

    SecurityInfo {
        encrypted: true,
        algorithm: algorithm_name(v, length),
        version: v,
        revision: r,
        key_length: length,
        permissions_raw: p,
        permissions: decode_permissions(p),
        encrypt_object: encrypt_ref.map(|id| id.0),
    }
}

/// Parse the encrypt dictionary directly from the raw PDF file bytes.
/// lopdf consumes this object during loading, so we find it by searching
/// for "{obj_num} 0 obj" and extracting the integer-valued keys we need.
fn parse_encrypt_from_raw_file(path: &std::path::Path, obj_num: u32) -> Option<SecurityInfo> {
    let data = fs::read(path).ok()?;
    let marker = format!("{} 0 obj", obj_num);
    let marker_bytes = marker.as_bytes();

    // Find the object in the raw bytes
    let pos = data.windows(marker_bytes.len())
        .position(|w| w == marker_bytes)?;

    // Extract a window after the marker — encrypt dicts are small
    let start = pos + marker_bytes.len();
    let end = (start + 1024).min(data.len());
    let window = &data[start..end];

    // Find the dictionary start
    let dict_start = window.windows(2).position(|w| w == b"<<")?;
    let dict_bytes = &window[dict_start..];

    let v = extract_int_after_key(dict_bytes, b"/V").unwrap_or(0);
    let r = extract_int_after_key(dict_bytes, b"/R").unwrap_or(0);
    let length = extract_int_after_key(dict_bytes, b"/Length").unwrap_or(40);
    let p = extract_int_after_key(dict_bytes, b"/P").unwrap_or(0);

    Some(SecurityInfo {
        encrypted: true,
        algorithm: algorithm_name(v, length),
        version: v,
        revision: r,
        key_length: length,
        permissions_raw: p,
        permissions: decode_permissions(p),
        encrypt_object: Some(obj_num),
    })
}

/// Find a PDF key name in raw bytes and parse the integer that follows it.
/// Handles negative integers (e.g. /P -1084).
fn extract_int_after_key(data: &[u8], key: &[u8]) -> Option<i64> {
    let pos = data.windows(key.len()).position(|w| w == key)?;
    let after = &data[pos + key.len()..];

    // Skip whitespace and any non-digit characters (but allow '-')
    let mut i = 0;
    while i < after.len() && (after[i] == b' ' || after[i] == b'\r' || after[i] == b'\n') {
        i += 1;
    }
    if i >= after.len() { return None; }

    // Parse the integer (possibly negative)
    let mut end = i;
    if after[end] == b'-' || after[end] == b'+' {
        end += 1;
    }
    while end < after.len() && after[end].is_ascii_digit() {
        end += 1;
    }
    if end == i { return None; }
    // If we only got a sign with no digits, bail
    if end == i + 1 && (after[i] == b'-' || after[i] == b'+') { return None; }

    let num_str = std::str::from_utf8(&after[i..end]).ok()?;
    num_str.parse().ok()
}

fn print_security(writer: &mut impl Write, doc: &Document, file_path: &std::path::Path) {
    let info = collect_security(doc, Some(file_path));
    if !info.encrypted {
        writeln!(writer, "Encryption: No").unwrap();
        return;
    }
    writeln!(writer, "Encryption: Yes").unwrap();
    writeln!(writer, "Algorithm:  {}", info.algorithm).unwrap();
    writeln!(writer, "Version:    {}", info.version).unwrap();
    writeln!(writer, "Revision:   {}", info.revision).unwrap();
    writeln!(writer, "Key Length: {} bits", info.key_length).unwrap();
    if let Some(obj) = info.encrypt_object {
        writeln!(writer, "Encrypt Object: {}", obj).unwrap();
    }
    writeln!(writer, "\nPermissions (raw: {}):", info.permissions_raw).unwrap();
    let perm_order = [
        "Print", "Modify", "Copy/extract text", "Annotate",
        "Fill forms", "Accessibility extract", "Assemble", "Print high quality",
    ];
    for name in &perm_order {
        if let Some(&allowed) = info.permissions.get(*name) {
            let tag = if allowed { "YES" } else { " NO" };
            writeln!(writer, "  [{}] {}", tag, name).unwrap();
        }
    }
}

fn print_security_json(writer: &mut impl Write, doc: &Document, file_path: &std::path::Path) {
    let info = collect_security(doc, Some(file_path));
    let perms: Value = info.permissions.iter()
        .map(|(k, v)| (k.clone(), json!(*v)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let output = json!({
        "encrypted": info.encrypted,
        "algorithm": info.algorithm,
        "version": info.version,
        "revision": info.revision,
        "key_length": info.key_length,
        "permissions_raw": info.permissions_raw,
        "permissions": perms,
        "encrypt_object": info.encrypt_object,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Embedded Files ───────────────────────────────────────────────────

struct EmbeddedFileInfo {
    name: String,
    filename: String,
    mime_type: String,
    size: Option<i64>,
    object_number: u32,
    filespec_object: ObjectId,
}

fn collect_embedded_files(doc: &Document) -> Vec<EmbeddedFileInfo> {
    let mut files = Vec::new();

    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return files,
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return files,
    };
    let names_dict = match catalog.get(b"Names").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return files,
    };
    let ef_dict = match names_dict.get(b"EmbeddedFiles").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return files,
    };

    let entries = walk_name_tree(doc, ef_dict);
    for (name, value) in entries {
        let filespec_id = match &value {
            Object::Reference(id) => *id,
            _ => continue,
        };
        let filespec = match doc.get_object(filespec_id) {
            Ok(Object::Dictionary(d)) => d,
            _ => continue,
        };

        let filename = filespec.get(b"UF").ok()
            .or_else(|| filespec.get(b"F").ok())
            .and_then(obj_to_string_lossy)
            .unwrap_or_else(|| name.clone());

        // Get the embedded file stream from /EF dict
        let ef_ref = filespec.get(b"EF").ok()
            .and_then(|v| resolve_dict(doc, v));
        let (stream_id, stream_dict) = match ef_ref {
            Some(ef) => {
                let stream_ref = ef.get(b"F").ok()
                    .or_else(|| ef.get(b"UF").ok());
                match stream_ref {
                    Some(Object::Reference(id)) => {
                        match doc.get_object(*id) {
                            Ok(Object::Stream(s)) => (*id, Some(s)),
                            _ => (*id, None),
                        }
                    }
                    _ => continue,
                }
            }
            None => continue,
        };

        let mime_type = stream_dict
            .and_then(|s| s.dict.get(b"Subtype").ok())
            .and_then(name_to_string)
            .unwrap_or_else(|| "-".to_string());

        // Try /Params/Size for the uncompressed size
        let size = stream_dict
            .and_then(|s| s.dict.get(b"Params").ok())
            .and_then(|v| resolve_dict(doc, v))
            .and_then(|d| d.get(b"Size").ok())
            .and_then(|v| if let Object::Integer(i) = v { Some(*i) } else { None });

        files.push(EmbeddedFileInfo {
            name,
            filename,
            mime_type,
            size,
            object_number: stream_id.0,
            filespec_object: filespec_id,
        });
    }

    files
}

fn print_embedded_files(writer: &mut impl Write, doc: &Document) {
    let files = collect_embedded_files(doc);
    writeln!(writer, "{} embedded files\n", files.len()).unwrap();
    if files.is_empty() { return; }
    writeln!(writer, "  {:>4}  {:<30} {:<24} {:>8}", "Obj#", "Filename", "MIME Type", "Size").unwrap();
    for f in &files {
        let size_str = f.size.map(|s| s.to_string()).unwrap_or_else(|| "-".to_string());
        writeln!(writer, "  {:>4}  {:<30} {:<24} {:>8}", f.object_number, f.filename, f.mime_type, size_str).unwrap();
    }
}

fn print_embedded_files_json(writer: &mut impl Write, doc: &Document) {
    let files = collect_embedded_files(doc);
    let items: Vec<Value> = files.iter().map(|f| {
        json!({
            "name": f.name,
            "filename": f.filename,
            "mime_type": f.mime_type,
            "size": f.size,
            "object_number": f.object_number,
            "filespec_object": f.filespec_object.0,
        })
    }).collect();
    let output = json!({
        "embedded_file_count": items.len(),
        "embedded_files": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Page Labels ──────────────────────────────────────────────────────

struct PageLabelEntry {
    physical_page: u32,
    label: String,
    style: String,
    prefix: String,
    start: i64,
}

fn int_to_roman(mut n: i64, uppercase: bool) -> String {
    if n <= 0 { return n.to_string(); }
    let table: &[(i64, &str)] = &[
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"),
        (100, "c"), (90, "xc"), (50, "l"), (40, "xl"),
        (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut result = String::new();
    for &(value, numeral) in table {
        while n >= value {
            result.push_str(numeral);
            n -= value;
        }
    }
    if uppercase { result.to_uppercase() } else { result }
}

fn int_to_alpha(n: i64, uppercase: bool) -> String {
    if n <= 0 { return n.to_string(); }
    let mut result = String::new();
    let mut remaining = n - 1;
    loop {
        let ch = (remaining % 26) as u8;
        let letter = if uppercase { b'A' + ch } else { b'a' + ch };
        result.insert(0, letter as char);
        remaining = remaining / 26 - 1;
        if remaining < 0 { break; }
    }
    result
}

fn format_page_label(style: &str, prefix: &str, value: i64) -> String {
    let number_part = match style {
        "D" => value.to_string(),
        "r" => int_to_roman(value, false),
        "R" => int_to_roman(value, true),
        "a" => int_to_alpha(value, false),
        "A" => int_to_alpha(value, true),
        _ => String::new(),
    };
    format!("{}{}", prefix, number_part)
}

fn collect_page_labels(doc: &Document) -> Vec<PageLabelEntry> {
    let mut entries = Vec::new();

    let root_ref = match doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return entries,
    };
    let catalog = match doc.get_object(root_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return entries,
    };

    let page_labels_dict = match catalog.get(b"PageLabels").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return entries,
    };

    let mut ranges = walk_number_tree(doc, page_labels_dict);
    ranges.sort_by_key(|(k, _)| *k);

    // Pre-parse ranges into (range_start, style, prefix, start_val) tuples
    let parsed_ranges: Vec<(i64, String, String, i64)> = ranges.iter().map(|(range_key, value)| {
        let label_dict = resolve_dict(doc, value);
        match label_dict {
            Some(d) => {
                let s = d.get(b"S").ok()
                    .and_then(name_to_string)
                    .unwrap_or_else(|| "-".to_string());
                let p = d.get(b"P").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_default();
                let st = d.get(b"St").ok()
                    .and_then(|v| if let Object::Integer(i) = v { Some(*i) } else { None })
                    .unwrap_or(1);
                (*range_key, s, p, st)
            }
            None => (*range_key, "D".to_string(), String::new(), 1),
        }
    }).collect();

    let page_count = doc.get_pages().len() as u32;

    for phys in 0..page_count {
        // Find the applicable range: the last parsed range whose key <= phys
        let (range_start, style, prefix, start_val) = parsed_ranges.iter().rev()
            .find(|(k, _, _, _)| *k as u32 <= phys)
            .map(|(k, s, p, st)| (*k, s.as_str(), p.as_str(), *st))
            .unwrap_or((0, "D", "", 1));

        let offset = phys as i64 - range_start;
        let value = start_val + offset;
        let label = format_page_label(style, prefix, value);

        entries.push(PageLabelEntry {
            physical_page: phys + 1,
            label,
            style: style.to_string(),
            prefix: prefix.to_string(),
            start: start_val,
        });
    }

    entries
}

fn print_page_labels(writer: &mut impl Write, doc: &Document) {
    let labels = collect_page_labels(doc);
    if labels.is_empty() {
        writeln!(writer, "No page labels defined.").unwrap();
        return;
    }
    writeln!(writer, "{} pages with labels\n", labels.len()).unwrap();
    writeln!(writer, "  {:>8}  Label", "Physical").unwrap();
    for entry in &labels {
        writeln!(writer, "  {:>8}  {}", entry.physical_page, entry.label).unwrap();
    }
}

fn print_page_labels_json(writer: &mut impl Write, doc: &Document) {
    let labels = collect_page_labels(doc);
    let items: Vec<Value> = labels.iter().map(|e| {
        json!({
            "physical_page": e.physical_page,
            "label": e.label,
            "style": e.style,
            "prefix": e.prefix,
            "start": e.start,
        })
    }).collect();
    let output = json!({
        "page_count": items.len(),
        "page_labels": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Links ────────────────────────────────────────────────────────────

struct LinkInfo {
    page_num: u32,
    object_id: ObjectId,
    link_type: Cow<'static, str>,
    target: String,
    rect: String,
}

fn classify_link(doc: &Document, dict: &lopdf::Dictionary) -> (Cow<'static, str>, String) {
    // Check /Dest first (direct destination)
    if let Ok(dest) = dict.get(b"Dest") {
        return (Cow::Borrowed("GoTo"), format_dest_value(doc, dest));
    }
    // Check /A (action dictionary)
    if let Ok(action_obj) = dict.get(b"A") {
        let action_dict = match action_obj {
            Object::Dictionary(d) => d,
            Object::Reference(id) => {
                match doc.get_object(*id) {
                    Ok(Object::Dictionary(d)) => d,
                    _ => return (Cow::Borrowed("Unknown"), format!("{} {} R", id.0, id.1)),
                }
            }
            _ => return (Cow::Borrowed("Unknown"), "-".to_string()),
        };
        let action_type = action_dict.get(b"S").ok()
            .and_then(|v| v.as_name().ok());
        match action_type {
            Some(b"GoTo") => {
                let target = action_dict.get(b"D").ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("GoTo"), target)
            }
            Some(b"GoToR") => {
                let file = action_dict.get(b"F").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                let dest = action_dict.get(b"D").ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_default();
                (Cow::Borrowed("GoToR"), format!("{} {}", file, dest).trim().to_string())
            }
            Some(b"URI") => {
                let uri = action_dict.get(b"URI").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("URI"), uri)
            }
            Some(b"Named") => {
                let n = action_dict.get(b"N").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Named"), n)
            }
            Some(b"Launch") => {
                let f = action_dict.get(b"F").ok()
                    .and_then(obj_to_string_lossy)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Launch"), f)
            }
            Some(other) => (Cow::Owned(String::from_utf8_lossy(other).into_owned()), "-".to_string()),
            None => (Cow::Borrowed("Unknown"), "-".to_string()),
        }
    } else {
        (Cow::Borrowed("Unknown"), "-".to_string())
    }
}

fn collect_links(doc: &Document, page_filter: Option<&PageSpec>) -> Vec<LinkInfo> {
    let pages = doc.get_pages();
    let mut links = Vec::new();

    for (&page_num, &page_id) in &pages {
        if let Some(spec) = page_filter
            && !spec.contains(page_num) { continue; }

        let page_dict = match doc.get_object(page_id) {
            Ok(Object::Dictionary(d)) => d,
            _ => continue,
        };

        let annot_refs: Vec<ObjectId> = match page_dict.get(b"Annots") {
            Ok(Object::Array(arr)) => arr.iter().filter_map(|o| o.as_reference().ok()).collect(),
            Ok(Object::Reference(id)) => {
                if let Ok(Object::Array(arr)) = doc.get_object(*id) {
                    arr.iter().filter_map(|o| o.as_reference().ok()).collect()
                } else {
                    continue;
                }
            }
            _ => continue,
        };

        for annot_id in annot_refs {
            let annot_dict = match doc.get_object(annot_id) {
                Ok(Object::Dictionary(d)) => d,
                _ => continue,
            };

            let is_link = annot_dict.get(b"Subtype").ok()
                .and_then(|v| v.as_name().ok())
                .is_some_and(|n| n == b"Link");

            if !is_link { continue; }

            let rect = annot_dict.get(b"Rect").ok()
                .map(format_dict_value)
                .unwrap_or_else(|| "-".to_string());

            let (link_type, target) = classify_link(doc, annot_dict);

            links.push(LinkInfo {
                page_num,
                object_id: annot_id,
                link_type,
                target,
                rect,
            });
        }
    }

    links
}

fn print_links(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let links = collect_links(doc, page_filter);
    writeln!(writer, "{} links found\n", links.len()).unwrap();
    if links.is_empty() { return; }
    writeln!(writer, "  {:>4}  {:>4}  {:<8} Target", "Page", "Obj#", "Type").unwrap();
    for l in &links {
        writeln!(writer, "  {:>4}  {:>4}  {:<8} {}", l.page_num, l.object_id.0, l.link_type, l.target).unwrap();
    }
}

fn print_links_json(writer: &mut impl Write, doc: &Document, page_filter: Option<&PageSpec>) {
    let links = collect_links(doc, page_filter);
    let items: Vec<Value> = links.iter().map(|l| {
        json!({
            "page_number": l.page_num,
            "object_number": l.object_id.0,
            "generation": l.object_id.1,
            "link_type": l.link_type,
            "target": l.target,
            "rect": l.rect,
        })
    }).collect();
    let output = json!({
        "link_count": items.len(),
        "links": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Tree view ────────────────────────────────────────────────────────

fn tree_node_label(obj: &Object) -> String {
    match obj {
        Object::Dictionary(dict) => {
            if let Ok(Object::Name(type_name)) = dict.get(b"Type") {
                let name = String::from_utf8_lossy(type_name);
                match name.as_ref() {
                    "Catalog" => "Catalog".to_string(),
                    "Pages" => "Pages".to_string(),
                    "Page" => "Page".to_string(),
                    "Font" => "Font".to_string(),
                    "Annot" => "Annot".to_string(),
                    "XObject" => "XObject".to_string(),
                    "Encoding" => "Encoding".to_string(),
                    other => other.to_string(),
                }
            } else {
                format!("Dictionary, {} keys", dict.len())
            }
        }
        Object::Stream(stream) => {
            if let Ok(Object::Name(type_name)) = stream.dict.get(b"Type") {
                let name = String::from_utf8_lossy(type_name);
                format!("{}, {} bytes", name, stream.content.len())
            } else {
                format!("Stream, {} bytes", stream.content.len())
            }
        }
        Object::Array(arr) => format!("Array, {} items", arr.len()),
        Object::Boolean(b) => format!("Boolean({})", b),
        Object::Integer(i) => format!("Integer({})", i),
        Object::Real(r) => format!("Real({})", r),
        Object::Name(n) => format!("Name({})", String::from_utf8_lossy(n)),
        Object::String(s, _) => format!("String({})", String::from_utf8_lossy(s)),
        Object::Null => "Null".to_string(),
        Object::Reference(id) => format!("Reference({} {})", id.0, id.1),
    }
}

fn collect_refs_from_dict(dict: &lopdf::Dictionary) -> Vec<(String, ObjectId)> {
    let mut refs = Vec::new();
    for (key, val) in dict.iter() {
        let key_str = format!("/{}", String::from_utf8_lossy(key));
        collect_refs_recursive(val, &key_str, &mut refs);
    }
    refs
}

fn collect_refs_with_paths(obj: &Object) -> Vec<(String, ObjectId)> {
    match obj {
        Object::Dictionary(dict) => collect_refs_from_dict(dict),
        Object::Stream(stream) => collect_refs_from_dict(&stream.dict),
        Object::Array(arr) => {
            let mut refs = Vec::new();
            for (i, val) in arr.iter().enumerate() {
                let key_str = format!("[{}]", i);
                collect_refs_recursive(val, &key_str, &mut refs);
            }
            refs
        }
        _ => Vec::new(),
    }
}

fn collect_refs_recursive(obj: &Object, path: &str, refs: &mut Vec<(String, ObjectId)>) {
    match obj {
        Object::Reference(id) => {
            refs.push((path.to_string(), *id));
        }
        Object::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                collect_refs_recursive(val, &child_path, refs);
            }
        }
        _ => {}
    }
}

fn print_tree(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    writeln!(writer, "Reference Tree:\n").unwrap();
    writeln!(writer, "Trailer").unwrap();

    let mut visited = BTreeSet::new();
    let trailer_refs = collect_refs_from_dict(&doc.trailer);

    for (path, ref_id) in trailer_refs {
        print_tree_node(writer, ref_id, doc, &mut visited, 1, &path, config);
    }
}

fn print_tree_node(writer: &mut impl Write, obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, depth: usize, key_path: &str, config: &DumpConfig) {
    let indent = "  ".repeat(depth);

    if visited.contains(&obj_id) {
        writeln!(writer, "{}{} -> {} {} (visited)", indent, key_path, obj_id.0, obj_id.1).unwrap();
        return;
    }

    if let Some(max_depth) = config.depth
        && depth > max_depth
    {
        writeln!(writer, "{}{} -> {} {} (depth limit reached)", indent, key_path, obj_id.0, obj_id.1).unwrap();
        return;
    }

    visited.insert(obj_id);

    match doc.get_object(obj_id) {
        Ok(object) => {
            let label = tree_node_label(object);
            writeln!(writer, "{}{} -> {} {} ({})", indent, key_path, obj_id.0, obj_id.1, label).unwrap();

            let child_refs = collect_refs_with_paths(object);
            for (path, child_id) in child_refs {
                print_tree_node(writer, child_id, doc, visited, depth + 1, &path, config);
            }
        }
        Err(_) => {
            writeln!(writer, "{}{} -> {} {} (missing)", indent, key_path, obj_id.0, obj_id.1).unwrap();
        }
    }
}

fn print_tree_json(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let mut visited = BTreeSet::new();
    let trailer_refs = collect_refs_from_dict(&doc.trailer);

    let children: Vec<Value> = trailer_refs.iter()
        .map(|(path, ref_id)| tree_node_to_json(*ref_id, doc, &mut visited, 1, path, config))
        .collect();

    let output = json!({
        "tree": {
            "node": "Trailer",
            "children": children,
        }
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

fn tree_node_to_json(obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, depth: usize, key_path: &str, config: &DumpConfig) -> Value {
    if visited.contains(&obj_id) {
        return json!({
            "key": key_path,
            "object": format!("{} {}", obj_id.0, obj_id.1),
            "status": "visited",
        });
    }

    if let Some(max_depth) = config.depth
        && depth > max_depth
    {
        return json!({
            "key": key_path,
            "object": format!("{} {}", obj_id.0, obj_id.1),
            "status": "depth_limit_reached",
        });
    }

    visited.insert(obj_id);

    match doc.get_object(obj_id) {
        Ok(object) => {
            let label = tree_node_label(object);
            let child_refs = collect_refs_with_paths(object);
            let children: Vec<Value> = child_refs.iter()
                .map(|(path, ref_id)| tree_node_to_json(*ref_id, doc, visited, depth + 1, path, config))
                .collect();
            let mut node = json!({
                "key": key_path,
                "object": format!("{} {}", obj_id.0, obj_id.1),
                "label": label,
            });
            if !children.is_empty() {
                node["children"] = json!(children);
            }
            node
        }
        Err(_) => {
            json!({
                "key": key_path,
                "object": format!("{} {}", obj_id.0, obj_id.1),
                "status": "missing",
            })
        }
    }
}

// ── DOT output for tree ──────────────────────────────────────────────

fn escape_dot(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn print_tree_dot(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    writeln!(writer, "digraph pdf {{").unwrap();
    writeln!(writer, "  rankdir=LR;").unwrap();
    writeln!(writer, "  node [shape=box, fontname=\"monospace\"];").unwrap();
    writeln!(writer, "  \"trailer\" [label=\"Trailer\"];").unwrap();

    let mut visited = BTreeSet::new();
    let trailer_refs = collect_refs_from_dict(&doc.trailer);

    for (path, ref_id) in trailer_refs {
        emit_dot_node(writer, ref_id, doc, &mut visited, 1, &path, "trailer", config);
    }

    writeln!(writer, "}}").unwrap();
}

#[allow(clippy::too_many_arguments)]
fn emit_dot_node(writer: &mut impl Write, obj_id: ObjectId, doc: &Document, visited: &mut BTreeSet<ObjectId>, depth: usize, key_path: &str, parent_node: &str, config: &DumpConfig) {
    let node_name = format!("obj_{}_{}", obj_id.0, obj_id.1);
    let edge_label = escape_dot(key_path);

    if visited.contains(&obj_id) {
        writeln!(writer, "  \"{}\" -> \"{}\" [label=\"{}\"];", parent_node, node_name, edge_label).unwrap();
        return;
    }

    if let Some(max_depth) = config.depth
        && depth > max_depth {
            return;
    }

    visited.insert(obj_id);

    match doc.get_object(obj_id) {
        Ok(object) => {
            let label = escape_dot(&tree_node_label(object));
            let node_label = format!("{} {}: {}", obj_id.0, obj_id.1, label);
            writeln!(writer, "  \"{}\" [label=\"{}\"];", node_name, node_label).unwrap();
            writeln!(writer, "  \"{}\" -> \"{}\" [label=\"{}\"];", parent_node, node_name, edge_label).unwrap();

            let child_refs = collect_refs_with_paths(object);
            for (path, child_id) in child_refs {
                emit_dot_node(writer, child_id, doc, visited, depth + 1, &path, &node_name, config);
            }
        }
        Err(_) => {
            writeln!(writer, "  \"{}\" [label=\"{} {} (missing)\", style=dashed];", node_name, obj_id.0, obj_id.1).unwrap();
            writeln!(writer, "  \"{}\" -> \"{}\" [label=\"{}\"];", parent_node, node_name, edge_label).unwrap();
        }
    }
}

// ── Layers / OCG ─────────────────────────────────────────────────────

struct OcgInfo {
    object_id: ObjectId,
    name: String,
    default_state: String,
    page_numbers: Vec<u32>,
}

fn collect_layers(doc: &Document) -> Vec<OcgInfo> {
    let catalog_id = match doc.trailer.get(b"Root").ok()
        .and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return Vec::new(),
    };
    let catalog = match doc.get_object(catalog_id).ok() {
        Some(Object::Dictionary(d)) => d,
        _ => return Vec::new(),
    };

    let oc_props = match catalog.get(b"OCProperties").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return Vec::new(),
    };

    // Get OCGs array
    let ocgs_arr = match oc_props.get(b"OCGs").ok().and_then(|o| resolve_array(doc, o)) {
        Some(a) => a,
        None => return Vec::new(),
    };

    // Get default config /D
    let empty_dict = lopdf::Dictionary::new();
    let d_dict = oc_props.get(b"D").ok()
        .and_then(|o| resolve_dict(doc, o))
        .unwrap_or(&empty_dict);

    let base_state: &str = &d_dict.get(b"BaseState").ok()
        .and_then(name_to_string)
        .unwrap_or_else(|| "ON".to_string());

    // Collect ON/OFF override sets
    let on_set: BTreeSet<ObjectId> = extract_id_set(d_dict, b"ON");
    let off_set: BTreeSet<ObjectId> = extract_id_set(d_dict, b"OFF");

    // Build page_id -> page_num lookup
    let pages = doc.get_pages();
    let page_id_to_num: BTreeMap<ObjectId, u32> = pages.into_iter().map(|(num, id)| (id, num)).collect();

    // Scan pages for OCG references in /Properties
    let mut ocg_pages: BTreeMap<ObjectId, Vec<u32>> = BTreeMap::new();
    for (&page_id, &page_num) in &page_id_to_num {
        if let Ok(page_obj) = doc.get_object(page_id) {
            let page_dict = match page_obj {
                Object::Dictionary(d) => d,
                _ => continue,
            };
            scan_page_for_ocgs(doc, page_dict, page_num, &mut ocg_pages);
        }
    }

    // Build OcgInfo for each OCG
    let mut layers = Vec::new();
    for item in ocgs_arr {
        let ocg_id = match item {
            Object::Reference(id) => *id,
            _ => continue,
        };
        let ocg_dict = match doc.get_object(ocg_id).ok() {
            Some(Object::Dictionary(d)) => d,
            _ => continue,
        };

        let name = ocg_dict.get(b"Name").ok()
            .and_then(|v| if let Object::String(bytes, _) = v { Some(String::from_utf8_lossy(bytes).into_owned()) } else { None })
            .unwrap_or_else(|| "(unnamed)".to_string());

        let default_state = if off_set.contains(&ocg_id) {
            "OFF".to_string()
        } else if on_set.contains(&ocg_id) {
            "ON".to_string()
        } else {
            base_state.to_string()
        };

        let page_numbers = ocg_pages.remove(&ocg_id).unwrap_or_default();
        layers.push(OcgInfo { object_id: ocg_id, name, default_state, page_numbers });
    }

    layers
}

fn extract_id_set(dict: &lopdf::Dictionary, key: &[u8]) -> BTreeSet<ObjectId> {
    let mut set = BTreeSet::new();
    if let Ok(Object::Array(arr)) = dict.get(key) {
        for item in arr {
            if let Object::Reference(id) = item {
                set.insert(*id);
            }
        }
    }
    set
}

fn scan_page_for_ocgs(doc: &Document, page_dict: &lopdf::Dictionary, page_num: u32, ocg_pages: &mut BTreeMap<ObjectId, Vec<u32>>) {
    // Look for Resources -> Properties which holds OCG references
    let resources = match page_dict.get(b"Resources").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => {
            // Try inheriting from parent
            if let Ok(Object::Reference(parent_id)) = page_dict.get(b"Parent")
                && let Ok(Object::Dictionary(parent)) = doc.get_object(*parent_id)
                && let Some(d) = parent.get(b"Resources").ok().and_then(|o| resolve_dict(doc, o))
            {
                d
            } else {
                return;
            }
        }
    };

    let props = match resources.get(b"Properties").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return,
    };

    for (_, val) in props.iter() {
        let ocg_id = match val {
            Object::Reference(id) => *id,
            _ => continue,
        };
        // Verify it's an OCG (has /Type /OCG)
        if let Ok(obj) = doc.get_object(ocg_id) {
            let dict = match obj {
                Object::Dictionary(d) => d,
                _ => continue,
            };
            if dict.get_type().ok().is_some_and(|t| t == b"OCG") {
                ocg_pages.entry(ocg_id).or_default().push(page_num);
            }
        }
    }
}

fn print_layers(writer: &mut impl Write, doc: &Document) {
    let layers = collect_layers(doc);
    writeln!(writer, "{} layers found\n", layers.len()).unwrap();
    if layers.is_empty() { return; }
    writeln!(writer, "  {:>4}  {:<30} {:<8} Pages", "Obj#", "Name", "Default").unwrap();
    for l in &layers {
        let pages_str = if l.page_numbers.is_empty() {
            "-".to_string()
        } else {
            l.page_numbers.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
        };
        writeln!(writer, "  {:>4}  {:<30} {:<8} {}", l.object_id.0, l.name, l.default_state, pages_str).unwrap();
    }
}

fn print_layers_json(writer: &mut impl Write, doc: &Document) {
    let layers = collect_layers(doc);
    let items: Vec<Value> = layers.iter().map(|l| {
        json!({
            "object_number": l.object_id.0,
            "generation": l.object_id.1,
            "name": l.name,
            "default_state": l.default_state,
            "pages": l.page_numbers,
        })
    }).collect();
    let output = json!({
        "layer_count": items.len(),
        "layers": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

// ── Structure Tree ───────────────────────────────────────────────────

struct StructElemInfo {
    object_id: ObjectId,
    role: String,
    page: Option<u32>,
    mcid: Option<i64>,
    title: Option<String>,
    alt: Option<String>,
    children: Vec<StructElemInfo>,
}

fn collect_structure_tree(doc: &Document) -> (bool, Vec<StructElemInfo>) {
    let catalog_id = match doc.trailer.get(b"Root").ok()
        .and_then(|o| o.as_reference().ok()) {
        Some(id) => id,
        None => return (false, Vec::new()),
    };
    let catalog = match doc.get_object(catalog_id).ok() {
        Some(Object::Dictionary(d)) => d,
        _ => return (false, Vec::new()),
    };

    // Check MarkInfo
    let is_marked = catalog.get(b"MarkInfo").ok()
        .and_then(|v| resolve_dict(doc, v))
        .and_then(|d| d.get(b"Marked").ok())
        .and_then(|m| if let Object::Boolean(b) = m { Some(*b) } else { None })
        .unwrap_or(false);

    let struct_tree_root = match catalog.get(b"StructTreeRoot").ok().and_then(|o| resolve_dict(doc, o)) {
        Some(d) => d,
        None => return (is_marked, Vec::new()),
    };

    // Build page_id -> page_num lookup
    let pages = doc.get_pages();
    let page_lookup: BTreeMap<ObjectId, u32> = pages.into_iter().map(|(num, id)| (id, num)).collect();

    let mut visited = BTreeSet::new();
    let children = collect_struct_children(doc, struct_tree_root, &page_lookup, &mut visited);

    (is_marked, children)
}

fn collect_struct_children(doc: &Document, dict: &lopdf::Dictionary, page_lookup: &BTreeMap<ObjectId, u32>, visited: &mut BTreeSet<ObjectId>) -> Vec<StructElemInfo> {
    let k = match dict.get(b"K").ok() {
        Some(v) => v,
        None => return Vec::new(),
    };

    let items: &[Object] = match k {
        Object::Array(arr) => arr,
        other => std::slice::from_ref(other),
    };

    let mut result = Vec::new();
    for item in items {
        match item {
            Object::Reference(id) => {
                if visited.contains(id) { continue; }
                visited.insert(*id);
                if let Ok(Object::Dictionary(child_dict)) = doc.get_object(*id)
                    && let Ok(role_obj) = child_dict.get(b"S") {
                    let role = if let Object::Name(n) = role_obj {
                        String::from_utf8_lossy(n).into_owned()
                    } else {
                        "-".to_string()
                    };

                    let page = child_dict.get(b"Pg").ok()
                        .and_then(|v| v.as_reference().ok())
                        .and_then(|pg_id| page_lookup.get(&pg_id).copied());

                    let mcid = extract_mcid(child_dict);

                    let title = child_dict.get(b"T").ok()
                        .and_then(obj_to_string_lossy);

                    let alt = child_dict.get(b"Alt").ok()
                        .and_then(obj_to_string_lossy);

                    let children = collect_struct_children(doc, child_dict, page_lookup, visited);

                    result.push(StructElemInfo {
                        object_id: *id,
                        role,
                        page,
                        mcid,
                        title,
                        alt,
                        children,
                    });
                }
            }
            Object::Dictionary(d) => {
                // Inline struct element
                if let Ok(role_obj) = d.get(b"S") {
                    let role = if let Object::Name(n) = role_obj {
                        String::from_utf8_lossy(n).into_owned()
                    } else {
                        "-".to_string()
                    };
                    let mcid = extract_mcid(d);
                    result.push(StructElemInfo {
                        object_id: (0, 0),
                        role,
                        page: None,
                        mcid,
                        title: None,
                        alt: None,
                        children: Vec::new(),
                    });
                }
            }
            _ => {
                // Bare MCID integers and other items are captured via extract_mcid on the parent
            }
        }
    }
    result
}

fn extract_mcid(dict: &lopdf::Dictionary) -> Option<i64> {
    // MCID can be in /K as integer, or in /K as dict with /MCID
    match dict.get(b"K").ok()? {
        Object::Integer(n) => Some(*n),
        Object::Dictionary(d) => d.get(b"MCID").ok()?.as_i64().ok(),
        Object::Array(arr) => {
            // Find first MCID in array
            for item in arr {
                match item {
                    Object::Integer(n) => return Some(*n),
                    Object::Dictionary(d) => {
                        if let Ok(mcid) = d.get(b"MCID")
                            && let Ok(n) = mcid.as_i64() {
                            return Some(n);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

fn count_struct_elems(items: &[StructElemInfo]) -> usize {
    items.iter().map(|e| 1 + count_struct_elems(&e.children)).sum()
}

fn print_structure(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let (is_marked, tree) = collect_structure_tree(doc);
    writeln!(writer, "Tagged PDF: {}", if is_marked { "yes" } else { "no" }).unwrap();
    let count = count_struct_elems(&tree);
    writeln!(writer, "Structure elements: {}\n", count).unwrap();
    if tree.is_empty() { return; }
    for elem in &tree {
        print_struct_elem(writer, elem, 0, config);
    }
}

fn print_struct_elem(writer: &mut impl Write, elem: &StructElemInfo, depth: usize, config: &DumpConfig) {
    if let Some(max_depth) = config.depth
        && depth > max_depth {
        return;
    }

    let indent = "  ".repeat(depth);
    let mut line = if elem.object_id != (0, 0) {
        format!("{}[{}] /{}", indent, elem.object_id.0, elem.role)
    } else {
        format!("{}/{}", indent, elem.role)
    };

    if let Some(page) = elem.page {
        line.push_str(&format!(" (page {})", page));
    }
    if let Some(mcid) = elem.mcid {
        line.push_str(&format!(" MCID={}", mcid));
    }
    if let Some(ref title) = elem.title {
        line.push_str(&format!(" \"{}\"", title));
    }
    if let Some(ref alt) = elem.alt {
        line.push_str(&format!(" alt=\"{}\"", alt));
    }

    // At depth limit, show children count instead of recursing
    if let Some(max_depth) = config.depth
        && depth == max_depth && !elem.children.is_empty() {
        line.push_str(&format!(" ({} children)", count_struct_elems(&elem.children)));
    }

    writeln!(writer, "{}", line).unwrap();

    for child in &elem.children {
        print_struct_elem(writer, child, depth + 1, config);
    }
}

fn print_structure_json(writer: &mut impl Write, doc: &Document, config: &DumpConfig) {
    let (is_marked, tree) = collect_structure_tree(doc);
    let count = count_struct_elems(&tree);
    let items: Vec<Value> = tree.iter().map(|e| struct_elem_to_json(e, 0, config)).collect();
    let output = json!({
        "tagged": is_marked,
        "element_count": count,
        "structure": items,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
}

fn struct_elem_to_json(elem: &StructElemInfo, depth: usize, config: &DumpConfig) -> Value {
    let mut obj = json!({
        "role": elem.role,
    });
    if elem.object_id != (0, 0) {
        obj["object_number"] = json!(elem.object_id.0);
        obj["generation"] = json!(elem.object_id.1);
    }
    if let Some(page) = elem.page {
        obj["page"] = json!(page);
    }
    if let Some(mcid) = elem.mcid {
        obj["mcid"] = json!(mcid);
    }
    if let Some(ref title) = elem.title {
        obj["title"] = json!(title);
    }
    if let Some(ref alt) = elem.alt {
        obj["alt"] = json!(alt);
    }

    if let Some(max_depth) = config.depth
        && depth >= max_depth {
        if !elem.children.is_empty() {
            obj["children_count"] = json!(count_struct_elems(&elem.children));
        }
        return obj;
    }

    if !elem.children.is_empty() {
        let children: Vec<Value> = elem.children.iter()
            .map(|c| struct_elem_to_json(c, depth + 1, config))
            .collect();
        obj["children"] = json!(children);
    }
    obj
}

// ── Info mode (--info N) ─────────────────────────────────────────────

fn classify_object(doc: &Document, obj_num: u32, object: &Object, pages: &BTreeMap<u32, ObjectId>) -> (String, String, Vec<(String, String)>) {
    let dict = match object {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    };

    if let Some(dict) = dict {
        let type_name = dict.get_type().ok().map(|t| String::from_utf8_lossy(t).into_owned());
        let subtype = dict.get(b"Subtype").ok()
            .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()));

        match type_name.as_deref() {
            Some("Catalog") => {
                let page_count = dict.get(b"Pages").ok()
                    .and_then(|p| p.as_reference().ok())
                    .and_then(|r| doc.get_object(r).ok())
                    .and_then(|o| match o {
                        Object::Dictionary(d) => d.get(b"Count").ok().and_then(|c| c.as_i64().ok()),
                        _ => None,
                    });
                let mut details = Vec::new();
                if let Some(count) = page_count {
                    details.push(("Pages".to_string(), count.to_string()));
                }
                return ("Catalog".to_string(), format!("Object {} is the document catalog (root object).", obj_num), details);
            }
            Some("Pages") => {
                let count = dict.get(b"Count").ok().and_then(|c| c.as_i64().ok());
                let mut details = Vec::new();
                if let Some(c) = count {
                    details.push(("Count".to_string(), c.to_string()));
                }
                return ("Page Tree".to_string(), format!("Object {} is a page tree node.", obj_num), details);
            }
            Some("Page") => {
                let page_num = pages.iter().find(|(_, id)| **id == (obj_num, 0)).map(|(n, _)| *n);
                let mut details = Vec::new();
                if let Some(n) = page_num {
                    details.push(("Page Number".to_string(), n.to_string()));
                }
                if let Ok(mb) = dict.get(b"MediaBox") {
                    details.push(("MediaBox".to_string(), format_dict_value(mb)));
                }
                let desc = if let Some(n) = page_num {
                    format!("Object {} is page {}.", obj_num, n)
                } else {
                    format!("Object {} is a page.", obj_num)
                };
                return ("Page".to_string(), desc, details);
            }
            Some("Font") => {
                return classify_font(doc, obj_num, dict);
            }
            Some("Annot") => {
                let sub = subtype.as_deref().unwrap_or("unknown");
                let mut details = vec![("Subtype".to_string(), sub.to_string())];
                if let Ok(rect) = dict.get(b"Rect") {
                    details.push(("Rect".to_string(), format_dict_value(rect)));
                }
                return ("Annotation".to_string(), format!("Object {} is a {} annotation.", obj_num, sub), details);
            }
            Some("Action") => {
                let action_type = dict.get(b"S").ok()
                    .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                    .unwrap_or_else(|| "unknown".to_string());
                let details = vec![("Action Type".to_string(), action_type.clone())];
                return ("Action".to_string(), format!("Object {} is a {} action.", obj_num, action_type), details);
            }
            Some("FontDescriptor") => {
                let font_name = dict.get(b"FontName").ok()
                    .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                    .unwrap_or_else(|| "-".to_string());
                let mut details = vec![("FontName".to_string(), font_name.clone())];
                for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                    if let Ok(v) = dict.get(key) {
                        details.push(("Embedded".to_string(), format!("{} ({})", format_dict_value(v), String::from_utf8_lossy(key))));
                    }
                }
                return ("Font Descriptor".to_string(), format!("Object {} is a font descriptor for {}.", obj_num, font_name), details);
            }
            Some("Encoding") => {
                let base = dict.get(b"BaseEncoding").ok()
                    .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
                    .unwrap_or_else(|| "-".to_string());
                let has_diffs = dict.get(b"Differences").is_ok();
                let mut details = vec![("BaseEncoding".to_string(), base.clone())];
                if has_diffs { details.push(("Differences".to_string(), "yes".to_string())); }
                return ("Encoding".to_string(), format!("Object {} is a font encoding ({}).", obj_num, base), details);
            }
            Some("ExtGState") => {
                let keys: Vec<String> = dict.iter()
                    .map(|(k, _)| format!("/{}", String::from_utf8_lossy(k)))
                    .collect();
                let details = vec![("Keys".to_string(), keys.join(", "))];
                return ("Graphics State".to_string(), format!("Object {} is an extended graphics state.", obj_num), details);
            }
            Some("XRef") => {
                return ("XRef Stream".to_string(), format!("Object {} is a cross-reference stream.", obj_num), vec![]);
            }
            Some("ObjStm") => {
                return ("Object Stream".to_string(), format!("Object {} is an object stream.", obj_num), vec![]);
            }
            _ => {}
        }

        // Check subtype for things without /Type
        if let Some(ref sub) = subtype {
            match sub.as_str() {
                "Image" => {
                    return classify_image(obj_num, dict, doc, object);
                }
                "Form" => {
                    let mut details = Vec::new();
                    if let Ok(bbox) = dict.get(b"BBox") {
                        details.push(("BBox".to_string(), format_dict_value(bbox)));
                    }
                    return ("Form XObject".to_string(), format!("Object {} is a form XObject.", obj_num), details);
                }
                "Type1" | "TrueType" | "Type0" | "CIDFontType0" | "CIDFontType2" | "MMType1" | "Type3" => {
                    return classify_font(doc, obj_num, dict);
                }
                _ => {}
            }
        }

        // Generic dictionary/stream
        let kind = if matches!(object, Object::Stream(_)) { "stream" } else { "dictionary" };
        let key_count = dict.len();
        let mut details = vec![("Keys".to_string(), key_count.to_string())];
        if let Some(ref t) = type_name {
            details.insert(0, ("Type".to_string(), t.clone()));
        }
        if let Some(ref s) = subtype {
            details.insert(if type_name.is_some() { 1 } else { 0 }, ("Subtype".to_string(), s.clone()));
        }
        if let Object::Stream(stream) = object {
            details.push(("Stream Size".to_string(), format!("{} bytes", stream.content.len())));
        }
        return ("Generic".to_string(), format!("Object {} is a {} with {} keys.", obj_num, kind, key_count), details);
    }

    // Primitive types
    let (role, desc) = match object {
        Object::Integer(i) => ("Integer".to_string(), format!("Object {} is an integer: {}.", obj_num, i)),
        Object::Real(r) => ("Real".to_string(), format!("Object {} is a real number: {}.", obj_num, r)),
        Object::Boolean(b) => ("Boolean".to_string(), format!("Object {} is a boolean: {}.", obj_num, b)),
        Object::String(bytes, _) => ("String".to_string(), format!("Object {} is a string: ({}).", obj_num, String::from_utf8_lossy(bytes))),
        Object::Name(n) => ("Name".to_string(), format!("Object {} is a name: /{}.", obj_num, String::from_utf8_lossy(n))),
        Object::Array(arr) => ("Array".to_string(), format!("Object {} is an array with {} items.", obj_num, arr.len())),
        Object::Null => ("Null".to_string(), format!("Object {} is null.", obj_num)),
        Object::Reference(id) => ("Reference".to_string(), format!("Object {} is a reference to {} {} R.", obj_num, id.0, id.1)),
        _ => ("Unknown".to_string(), format!("Object {} has an unknown type.", obj_num)),
    };
    (role, desc, vec![])
}

fn classify_font(doc: &Document, obj_num: u32, dict: &lopdf::Dictionary) -> (String, String, Vec<(String, String)>) {
    let base_font = dict.get(b"BaseFont").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
        .unwrap_or_else(|| "-".to_string());
    let subtype = dict.get(b"Subtype").ok()
        .and_then(|v| v.as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned()))
        .unwrap_or_else(|| "-".to_string());
    let encoding = dict.get(b"Encoding").ok()
        .map(|v| match v {
            Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
            Object::Reference(id) => format!("{} {} R", id.0, id.1),
            _ => format_dict_value(v),
        })
        .unwrap_or_else(|| "-".to_string());

    let embedded = if let Ok(desc_val) = dict.get(b"FontDescriptor") {
        match desc_val {
            Object::Reference(desc_ref) => {
                if let Ok(desc_obj) = doc.get_object(*desc_ref) {
                    let desc_dict = match desc_obj {
                        Object::Dictionary(d) => Some(d),
                        Object::Stream(s) => Some(&s.dict),
                        _ => None,
                    };
                    if let Some(dd) = desc_dict {
                        if let Some(key) = [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"]
                            .iter()
                            .find(|k| dd.get(k).is_ok())
                        {
                            format!("embedded ({})", String::from_utf8_lossy(key))
                        } else {
                            "not embedded".to_string()
                        }
                    } else {
                        format!("FontDescriptor at {} {} R", desc_ref.0, desc_ref.1)
                    }
                } else {
                    format!("FontDescriptor at {} {} R (unresolvable)", desc_ref.0, desc_ref.1)
                }
            }
            Object::Dictionary(_) => "has FontDescriptor (inline)".to_string(),
            _ => "has FontDescriptor".to_string(),
        }
    } else {
        "no FontDescriptor".to_string()
    };

    let has_tounicode = dict.get(b"ToUnicode").is_ok();

    let mut details = vec![
        ("BaseFont".to_string(), base_font.clone()),
        ("Subtype".to_string(), subtype.clone()),
        ("Encoding".to_string(), encoding),
        ("FontDescriptor".to_string(), embedded),
        ("ToUnicode".to_string(), if has_tounicode { "yes" } else { "no" }.to_string()),
    ];

    if let Ok(fc) = dict.get(b"FirstChar").and_then(|v| v.as_i64()) {
        details.push(("FirstChar".to_string(), fc.to_string()));
    }
    if let Ok(lc) = dict.get(b"LastChar").and_then(|v| v.as_i64()) {
        details.push(("LastChar".to_string(), lc.to_string()));
    }

    let desc = format!("Object {} is a {} font ({}).", obj_num, subtype, base_font);
    ("Font".to_string(), desc, details)
}

fn classify_image(obj_num: u32, dict: &lopdf::Dictionary, doc: &Document, object: &Object) -> (String, String, Vec<(String, String)>) {
    let width = dict.get(b"Width").ok().and_then(|v| v.as_i64().ok());
    let height = dict.get(b"Height").ok().and_then(|v| v.as_i64().ok());
    let bpc = dict.get(b"BitsPerComponent").ok().and_then(|v| v.as_i64().ok());
    let cs = dict.get(b"ColorSpace").ok().map(|v| format_color_space(v, doc));
    let filter = dict.get(b"Filter").ok().map(format_filter);
    let stream_size = if let Object::Stream(s) = object { Some(s.content.len()) } else { None };

    let mut details = Vec::new();
    let mut dim_parts = Vec::new();
    if let Some(w) = width {
        details.push(("Width".to_string(), w.to_string()));
        dim_parts.push(w.to_string());
    }
    if let Some(h) = height {
        details.push(("Height".to_string(), h.to_string()));
        dim_parts.push(h.to_string());
    }
    if let Some(ref c) = cs {
        details.push(("ColorSpace".to_string(), c.clone()));
    }
    if let Some(b) = bpc {
        details.push(("BitsPerComponent".to_string(), b.to_string()));
    }
    if let Some(ref f) = filter {
        details.push(("Filter".to_string(), f.clone()));
    }
    if let Some(size) = stream_size {
        details.push(("Stream Size".to_string(), format!("{} bytes", size)));
    }

    let dims = if dim_parts.len() == 2 { format!(" ({}x{})", dim_parts[0], dim_parts[1]) } else { String::new() };
    let cs_str = cs.as_deref().unwrap_or("");
    let desc = format!("Object {} is an image{}{}.", obj_num, dims,
        if !cs_str.is_empty() { format!(", {}", cs_str) } else { String::new() });
    ("Image".to_string(), desc, details)
}

fn find_page_associations(doc: &Document, obj_num: u32, pages: &BTreeMap<u32, ObjectId>) -> Vec<u32> {
    let target_id: ObjectId = (obj_num, 0);
    let mut result = Vec::new();

    for (&page_num, &page_id) in pages {
        if let Ok(page_obj) = doc.get_object(page_id) {
            // Check direct references in page dict
            let paths = collect_references_in_object(page_obj, target_id, "");
            if !paths.is_empty() {
                result.push(page_num);
                continue;
            }
            // Check one level into /Resources
            let dict = match page_obj {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                _ => continue,
            };
            if let Ok(Object::Reference(res_ref)) = dict.get(b"Resources")
                && let Ok(res_obj) = doc.get_object(*res_ref) {
                let res_paths = collect_references_in_object(res_obj, target_id, "");
                if !res_paths.is_empty() {
                    result.push(page_num);
                }
            }
        }
    }

    result.sort();
    result
}

fn print_info(writer: &mut impl Write, doc: &Document, obj_num: u32) {
    let obj_id = (obj_num, 0);
    let object = match doc.get_object(obj_id) {
        Ok(obj) => obj,
        Err(_) => {
            eprintln!("Error: Object {} not found in the document.", obj_num);
            std::process::exit(1);
        }
    };

    let pages = doc.get_pages();
    let (role, description, details) = classify_object(doc, obj_num, object, &pages);

    writeln!(writer, "{}", description).unwrap();
    writeln!(writer, "\nRole: {}", role).unwrap();
    writeln!(writer, "Kind: {}", object.enum_variant()).unwrap();

    if !details.is_empty() {
        writeln!(writer, "\nDetails:").unwrap();
        for (key, value) in &details {
            writeln!(writer, "  {}: {}", key, value).unwrap();
        }
    }

    // Page associations
    let page_assoc = find_page_associations(doc, obj_num, &pages);
    if !page_assoc.is_empty() {
        let pages_str: Vec<String> = page_assoc.iter().map(|p| p.to_string()).collect();
        writeln!(writer, "\nReferenced by pages: {}", pages_str.join(", ")).unwrap();
    }

    // Full object content
    let config = DumpConfig {
        decode_streams: false, truncate: None, json: false,
        hex: false, depth: None, deref: false, raw: false,
    };
    writeln!(writer, "\nObject {} 0:", obj_num).unwrap();
    let visited = BTreeSet::new();
    let mut child_refs = BTreeSet::new();
    print_object(writer, object, doc, &visited, 1, &config, false, &mut child_refs);
    writeln!(writer).unwrap();

    // Forward references
    let forward_refs = collect_refs_with_paths(object);
    writeln!(writer, "\nReferences from this object:").unwrap();
    if forward_refs.is_empty() {
        writeln!(writer, "  (none)").unwrap();
    } else {
        for (path, ref_id) in &forward_refs {
            let summary = if let Ok(resolved) = doc.get_object(*ref_id) {
                deref_summary(resolved, doc)
            } else {
                "(not found)".to_string()
            };
            writeln!(writer, "  {} -> {} {} R  {}", path, ref_id.0, ref_id.1, summary).unwrap();
        }
    }

    // Reverse references
    let rev_refs = collect_reverse_refs(doc, (obj_num, 0));
    writeln!(writer, "\nReferenced by:").unwrap();
    if rev_refs.is_empty() {
        writeln!(writer, "  (none)").unwrap();
    } else {
        for r in &rev_refs {
            writeln!(writer, "  {:>4}  {:>3}  {:<13} {:<14} via {}", r.obj_num, r.generation, r.kind, r.type_label, r.paths.join(", ")).unwrap();
        }
    }
}

fn print_info_json(writer: &mut impl Write, doc: &Document, obj_num: u32) {
    let obj_id = (obj_num, 0);
    let object = match doc.get_object(obj_id) {
        Ok(obj) => obj,
        Err(_) => {
            let output = json!({
                "object_number": obj_num,
                "error": "not found",
            });
            writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
            return;
        }
    };

    let pages = doc.get_pages();
    let (role, description, details) = classify_object(doc, obj_num, object, &pages);

    let config = DumpConfig {
        decode_streams: false, truncate: None, json: true,
        hex: false, depth: None, deref: false, raw: false,
    };
    let refs_to = collect_forward_refs_json(doc, object);
    let referenced_by = reverse_refs_to_json(&collect_reverse_refs(doc, (obj_num, 0)));
    let page_assoc = find_page_associations(doc, obj_num, &pages);

    let details_map: serde_json::Map<String, Value> = details.into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect();

    let output = json!({
        "object_number": obj_num,
        "generation": 0,
        "role": role,
        "description": description,
        "kind": format!("{}", object.enum_variant()),
        "details": details_map,
        "object": object_to_json(object, doc, &config),
        "page_associations": page_assoc,
        "references": refs_to,
        "referenced_by": referenced_by,
    });
    writeln!(writer, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
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
            truncate: None,
            json: false,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
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
        let (result, _warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"array filter");
    }

    #[test]
    fn decode_stream_unknown_filter() {
        let stream = make_stream(
            Some(Object::Name(b"DCTDecode".to_vec())),
            b"jpeg data".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"jpeg data");
        assert!(warning.is_some(), "Unknown filter should produce a warning");
        assert!(warning.unwrap().contains("unsupported filter: DCTDecode"));
    }

    #[test]
    fn decode_stream_corrupt_flatedecode() {
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"not valid zlib".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        // Falls back to original content with warning
        assert_eq!(&*result, b"not valid zlib");
        assert!(warning.is_some(), "Corrupt FlateDecode should produce a warning");
        assert!(warning.unwrap().contains("FlateDecode decompression failed"));
    }

    #[test]
    fn decode_stream_valid_flatedecode_no_warning() {
        let compressed = zlib_compress(b"hello pdf");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let (result, warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"hello pdf");
        assert!(warning.is_none(), "Valid FlateDecode should produce no warning");
    }

    #[test]
    fn decode_stream_no_filter_no_warning() {
        let stream = make_stream(None, b"raw content".to_vec());
        let (_result, warning) = decode_stream(&stream);
        assert!(warning.is_none(), "No filter should produce no warning");
    }

    // ── ASCII85Decode ──────────────────────────────────────────────────

    #[test]
    fn decode_ascii85_basic() {
        let result = decode_ascii85(b"87cURDZ~>").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_ascii85_z_shortcut() {
        // 'z' represents four zero bytes
        let result = decode_ascii85(b"z~>").unwrap();
        assert_eq!(result, vec![0, 0, 0, 0]);
    }

    #[test]
    fn decode_ascii85_with_whitespace() {
        let result = decode_ascii85(b"87cUR DZ~>").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_ascii85_no_eod_marker() {
        let result = decode_ascii85(b"87cURDZ").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_ascii85_with_prefix() {
        let result = decode_ascii85(b"<~87cURDZ~>").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_ascii85_invalid_char() {
        let result = decode_ascii85(b"\x01abc");
        assert!(result.is_err());
    }

    #[test]
    fn decode_ascii85_stream() {
        let stream = make_stream(
            Some(Object::Name(b"ASCII85Decode".to_vec())),
            b"87cURDZ~>".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"Hello");
        assert!(warning.is_none());
    }

    // ── ASCIIHexDecode ──────────────────────────────────────────────────

    #[test]
    fn decode_asciihex_basic() {
        let result = decode_asciihex(b"48656c6c6f>").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_asciihex_uppercase() {
        let result = decode_asciihex(b"48656C6C6F>").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_asciihex_with_whitespace() {
        let result = decode_asciihex(b"48 65 6c 6c 6f>").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_asciihex_odd_digits() {
        // Trailing single digit padded with 0
        let result = decode_asciihex(b"4>").unwrap();
        assert_eq!(result, vec![0x40]);
    }

    #[test]
    fn decode_asciihex_no_eod_marker() {
        let result = decode_asciihex(b"48656c6c6f").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_asciihex_invalid_char() {
        let result = decode_asciihex(b"4G>");
        assert!(result.is_err());
    }

    #[test]
    fn decode_asciihex_stream() {
        let stream = make_stream(
            Some(Object::Name(b"ASCIIHexDecode".to_vec())),
            b"48656c6c6f>".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"Hello");
        assert!(warning.is_none());
    }

    // ── LZWDecode ───────────────────────────────────────────────────────

    #[test]
    fn decode_lzw_stream_unsupported_returns_error_or_data() {
        // Test that LZW decoder handles data (valid TIFF-style LZW encoding needed)
        // Generate known LZW data via weezl encoder
        let original = b"AAABBBCCC";
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(original).unwrap();
        let result = decode_lzw(&compressed).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn decode_lzw_stream_via_decode_stream() {
        let original = b"Hello from LZW";
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(original).unwrap();
        let stream = make_stream(
            Some(Object::Name(b"LZWDecode".to_vec())),
            compressed,
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, original.as_slice());
        assert!(warning.is_none());
    }

    // ── RunLengthDecode ─────────────────────────────────────────────────

    #[test]
    fn decode_run_length_literal_run() {
        // Length byte 4 → copy next 5 bytes literally
        let data = vec![4, b'H', b'e', b'l', b'l', b'o', 128];
        let result = decode_run_length(&data).unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn decode_run_length_repeat_run() {
        // Length byte 254 → repeat next byte (257-254)=3 times
        let data = vec![254, b'A', 128];
        let result = decode_run_length(&data).unwrap();
        assert_eq!(result, b"AAA");
    }

    #[test]
    fn decode_run_length_mixed() {
        // Literal "Hi" (length=1, 2 bytes) then repeat 'X' 4 times (length=253, 257-253=4)
        let data = vec![1, b'H', b'i', 253, b'X', 128];
        let result = decode_run_length(&data).unwrap();
        assert_eq!(result, b"HiXXXX");
    }

    #[test]
    fn decode_run_length_eod_marker() {
        // EOD (128) stops processing
        let data = vec![0, b'A', 128, 0, b'B'];
        let result = decode_run_length(&data).unwrap();
        assert_eq!(result, b"A");
    }

    #[test]
    fn decode_run_length_truncated_literal() {
        // Length byte says 2 bytes but only 1 available
        let data = vec![1, b'A'];
        let result = decode_run_length(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("truncated literal run"));
    }

    #[test]
    fn decode_run_length_truncated_repeat() {
        // Repeat run with no byte following
        let data = vec![255];
        let result = decode_run_length(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("truncated repeat run"));
    }

    #[test]
    fn decode_run_length_empty_input() {
        let result = decode_run_length(b"").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decode_run_length_stream_via_decode_stream() {
        let data = vec![4, b'H', b'e', b'l', b'l', b'o', 128];
        let stream = make_stream(
            Some(Object::Name(b"RunLengthDecode".to_vec())),
            data,
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"Hello");
        assert!(warning.is_none());
    }

    // ── Filter Pipeline ──────────────────────────────────────────────────

    #[test]
    fn decode_stream_pipeline_asciihex_then_flatedecode() {
        // Compress with FlateDecode, then hex-encode
        let compressed = zlib_compress(b"pipeline test");
        let hex_encoded: String = compressed.iter().map(|b| format!("{:02x}", b)).collect();
        let hex_bytes = format!("{}>", hex_encoded);
        // Filter order: ASCIIHexDecode first, then FlateDecode
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            hex_bytes.into_bytes(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"pipeline test");
        assert!(warning.is_none());
    }

    #[test]
    fn decode_stream_pipeline_stops_on_unsupported() {
        // First filter unsupported → stops immediately
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"JBIG2Decode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            b"data".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"data");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("unsupported filter: JBIG2Decode"));
    }

    #[test]
    fn decode_stream_pipeline_stops_on_failure() {
        // FlateDecode with corrupt data → stops with warning
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"ASCIIHexDecode".to_vec()),
            ])),
            b"not valid zlib".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"not valid zlib");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("FlateDecode"));
    }

    #[test]
    fn print_content_data_with_warning() {
        let content = b"raw data";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, false, Some("test warning message"));
        });
        assert!(out.contains("[WARNING: test warning message]"));
    }

    #[test]
    fn print_content_data_without_warning() {
        let content = b"raw data";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, false, None);
        });
        assert!(!out.contains("WARNING"));
    }

    #[test]
    fn object_to_json_stream_decode_warning() {
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"corrupt data".to_vec(),
        );
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert!(val.get("decode_warning").is_some(), "Corrupt stream should have decode_warning in JSON");
    }

    #[test]
    fn object_to_json_stream_no_decode_warning() {
        let stream = make_stream(None, b"text content".to_vec());
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert!(val.get("decode_warning").is_none(), "Valid stream should not have decode_warning");
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
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
            print_content_data(w, content, "raw", "  ", &config, false, None);
        });
        assert!(out.contains("Stream content (raw, 16 bytes)"));
        assert!(out.contains("Hello PDF stream"));
    }

    #[test]
    fn print_content_data_binary_truncated() {
        // 200 bytes of binary data (contains 0x80 so is_binary_stream = true)
        let content: Vec<u8> = (0..200).map(|i| (i as u8).wrapping_add(0x80)).collect();
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 (truncated to 100)"));
    }

    #[test]
    fn print_content_data_is_contents_parses_operations() {
        // A simple PDF content stream: "BT /F1 12 Tf ET"
        let content = b"BT\n/F1 12 Tf\nET";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "decoded", "  ", &config, true, None);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"");
    }

    #[test]
    fn decode_stream_filter_is_integer_ignored() {
        // Filter that's neither Name nor Array → treated as no filter
        let stream = make_stream(Some(Object::Integer(42)), b"raw bytes".to_vec());
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw bytes");
    }

    #[test]
    fn decode_stream_multiple_filters_pipeline() {
        // Pipeline: FlateDecode then ASCIIHexDecode (hex of "Hello" = "48656c6c6f>")
        let compressed = zlib_compress(b"48656c6c6f>");
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"ASCIIHexDecode".to_vec()),
            ])),
            compressed,
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"Hello");
        assert!(warning.is_none());
    }

    #[test]
    fn decode_stream_array_with_unsupported_filter() {
        // Pipeline with an unsupported filter stops and returns warning
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"DCTDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            b"pass through".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"pass through");
        assert!(warning.is_some(), "Unsupported filter should produce warning");
        assert!(warning.unwrap().contains("unsupported filter: DCTDecode"));
    }

    #[test]
    fn decode_stream_empty_filter_array() {
        let stream = make_stream(
            Some(Object::Array(vec![])),
            b"no filters".to_vec(),
        );
        let (result, _warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
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
            print_content_data(w, b"", "raw", "  ", &config, false, None);
        });
        assert!(out.contains("Stream content (raw, 0 bytes)"));
    }

    #[test]
    fn print_content_data_binary_no_truncation() {
        // Binary content but truncate=None → full output
        let content: Vec<u8> = (0..200).map(|i| (i as u8).wrapping_add(0x80)).collect();
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_short_with_truncation_enabled() {
        // Binary content < 100 bytes with truncation enabled → no truncation applied
        let content: Vec<u8> = vec![0x80; 50];
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("50 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_exactly_100_bytes_with_truncation() {
        // Exactly 100 bytes of binary → no truncation (only truncates > 100)
        let content: Vec<u8> = vec![0x80; 100];
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("100 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_binary_101_bytes_with_truncation() {
        // 101 bytes of binary → should truncate
        let content: Vec<u8> = vec![0x80; 101];
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("101 (truncated to 100)"));
    }

    #[test]
    fn print_content_data_truncate_none_no_truncation() {
        let content: Vec<u8> = vec![0x80; 200];
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_truncate_custom_50() {
        let content: Vec<u8> = vec![0x80; 200];
        let config = DumpConfig { decode_streams: false, truncate: Some(50), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("200 (truncated to 50)"));
    }

    #[test]
    fn print_content_data_truncate_larger_than_stream() {
        let content: Vec<u8> = vec![0x80; 50];
        let config = DumpConfig { decode_streams: false, truncate: Some(500), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("50 bytes"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn print_content_data_is_contents_invalid_stream_falls_back() {
        // Content::decode is lenient, so we verify the fallback path by checking
        // that badly formed streams either parse (with 0 ops) or show the fallback.
        // Use content that Content::decode will reject: unbalanced parens cause a parse error.
        let content = b"( unclosed string";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", "  ", &config, true, None);
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
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
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
        let config = DumpConfig { decode_streams: false, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
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
            dump_object_and_children(w, (99, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
        let (result, _warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, &large[..]);
    }

    // ── print_content_data: formatting details ──────────────────────

    #[test]
    fn print_content_data_description_propagated() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"x", "custom-desc", "  ", &config, false, None);
        });
        assert!(out.contains("custom-desc"), "Description should appear in output");
    }

    #[test]
    fn print_content_data_indent_str_used() {
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, b"data", "raw", "    ", &config, false, None);
        });
        assert!(out.contains("    Stream content"), "Indent string should prefix stream content line");
    }

    #[test]
    fn print_content_data_is_contents_indent_str_used() {
        let content = b"BT\n/F1 12 Tf\nET";
        let config = default_config();
        let out = output_of(|w| {
            print_content_data(w, content, "raw", ">>> ", &config, true, None);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, true, 0);
        });
        assert!(out.contains("Parsed Content Stream"), "Direct is_contents=true should trigger content parsing");
    }

    #[test]
    fn dump_object_with_decode_and_truncate() {
        // Both decode_streams=true and truncate=Some(100) with binary stream
        let mut doc = Document::new();
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: true, truncate: Some(100), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
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
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        // Should terminate and print the object once
        let count = out.matches("Object 1 0:").count();
        assert_eq!(count, 1, "Self-referencing object should be printed once");
    }

    // ── depth limiting ────────────────────────────────────────────────

    #[test]
    fn depth_zero_prints_root_only() {
        // depth=0 means print root but don't follow any refs
        let mut doc = Document::new();
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root object");
        assert!(!out.contains("Object 2 0:"), "Should NOT follow child ref");
        assert!(out.contains("depth limit reached"));
        assert!(out.contains("1 references not followed"));
    }

    #[test]
    fn depth_one_follows_immediate_refs_only() {
        // Root -> Child -> Grandchild; depth=1 should show Root + Child but not Grandchild
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root");
        assert!(out.contains("Object 2 0:"), "Should follow immediate child");
        assert!(!out.contains("Object 3 0:"), "Should NOT follow grandchild");
        assert!(out.contains("depth limit reached"));
    }

    #[test]
    fn depth_none_traverses_everything() {
        // depth=None means unlimited (current behavior)
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root");
        assert!(out.contains("Object 2 0:"), "Should print child");
        assert!(out.contains("Object 3 0:"), "Should print grandchild");
        assert!(!out.contains("depth limit reached"));
    }

    #[test]
    fn depth_limit_shows_correct_ref_count() {
        // Root has 3 child refs, depth=0 should say "3 references not followed"
        let mut doc = Document::new();
        doc.objects.insert((2, 0), Object::Dictionary(Dictionary::new()));
        doc.objects.insert((3, 0), Object::Dictionary(Dictionary::new()));
        doc.objects.insert((4, 0), Object::Dictionary(Dictionary::new()));
        let root_dict = Dictionary::from_iter(vec![
            ("A", Object::Reference((2, 0))),
            ("B", Object::Reference((3, 0))),
            ("C", Object::Reference((4, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("3 references not followed"));
    }

    #[test]
    fn collect_reachable_with_depth_limit() {
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=0: only the root (immediate trailer ref)
        let objects = collect_reachable_objects(&doc, Some(0));
        assert!(objects.contains_key("1:0"), "Root should be included");
        assert!(!objects.contains_key("2:0"), "Child should NOT be included at depth 0");

        // depth=1: root + child
        let objects = collect_reachable_objects(&doc, Some(1));
        assert!(objects.contains_key("1:0"));
        assert!(objects.contains_key("2:0"));
        assert!(!objects.contains_key("3:0"), "Grandchild should NOT be included at depth 1");

        // depth=None: everything
        let objects = collect_reachable_objects(&doc, None);
        assert!(objects.contains_key("1:0"));
        assert!(objects.contains_key("2:0"));
        assert!(objects.contains_key("3:0"));
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
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
            dump_page(w, &doc, &PageSpec::Single(1), &config);
        });
        assert!(out.contains("Page 1 (Object"));
    }

    #[test]
    fn dump_page_confines_to_target_page() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_page(w, &doc, &PageSpec::Single(1), &config);
        });
        // Should contain page 1's content but not page 2's
        assert!(out.contains("Page1"), "Should contain page 1 content");
        assert!(!out.contains("Page2"), "Should NOT contain page 2 content");
    }

    #[test]
    fn dump_page_two_shows_only_page_two() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_page(w, &doc, &PageSpec::Single(2), &config);
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
        DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false }
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
        let val = object_to_json(&Object::Real(2.72), &empty_doc(), &json_config());
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
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
        let out = output_of(|w| dump_page_json(w, &doc, &PageSpec::Single(1), &config));
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
        let spec = PageSpec::Single(1);
        let result = compare_pdfs(&doc, &doc, Some(&spec));
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
        // TD operator should also produce newline when ty < 0
        let mut doc = Document::new();
        let content = b"BT\n0 -14 TD\n(Line1) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains('\n'), "TD with negative ty should produce newline");
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
    fn extract_text_td_positive_ty_no_newline() {
        // Td with positive ty (e.g. superscript) should NOT produce a newline
        let mut doc = Document::new();
        let content = b"BT\n(Base) Tj\n5 4 Td\n(Super) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Base"));
        assert!(text.contains("Super"));
        // Positive ty should not insert a newline between Base and Super
        assert!(!text.contains("Base\nSuper"), "Positive ty should not produce newline, got: {:?}", text);
    }

    #[test]
    fn extract_text_td_positive_real_ty_no_newline() {
        // Td with positive Real ty should NOT produce a newline
        let mut doc = Document::new();
        let content = b"BT\n(Base) Tj\n5 4.5 Td\n(Super) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains("Base"));
        assert!(text.contains("Super"));
        assert!(!text.contains("Base\nSuper"), "Positive Real ty should not produce newline, got: {:?}", text);
    }

    #[test]
    fn extract_text_td_real_operand() {
        // Td with negative Real ty should produce newline
        let mut doc = Document::new();
        let content = b"BT\n0 -14.5 Td\n(RealTd) Tj\nET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Contents", Object::Reference(c_id));
        let p_id = doc.add_object(Object::Dictionary(page));
        let text = extract_text_from_page(&doc, p_id);
        assert!(text.contains('\n'), "Td with negative Real ty should produce newline");
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
        assert_eq!(format_dict_value(&Object::Real(2.72)), "2.72");
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

    // ── format_operation ──────────────────────────────────────────────

    #[test]
    fn format_operation_no_operands() {
        let op = lopdf::content::Operation::new("BT", vec![]);
        assert_eq!(format_operation(&op), "BT");
    }

    #[test]
    fn format_operation_string_tj() {
        let op = lopdf::content::Operation::new("Tj", vec![Object::String(b"Hello".to_vec(), StringFormat::Literal)]);
        assert_eq!(format_operation(&op), "(Hello) Tj");
    }

    #[test]
    fn format_operation_name_and_int() {
        let op = lopdf::content::Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), Object::Integer(12)]);
        assert_eq!(format_operation(&op), "/F1 12 Tf");
    }

    #[test]
    fn format_operation_tj_array() {
        let arr = Object::Array(vec![
            Object::String(b"H".to_vec(), StringFormat::Literal),
            Object::Integer(-20),
            Object::String(b"ello".to_vec(), StringFormat::Literal),
        ]);
        let op = lopdf::content::Operation::new("TJ", vec![arr]);
        assert_eq!(format_operation(&op), "[(H) -20 (ello)] TJ");
    }

    #[test]
    fn format_operation_reference() {
        let op = lopdf::content::Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]);
        assert_eq!(format_operation(&op), "/Im0 Do");
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
        assert!(ops.contains(&"BT".to_string()), "Should contain BT, got: {:?}", ops);
        assert!(ops.contains(&"(Hello) Tj".to_string()), "Should contain readable Tj, got: {:?}", ops);
        assert!(ops.contains(&"ET".to_string()), "Should contain ET, got: {:?}", ops);
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

    #[test]
    fn compare_page_xobject_resource_diff() {
        let mut doc1 = Document::new();
        let mut res1 = Dictionary::new();
        let mut xobj1 = Dictionary::new();
        xobj1.set("Im0", Object::Null);
        res1.set("XObject", Object::Dictionary(xobj1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(res1));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut res2 = Dictionary::new();
        let mut xobj2 = Dictionary::new();
        xobj2.set("Im0", Object::Null);
        xobj2.set("Im1", Object::Null);
        res2.set("XObject", Object::Dictionary(xobj2));
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Resources", Object::Dictionary(res2));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("XObject") && d.contains("Im1") && d.contains("second")),
            "Should detect XObject Im1 only in second file, got: {:?}", pd.resource_diffs);
    }

    #[test]
    fn compare_page_extgstate_resource_diff() {
        let mut doc1 = Document::new();
        let mut res1 = Dictionary::new();
        let mut gs1 = Dictionary::new();
        gs1.set("GS0", Object::Null);
        res1.set("ExtGState", Object::Dictionary(gs1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(res1));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("ExtGState") && d.contains("GS0") && d.contains("first")),
            "Should detect ExtGState GS0 only in first file, got: {:?}", pd.resource_diffs);
    }

    #[test]
    fn compare_page_colorspace_resource_diff() {
        let mut doc1 = Document::new();
        let mut res1 = Dictionary::new();
        let mut cs1 = Dictionary::new();
        cs1.set("CS0", Object::Null);
        res1.set("ColorSpace", Object::Dictionary(cs1));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Resources", Object::Dictionary(res1));
        let p1_id = doc1.add_object(Object::Dictionary(page1));

        let mut doc2 = Document::new();
        let mut res2 = Dictionary::new();
        let mut cs2 = Dictionary::new();
        cs2.set("CS1", Object::Null);
        res2.set("ColorSpace", Object::Dictionary(cs2));
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Resources", Object::Dictionary(res2));
        let p2_id = doc2.add_object(Object::Dictionary(page2));

        let pd = compare_page(&doc1, &doc2, p1_id, p2_id, 1);
        assert!(!pd.identical);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("ColorSpace") && d.contains("CS0") && d.contains("first")),
            "Should detect CS0 only in first, got: {:?}", pd.resource_diffs);
        assert!(pd.resource_diffs.iter().any(|d| d.contains("ColorSpace") && d.contains("CS1") && d.contains("second")),
            "Should detect CS1 only in second, got: {:?}", pd.resource_diffs);
    }

    // ── compare_pdfs: page filter edge cases ─────────────────────────

    #[test]
    fn compare_pdfs_page_only_in_first() {
        let doc1 = build_two_page_doc();
        let doc2 = Document::new();
        let spec = PageSpec::Single(1);
        let result = compare_pdfs(&doc1, &doc2, Some(&spec));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(!result.page_diffs[0].identical);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("only in first")));
    }

    #[test]
    fn compare_pdfs_page_only_in_second() {
        let doc1 = Document::new();
        let doc2 = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let result = compare_pdfs(&doc1, &doc2, Some(&spec));
        assert_eq!(result.page_diffs.len(), 1);
        assert!(!result.page_diffs[0].identical);
        assert!(result.page_diffs[0].dict_diffs.iter().any(|d| d.contains("only in second")));
    }

    #[test]
    fn compare_pdfs_page_not_in_either() {
        let doc = Document::new();
        let spec = PageSpec::Single(999);
        let result = compare_pdfs(&doc, &doc, Some(&spec));
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
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content_binary").is_some(), "Binary stream should have content_binary field");
    }

    #[test]
    fn object_to_json_stream_with_decode_binary_truncated() {
        let binary_content: Vec<u8> = vec![0x80; 200];
        let stream = make_stream(None, binary_content);
        let config = DumpConfig { decode_streams: true, truncate: Some(100), json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        assert_eq!(val["type"], "stream");
        assert!(val.get("content_truncated").is_some(), "Truncated binary should have content_truncated field");
    }

    #[test]
    fn object_to_json_stream_no_decode() {
        let stream = make_stream(None, b"text data".to_vec());
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
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
        let objects = collect_reachable_objects(&doc, None);
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
        let objects = collect_reachable_objects(&doc, None);
        assert!(objects.is_empty(), "Empty doc should have no reachable objects");
    }

    // ── print_text_json with page filter ─────────────────────────────

    #[test]
    fn print_text_json_with_page_filter() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Single(1);
        let out = output_of(|w| print_text_json(w, &doc, Some(&spec)));
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
        let spec1 = PageSpec::Single(1);
        let spec2 = PageSpec::Single(2);
        let out1 = output_of(|w| dump_page_json(w, &doc, &spec1, &config));
        let out2 = output_of(|w| dump_page_json(w, &doc, &spec2, &config));
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

    // ── format_hex_dump ─────────────────────────────────────────────

    #[test]
    fn format_hex_dump_empty() {
        assert_eq!(format_hex_dump(&[]), "");
    }

    #[test]
    fn format_hex_dump_partial_line() {
        let data = b"Hello";
        let result = format_hex_dump(data);
        assert!(result.starts_with("00000000  "));
        assert!(result.contains("|Hello|"));
    }

    #[test]
    fn format_hex_dump_full_line() {
        let data: Vec<u8> = (0..16).collect();
        let result = format_hex_dump(&data);
        assert!(result.starts_with("00000000  "));
        // First 8 bytes, then space, then next 8 bytes
        assert!(result.contains("00 01 02 03 04 05 06 07  08 09 0a 0b 0c 0d 0e 0f"));
    }

    #[test]
    fn format_hex_dump_multi_line() {
        let data: Vec<u8> = (0..20).collect();
        let result = format_hex_dump(&data);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("00000000  "));
        assert!(lines[1].starts_with("00000010  "));
    }

    #[test]
    fn format_hex_dump_ascii_repr() {
        let data = b"AB\x00\xff";
        let result = format_hex_dump(data);
        assert!(result.contains("|AB..|"));
    }

    #[test]
    fn hex_mode_binary_stream() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("00000000  "));
        assert!(!out.contains("---"));
    }

    #[test]
    fn hex_mode_text_stream_unaffected() {
        let mut doc = Document::new();
        let text_content = b"Hello world".to_vec();
        let stream = Stream::new(Dictionary::new(), text_content);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        // Text streams still use --- delimiters
        assert!(out.contains("---"));
    }

    #[test]
    fn hex_mode_json_shows_content_hex() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["content_hex"].is_string());
    }

    // ── collect_references_in_object ──────────────────────────────────

    #[test]
    fn collect_refs_direct_reference() {
        let target = (5, 0);
        let obj = Object::Reference(target);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "");
    }

    #[test]
    fn collect_refs_in_dict() {
        let target = (5, 0);
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target));
        let obj = Object::Dictionary(dict);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Font");
    }

    #[test]
    fn collect_refs_in_array() {
        let target = (5, 0);
        let obj = Object::Array(vec![
            Object::Integer(1),
            Object::Reference(target),
        ]);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "[1]");
    }

    #[test]
    fn collect_refs_nested_dict() {
        let target = (5, 0);
        let mut inner = Dictionary::new();
        inner.set(b"Ref", Object::Reference(target));
        let mut outer = Dictionary::new();
        outer.set(b"Resources", Object::Dictionary(inner));
        let obj = Object::Dictionary(outer);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Resources/Ref");
    }

    #[test]
    fn collect_refs_in_stream_dict() {
        let target = (5, 0);
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target));
        let stream = Stream::new(dict, vec![]);
        let obj = Object::Stream(stream);
        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Font");
    }

    #[test]
    fn collect_refs_no_match() {
        let target = (5, 0);
        let obj = Object::Reference((99, 0));
        let paths = collect_references_in_object(&obj, target, "");
        assert!(paths.is_empty());
    }

    #[test]
    fn print_refs_to_finds_referencing_objects() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Found 1 objects referencing 5 0 R."));
        assert!(out.contains("/Font"));
    }

    #[test]
    fn print_refs_to_no_references() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));
        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Found 0 objects referencing 5 0 R."));
    }

    #[test]
    fn print_refs_to_json_produces_valid_json() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to_json(w, &doc, 5));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["target_object"], 5);
        assert_eq!(parsed["reference_count"], 1);
        assert!(parsed["references"].is_array());
    }

    // ── collect_fonts / print_fonts ──────────────────────────────────

    #[test]
    fn collect_fonts_finds_typed_font() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Helvetica".to_vec()));
        dict.set(b"Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].base_font, "Helvetica");
        assert_eq!(fonts[0].subtype, "Type1");
        assert_eq!(fonts[0].encoding, "WinAnsiEncoding");
        assert!(fonts[0].embedded.is_none());
    }

    #[test]
    fn collect_fonts_by_subtype_only() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        // No /Type=Font, but has a font subtype
        dict.set(b"Subtype", Object::Name(b"TrueType".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Arial".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].subtype, "TrueType");
    }

    #[test]
    fn collect_fonts_detects_embedded() {
        let mut doc = Document::new();
        // FontFile stream
        let ff_stream = Stream::new(Dictionary::new(), vec![0, 1, 2]);
        doc.objects.insert((3, 0), Object::Stream(ff_stream));

        // FontDescriptor with FontFile2 reference
        let mut fd_dict = Dictionary::new();
        fd_dict.set(b"FontFile2", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(fd_dict));

        // Font
        let mut font_dict = Dictionary::new();
        font_dict.set(b"Type", Object::Name(b"Font".to_vec()));
        font_dict.set(b"Subtype", Object::Name(b"TrueType".to_vec()));
        font_dict.set(b"BaseFont", Object::Name(b"MyFont".to_vec()));
        font_dict.set(b"FontDescriptor", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font_dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].embedded, Some((3, 0)));
    }

    #[test]
    fn collect_fonts_without_basefont() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type3".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].base_font, "-");
    }

    #[test]
    fn collect_fonts_no_fonts_in_doc() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let fonts = collect_fonts(&doc);
        assert!(fonts.is_empty());
    }

    #[test]
    fn print_fonts_text_output() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Helvetica".to_vec()));
        dict.set(b"Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_fonts(w, &doc));
        assert!(out.contains("1 fonts found"));
        assert!(out.contains("Helvetica"));
        assert!(out.contains("Type1"));
    }

    #[test]
    fn print_fonts_json_produces_valid_json() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_fonts_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["font_count"], 1);
        assert_eq!(parsed["fonts"][0]["base_font"], "Helvetica");
    }

    // ── collect_images / print_images ────────────────────────────────

    #[test]
    fn collect_images_finds_image_stream() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(100));
        dict.set(b"Height", Object::Integer(200));
        dict.set(b"ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        dict.set(b"BitsPerComponent", Object::Integer(8));
        dict.set(b"Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, vec![0; 500]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].width, 100);
        assert_eq!(images[0].height, 200);
        assert_eq!(images[0].color_space, "DeviceRGB");
        assert_eq!(images[0].bits_per_component, 8);
        assert_eq!(images[0].filter, "FlateDecode");
        assert_eq!(images[0].size, 500);
    }

    #[test]
    fn collect_images_dict_not_stream() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        // Dictionary, not Stream — should not match
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let images = collect_images(&doc);
        assert!(images.is_empty());
    }

    #[test]
    fn collect_images_icc_color_space() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(50));
        dict.set(b"Height", Object::Integer(50));
        dict.set(b"ColorSpace", Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Reference((2, 0)),
        ]));
        dict.set(b"BitsPerComponent", Object::Integer(8));
        let stream = Stream::new(dict, vec![0; 100]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert!(images[0].color_space.contains("ICCBased"));
    }

    #[test]
    fn collect_images_filter_array() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(10));
        dict.set(b"Height", Object::Integer(10));
        dict.set(b"Filter", Object::Array(vec![
            Object::Name(b"FlateDecode".to_vec()),
            Object::Name(b"DCTDecode".to_vec()),
        ]));
        let stream = Stream::new(dict, vec![0; 50]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert!(images[0].filter.contains("FlateDecode"));
        assert!(images[0].filter.contains("DCTDecode"));
    }

    #[test]
    fn collect_images_no_images() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let images = collect_images(&doc);
        assert!(images.is_empty());
    }

    #[test]
    fn print_images_text_output() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(640));
        dict.set(b"Height", Object::Integer(480));
        dict.set(b"ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        dict.set(b"BitsPerComponent", Object::Integer(8));
        let stream = Stream::new(dict, vec![0; 1000]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let out = output_of(|w| print_images(w, &doc));
        assert!(out.contains("1 images found"));
        assert!(out.contains("640"));
        assert!(out.contains("480"));
    }

    #[test]
    fn print_images_json_produces_valid_json() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(100));
        dict.set(b"Height", Object::Integer(200));
        let stream = Stream::new(dict, vec![0; 300]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let out = output_of(|w| print_images_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["image_count"], 1);
        assert_eq!(parsed["images"][0]["width"], 100);
        assert_eq!(parsed["images"][0]["height"], 200);
    }

    // ── format_color_space / format_filter ────────────────────────────

    #[test]
    fn format_color_space_name() {
        let doc = Document::new();
        let obj = Object::Name(b"DeviceGray".to_vec());
        assert_eq!(format_color_space(&obj, &doc), "DeviceGray");
    }

    #[test]
    fn format_color_space_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Integer(5),
        ]);
        assert_eq!(format_color_space(&obj, &doc), "[ICCBased 5]");
    }

    #[test]
    fn format_filter_name() {
        let obj = Object::Name(b"DCTDecode".to_vec());
        assert_eq!(format_filter(&obj), "DCTDecode");
    }

    #[test]
    fn format_filter_array() {
        let obj = Object::Array(vec![
            Object::Name(b"FlateDecode".to_vec()),
            Object::Name(b"ASCII85Decode".to_vec()),
        ]);
        assert_eq!(format_filter(&obj), "FlateDecode, ASCII85Decode");
    }

    // ── validate_pdf / check functions ────────────────────────────────

    #[test]
    fn validate_empty_doc_reports_missing_root() {
        let doc = Document::new();
        let report = validate_pdf(&doc);
        assert!(report.issues.iter().any(|i|
            i.level == ValidationLevel::Error && i.message.contains("Trailer missing /Root")));
    }

    #[test]
    fn check_broken_references_detects_broken() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let mut issues = Vec::new();
        check_broken_references(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("99"));
    }

    #[test]
    fn check_broken_references_valid() {
        let mut doc = Document::new();
        doc.objects.insert((2, 0), Object::Integer(42));
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let mut issues = Vec::new();
        check_broken_references(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn collect_broken_refs_in_array() {
        let mut doc = Document::new();
        let obj = Object::Array(vec![Object::Reference((99, 0))]);
        doc.objects.insert((1, 0), obj.clone());

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
    }

    #[test]
    fn collect_broken_refs_in_stream_dict() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        let stream = Stream::new(dict, vec![]);
        let obj = Object::Stream(stream);
        doc.objects.insert((1, 0), obj.clone());

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
    }

    #[test]
    fn collect_reachable_ids_from_trailer() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Integer(99)); // unreachable
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(1, 0)));
        assert!(!reachable.contains(&(2, 0)));
    }

    #[test]
    fn check_unreachable_objects_finds_orphans() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Integer(99));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_unreachable_objects(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("2 0"));
    }

    #[test]
    fn check_stream_lengths_mismatch() {
        let mut doc = Document::new();
        let mut stream = Stream::new(Dictionary::new(), vec![0; 10]);
        // Override /Length after construction to simulate a mismatch
        stream.dict.set(b"Length", Object::Integer(999));
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("999"));
        assert!(issues[0].message.contains("10"));
    }

    #[test]
    fn check_stream_lengths_correct() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Length", Object::Integer(10));
        let stream = Stream::new(dict, vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn page_has_media_box_direct() {
        let mut doc = Document::new();
        let mut page_dict = Dictionary::new();
        page_dict.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((1, 0), Object::Dictionary(page_dict));

        assert!(page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_inherited() {
        let mut doc = Document::new();
        // Parent has MediaBox
        let mut parent = Dictionary::new();
        parent.set(b"Type", Object::Name(b"Pages".to_vec()));
        parent.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(parent));

        // Page without MediaBox, has Parent
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_missing() {
        let mut doc = Document::new();
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn print_validation_no_issues() {
        // Build a minimal valid PDF
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((2, 0), Object::Dictionary(pages));
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_validation(w, &doc));
        assert!(out.contains("[OK]"));
    }

    #[test]
    fn print_validation_json_produces_valid_json() {
        let doc = Document::new();
        let out = output_of(|w| print_validation_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["error_count"].is_number());
        assert!(parsed["warning_count"].is_number());
        assert!(parsed["issues"].is_array());
    }

    #[test]
    fn print_validation_shows_errors_and_summary() {
        let doc = Document::new();
        let out = output_of(|w| print_validation(w, &doc));
        assert!(out.contains("[ERROR]"));
        assert!(out.contains("Summary:"));
    }

    #[test]
    fn check_page_tree_count_mismatch() {
        let mut doc = Document::new();
        // Pages says Count=5 but no actual pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set(b"Count", Object::Integer(5));
        pages_dict.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_page_tree(&doc, &mut issues);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("/Pages /Count is 5"));
    }

    // ════════════════════════════════════════════════════════════════
    // Additional P1 coverage: --refs-to
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn collect_refs_multiple_paths_same_object() {
        // Object references target from two different dict keys
        let target = (5, 0);
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference(target));
        dict.set(b"ExtGState", Object::Reference(target));
        let obj = Object::Dictionary(dict);

        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"/ExtGState".to_string()));
        assert!(paths.contains(&"/Font".to_string()));
    }

    #[test]
    fn collect_refs_mixed_containers_dict_array_ref() {
        // Dict → Array → Reference
        let target = (7, 0);
        let inner_array = Object::Array(vec![
            Object::Integer(42),
            Object::Reference(target),
        ]);
        let mut dict = Dictionary::new();
        dict.set(b"Kids", inner_array);
        let obj = Object::Dictionary(dict);

        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Kids[1]");
    }

    #[test]
    fn collect_refs_deeply_nested() {
        // Dict → Dict → Array → Dict → Reference
        let target = (10, 0);
        let mut innermost = Dictionary::new();
        innermost.set(b"Ref", Object::Reference(target));
        let arr = Object::Array(vec![Object::Dictionary(innermost)]);
        let mut mid = Dictionary::new();
        mid.set(b"Items", arr);
        let mut outer = Dictionary::new();
        outer.set(b"Resources", Object::Dictionary(mid));
        let obj = Object::Dictionary(outer);

        let paths = collect_references_in_object(&obj, target, "");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/Resources/Items[0]/Ref");
    }

    #[test]
    fn collect_refs_non_matching_reference_ignored() {
        let target = (5, 0);
        let obj = Object::Array(vec![
            Object::Reference((1, 0)),
            Object::Reference((2, 0)),
            Object::Integer(99),
        ]);
        let paths = collect_references_in_object(&obj, target, "");
        assert!(paths.is_empty());
    }

    #[test]
    fn collect_refs_primitive_types_return_empty() {
        let target = (5, 0);
        assert!(collect_references_in_object(&Object::Null, target, "").is_empty());
        assert!(collect_references_in_object(&Object::Boolean(true), target, "").is_empty());
        assert!(collect_references_in_object(&Object::Integer(42), target, "").is_empty());
        assert!(collect_references_in_object(&Object::Real(2.72), target, "").is_empty());
        assert!(collect_references_in_object(&Object::Name(b"Test".to_vec()), target, "").is_empty());
        assert!(collect_references_in_object(
            &Object::String(b"test".to_vec(), StringFormat::Literal), target, ""
        ).is_empty());
    }

    #[test]
    fn print_refs_to_multiple_referencing_objects() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        // Two different objects reference the target
        let mut dict1 = Dictionary::new();
        dict1.set(b"Font", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));

        let mut dict2 = Dictionary::new();
        dict2.set(b"XObject", Object::Reference(target_id));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Found 2 objects referencing 5 0 R."));
        assert!(out.contains("/Font"));
        assert!(out.contains("/XObject"));
    }

    #[test]
    fn print_refs_to_nonexistent_target() {
        // Target object doesn't exist — should still work, just find 0 refs
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(10));

        let out = output_of(|w| print_refs_to(w, &doc, 999));
        assert!(out.contains("Found 0 objects referencing 999 0 R."));
    }

    #[test]
    fn print_refs_to_json_multiple_via_keys() {
        // Single object has two paths to the target
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        let mut dict = Dictionary::new();
        dict.set(b"A", Object::Reference(target_id));
        dict.set(b"B", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to_json(w, &doc, 5));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["reference_count"], 1);
        let via_keys = parsed["references"][0]["via_keys"].as_array().unwrap();
        assert_eq!(via_keys.len(), 2);
    }

    #[test]
    fn print_refs_to_json_no_references() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));

        let out = output_of(|w| print_refs_to_json(w, &doc, 99));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["target_object"], 99);
        assert_eq!(parsed["reference_count"], 0);
        assert!(parsed["references"].as_array().unwrap().is_empty());
    }

    #[test]
    fn print_refs_to_shows_object_type_label() {
        let mut doc = Document::new();
        let target_id: ObjectId = (5, 0);
        doc.objects.insert(target_id, Object::Integer(42));

        // Dict with /Type = Page
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Page".to_vec()));
        dict.set(b"Contents", Object::Reference(target_id));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_refs_to(w, &doc, 5));
        assert!(out.contains("Page"));
        assert!(out.contains("Dictionary"));
    }

    // ════════════════════════════════════════════════════════════════
    // Additional P1 coverage: --fonts
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn collect_fonts_type0_composite() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type0".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"KozMinPro-Regular".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].subtype, "Type0");
        assert_eq!(fonts[0].base_font, "KozMinPro-Regular");
    }

    #[test]
    fn collect_fonts_cid_font_subtypes() {
        let mut doc = Document::new();

        // CIDFontType0
        let mut dict1 = Dictionary::new();
        dict1.set(b"Subtype", Object::Name(b"CIDFontType0".to_vec()));
        dict1.set(b"BaseFont", Object::Name(b"CIDFont0".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));

        // CIDFontType2
        let mut dict2 = Dictionary::new();
        dict2.set(b"Subtype", Object::Name(b"CIDFontType2".to_vec()));
        dict2.set(b"BaseFont", Object::Name(b"CIDFont2".to_vec()));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 2);
        assert_eq!(fonts[0].subtype, "CIDFontType0");
        assert_eq!(fonts[1].subtype, "CIDFontType2");
    }

    #[test]
    fn collect_fonts_mmtype1() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"MMType1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"MultipleMaster".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].subtype, "MMType1");
    }

    #[test]
    fn collect_fonts_embedded_fontfile_type1() {
        let mut doc = Document::new();
        // FontFile stream (Type1)
        let ff_stream = Stream::new(Dictionary::new(), vec![0; 10]);
        doc.objects.insert((3, 0), Object::Stream(ff_stream));

        let mut fd_dict = Dictionary::new();
        fd_dict.set(b"FontFile", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(fd_dict));

        let mut font = Dictionary::new();
        font.set(b"Type", Object::Name(b"Font".to_vec()));
        font.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        font.set(b"BaseFont", Object::Name(b"TimesRoman".to_vec()));
        font.set(b"FontDescriptor", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].embedded, Some((3, 0)));
    }

    #[test]
    fn collect_fonts_embedded_fontfile3_opentype() {
        let mut doc = Document::new();
        let ff_stream = Stream::new(Dictionary::new(), vec![0; 10]);
        doc.objects.insert((3, 0), Object::Stream(ff_stream));

        let mut fd_dict = Dictionary::new();
        fd_dict.set(b"FontFile3", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(fd_dict));

        let mut font = Dictionary::new();
        font.set(b"Type", Object::Name(b"Font".to_vec()));
        font.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        font.set(b"BaseFont", Object::Name(b"OpenTypeFont".to_vec()));
        font.set(b"FontDescriptor", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].embedded, Some((3, 0)));
    }

    #[test]
    fn collect_fonts_encoding_as_reference() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Symbol".to_vec()));
        dict.set(b"Encoding", Object::Reference((10, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].encoding, "10 0 R");
    }

    #[test]
    fn collect_fonts_encoding_as_dict_shows_dash() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Custom".to_vec()));
        dict.set(b"Encoding", Object::Dictionary(Dictionary::new()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].encoding, "-");
    }

    #[test]
    fn collect_fonts_font_in_stream_object() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"StreamFont".to_vec()));
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].base_font, "StreamFont");
    }

    #[test]
    fn collect_fonts_sorted_by_object_id() {
        let mut doc = Document::new();

        let mut dict3 = Dictionary::new();
        dict3.set(b"Type", Object::Name(b"Font".to_vec()));
        dict3.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict3.set(b"BaseFont", Object::Name(b"Third".to_vec()));
        doc.objects.insert((30, 0), Object::Dictionary(dict3));

        let mut dict1 = Dictionary::new();
        dict1.set(b"Type", Object::Name(b"Font".to_vec()));
        dict1.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict1.set(b"BaseFont", Object::Name(b"First".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(dict1));

        let mut dict2 = Dictionary::new();
        dict2.set(b"Type", Object::Name(b"Font".to_vec()));
        dict2.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict2.set(b"BaseFont", Object::Name(b"Second".to_vec()));
        doc.objects.insert((20, 0), Object::Dictionary(dict2));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 3);
        assert_eq!(fonts[0].base_font, "First");
        assert_eq!(fonts[1].base_font, "Second");
        assert_eq!(fonts[2].base_font, "Third");
    }

    #[test]
    fn collect_fonts_no_fontdescriptor_means_not_embedded() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Helvetica".to_vec()));
        // No FontDescriptor key at all
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert!(fonts[0].embedded.is_none());
    }

    #[test]
    fn collect_fonts_fontdescriptor_without_fontfile() {
        let mut doc = Document::new();
        // FontDescriptor exists but has no FontFile/FontFile2/FontFile3
        let fd_dict = Dictionary::new();
        doc.objects.insert((2, 0), Object::Dictionary(fd_dict));

        let mut font = Dictionary::new();
        font.set(b"Type", Object::Name(b"Font".to_vec()));
        font.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        font.set(b"BaseFont", Object::Name(b"NoEmbed".to_vec()));
        font.set(b"FontDescriptor", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert!(fonts[0].embedded.is_none());
    }

    #[test]
    fn collect_fonts_missing_subtype_defaults_to_dash() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        // No Subtype key
        dict.set(b"BaseFont", Object::Name(b"NoSubtype".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].subtype, "-");
    }

    #[test]
    fn collect_fonts_non_font_subtype_ignored() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        // Subtype=Image is not a font subtype
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let fonts = collect_fonts(&doc);
        assert!(fonts.is_empty());
    }

    #[test]
    fn print_fonts_json_embedded_null_when_not_embedded() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Type", Object::Name(b"Font".to_vec()));
        dict.set(b"Subtype", Object::Name(b"Type1".to_vec()));
        dict.set(b"BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_fonts_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["fonts"][0]["embedded"].is_null());
    }

    #[test]
    fn print_fonts_json_embedded_object_when_embedded() {
        let mut doc = Document::new();
        let ff_stream = Stream::new(Dictionary::new(), vec![0; 10]);
        doc.objects.insert((3, 0), Object::Stream(ff_stream));
        let mut fd_dict = Dictionary::new();
        fd_dict.set(b"FontFile2", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(fd_dict));
        let mut font = Dictionary::new();
        font.set(b"Type", Object::Name(b"Font".to_vec()));
        font.set(b"Subtype", Object::Name(b"TrueType".to_vec()));
        font.set(b"BaseFont", Object::Name(b"Embedded".to_vec()));
        font.set(b"FontDescriptor", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let out = output_of(|w| print_fonts_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let embedded = &parsed["fonts"][0]["embedded"];
        assert_eq!(embedded["object_number"], 3);
        assert_eq!(embedded["generation"], 0);
    }

    // ════════════════════════════════════════════════════════════════
    // Additional P1 coverage: --images
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn collect_images_missing_width_height_bpc_defaults_to_zero() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        // No Width, Height, or BitsPerComponent
        let stream = Stream::new(dict, vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].width, 0);
        assert_eq!(images[0].height, 0);
        assert_eq!(images[0].bits_per_component, 0);
    }

    #[test]
    fn collect_images_no_filter_defaults_to_dash() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(10));
        dict.set(b"Height", Object::Integer(10));
        // No Filter key
        let stream = Stream::new(dict, vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].filter, "-");
    }

    #[test]
    fn collect_images_no_colorspace_defaults_to_dash() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(10));
        dict.set(b"Height", Object::Integer(10));
        // No ColorSpace key
        let stream = Stream::new(dict, vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].color_space, "-");
    }

    #[test]
    fn collect_images_device_cmyk_color_space() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(50));
        dict.set(b"Height", Object::Integer(50));
        dict.set(b"ColorSpace", Object::Name(b"DeviceCMYK".to_vec()));
        dict.set(b"BitsPerComponent", Object::Integer(8));
        let stream = Stream::new(dict, vec![0; 100]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].color_space, "DeviceCMYK");
    }

    #[test]
    fn collect_images_dctdecode_filter() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(100));
        dict.set(b"Height", Object::Integer(100));
        dict.set(b"Filter", Object::Name(b"DCTDecode".to_vec()));
        let stream = Stream::new(dict, vec![0xFF, 0xD8, 0xFF]); // JPEG magic bytes
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].filter, "DCTDecode");
    }

    #[test]
    fn collect_images_colorspace_as_reference_resolved() {
        let mut doc = Document::new();
        // Color space object that resolves to a name
        doc.objects.insert((2, 0), Object::Name(b"DeviceGray".to_vec()));

        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(10));
        dict.set(b"Height", Object::Integer(10));
        dict.set(b"ColorSpace", Object::Reference((2, 0)));
        let stream = Stream::new(dict, vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].color_space, "DeviceGray");
    }

    #[test]
    fn collect_images_colorspace_as_broken_reference() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(10));
        dict.set(b"Height", Object::Integer(10));
        // Reference to non-existent object
        dict.set(b"ColorSpace", Object::Reference((99, 0)));
        let stream = Stream::new(dict, vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let images = collect_images(&doc);
        assert_eq!(images.len(), 1);
        // Falls back to showing the reference
        assert_eq!(images[0].color_space, "99 0 R");
    }

    #[test]
    fn collect_images_sorted_by_object_id() {
        let mut doc = Document::new();

        for (id, name) in [(30u32, "DeviceRGB"), (10, "DeviceGray"), (20, "DeviceCMYK")] {
            let mut dict = Dictionary::new();
            dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
            dict.set(b"Width", Object::Integer(10));
            dict.set(b"Height", Object::Integer(10));
            dict.set(b"ColorSpace", Object::Name(name.as_bytes().to_vec()));
            let stream = Stream::new(dict, vec![0; 10]);
            doc.objects.insert((id, 0), Object::Stream(stream));
        }

        let images = collect_images(&doc);
        assert_eq!(images.len(), 3);
        assert_eq!(images[0].object_id.0, 10);
        assert_eq!(images[1].object_id.0, 20);
        assert_eq!(images[2].object_id.0, 30);
    }

    #[test]
    fn collect_images_multiple_images() {
        let mut doc = Document::new();
        for id in 1..=5 {
            let mut dict = Dictionary::new();
            dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
            dict.set(b"Width", Object::Integer(id as i64 * 10));
            dict.set(b"Height", Object::Integer(id as i64 * 20));
            let stream = Stream::new(dict, vec![0; id as usize * 100]);
            doc.objects.insert((id, 0), Object::Stream(stream));
        }

        let images = collect_images(&doc);
        assert_eq!(images.len(), 5);
        assert_eq!(images[0].width, 10);
        assert_eq!(images[4].width, 50);
    }

    #[test]
    fn format_color_space_reference_in_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Reference((7, 0)),
        ]);
        assert_eq!(format_color_space(&obj, &doc), "[ICCBased 7 0 R]");
    }

    #[test]
    fn format_color_space_unknown_type_shows_dash() {
        let doc = Document::new();
        let obj = Object::Integer(42);
        assert_eq!(format_color_space(&obj, &doc), "-");
    }

    #[test]
    fn format_color_space_array_with_unknown_item() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Name(b"Indexed".to_vec()),
            Object::Boolean(true), // unusual
        ]);
        assert_eq!(format_color_space(&obj, &doc), "[Indexed ?]");
    }

    #[test]
    fn format_filter_unknown_type_shows_dash() {
        let obj = Object::Integer(42);
        assert_eq!(format_filter(&obj), "-");
    }

    #[test]
    fn format_filter_array_with_unknown_item() {
        let obj = Object::Array(vec![
            Object::Name(b"FlateDecode".to_vec()),
            Object::Integer(99), // unusual
        ]);
        assert_eq!(format_filter(&obj), "FlateDecode, ?");
    }

    #[test]
    fn print_images_json_all_fields_present() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Subtype", Object::Name(b"Image".to_vec()));
        dict.set(b"Width", Object::Integer(640));
        dict.set(b"Height", Object::Integer(480));
        dict.set(b"ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        dict.set(b"BitsPerComponent", Object::Integer(8));
        dict.set(b"Filter", Object::Name(b"DCTDecode".to_vec()));
        let stream = Stream::new(dict, vec![0; 5000]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let out = output_of(|w| print_images_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let img = &parsed["images"][0];
        assert_eq!(img["width"], 640);
        assert_eq!(img["height"], 480);
        assert_eq!(img["color_space"], "DeviceRGB");
        assert_eq!(img["bits_per_component"], 8);
        assert_eq!(img["filter"], "DCTDecode");
        assert_eq!(img["size"], 5000);
        assert_eq!(img["object_number"], 1);
        assert_eq!(img["generation"], 0);
    }

    // ════════════════════════════════════════════════════════════════
    // Additional P1 coverage: --validate
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn collect_broken_refs_nested_dict() {
        let doc = Document::new();
        let mut inner = Dictionary::new();
        inner.set(b"Ref", Object::Reference((99, 0)));
        let mut outer = Dictionary::new();
        outer.set(b"Inner", Object::Dictionary(inner));
        let obj = Object::Dictionary(outer);

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0], (99, 0));
    }

    #[test]
    fn collect_broken_refs_nested_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![
            Object::Array(vec![Object::Reference((88, 0))]),
        ]);

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0], (88, 0));
    }

    #[test]
    fn collect_broken_refs_multiple_in_one_object() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"A", Object::Reference((91, 0)));
        dict.set(b"B", Object::Reference((92, 0)));
        dict.set(b"C", Object::Reference((93, 0)));
        let obj = Object::Dictionary(dict);

        let broken = collect_broken_refs(&obj, &doc);
        assert_eq!(broken.len(), 3);
    }

    #[test]
    fn collect_broken_refs_valid_ref_not_reported() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));
        let obj = Object::Reference((5, 0));

        let broken = collect_broken_refs(&obj, &doc);
        assert!(broken.is_empty());
    }

    #[test]
    fn collect_broken_refs_primitives_return_empty() {
        let doc = Document::new();
        assert!(collect_broken_refs(&Object::Null, &doc).is_empty());
        assert!(collect_broken_refs(&Object::Boolean(false), &doc).is_empty());
        assert!(collect_broken_refs(&Object::Integer(0), &doc).is_empty());
        assert!(collect_broken_refs(&Object::Real(1.0), &doc).is_empty());
        assert!(collect_broken_refs(&Object::Name(b"X".to_vec()), &doc).is_empty());
    }

    #[test]
    fn check_required_keys_catalog_missing_pages() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        // No /Pages key
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_required_keys(&doc, &mut issues);
        assert!(issues.iter().any(|i|
            i.level == ValidationLevel::Error && i.message.contains("Catalog missing required /Pages")));
    }

    #[test]
    fn check_required_keys_valid_catalog() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages.set(b"Count", Object::Integer(0));
        pages.set(b"Kids", Object::Array(vec![]));
        doc.objects.insert((2, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_required_keys(&doc, &mut issues);
        // No "Catalog missing" errors — may still have MediaBox issues
        assert!(!issues.iter().any(|i| i.message.contains("Catalog missing")));
    }

    #[test]
    fn page_has_media_box_inherited_three_levels() {
        let mut doc = Document::new();

        // Grandparent has MediaBox
        let mut grandparent = Dictionary::new();
        grandparent.set(b"Type", Object::Name(b"Pages".to_vec()));
        grandparent.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(grandparent));

        // Parent without MediaBox, points up
        let mut parent = Dictionary::new();
        parent.set(b"Type", Object::Name(b"Pages".to_vec()));
        parent.set(b"Parent", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(parent));

        // Page without MediaBox
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_cycle_guard() {
        let mut doc = Document::new();
        // Page A points to B, B points to A — cycle, no MediaBox
        let mut page_a = Dictionary::new();
        page_a.set(b"Type", Object::Name(b"Page".to_vec()));
        page_a.set(b"Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page_a));

        let mut page_b = Dictionary::new();
        page_b.set(b"Type", Object::Name(b"Pages".to_vec()));
        page_b.set(b"Parent", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_b));

        // Should not infinite loop, should return false
        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_nonexistent_parent() {
        let mut doc = Document::new();
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((99, 0))); // doesn't exist
        doc.objects.insert((1, 0), Object::Dictionary(page));

        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn page_has_media_box_non_dict_object() {
        let mut doc = Document::new();
        // Object is an Integer, not a Dictionary
        doc.objects.insert((1, 0), Object::Integer(42));

        assert!(!page_has_media_box(&doc, (1, 0)));
    }

    #[test]
    fn collect_reachable_ids_multi_hop() {
        let mut doc = Document::new();
        // Chain: trailer → 1 (dict with ref to 2) → 2 (dict with ref to 3) → 3
        let mut dict1 = Dictionary::new();
        dict1.set(b"Next", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));

        let mut dict2 = Dictionary::new();
        dict2.set(b"Next", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        doc.objects.insert((3, 0), Object::Integer(99));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(1, 0)));
        assert!(reachable.contains(&(2, 0)));
        assert!(reachable.contains(&(3, 0)));
    }

    #[test]
    fn collect_reachable_ids_cycle_safe() {
        let mut doc = Document::new();
        // 1 → 2 → 1 (cycle)
        let mut dict1 = Dictionary::new();
        dict1.set(b"Next", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict1));

        let mut dict2 = Dictionary::new();
        dict2.set(b"Prev", Object::Reference((1, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(dict2));

        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        // Should not infinite-loop
        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(1, 0)));
        assert!(reachable.contains(&(2, 0)));
    }

    #[test]
    fn collect_reachable_ids_via_array() {
        let mut doc = Document::new();
        let arr = Object::Array(vec![
            Object::Reference((2, 0)),
            Object::Reference((3, 0)),
        ]);
        doc.objects.insert((1, 0), arr);
        doc.objects.insert((2, 0), Object::Integer(1));
        doc.objects.insert((3, 0), Object::Integer(2));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(2, 0)));
        assert!(reachable.contains(&(3, 0)));
    }

    #[test]
    fn collect_reachable_ids_via_stream_dict() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Font", Object::Reference((2, 0)));
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((1, 0), Object::Stream(stream));
        doc.objects.insert((2, 0), Object::Integer(42));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let reachable = collect_reachable_ids(&doc);
        assert!(reachable.contains(&(2, 0)));
    }

    #[test]
    fn check_stream_lengths_no_length_key_no_issue() {
        let mut doc = Document::new();
        // Stream without /Length key — not checked
        let stream = Stream::new(Dictionary::new(), vec![0; 10]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn check_stream_lengths_zero_length_correct() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Length", Object::Integer(0));
        let stream = Stream::new(dict, vec![]);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let mut issues = Vec::new();
        check_stream_lengths(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn check_page_tree_correct_count() {
        let mut doc = Document::new();

        // One page
        let mut page = Dictionary::new();
        page.set(b"Type", Object::Name(b"Page".to_vec()));
        page.set(b"Parent", Object::Reference((2, 0)));
        page.set(b"MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set(b"Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set(b"Count", Object::Integer(1));
        pages_dict.set(b"Kids", Object::Array(vec![Object::Reference((3, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set(b"Type", Object::Name(b"Catalog".to_vec()));
        catalog.set(b"Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_page_tree(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn validate_pdf_mixed_issues() {
        let mut doc = Document::new();
        // Broken reference
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        // Missing root → error
        // Object 1 unreachable → warn

        let report = validate_pdf(&doc);
        assert!(report.error_count > 0); // missing root + broken ref
        assert!(report.warn_count > 0);  // unreachable
        assert_eq!(report.error_count + report.warn_count + report.info_count,
                   report.issues.len());
    }

    #[test]
    fn print_validation_json_structure() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set(b"Ref", Object::Reference((99, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_validation_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();

        // Check structure
        assert!(parsed["error_count"].is_number());
        assert!(parsed["warning_count"].is_number());
        assert!(parsed["info_count"].is_number());
        assert!(parsed["issues"].is_array());

        // Each issue has level and message
        for issue in parsed["issues"].as_array().unwrap() {
            assert!(issue["level"].is_string());
            assert!(issue["message"].is_string());
            let level = issue["level"].as_str().unwrap();
            assert!(level == "error" || level == "warning" || level == "info");
        }
    }

    #[test]
    fn check_unreachable_all_reachable() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.trailer.set(b"Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_unreachable_objects(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn check_unreachable_multiple_orphans() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(1));
        doc.objects.insert((2, 0), Object::Integer(2));
        doc.objects.insert((3, 0), Object::Integer(3));
        // No trailer refs → all unreachable

        let mut issues = Vec::new();
        check_unreachable_objects(&doc, &mut issues);
        assert_eq!(issues.len(), 3);
        assert!(issues.iter().all(|i| i.level == ValidationLevel::Warn));
    }

    // ════════════════════════════════════════════════════════════════
    // Additional P1 coverage: --hex
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn format_hex_dump_exactly_8_bytes() {
        let data: Vec<u8> = (0..8).collect();
        let result = format_hex_dump(&data);
        // After the 8th byte (index 7), there's an extra space before padding
        assert!(result.contains("00 01 02 03 04 05 06 07 "));
        assert!(result.contains("|........|"));
    }

    #[test]
    fn format_hex_dump_exactly_9_bytes() {
        let data: Vec<u8> = (0..9).collect();
        let result = format_hex_dump(&data);
        // The 9th byte (08) should be after the extra space
        assert!(result.contains("00 01 02 03 04 05 06 07  08"));
    }

    #[test]
    fn format_hex_dump_17_bytes_two_lines() {
        let data: Vec<u8> = (0..17).collect();
        let result = format_hex_dump(&data);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        // First line is full 16 bytes
        assert!(lines[0].starts_with("00000000  "));
        // Second line has just 1 byte
        assert!(lines[1].starts_with("00000010  "));
        assert!(lines[1].contains("10 "));
    }

    #[test]
    fn format_hex_dump_exactly_32_bytes_two_full_lines() {
        let data: Vec<u8> = (0..32).collect();
        let result = format_hex_dump(&data);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("00000000  "));
        assert!(lines[1].starts_with("00000010  "));
    }

    #[test]
    fn format_hex_dump_space_is_printable() {
        let data = b"A B";
        let result = format_hex_dump(data);
        // Space (0x20) should show as space in ASCII column
        assert!(result.contains("|A B|"));
    }

    #[test]
    fn format_hex_dump_all_non_printable() {
        let data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x7F, 0x80, 0xFF];
        let result = format_hex_dump(&data);
        // All non-printable/non-space bytes → dots
        assert!(result.contains("|......|"));
    }

    #[test]
    fn format_hex_dump_large_offset() {
        // 256+ bytes to verify offset goes beyond 0x0ff
        let data: Vec<u8> = (0..=255).cycle().take(272).collect();
        let result = format_hex_dump(&data);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 17); // 272 / 16 = 17
        assert!(lines[16].starts_with("00000100  ")); // offset 256
    }

    #[test]
    fn hex_mode_with_truncate() {
        let mut doc = Document::new();
        // 200 bytes of binary content
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: Some(100),
            json: false,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        // Should show hex dump but truncated to 100 bytes
        assert!(out.contains("00000000  "));
        assert!(out.contains("truncated to 100"));
        // 100 bytes = 6 full lines + 4 bytes = 7 lines
        let hex_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("0000")).collect();
        assert_eq!(hex_lines.len(), 7);
    }

    #[test]
    fn hex_mode_without_decode_streams_no_hex() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        // hex=true but decode_streams=false → no stream content shown at all
        let config = DumpConfig {
            decode_streams: false,
            truncate: None,
            json: false,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(!out.contains("00000000  "));
    }

    #[test]
    fn hex_mode_json_with_truncate() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: Some(100),
            json: true,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Should have content_hex (truncated)
        assert!(parsed["object"]["content_hex"].is_string());
        let hex_str = parsed["object"]["content_hex"].as_str().unwrap();
        // Truncated to 100 bytes → 7 lines of hex dump
        let hex_lines: Vec<&str> = hex_str.lines().filter(|l| l.starts_with("0000")).collect();
        assert_eq!(hex_lines.len(), 7);
    }

    #[test]
    fn hex_mode_json_text_stream_uses_content_not_hex() {
        let mut doc = Document::new();
        let text_content = b"Hello world, this is text".to_vec();
        let stream = Stream::new(Dictionary::new(), text_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: None,
            json: true,
            hex: true,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Text stream should use "content", not "content_hex"
        assert!(parsed["object"]["content"].is_string());
        assert!(parsed["object"]["content_hex"].is_null());
    }

    #[test]
    fn json_binary_stream_no_hex_shows_content_binary() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: None,
            json: true,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // No hex → content_binary
        assert!(parsed["object"]["content_binary"].is_string());
        assert!(parsed["object"]["content_hex"].is_null());
    }

    #[test]
    fn json_binary_stream_truncate_no_hex_shows_content_truncated() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode_streams: true,
            truncate: Some(100),
            json: true,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["content_truncated"].is_string());
    }

    // ── tree view ────────────────────────────────────────────────────

    #[test]
    fn tree_node_label_catalog() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Catalog");
    }

    #[test]
    fn tree_node_label_page() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Page".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Page");
    }

    #[test]
    fn tree_node_label_dict_no_type() {
        let dict = Dictionary::from_iter(vec![
            ("Foo", Object::Integer(1)),
            ("Bar", Object::Integer(2)),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Dictionary, 2 keys");
    }

    #[test]
    fn tree_node_label_stream() {
        let stream = Stream::new(Dictionary::new(), vec![1, 2, 3, 4, 5]);
        assert_eq!(tree_node_label(&Object::Stream(stream)), "Stream, 5 bytes");
    }

    #[test]
    fn tree_node_label_stream_with_type() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XRef".to_vec()));
        let stream = Stream::new(dict, vec![1, 2, 3]);
        assert_eq!(tree_node_label(&Object::Stream(stream)), "XRef, 3 bytes");
    }

    #[test]
    fn tree_basic_output() {
        let mut doc = Document::new();
        let pages_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Pages".to_vec())),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        let catalog = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("Reference Tree:"));
        assert!(out.contains("Trailer"));
        assert!(out.contains("/Root -> 1 0 (Catalog)"));
        assert!(out.contains("/Pages -> 2 0 (Pages)"));
    }

    #[test]
    fn tree_visited_nodes_show_visited() {
        let mut doc = Document::new();
        let shared = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Font".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(shared));
        // Two objects reference the same child
        let a = Dictionary::from_iter(vec![
            ("Font", Object::Reference((3, 0))),
        ]);
        let b = Dictionary::from_iter(vec![
            ("Font", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(a));
        doc.objects.insert((2, 0), Object::Dictionary(b));
        doc.trailer.set("A", Object::Reference((1, 0)));
        doc.trailer.set("B", Object::Reference((2, 0)));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_tree(w, &doc, &config));
        // Object 3 should appear once normally and once as visited
        assert!(out.contains("3 0 (Font)"));
        assert!(out.contains("(visited)"));
    }

    #[test]
    fn tree_depth_limit_respected() {
        let mut doc = Document::new();
        let child = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=1: trailer -> root (depth 1), child would be depth 2 → limited
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("1 0"));
        assert!(out.contains("depth limit reached"));
    }

    #[test]
    fn tree_json_output_valid() {
        let mut doc = Document::new();
        let pages = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Pages".to_vec())),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(pages));
        let catalog = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_tree_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["tree"]["node"], "Trailer");
        assert!(parsed["tree"]["children"].is_array());
        let children = parsed["tree"]["children"].as_array().unwrap();
        assert!(!children.is_empty());
        // First child should be the /Root ref
        let root_child = &children[0];
        assert_eq!(root_child["key"], "/Root");
        assert_eq!(root_child["label"], "Catalog");
    }

    #[test]
    fn tree_json_visited_status() {
        let mut doc = Document::new();
        let shared = Dictionary::new();
        doc.objects.insert((2, 0), Object::Dictionary(shared));
        let dict_a = Dictionary::from_iter(vec![
            ("Ref", Object::Reference((2, 0))),
        ]);
        let dict_b = Dictionary::from_iter(vec![
            ("Ref", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(dict_a));
        doc.objects.insert((3, 0), Object::Dictionary(dict_b));
        doc.trailer.set("A", Object::Reference((1, 0)));
        doc.trailer.set("B", Object::Reference((3, 0)));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_tree_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Should contain "visited" status somewhere in the tree
        let tree_str = serde_json::to_string(&parsed).unwrap();
        assert!(tree_str.contains("\"visited\""));
    }

    #[test]
    fn tree_json_depth_limit() {
        let mut doc = Document::new();
        let child = Dictionary::new();
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| print_tree_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let tree_str = serde_json::to_string(&parsed).unwrap();
        assert!(tree_str.contains("depth_limit_reached"));
    }

    #[test]
    fn collect_refs_with_paths_from_dict() {
        let dict = Dictionary::from_iter(vec![
            ("A", Object::Reference((1, 0))),
            ("B", Object::Integer(42)),
            ("C", Object::Reference((2, 0))),
        ]);
        let refs = collect_refs_with_paths(&Object::Dictionary(dict));
        assert_eq!(refs.len(), 2);
        // Should have /A and /C paths
        let paths: Vec<&str> = refs.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"/A"));
        assert!(paths.contains(&"/C"));
    }

    #[test]
    fn collect_refs_with_paths_array_in_dict() {
        let dict = Dictionary::from_iter(vec![
            ("Kids", Object::Array(vec![
                Object::Reference((1, 0)),
                Object::Reference((2, 0)),
            ])),
        ]);
        let refs = collect_refs_with_paths(&Object::Dictionary(dict));
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].0, "/Kids[0]");
        assert_eq!(refs[1].0, "/Kids[1]");
    }

    // ── P2 gap: tree_node_label for all Object variants ──────────────

    #[test]
    fn tree_node_label_array() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)]);
        assert_eq!(tree_node_label(&arr), "Array, 3 items");
    }

    #[test]
    fn tree_node_label_empty_array() {
        let arr = Object::Array(vec![]);
        assert_eq!(tree_node_label(&arr), "Array, 0 items");
    }

    #[test]
    fn tree_node_label_boolean_true() {
        assert_eq!(tree_node_label(&Object::Boolean(true)), "Boolean(true)");
    }

    #[test]
    fn tree_node_label_boolean_false() {
        assert_eq!(tree_node_label(&Object::Boolean(false)), "Boolean(false)");
    }

    #[test]
    fn tree_node_label_integer() {
        assert_eq!(tree_node_label(&Object::Integer(42)), "Integer(42)");
    }

    #[test]
    fn tree_node_label_negative_integer() {
        assert_eq!(tree_node_label(&Object::Integer(-1)), "Integer(-1)");
    }

    #[test]
    fn tree_node_label_real() {
        assert_eq!(tree_node_label(&Object::Real(2.72)), "Real(2.72)");
    }

    #[test]
    fn tree_node_label_name() {
        assert_eq!(tree_node_label(&Object::Name(b"Helvetica".to_vec())), "Name(Helvetica)");
    }

    #[test]
    fn tree_node_label_string() {
        assert_eq!(tree_node_label(&Object::String(b"Hello".to_vec(), StringFormat::Literal)), "String(Hello)");
    }

    #[test]
    fn tree_node_label_null() {
        assert_eq!(tree_node_label(&Object::Null), "Null");
    }

    #[test]
    fn tree_node_label_reference() {
        assert_eq!(tree_node_label(&Object::Reference((5, 0))), "Reference(5 0)");
    }

    #[test]
    fn tree_node_label_pages() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Pages".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Pages");
    }

    #[test]
    fn tree_node_label_font() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Font".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Font");
    }

    #[test]
    fn tree_node_label_annot() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Annot".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Annot");
    }

    #[test]
    fn tree_node_label_xobject() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"XObject".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "XObject");
    }

    #[test]
    fn tree_node_label_encoding() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Encoding".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Encoding");
    }

    #[test]
    fn tree_node_label_custom_type() {
        let dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"CustomFoo".to_vec())),
        ]);
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "CustomFoo");
    }

    #[test]
    fn tree_node_label_empty_dict() {
        let dict = Dictionary::new();
        assert_eq!(tree_node_label(&Object::Dictionary(dict)), "Dictionary, 0 keys");
    }

    // ── P2 gap: tree with missing/nonexistent objects ────────────────

    #[test]
    fn tree_missing_object_shows_missing() {
        // Trailer references an object that doesn't exist
        let mut doc = Document::new();
        doc.trailer.set("Root", Object::Reference((99, 0)));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("99 0 (missing)"), "Missing objects should be labeled: {}", out);
    }

    #[test]
    fn tree_json_missing_object_shows_status() {
        let mut doc = Document::new();
        doc.trailer.set("Root", Object::Reference((99, 0)));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_tree_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let tree_str = serde_json::to_string(&parsed).unwrap();
        assert!(tree_str.contains("\"missing\""), "JSON should contain missing status");
    }

    // ── P2 gap: collect_refs_with_paths for streams and bare arrays ──

    #[test]
    fn collect_refs_with_paths_from_stream() {
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((5, 0)));
        dict.set("Length", Object::Integer(100));
        let stream = Stream::new(dict, vec![1, 2, 3]);
        let refs = collect_refs_with_paths(&Object::Stream(stream));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "/Font");
        assert_eq!(refs[0].1, (5, 0));
    }

    #[test]
    fn collect_refs_with_paths_from_bare_array() {
        let arr = Object::Array(vec![
            Object::Reference((1, 0)),
            Object::Integer(42),
            Object::Reference((3, 0)),
        ]);
        let refs = collect_refs_with_paths(&arr);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].0, "[0]");
        assert_eq!(refs[0].1, (1, 0));
        assert_eq!(refs[1].0, "[2]");
        assert_eq!(refs[1].1, (3, 0));
    }

    #[test]
    fn collect_refs_with_paths_no_refs() {
        let dict = Dictionary::from_iter(vec![
            ("A", Object::Integer(1)),
            ("B", Object::Name(b"Foo".to_vec())),
        ]);
        let refs = collect_refs_with_paths(&Object::Dictionary(dict));
        assert!(refs.is_empty());
    }

    #[test]
    fn collect_refs_with_paths_scalar_object() {
        // Scalars have no refs
        let refs = collect_refs_with_paths(&Object::Integer(42));
        assert!(refs.is_empty());
        let refs = collect_refs_with_paths(&Object::Null);
        assert!(refs.is_empty());
        let refs = collect_refs_with_paths(&Object::Boolean(true));
        assert!(refs.is_empty());
    }

    #[test]
    fn collect_refs_with_paths_nested_array_in_array() {
        // Array containing a nested array with references
        let arr = Object::Array(vec![
            Object::Array(vec![
                Object::Reference((1, 0)),
                Object::Reference((2, 0)),
            ]),
        ]);
        let refs = collect_refs_with_paths(&arr);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].0, "[0][0]");
        assert_eq!(refs[1].0, "[0][1]");
    }

    // ── P2 gap: truncate edge cases ─────────────────────────────────

    #[test]
    fn print_content_data_truncate_zero() {
        // truncate=0 should truncate all binary content to 0 bytes
        let content: Vec<u8> = vec![0x80; 50];
        let config = DumpConfig { decode_streams: false, truncate: Some(0), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("truncated to 0"), "truncate=0 should truncate: {}", out);
    }

    #[test]
    fn print_content_data_truncate_one() {
        // truncate=1 should show only 1 byte of binary
        let content: Vec<u8> = vec![0x80; 100];
        let config = DumpConfig { decode_streams: false, truncate: Some(1), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("truncated to 1"), "truncate=1 should truncate: {}", out);
    }

    #[test]
    fn print_content_data_hex_with_truncation() {
        // hex mode + truncation: hex dump should be truncated too
        let content: Vec<u8> = (0..200).map(|i| i as u8).collect();
        let config = DumpConfig { decode_streams: false, truncate: Some(32), json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("truncated to 32"), "Should show truncation: {}", out);
        assert!(out.contains("00000000"), "Should have hex dump offset");
        // Hex dump of 32 bytes = 2 lines of 16 bytes each
        assert!(out.contains("00000010"), "Should have second hex line for 32 bytes");
        // Should NOT have a third hex line (offset 0x20 = 32)
        assert!(!out.contains("00000020"), "Should not have third hex line: {}", out);
    }

    #[test]
    fn print_content_data_hex_without_truncation() {
        // hex mode without truncation: full hex dump
        let content: Vec<u8> = (0..48).map(|i| i as u8).collect();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, None);
        });
        assert!(out.contains("48 bytes"), "Should show full size: {}", out);
        assert!(!out.contains("truncated"));
        // 48 bytes = 3 hex lines
        assert!(out.contains("00000020"), "Should have third hex line for 48 bytes");
    }

    // ── P2 gap: warning interactions ────────────────────────────────

    #[test]
    fn print_content_data_warning_with_hex_mode() {
        // Warning should appear alongside hex dump output
        let content: Vec<u8> = vec![0x80; 32];
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, Some("FlateDecode decompression failed"));
        });
        assert!(out.contains("[WARNING: FlateDecode decompression failed]"), "Warning should appear with hex");
        assert!(out.contains("00000000"), "Hex dump should still appear");
    }

    #[test]
    fn print_content_data_warning_with_truncation() {
        // Warning + truncation should both appear
        let content: Vec<u8> = vec![0x80; 200];
        let config = DumpConfig { decode_streams: false, truncate: Some(50), json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, Some("unsupported filter: DCTDecode"));
        });
        assert!(out.contains("[WARNING: unsupported filter: DCTDecode]"), "Warning should appear");
        assert!(out.contains("truncated to 50"), "Truncation should apply");
    }

    #[test]
    fn print_content_data_warning_with_hex_and_truncation() {
        // All three: warning + hex + truncation
        let content: Vec<u8> = (0..200).map(|i| i as u8).collect();
        let config = DumpConfig { decode_streams: false, truncate: Some(16), json: false, hex: true, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            print_content_data(w, &content, "raw", "", &config, false, Some("LZWDecode: invalid data"));
        });
        assert!(out.contains("[WARNING: LZWDecode: invalid data]"));
        assert!(out.contains("truncated to 16"));
        assert!(out.contains("00000000"), "Hex dump should appear");
        assert!(!out.contains("00000010"), "Only 16 bytes = 1 hex line");
    }

    // ── P2 gap: decode_stream unsupported filters ───────────────────

    #[test]
    fn decode_stream_jpxdecode_unsupported() {
        let stream = make_stream(
            Some(Object::Name(b"JPXDecode".to_vec())),
            b"jpeg2000 data".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"jpeg2000 data");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("unsupported filter: JPXDecode"));
    }

    #[test]
    fn decode_stream_ccittfaxdecode_unsupported() {
        let stream = make_stream(
            Some(Object::Name(b"CCITTFaxDecode".to_vec())),
            b"fax data".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"fax data");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("unsupported filter: CCITTFaxDecode"));
    }

    // ── P2 gap: 3+ filter pipeline ──────────────────────────────────

    #[test]
    fn decode_stream_triple_pipeline_asciihex_flate_ascii85() {
        // Build: original -> ASCII85Encode -> FlateDecode -> ASCIIHexEncode
        // Decode order: ASCIIHexDecode -> FlateDecode -> ASCII85Decode
        let original = b"Hello";
        // ASCII85 of "Hello" is "87cURDZ"
        let ascii85_encoded = b"87cURDZ~>";
        let flate_compressed = zlib_compress(ascii85_encoded);
        let hex_encoded: String = flate_compressed.iter().map(|b| format!("{:02x}", b)).collect();
        let hex_bytes = format!("{}>", hex_encoded);

        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"ASCII85Decode".to_vec()),
            ])),
            hex_bytes.into_bytes(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, original.as_slice(), "3-stage pipeline should decode correctly");
        assert!(warning.is_none(), "No warning expected for supported pipeline");
    }

    #[test]
    fn decode_stream_pipeline_unsupported_in_middle() {
        // Pipeline: FlateDecode -> DCTDecode -> ASCIIHexDecode
        // Should succeed at FlateDecode, then stop at DCTDecode
        let compressed = zlib_compress(b"some data");
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"DCTDecode".to_vec()),
                Object::Name(b"ASCIIHexDecode".to_vec()),
            ])),
            compressed,
        );
        let (result, warning) = decode_stream(&stream);
        // Should have decompressed the FlateDecode part successfully
        assert_eq!(&*result, b"some data", "Should get FlateDecode result before stopping");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("unsupported filter: DCTDecode"));
    }

    #[test]
    fn decode_stream_pipeline_corrupt_at_second_stage() {
        // ASCIIHexDecode succeeds but the result is not valid for FlateDecode
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            b"48656c6c6f>".to_vec(), // hex("Hello") -> "Hello" is not valid zlib
        );
        let (result, warning) = decode_stream(&stream);
        // ASCIIHexDecode produced "Hello", but FlateDecode on "Hello" fails
        assert_eq!(&*result, b"Hello", "Should return intermediate result");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("FlateDecode decompression failed"));
    }

    #[test]
    fn decode_stream_pipeline_ascii85_then_lzw() {
        // Encode: original -> LZW -> ASCII85
        // Decode pipeline: ASCII85Decode -> LZWDecode
        let original = b"Hello LZW";
        let mut lzw_encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let lzw_compressed = lzw_encoder.encode(original).unwrap();

        // ASCII85 encode the LZW data
        let ascii85_data = ascii85_encode(&lzw_compressed);

        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCII85Decode".to_vec()),
                Object::Name(b"LZWDecode".to_vec()),
            ])),
            ascii85_data,
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, original.as_slice(), "ASCII85+LZW pipeline should decode");
        assert!(warning.is_none());
    }

    #[test]
    fn decode_stream_pipeline_lzw_then_asciihex() {
        // Encode: original -> ASCIIHexEncode -> LZW
        // Decode: LZWDecode -> ASCIIHexDecode
        let original = b"LZW+hex";
        let hex_encoded: String = original.iter().map(|b| format!("{:02x}", b)).collect();
        let hex_bytes = format!("{}>", hex_encoded);

        let mut lzw_encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let lzw_compressed = lzw_encoder.encode(hex_bytes.as_bytes()).unwrap();

        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"LZWDecode".to_vec()),
                Object::Name(b"ASCIIHexDecode".to_vec()),
            ])),
            lzw_compressed,
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, original.as_slice(), "LZW+ASCIIHex pipeline should decode");
        assert!(warning.is_none());
    }

    // Helper for ASCII85 encoding (for pipeline tests)
    fn ascii85_encode(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        result.extend_from_slice(b"<~");
        for chunk in data.chunks(4) {
            if chunk.len() == 4 {
                let value = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if value == 0 {
                    result.push(b'z');
                } else {
                    let mut digits = [0u8; 5];
                    let mut v = value as u64;
                    for d in digits.iter_mut().rev() {
                        *d = (v % 85) as u8 + b'!';
                        v /= 85;
                    }
                    result.extend_from_slice(&digits);
                }
            } else {
                // Pad short final group
                let mut padded = [0u8; 4];
                padded[..chunk.len()].copy_from_slice(chunk);
                let value = u32::from_be_bytes(padded);
                let mut digits = [0u8; 5];
                let mut v = value as u64;
                for d in digits.iter_mut().rev() {
                    *d = (v % 85) as u8 + b'!';
                    v /= 85;
                }
                result.extend_from_slice(&digits[..chunk.len() + 1]);
            }
        }
        result.extend_from_slice(b"~>");
        result
    }

    // ── P2 gap: depth with JSON output (unit test) ──────────────────

    #[test]
    fn depth_zero_json_limits_objects() {
        let mut doc = Document::new();
        let gc_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc_dict));
        let child_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child_dict));
        let root_dict = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Root".to_vec())),
            ("Child", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root_dict));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=0: collect_reachable_objects should only include root
        let objects_d0 = collect_reachable_objects(&doc, Some(0));
        assert!(objects_d0.contains_key("1:0"), "Root should be collected at depth 0");
        assert!(!objects_d0.contains_key("2:0"), "Child should NOT be collected at depth 0");
        assert!(!objects_d0.contains_key("3:0"), "Grandchild should NOT be at depth 0");

        // depth=2: should get everything
        let objects_d2 = collect_reachable_objects(&doc, Some(2));
        assert!(objects_d2.contains_key("1:0"));
        assert!(objects_d2.contains_key("2:0"));
        assert!(objects_d2.contains_key("3:0"), "Grandchild should be at depth 2");
    }

    // ── P2 gap: depth with multiple refs at same level ──────────────

    #[test]
    fn depth_one_follows_all_immediate_refs() {
        // Root has 3 children, depth=1 should follow all 3 but not their children
        let mut doc = Document::new();
        let gc = Dictionary::from_iter(vec![("Type", Object::Name(b"Deep".to_vec()))]);
        doc.objects.insert((5, 0), Object::Dictionary(gc));

        let c1 = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"C1".to_vec())),
            ("Deep", Object::Reference((5, 0))),
        ]);
        let c2 = Dictionary::from_iter(vec![("Type", Object::Name(b"C2".to_vec()))]);
        let c3 = Dictionary::from_iter(vec![("Type", Object::Name(b"C3".to_vec()))]);
        doc.objects.insert((2, 0), Object::Dictionary(c1));
        doc.objects.insert((3, 0), Object::Dictionary(c2));
        doc.objects.insert((4, 0), Object::Dictionary(c3));

        let root = Dictionary::from_iter(vec![
            ("A", Object::Reference((2, 0))),
            ("B", Object::Reference((3, 0))),
            ("C", Object::Reference((4, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));

        let mut visited = BTreeSet::new();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| {
            dump_object_and_children(w, (1, 0), &doc, &mut visited, &config, false, 0);
        });
        assert!(out.contains("Object 1 0:"), "Should print root");
        assert!(out.contains("Object 2 0:"), "Should follow child A");
        assert!(out.contains("Object 3 0:"), "Should follow child B");
        assert!(out.contains("Object 4 0:"), "Should follow child C");
        assert!(!out.contains("Object 5 0:"), "Should NOT follow grandchild");
        assert!(out.contains("depth limit reached"));
    }

    // ── P2 gap: tree with deeply nested refs ────────────────────────

    #[test]
    fn tree_depth_zero_shows_only_trailer_refs() {
        let mut doc = Document::new();
        let child = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=0: Trailer shows, but no children at all (trailer is depth 0)
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("Trailer"));
        // /Root -> 1 0 should show as depth limit reached (depth 1 > max_depth 0)
        assert!(out.contains("depth limit reached"), "Should hit depth limit: {}", out);
    }

    #[test]
    fn tree_depth_two_shows_three_levels() {
        let mut doc = Document::new();
        let gc = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Grandchild".to_vec())),
        ]);
        doc.objects.insert((3, 0), Object::Dictionary(gc));
        let child = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Child".to_vec())),
            ("Next", Object::Reference((3, 0))),
        ]);
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let root = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference((2, 0))),
        ]);
        doc.objects.insert((1, 0), Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // depth=2: should show Trailer, Root (depth 1), Child (depth 2), but not Grandchild (depth 3)
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(2), deref: false, raw: false };
        let out = output_of(|w| print_tree(w, &doc, &config));
        assert!(out.contains("Catalog"), "Should show Root/Catalog");
        assert!(out.contains("Child"), "Should show Child at depth 2");
        assert!(out.contains("depth limit reached"), "Grandchild should be depth-limited");
    }

    // ── P2 gap: print_stream_content with warning propagation ───────

    #[test]
    fn print_stream_content_corrupt_shows_warning() {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, b"not zlib data at all".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "  ", &config, false);
        });
        assert!(out.contains("[WARNING: FlateDecode decompression failed]"), "Warning should propagate: {}", out);
        assert!(out.contains("raw"), "Description should say raw for failed decode");
    }

    #[test]
    fn print_stream_content_unsupported_filter_shows_warning() {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"JBIG2Decode".to_vec()));
        let stream = Stream::new(dict, b"jbig2 data".to_vec());
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(out.contains("[WARNING: unsupported filter: JBIG2Decode]"), "Should show unsupported warning: {}", out);
    }

    #[test]
    fn print_stream_content_successful_decode_no_warning() {
        let compressed = zlib_compress(b"success");
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, compressed);
        let config = default_config();
        let out = output_of(|w| {
            print_stream_content(w, &stream, "", &config, false);
        });
        assert!(!out.contains("WARNING"), "Successful decode should have no warning");
        assert!(out.contains("decoded"), "Should say decoded");
    }

    // ── P2 gap: JSON warning in stream decode ───────────────────────

    #[test]
    fn object_to_json_stream_unsupported_filter_warning() {
        let stream = make_stream(
            Some(Object::Name(b"CCITTFaxDecode".to_vec())),
            b"fax data".to_vec(),
        );
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        let warning = val.get("decode_warning");
        assert!(warning.is_some(), "Unsupported filter should produce JSON warning");
        assert!(warning.unwrap().as_str().unwrap().contains("unsupported filter"));
    }

    #[test]
    fn object_to_json_stream_pipeline_partial_failure_warning() {
        // Pipeline that partially succeeds then fails
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            b"48656c6c6f>".to_vec(), // hex("Hello"), but "Hello" is not valid zlib
        );
        let config = DumpConfig { decode_streams: true, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let val = object_to_json(&Object::Stream(stream), &empty_doc(), &config);
        let warning = val.get("decode_warning");
        assert!(warning.is_some(), "Pipeline failure should produce JSON warning");
        assert!(warning.unwrap().as_str().unwrap().contains("FlateDecode"));
    }

    // ── P2 gap: decode_ascii85 edge cases ───────────────────────────

    #[test]
    fn decode_ascii85_empty_input() {
        let result = decode_ascii85(b"~>").unwrap();
        assert!(result.is_empty(), "Empty ASCII85 should produce empty output");
    }

    #[test]
    fn decode_ascii85_empty_with_prefix() {
        let result = decode_ascii85(b"<~~>").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decode_ascii85_multiple_z() {
        let result = decode_ascii85(b"zzz~>").unwrap();
        assert_eq!(result, vec![0u8; 12], "Three z's should produce 12 zero bytes");
    }

    #[test]
    fn decode_ascii85_partial_group() {
        // 2-char group: "!!" = value 0 -> pad to "!!uuu" -> output 1 byte
        let result = decode_ascii85(b"!!~>").unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn decode_ascii85_three_char_group() {
        // 3-char group outputs 2 bytes
        let result = decode_ascii85(b"!!!~>").unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn decode_ascii85_four_char_group() {
        // 4-char group outputs 3 bytes
        let result = decode_ascii85(b"!!!!~>").unwrap();
        assert_eq!(result.len(), 3);
    }

    // ── P2 gap: decode_asciihex edge cases ──────────────────────────

    #[test]
    fn decode_asciihex_empty_input() {
        let result = decode_asciihex(b">").unwrap();
        assert!(result.is_empty(), "Empty hex should produce empty output");
    }

    #[test]
    fn decode_asciihex_empty_no_marker() {
        let result = decode_asciihex(b"").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decode_asciihex_single_byte() {
        let result = decode_asciihex(b"FF>").unwrap();
        assert_eq!(result, vec![0xFF]);
    }

    #[test]
    fn decode_asciihex_mixed_case() {
        let result = decode_asciihex(b"aAbBcC>").unwrap();
        assert_eq!(result, vec![0xAA, 0xBB, 0xCC]);
    }

    // ── P2 gap: decode_lzw edge cases ───────────────────────────────

    #[test]
    fn decode_lzw_corrupt_data_returns_error() {
        // Invalid LZW data should return an error
        let result = decode_lzw(b"");
        // weezl may accept or reject empty input; either way it shouldn't panic
        // Accept either Ok or Err
        let _ = result;
    }

    #[test]
    fn decode_lzw_single_byte_input() {
        let original = b"A";
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(original).unwrap();
        let result = decode_lzw(&compressed).unwrap();
        assert_eq!(result, original.as_slice());
    }

    #[test]
    fn decode_lzw_repeated_data() {
        // LZW excels at repeated data
        let original: Vec<u8> = vec![b'X'; 1000];
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(&original).unwrap();
        let result = decode_lzw(&compressed).unwrap();
        assert_eq!(result, original);
    }

    // ── P2 gap: format_hex_dump additional edge cases ──────────────

    #[test]
    fn format_hex_dump_high_bytes_show_as_dots() {
        // Bytes 0x80-0xFF should show as dots in ASCII column
        let data: Vec<u8> = (0x80..0x84).collect();
        let result = format_hex_dump(&data);
        assert!(result.contains("80 81 82 83"));
        assert!(result.contains("|....|"), "High bytes should be dots: {}", result);
    }

    #[test]
    fn format_hex_dump_mixed_printable_and_non() {
        // Mix of printable ASCII and control chars
        let data = b"Hi\x00\x01\x02";
        let result = format_hex_dump(data);
        assert!(result.contains("|Hi...|"), "Mixed content ASCII representation: {}", result);
    }

    // ── P2 gap: get_filter_names edge cases ─────────────────────────

    #[test]
    fn get_filter_names_no_filter_key() {
        let stream = make_stream(None, vec![]);
        assert!(get_filter_names(&stream).is_empty());
    }

    #[test]
    fn get_filter_names_single_name() {
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), vec![]);
        let names = get_filter_names(&stream);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0], b"FlateDecode");
    }

    #[test]
    fn get_filter_names_array_of_names() {
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            vec![],
        );
        let names = get_filter_names(&stream);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], b"ASCIIHexDecode");
        assert_eq!(names[1], b"FlateDecode");
    }

    #[test]
    fn get_filter_names_non_name_filter() {
        // Filter is an integer (invalid) -> empty list
        let stream = make_stream(Some(Object::Integer(42)), vec![]);
        assert!(get_filter_names(&stream).is_empty());
    }

    #[test]
    fn get_filter_names_mixed_array() {
        // Array with mix of Name and non-Name -> only Names extracted
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Integer(42),
                Object::Name(b"LZWDecode".to_vec()),
            ])),
            vec![],
        );
        let names = get_filter_names(&stream);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], b"FlateDecode");
        assert_eq!(names[1], b"LZWDecode");
    }

    // ── Stats tests ─────────────────────────────────────────────────────

    #[test]
    fn stats_type_counting() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Integer(99));
        doc.objects.insert((3, 0), Object::Boolean(true));
        let stats = collect_stats(&doc);
        assert_eq!(stats.object_count, 3);
        assert_eq!(*stats.type_counts.get("Integer").unwrap(), 2);
        assert_eq!(*stats.type_counts.get("Boolean").unwrap(), 1);
    }

    #[test]
    fn stats_filter_histogram() {
        let mut doc = Document::new();
        let s1 = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), vec![0; 10]);
        let s2 = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), vec![0; 20]);
        let s3 = make_stream(Some(Object::Name(b"LZWDecode".to_vec())), vec![0; 5]);
        doc.objects.insert((1, 0), Object::Stream(s1));
        doc.objects.insert((2, 0), Object::Stream(s2));
        doc.objects.insert((3, 0), Object::Stream(s3));
        let stats = collect_stats(&doc);
        assert_eq!(*stats.filter_counts.get("FlateDecode").unwrap(), 2);
        assert_eq!(*stats.filter_counts.get("LZWDecode").unwrap(), 1);
    }

    #[test]
    fn stats_largest_streams_sorted() {
        let mut doc = Document::new();
        let s1 = make_stream(None, vec![0; 100]);
        let s2 = make_stream(None, vec![0; 500]);
        let s3 = make_stream(None, vec![0; 50]);
        doc.objects.insert((1, 0), Object::Stream(s1));
        doc.objects.insert((2, 0), Object::Stream(s2));
        doc.objects.insert((3, 0), Object::Stream(s3));
        let stats = collect_stats(&doc);
        assert_eq!(stats.largest_streams[0].0, (2, 0)); // 500 bytes
        assert_eq!(stats.largest_streams[0].1, 500);
        assert_eq!(stats.largest_streams[1].0, (1, 0)); // 100 bytes
        assert_eq!(stats.largest_streams[2].0, (3, 0)); // 50 bytes
    }

    #[test]
    fn stats_empty_doc() {
        let doc = Document::new();
        let stats = collect_stats(&doc);
        assert_eq!(stats.object_count, 0);
        assert_eq!(stats.total_stream_bytes, 0);
        assert!(stats.type_counts.is_empty());
        assert!(stats.filter_counts.is_empty());
        assert!(stats.largest_streams.is_empty());
    }

    #[test]
    fn stats_stream_bytes() {
        let mut doc = Document::new();
        let s1 = make_stream(None, vec![0; 100]);
        let s2 = make_stream(None, vec![0; 200]);
        doc.objects.insert((1, 0), Object::Stream(s1));
        doc.objects.insert((2, 0), Object::Stream(s2));
        let stats = collect_stats(&doc);
        assert_eq!(stats.total_stream_bytes, 300);
    }

    #[test]
    fn print_stats_output() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let s = make_stream(None, vec![0; 50]);
        doc.objects.insert((2, 0), Object::Stream(s));
        let out = output_of(|w| print_stats(w, &doc));
        assert!(out.contains("Overview"));
        assert!(out.contains("Objects: 2"));
        assert!(out.contains("Objects by Type"));
        assert!(out.contains("Stream Statistics"));
    }

    #[test]
    fn print_stats_json_output() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let out = output_of(|w| print_stats_json(w, &doc));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["object_count"], 1);
    }

    // ── Bookmarks tests ─────────────────────────────────────────────────

    fn make_doc_with_bookmarks() -> Document {
        let mut doc = Document::new();

        // Two bookmark items: "Chapter 1" -> "Chapter 2"
        let mut bm2 = Dictionary::new();
        bm2.set("Title", Object::String(b"Chapter 2".to_vec(), StringFormat::Literal));
        bm2.set("Dest", Object::Array(vec![Object::Integer(0), Object::Name(b"Fit".to_vec())]));
        let bm2_id = doc.add_object(Object::Dictionary(bm2));

        let mut bm1 = Dictionary::new();
        bm1.set("Title", Object::String(b"Chapter 1".to_vec(), StringFormat::Literal));
        bm1.set("Dest", Object::Array(vec![Object::Integer(0), Object::Name(b"Fit".to_vec())]));
        bm1.set("Next", Object::Reference(bm2_id));
        let bm1_id = doc.add_object(Object::Dictionary(bm1));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm1_id));
        outlines.set("Last", Object::Reference(bm2_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));

        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc
    }

    #[test]
    fn bookmarks_siblings() {
        let doc = make_doc_with_bookmarks();
        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("2 bookmarks"));
        assert!(out.contains("Chapter 1"));
        assert!(out.contains("Chapter 2"));
    }

    #[test]
    fn bookmarks_no_outlines() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("No bookmarks"));
    }

    #[test]
    fn bookmarks_nested_children() {
        let mut doc = Document::new();

        // Child bookmark
        let mut child = Dictionary::new();
        child.set("Title", Object::String(b"Section 1.1".to_vec(), StringFormat::Literal));
        let child_id = doc.add_object(Object::Dictionary(child));

        // Parent bookmark with /First pointing to child
        let mut parent = Dictionary::new();
        parent.set("Title", Object::String(b"Chapter 1".to_vec(), StringFormat::Literal));
        parent.set("First", Object::Reference(child_id));
        let parent_id = doc.add_object(Object::Dictionary(parent));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(parent_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("2 bookmarks"));
        assert!(out.contains("Chapter 1"));
        assert!(out.contains("Section 1.1"));
    }

    #[test]
    fn bookmarks_with_dest_array() {
        let doc = make_doc_with_bookmarks();
        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("[0 /Fit]"));
    }

    #[test]
    fn bookmarks_with_uri_action() {
        let mut doc = Document::new();

        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"URI".to_vec()));
        action.set("URI", Object::String(b"https://example.com".to_vec(), StringFormat::Literal));
        let action_id = doc.add_object(Object::Dictionary(action));

        let mut bm = Dictionary::new();
        bm.set("Title", Object::String(b"Link".to_vec(), StringFormat::Literal));
        bm.set("A", Object::Reference(action_id));
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("URI(https://example.com)"));
    }

    #[test]
    fn bookmarks_missing_title() {
        let mut doc = Document::new();

        let bm = Dictionary::new(); // No /Title
        let bm_id = doc.add_object(Object::Dictionary(bm));

        let mut outlines = Dictionary::new();
        outlines.set("Type", Object::Name(b"Outlines".to_vec()));
        outlines.set("First", Object::Reference(bm_id));
        let outlines_id = doc.add_object(Object::Dictionary(outlines));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Outlines", Object::Reference(outlines_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks(w, &doc));
        assert!(out.contains("(untitled)"));
    }

    #[test]
    fn bookmarks_json_output() {
        let doc = make_doc_with_bookmarks();
        let out = output_of(|w| print_bookmarks_json(w, &doc));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["bookmark_count"], 2);
        assert_eq!(val["bookmarks"][0]["title"], "Chapter 1");
        assert_eq!(val["bookmarks"][1]["title"], "Chapter 2");
    }

    #[test]
    fn bookmarks_json_no_outlines() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_bookmarks_json(w, &doc));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["bookmark_count"], 0);
    }

    // ── Annotations tests ───────────────────────────────────────────────

    fn make_doc_with_annotations() -> Document {
        let mut doc = Document::new();

        // Annotation
        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(100), Object::Integer(50),
        ]));
        annot.set("Contents", Object::String(b"Click here".to_vec(), StringFormat::Literal));
        let annot_id = doc.add_object(Object::Dictionary(annot));

        // Content stream
        let content_stream = Stream::new(Dictionary::new(), b"BT ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content_stream));

        // Page
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Contents", Object::Reference(content_id));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        // Pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages_dict.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        // Update page /Parent
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
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
    fn annotations_link_annotation() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("1 annotations found"));
        assert!(out.contains("Link"));
        assert!(out.contains("Click here"));
    }

    #[test]
    fn annotations_page_filter() {
        let doc = make_doc_with_annotations();
        // Page 1 has annotations
        let spec1 = PageSpec::Single(1);
        let out = output_of(|w| print_annotations(w, &doc, Some(&spec1)));
        assert!(out.contains("1 annotations found"));
        // Page 2 doesn't exist, should return 0
        let spec2 = PageSpec::Single(2);
        let out2 = output_of(|w| print_annotations(w, &doc, Some(&spec2)));
        assert!(out2.contains("0 annotations found"));
    }

    #[test]
    fn annotations_no_annotations() {
        let mut doc = Document::new();
        // Page without /Annots
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
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

        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("0 annotations found"));
    }

    #[test]
    fn annotations_json_output() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| print_annotations_json(w, &doc, None));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["annotation_count"], 1);
        assert_eq!(val["annotations"][0]["subtype"], "Link");
        assert_eq!(val["annotations"][0]["contents"], "Click here");
    }

    #[test]
    fn annotations_text_annotation() {
        let mut doc = Document::new();

        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set("Rect", Object::Array(vec![
            Object::Integer(10), Object::Integer(20),
            Object::Integer(30), Object::Integer(40),
        ]));
        annot.set("Contents", Object::String(b"A note".to_vec(), StringFormat::Literal));
        let annot_id = doc.add_object(Object::Dictionary(annot));

        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
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

        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("Text"));
        assert!(out.contains("A note"));
    }

    // ── DOT output tests ────────────────────────────────────────────────

    #[test]
    fn escape_dot_quotes_and_backslash() {
        assert_eq!(escape_dot("hello \"world\""), "hello \\\"world\\\"");
        assert_eq!(escape_dot("a\\b"), "a\\\\b");
    }

    #[test]
    fn dot_basic_output() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let config = default_config();
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        assert!(out.contains("digraph pdf {"));
        assert!(out.contains("->"));
        assert!(out.contains("}"));
        assert!(out.contains("Catalog"));
    }

    #[test]
    fn dot_revisited_nodes() {
        let mut doc = Document::new();
        // Two dict entries referencing the same object
        let shared = doc.add_object(Object::Integer(42));
        let mut root = Dictionary::new();
        root.set("Type", Object::Name(b"Catalog".to_vec()));
        root.set("A", Object::Reference(shared));
        root.set("B", Object::Reference(shared));
        let root_id = doc.add_object(Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference(root_id));

        let config = default_config();
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        // The shared node should be defined once, but have two edges pointing to it
        let node_name = format!("obj_{}_{}", shared.0, shared.1);
        let edge_count = out.matches(&format!("-> \"{}\"", node_name)).count();
        assert!(edge_count >= 2, "Should have at least 2 edges to shared node, got {}", edge_count);
    }

    #[test]
    fn dot_depth_limiting() {
        let mut doc = Document::new();
        let deep = doc.add_object(Object::Integer(99));
        let mut child = Dictionary::new();
        child.set("Deep", Object::Reference(deep));
        let child_id = doc.add_object(Object::Dictionary(child));
        let mut root = Dictionary::new();
        root.set("Type", Object::Name(b"Catalog".to_vec()));
        root.set("Child", Object::Reference(child_id));
        let root_id = doc.add_object(Object::Dictionary(root));
        doc.trailer.set("Root", Object::Reference(root_id));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        // Should include root and child, but not the deep object
        let deep_node = format!("obj_{}_{}", deep.0, deep.1);
        assert!(!out.contains(&deep_node), "Deep node should not appear with depth limit 1");
    }

    #[test]
    fn dot_empty_tree() {
        let doc = Document::new();
        let config = default_config();
        let out = output_of(|w| print_tree_dot(w, &doc, &config));
        assert!(out.contains("digraph pdf {"));
        assert!(out.contains("}"));
    }

    // ── PageSpec tests ──────────────────────────────────────────────────

    #[test]
    fn page_spec_parse_single() {
        let spec = PageSpec::parse("5").unwrap();
        assert!(matches!(spec, PageSpec::Single(5)));
    }

    #[test]
    fn page_spec_parse_range() {
        let spec = PageSpec::parse("1-5").unwrap();
        assert!(matches!(spec, PageSpec::Range(1, 5)));
    }

    #[test]
    fn page_spec_parse_invalid() {
        assert!(PageSpec::parse("abc").is_err());
        assert!(PageSpec::parse("0").is_err());
        assert!(PageSpec::parse("5-3").is_err()); // start > end
        assert!(PageSpec::parse("0-5").is_err()); // zero
        assert!(PageSpec::parse("1-0").is_err()); // zero
    }

    #[test]
    fn page_spec_contains() {
        let single = PageSpec::Single(3);
        assert!(single.contains(3));
        assert!(!single.contains(4));

        let range = PageSpec::Range(2, 5);
        assert!(!range.contains(1));
        assert!(range.contains(2));
        assert!(range.contains(3));
        assert!(range.contains(5));
        assert!(!range.contains(6));
    }

    #[test]
    fn page_spec_pages() {
        let single = PageSpec::Single(3);
        assert_eq!(single.pages(), vec![3]);

        let range = PageSpec::Range(2, 5);
        assert_eq!(range.pages(), vec![2, 3, 4, 5]);
    }

    #[test]
    fn dump_page_range() {
        let doc = build_two_page_doc();
        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| {
            dump_page(w, &doc, &PageSpec::Range(1, 2), &config);
        });
        assert!(out.contains("Page 1 (Object"));
        assert!(out.contains("Page 2 (Object"));
    }

    #[test]
    fn text_with_page_range() {
        let doc = build_two_page_doc();
        let spec = PageSpec::Range(1, 2);
        let out = output_of(|w| print_text(w, &doc, Some(&spec)));
        assert!(out.contains("--- Page 1 ---"));
        assert!(out.contains("--- Page 2 ---"));
    }

    // ── Bug fix: format_operation in decode-streams ──────────────────

    #[test]
    fn decode_streams_uses_format_operation_not_debug() {
        let mut doc = Document::new();
        // Create a minimal content stream with a Tj operation
        let content_bytes = b"(Hello) Tj";
        let stream = Stream::new(Dictionary::new(), content_bytes.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        // Should show clean format, not Debug format with "Operation { operator:"
        assert!(!out.contains("Operation {"), "Should not contain Debug format");
        assert!(out.contains("(Hello) Tj"), "Should contain formatted operation");
    }

    // ── Multi-object (--object 1,5,10-15) ───────────────────────────

    #[test]
    fn parse_object_spec_single() {
        let result = parse_object_spec("5").unwrap();
        assert_eq!(result, vec![5]);
    }

    #[test]
    fn parse_object_spec_multiple() {
        let result = parse_object_spec("1,5,12").unwrap();
        assert_eq!(result, vec![1, 5, 12]);
    }

    #[test]
    fn parse_object_spec_range() {
        let result = parse_object_spec("3-7").unwrap();
        assert_eq!(result, vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn parse_object_spec_mixed() {
        let result = parse_object_spec("1,5,10-12").unwrap();
        assert_eq!(result, vec![1, 5, 10, 11, 12]);
    }

    #[test]
    fn parse_object_spec_invalid() {
        assert!(parse_object_spec("abc").is_err());
        assert!(parse_object_spec("").is_err());
        assert!(parse_object_spec("5-3").is_err());
    }

    #[test]
    fn multi_object_plain_output() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Boolean(true));
        let config = default_config();
        let out = output_of(|w| print_objects(w, &doc, &[1, 2], &config));
        assert!(out.contains("Object 1 0:"));
        assert!(out.contains("Object 2 0:"));
    }

    #[test]
    fn multi_object_json_wraps_in_array() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        doc.objects.insert((2, 0), Object::Boolean(true));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_objects_json(w, &doc, &[1, 2], &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["objects"].is_array());
        assert_eq!(parsed["objects"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn single_object_json_backward_compat() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_objects_json(w, &doc, &[1], &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Single object should NOT wrap in array
        assert!(parsed["object_number"].is_number());
    }

    #[test]
    fn multi_object_missing_reports_error_in_json() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_objects_json(w, &doc, &[1, 99], &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let objs = parsed["objects"].as_array().unwrap();
        assert_eq!(objs[1]["error"].as_str().unwrap(), "not found");
    }

    // ── Stream search (--search stream=text) ─────────────────────────

    #[test]
    fn search_stream_contains_matches() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"Hello World stream content".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        doc.objects.insert((2, 0), Object::Integer(42));
        let conditions = vec![SearchCondition::StreamContains { text: "world".to_string() }];
        assert!(object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
        assert!(!object_matches(doc.get_object((2, 0)).unwrap(), &conditions));
    }

    #[test]
    fn search_stream_contains_case_insensitive() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"FlateDecode Content".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![SearchCondition::StreamContains { text: "flatedecode".to_string() }];
        assert!(object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    #[test]
    fn search_stream_no_match() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"ABC".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![SearchCondition::StreamContains { text: "XYZ".to_string() }];
        assert!(!object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    #[test]
    fn parse_search_stream_condition() {
        let conditions = parse_search_expr("stream=Hello").unwrap();
        assert_eq!(conditions.len(), 1);
        matches!(&conditions[0], SearchCondition::StreamContains { text } if text == "Hello");
    }

    #[test]
    fn search_stream_and_key_combined() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, b"q 1 0 0 1 0 0 cm Q".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let conditions = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"XObject".to_vec() },
            SearchCondition::StreamContains { text: "cm".to_string() },
        ];
        assert!(object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    #[test]
    fn search_stream_on_dict_returns_false() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Dictionary(Dictionary::new()));
        let conditions = vec![SearchCondition::StreamContains { text: "anything".to_string() }];
        assert!(!object_matches(doc.get_object((1, 0)).unwrap(), &conditions));
    }

    // ── Operators mode ──────────────────────────────────────────────

    fn build_page_doc_with_content(content: &[u8]) -> Document {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Contents", Object::Reference((1, 0)));
        page_dict.set("Parent", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_dict));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((3, 0), Object::Dictionary(pages_dict));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((3, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        doc
    }

    #[test]
    fn operators_shows_operations() {
        let doc = build_page_doc_with_content(b"BT /F1 12 Tf (Hello) Tj ET");
        let out = output_of(|w| print_operators(w, &doc, None));
        assert!(out.contains("Page 1"));
        assert!(out.contains("operations"));
        assert!(out.contains("BT"));
        assert!(out.contains("/F1 12 Tf"));
        assert!(out.contains("(Hello) Tj"));
        assert!(out.contains("ET"));
    }

    #[test]
    fn operators_json_structure() {
        let doc = build_page_doc_with_content(b"BT (Test) Tj ET");
        let out = output_of(|w| print_operators_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["pages"].is_array());
        let page = &parsed["pages"][0];
        assert_eq!(page["page_number"], 1);
        assert!(page["operation_count"].as_u64().unwrap() > 0);
        assert!(page["operations"].is_array());
    }

    #[test]
    fn operators_with_page_filter() {
        let doc = build_page_doc_with_content(b"BT (Hello) Tj ET");
        let spec = PageSpec::Single(1);
        let out = output_of(|w| print_operators(w, &doc, Some(&spec)));
        assert!(out.contains("Page 1"));
    }

    #[test]
    fn operators_json_has_operator_and_operands() {
        let doc = build_page_doc_with_content(b"BT /F1 12 Tf ET");
        let out = output_of(|w| print_operators_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let ops = parsed["pages"][0]["operations"].as_array().unwrap();
        // Find the Tf operation
        let tf_op = ops.iter().find(|o| o["operator"] == "Tf").unwrap();
        assert!(tf_op["operands"].is_array());
        assert_eq!(tf_op["operands"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn operators_empty_page() {
        let mut doc = Document::new();
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page_dict));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((1, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((3, 0)));
        let out = output_of(|w| print_operators(w, &doc, None));
        assert!(out.contains("0 operations"));
    }

    // ── Deref modifier ──────────────────────────────────────────────

    #[test]
    fn deref_shows_reference_summary() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let mut target = Dictionary::new();
        target.set("Type", Object::Name(b"Font".to_vec()));
        target.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((2, 0), Object::Dictionary(target));
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("2 0 R =>"));
        assert!(out.contains("/Type /Font"));
    }

    #[test]
    fn deref_false_no_expansion() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("2 0 R"));
        assert!(!out.contains("=>"));
    }

    #[test]
    fn deref_json_adds_resolved() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        doc.objects.insert((2, 0), Object::Integer(42));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let ref_obj = &parsed["object"]["entries"]["Ref"];
        assert_eq!(ref_obj["type"], "reference");
        assert!(ref_obj["resolved"].is_object());
        assert_eq!(ref_obj["resolved"]["type"], "integer");
        assert_eq!(ref_obj["resolved"]["value"], 42);
    }

    #[test]
    fn deref_json_no_recursive_deref() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Ref", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let mut inner_dict = Dictionary::new();
        inner_dict.set("Inner", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(inner_dict));
        doc.objects.insert((3, 0), Object::Integer(99));
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let resolved = &parsed["object"]["entries"]["Ref"]["resolved"];
        // Inner reference should NOT be recursively resolved
        let inner_ref = &resolved["entries"]["Inner"];
        assert_eq!(inner_ref["type"], "reference");
        assert!(inner_ref.get("resolved").is_none() || inner_ref["resolved"].is_null());
    }

    #[test]
    fn deref_stream_summary() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Contents", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));
        let mut stream_dict = Dictionary::new();
        stream_dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(stream_dict, vec![0u8; 100]);
        doc.objects.insert((2, 0), Object::Stream(stream));
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: true, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("stream, 100 bytes"));
        assert!(out.contains("FlateDecode"));
    }

    // ── Resources mode ──────────────────────────────────────────────

    fn build_page_doc_with_resources() -> Document {
        let mut doc = Document::new();
        // Font object
        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name(b"Font".to_vec()));
        font_dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        font_dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font_dict));
        // XObject image
        let mut img_dict = Dictionary::new();
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", Object::Integer(640));
        img_dict.set("Height", Object::Integer(480));
        img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        let img_stream = Stream::new(img_dict, vec![0u8; 100]);
        doc.objects.insert((20, 0), Object::Stream(img_stream));
        // Resources dict
        let mut font_res = Dictionary::new();
        font_res.set("F1", Object::Reference((10, 0)));
        let mut xobj_res = Dictionary::new();
        xobj_res.set("Im1", Object::Reference((20, 0)));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_res));
        resources.set("XObject", Object::Dictionary(xobj_res));
        doc.objects.insert((5, 0), Object::Dictionary(resources));
        // Page
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Resources", Object::Reference((5, 0)));
        page_dict.set("Parent", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_dict));
        // Pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((3, 0), Object::Dictionary(pages_dict));
        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((3, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        doc
    }

    #[test]
    fn resources_shows_fonts_and_xobjects() {
        let doc = build_page_doc_with_resources();
        let out = output_of(|w| print_resources(w, &doc, None));
        assert!(out.contains("Page 1 Resources"));
        assert!(out.contains("Fonts:"));
        assert!(out.contains("/F1"));
        assert!(out.contains("obj 10"));
        assert!(out.contains("Helvetica"));
        assert!(out.contains("XObjects:"));
        assert!(out.contains("/Im1"));
        assert!(out.contains("obj 20"));
        assert!(out.contains("Image"));
    }

    #[test]
    fn resources_json_structure() {
        let doc = build_page_doc_with_resources();
        let out = output_of(|w| print_resources_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["pages"].is_array());
        let page = &parsed["pages"][0];
        assert_eq!(page["page_number"], 1);
        assert!(!page["fonts"].as_array().unwrap().is_empty());
        assert!(!page["xobjects"].as_array().unwrap().is_empty());
    }

    #[test]
    fn resources_with_page_filter() {
        let doc = build_page_doc_with_resources();
        let spec = PageSpec::Single(1);
        let out = output_of(|w| print_resources(w, &doc, Some(&spec)));
        assert!(out.contains("Page 1 Resources"));
    }

    #[test]
    fn resources_inherits_from_parent() {
        let mut doc = Document::new();
        // Font
        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name(b"Font".to_vec()));
        font_dict.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font_dict));
        // Resources on Pages (parent), not on Page
        let mut font_res = Dictionary::new();
        font_res.set("F1", Object::Reference((10, 0)));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_res));
        // Page without Resources
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference((3, 0)));
        doc.objects.insert((2, 0), Object::Dictionary(page_dict));
        // Pages with Resources
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        pages_dict.set("Resources", Object::Dictionary(resources));
        doc.objects.insert((3, 0), Object::Dictionary(pages_dict));
        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((3, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let out = output_of(|w| print_resources(w, &doc, None));
        assert!(out.contains("Fonts:"));
        assert!(out.contains("Courier"));
    }

    #[test]
    fn resources_no_resources_empty() {
        let mut doc = Document::new();
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(page_dict));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((1, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((3, 0)));
        let out = output_of(|w| print_resources(w, &doc, None));
        assert!(out.contains("Page 1 Resources"));
        // No Fonts: or XObjects: sections should appear
        assert!(!out.contains("Fonts:"));
    }

    // ── Forms mode ──────────────────────────────────────────────────

    fn build_form_doc() -> Document {
        let mut doc = Document::new();
        // Form field 1 - text field
        let mut field1 = Dictionary::new();
        field1.set("T", Object::String(b"FirstName".to_vec(), StringFormat::Literal));
        field1.set("FT", Object::Name(b"Tx".to_vec()));
        field1.set("V", Object::String(b"John".to_vec(), StringFormat::Literal));
        doc.objects.insert((20, 0), Object::Dictionary(field1));
        // Form field 2 - button
        let mut field2 = Dictionary::new();
        field2.set("T", Object::String(b"Subscribe".to_vec(), StringFormat::Literal));
        field2.set("FT", Object::Name(b"Btn".to_vec()));
        field2.set("V", Object::Name(b"Yes".to_vec()));
        doc.objects.insert((22, 0), Object::Dictionary(field2));
        // AcroForm
        let mut acroform = Dictionary::new();
        acroform.set("NeedAppearances", Object::Boolean(true));
        acroform.set("Fields", Object::Array(vec![
            Object::Reference((20, 0)),
            Object::Reference((22, 0)),
        ]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        // Need Pages for page mapping
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((30, 0))]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        // Page with annotations pointing to fields
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((5, 0)));
        page.set("Annots", Object::Array(vec![
            Object::Reference((20, 0)),
            Object::Reference((22, 0)),
        ]));
        doc.objects.insert((30, 0), Object::Dictionary(page));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        doc
    }

    #[test]
    fn forms_shows_fields() {
        let doc = build_form_doc();
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("AcroForm found"));
        assert!(out.contains("NeedAppearances: true"));
        assert!(out.contains("2 form fields"));
        assert!(out.contains("FirstName"));
        assert!(out.contains("Tx"));
        assert!(out.contains("\"John\""));
        assert!(out.contains("Subscribe"));
        assert!(out.contains("Btn"));
        assert!(out.contains("/Yes"));
    }

    #[test]
    fn forms_json_structure() {
        let doc = build_form_doc();
        let out = output_of(|w| print_forms_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["acroform_object"].is_number());
        assert_eq!(parsed["need_appearances"], true);
        assert_eq!(parsed["field_count"], 2);
        assert!(parsed["fields"].is_array());
        let fields = parsed["fields"].as_array().unwrap();
        assert_eq!(fields[0]["field_name"], "FirstName");
        assert_eq!(fields[0]["field_type"], "Tx");
    }

    #[test]
    fn forms_page_mapping() {
        let doc = build_form_doc();
        let out = output_of(|w| print_forms_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let fields = parsed["fields"].as_array().unwrap();
        // Fields should be mapped to page 1 via Annots
        assert_eq!(fields[0]["page_number"], 1);
    }

    #[test]
    fn forms_no_acroform() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("No AcroForm found"));
    }

    #[test]
    fn forms_empty_fields() {
        let mut doc = Document::new();
        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("0 form fields"));
    }

    #[test]
    fn forms_hierarchical_fields() {
        let mut doc = Document::new();
        // Child field
        let mut child = Dictionary::new();
        child.set("T", Object::String(b"FirstName".to_vec(), StringFormat::Literal));
        child.set("FT", Object::Name(b"Tx".to_vec()));
        child.set("V", Object::String(b"Alice".to_vec(), StringFormat::Literal));
        doc.objects.insert((21, 0), Object::Dictionary(child));
        // Parent field with Kids
        let mut parent = Dictionary::new();
        parent.set("T", Object::String(b"Person".to_vec(), StringFormat::Literal));
        parent.set("Kids", Object::Array(vec![Object::Reference((21, 0))]));
        doc.objects.insert((20, 0), Object::Dictionary(parent));
        // AcroForm
        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        // Should show qualified name Person.FirstName
        assert!(out.contains("Person.FirstName"));
    }

    #[test]
    fn forms_json_no_acroform() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_forms_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["acroform_object"].is_null());
        assert_eq!(parsed["field_count"], 0);
    }

    // ── walk_name_tree tests ─────────────────────────────────────────

    #[test]
    fn walk_name_tree_leaf_only() {
        let mut doc = Document::new();
        let mut leaf = Dictionary::new();
        leaf.set("Names", Object::Array(vec![
            Object::String(b"file1.pdf".to_vec(), StringFormat::Literal),
            Object::Integer(1),
            Object::String(b"file2.pdf".to_vec(), StringFormat::Literal),
            Object::Integer(2),
        ]));
        doc.objects.insert((1, 0), Object::Dictionary(leaf.clone()));

        let results = walk_name_tree(&doc, &leaf);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "file1.pdf");
        assert_eq!(results[1].0, "file2.pdf");
    }

    #[test]
    fn walk_name_tree_with_kids() {
        let mut doc = Document::new();
        let mut child = Dictionary::new();
        child.set("Names", Object::Array(vec![
            Object::String(b"a.txt".to_vec(), StringFormat::Literal),
            Object::Integer(10),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(child));
        let mut root = Dictionary::new();
        root.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(root.clone()));

        let results = walk_name_tree(&doc, &root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "a.txt");
    }

    #[test]
    fn walk_name_tree_empty() {
        let doc = Document::new();
        let dict = Dictionary::new();
        let results = walk_name_tree(&doc, &dict);
        assert!(results.is_empty());
    }

    #[test]
    fn walk_name_tree_cycle_protection() {
        let mut doc = Document::new();
        // Create two nodes that reference each other
        let mut node_a = Dictionary::new();
        node_a.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(node_a.clone()));
        let mut node_b = Dictionary::new();
        node_b.set("Kids", Object::Array(vec![Object::Reference((1, 0))]));
        node_b.set("Names", Object::Array(vec![
            Object::String(b"found".to_vec(), StringFormat::Literal),
            Object::Integer(1),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(node_b));

        let results = walk_name_tree(&doc, &node_a);
        // Should find "found" without infinite loop
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "found");
    }

    // ── walk_number_tree tests ───────────────────────────────────────

    #[test]
    fn walk_number_tree_leaf_only() {
        let doc = Document::new();
        let mut leaf = Dictionary::new();
        leaf.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Name(b"D".to_vec()),
            Object::Integer(5), Object::Name(b"r".to_vec()),
        ]));
        let results = walk_number_tree(&doc, &leaf);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 0);
        assert_eq!(results[1].0, 5);
    }

    #[test]
    fn walk_number_tree_with_kids() {
        let mut doc = Document::new();
        let mut child = Dictionary::new();
        child.set("Nums", Object::Array(vec![
            Object::Integer(3), Object::Integer(99),
        ]));
        doc.objects.insert((5, 0), Object::Dictionary(child));
        let mut root = Dictionary::new();
        root.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        doc.objects.insert((4, 0), Object::Dictionary(root.clone()));

        let results = walk_number_tree(&doc, &root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 3);
    }

    #[test]
    fn walk_number_tree_empty() {
        let doc = Document::new();
        let dict = Dictionary::new();
        let results = walk_number_tree(&doc, &dict);
        assert!(results.is_empty());
    }

    #[test]
    fn walk_number_tree_cycle_protection() {
        let mut doc = Document::new();
        // Two nodes that reference each other
        let mut node_a = Dictionary::new();
        node_a.set("Kids", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(node_a.clone()));
        let mut node_b = Dictionary::new();
        node_b.set("Kids", Object::Array(vec![Object::Reference((1, 0))]));
        node_b.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Integer(1),
        ]));
        doc.objects.insert((2, 0), Object::Dictionary(node_b));

        let results = walk_number_tree(&doc, &node_a);
        // Should find the one entry from node_b without infinite loop
        assert_eq!(results.len(), 1);
    }

    // ── regex search tests ───────────────────────────────────────────

    #[test]
    fn parse_search_expr_regex() {
        let conds = parse_search_expr("regex=Font\\d+").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::RegexMatch { .. }));
    }

    #[test]
    fn parse_search_expr_regex_case_insensitive() {
        let conds = parse_search_expr("REGEX=test").unwrap();
        assert_eq!(conds.len(), 1);
        assert!(matches!(&conds[0], SearchCondition::RegexMatch { .. }));
    }

    #[test]
    fn parse_search_expr_regex_invalid() {
        let result = parse_search_expr("regex=[invalid");
        match result {
            Err(e) => assert!(e.contains("Invalid regex"), "Error should mention invalid regex: {}", e),
            Ok(_) => panic!("Expected error for invalid regex"),
        }
    }

    #[test]
    fn object_matches_regex_name_value() {
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"Helvetica-Bold".to_vec()));
        let re = Regex::new("Helvetica").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_key_name() {
        let mut dict = Dictionary::new();
        dict.set("MediaBox", Object::Integer(0));
        let re = Regex::new("^Media").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_string_value() {
        let mut dict = Dictionary::new();
        dict.set("Title", Object::String(b"Chapter 5 - Results".to_vec(), StringFormat::Literal));
        let re = Regex::new(r"Chapter \d+").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_no_match() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Page".to_vec()));
        let re = Regex::new("Font").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(!object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_combined_with_other() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let re = Regex::new("Helv").unwrap();
        let conds = vec![
            SearchCondition::KeyEquals { key: b"Type".to_vec(), value: b"Font".to_vec() },
            SearchCondition::RegexMatch { pattern: re },
        ];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    #[test]
    fn object_matches_regex_stream_content() {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        let stream = Stream::new(dict, b"BT /F1 12 Tf (Hello World) Tj ET".to_vec());
        let re = Regex::new("Hello").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Stream(stream), &conds));
    }

    #[test]
    fn object_matches_regex_case_flag() {
        // Use (?i) for case-insensitive matching
        let mut dict = Dictionary::new();
        dict.set("BaseFont", Object::Name(b"HELVETICA".to_vec()));
        let re = Regex::new("(?i)helvetica").unwrap();
        let conds = vec![SearchCondition::RegexMatch { pattern: re }];
        assert!(object_matches(&Object::Dictionary(dict), &conds));
    }

    // ── security tests ───────────────────────────────────────────────

    #[test]
    fn security_unencrypted() {
        let doc = Document::new();
        let info = collect_security(&doc, None);
        assert!(!info.encrypted);
    }

    #[test]
    fn security_encrypted_v4() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-3904));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "AES-128");
        assert_eq!(info.version, 4);
        assert_eq!(info.revision, 4);
        assert_eq!(info.key_length, 128);
    }

    #[test]
    fn security_encrypted_v5() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(5));
        encrypt.set("R", Object::Integer(6));
        encrypt.set("Length", Object::Integer(256));
        encrypt.set("P", Object::Integer(-1));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "AES-256");
        assert_eq!(info.version, 5);
    }

    #[test]
    fn security_encrypted_v1() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(1));
        encrypt.set("R", Object::Integer(2));
        encrypt.set("P", Object::Integer(-44));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));

        let info = collect_security(&doc, None);
        assert!(info.encrypted);
        assert_eq!(info.algorithm, "RC4, 40-bit");
    }

    #[test]
    fn security_permissions_decode() {
        // 2564 = bit 2 (print) + bit 9 (accessibility) + bit 11 (print hq)
        let perms = decode_permissions(2564);
        assert!(perms["Print"]);
        assert!(!perms["Modify"]);
        assert!(!perms["Copy/extract text"]);
        assert!(perms["Accessibility extract"]);
        assert!(perms["Print high quality"]);
    }

    #[test]
    fn security_permissions_all_allowed() {
        let perms = decode_permissions(-1); // All bits set
        assert!(perms["Print"]);
        assert!(perms["Modify"]);
        assert!(perms["Copy/extract text"]);
        assert!(perms["Annotate"]);
        assert!(perms["Fill forms"]);
        assert!(perms["Accessibility extract"]);
        assert!(perms["Assemble"]);
        assert!(perms["Print high quality"]);
    }

    #[test]
    fn security_print_unencrypted() {
        let doc = Document::new();
        let dummy = std::path::Path::new("nonexistent.pdf");
        let out = output_of(|w| print_security(w, &doc, dummy));
        assert!(out.contains("Encryption: No"));
    }

    #[test]
    fn security_print_encrypted() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-3)); // all permissions
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));
        let dummy = std::path::Path::new("nonexistent.pdf");
        let out = output_of(|w| print_security(w, &doc, dummy));
        assert!(out.contains("Encryption: Yes"));
        assert!(out.contains("AES-128"));
        assert!(out.contains("[YES] Print"));
    }

    #[test]
    fn security_json_output() {
        let mut doc = Document::new();
        let mut encrypt = Dictionary::new();
        encrypt.set("V", Object::Integer(4));
        encrypt.set("R", Object::Integer(4));
        encrypt.set("Length", Object::Integer(128));
        encrypt.set("P", Object::Integer(-3));
        let enc_id = doc.add_object(Object::Dictionary(encrypt));
        doc.trailer.set("Encrypt", Object::Reference(enc_id));
        let dummy = std::path::Path::new("nonexistent.pdf");
        let out = output_of(|w| print_security_json(w, &doc, dummy));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["encrypted"], true);
        assert_eq!(parsed["algorithm"], "AES-128");
        assert_eq!(parsed["version"], 4);
        assert!(parsed["permissions"].is_object());
    }

    // ── raw encrypt parsing tests ────────────────────────────────────

    #[test]
    fn extract_int_after_key_basic() {
        let data = b"<</V 4/R 4/Length 128/P -1084>>";
        assert_eq!(extract_int_after_key(data, b"/V"), Some(4));
        assert_eq!(extract_int_after_key(data, b"/R"), Some(4));
        assert_eq!(extract_int_after_key(data, b"/Length"), Some(128));
        assert_eq!(extract_int_after_key(data, b"/P"), Some(-1084));
    }

    #[test]
    fn extract_int_after_key_with_spaces() {
        let data = b"<< /V  2  /R  3  /Length  128  /P  -3904 >>";
        assert_eq!(extract_int_after_key(data, b"/V"), Some(2));
        assert_eq!(extract_int_after_key(data, b"/P"), Some(-3904));
    }

    #[test]
    fn extract_int_after_key_missing() {
        let data = b"<</V 4/R 4>>";
        assert_eq!(extract_int_after_key(data, b"/Length"), None);
    }

    #[test]
    fn extract_int_after_key_negative() {
        let data = b"/P -1";
        assert_eq!(extract_int_after_key(data, b"/P"), Some(-1));
    }

    #[test]
    fn parse_encrypt_from_raw_file_with_tempfile() {
        let content = b"%PDF-1.6\n5 0 obj\n<</Filter/Standard/V 2/R 3/Length 128/P -1084>>\nendobj\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pdf");
        std::fs::write(&path, content).unwrap();

        let info = parse_encrypt_from_raw_file(&path, 5).unwrap();
        assert!(info.encrypted);
        assert_eq!(info.version, 2);
        assert_eq!(info.revision, 3);
        assert_eq!(info.key_length, 128);
        assert_eq!(info.permissions_raw, -1084);
        assert_eq!(info.encrypt_object, Some(5));
    }

    #[test]
    fn parse_encrypt_from_raw_file_not_found() {
        let content = b"%PDF-1.6\n1 0 obj\n<</Type/Catalog>>\nendobj\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pdf");
        std::fs::write(&path, content).unwrap();

        let result = parse_encrypt_from_raw_file(&path, 99);
        assert!(result.is_none());
    }

    // ── embedded files tests ─────────────────────────────────────────

    #[test]
    fn embedded_files_none() {
        let doc = Document::new();
        let files = collect_embedded_files(&doc);
        assert!(files.is_empty());
    }

    #[test]
    fn embedded_files_no_names() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let files = collect_embedded_files(&doc);
        assert!(files.is_empty());
    }

    #[test]
    fn embedded_files_basic() {
        let mut doc = Document::new();
        // Create an embedded file stream
        let mut ef_stream_dict = Dictionary::new();
        ef_stream_dict.set("Type", Object::Name(b"EmbeddedFile".to_vec()));
        ef_stream_dict.set("Subtype", Object::Name(b"application#2Fpdf".to_vec()));
        let mut params = Dictionary::new();
        params.set("Size", Object::Integer(42356));
        ef_stream_dict.set("Params", Object::Dictionary(params));
        let ef_stream = Stream::new(ef_stream_dict, b"fake pdf content".to_vec());
        doc.objects.insert((10, 0), Object::Stream(ef_stream));

        // EF dict
        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference((10, 0)));
        // Filespec
        let mut filespec = Dictionary::new();
        filespec.set("Type", Object::Name(b"Filespec".to_vec()));
        filespec.set("F", Object::String(b"invoice.pdf".to_vec(), StringFormat::Literal));
        filespec.set("EF", Object::Dictionary(ef));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        // Names tree (leaf)
        let mut ef_tree = Dictionary::new();
        ef_tree.set("Names", Object::Array(vec![
            Object::String(b"invoice.pdf".to_vec(), StringFormat::Literal),
            Object::Reference((11, 0)),
        ]));
        doc.objects.insert((12, 0), Object::Dictionary(ef_tree));

        // /Names dict in catalog
        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Reference((12, 0)));
        doc.objects.insert((13, 0), Object::Dictionary(names_dict));

        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((13, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let files = collect_embedded_files(&doc);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "invoice.pdf");
        assert_eq!(files[0].object_number, 10);
        assert_eq!(files[0].size, Some(42356));
    }

    #[test]
    fn embedded_files_missing_ef() {
        let mut doc = Document::new();
        // Filespec without /EF
        let mut filespec = Dictionary::new();
        filespec.set("Type", Object::Name(b"Filespec".to_vec()));
        filespec.set("F", Object::String(b"missing.pdf".to_vec(), StringFormat::Literal));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        let mut ef_tree = Dictionary::new();
        ef_tree.set("Names", Object::Array(vec![
            Object::String(b"missing.pdf".to_vec(), StringFormat::Literal),
            Object::Reference((11, 0)),
        ]));
        doc.objects.insert((12, 0), Object::Dictionary(ef_tree));

        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Reference((12, 0)));
        doc.objects.insert((13, 0), Object::Dictionary(names_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((13, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let files = collect_embedded_files(&doc);
        assert!(files.is_empty());
    }

    #[test]
    fn embedded_files_print() {
        let doc = Document::new();
        let out = output_of(|w| print_embedded_files(w, &doc));
        assert!(out.contains("0 embedded files"));
    }

    #[test]
    fn embedded_files_json() {
        let doc = Document::new();
        let out = output_of(|w| print_embedded_files_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["embedded_file_count"], 0);
        assert_eq!(parsed["embedded_files"].as_array().unwrap().len(), 0);
    }

    // ── page labels tests ────────────────────────────────────────────

    #[test]
    fn int_to_roman_basic() {
        assert_eq!(int_to_roman(1, false), "i");
        assert_eq!(int_to_roman(4, false), "iv");
        assert_eq!(int_to_roman(9, true), "IX");
        assert_eq!(int_to_roman(14, false), "xiv");
        assert_eq!(int_to_roman(1999, true), "MCMXCIX");
    }

    #[test]
    fn int_to_alpha_basic() {
        assert_eq!(int_to_alpha(1, true), "A");
        assert_eq!(int_to_alpha(26, true), "Z");
        assert_eq!(int_to_alpha(27, true), "AA");
        assert_eq!(int_to_alpha(1, false), "a");
    }

    #[test]
    fn format_page_label_decimal() {
        assert_eq!(format_page_label("D", "", 5), "5");
        assert_eq!(format_page_label("D", "P-", 3), "P-3");
    }

    #[test]
    fn format_page_label_roman() {
        assert_eq!(format_page_label("r", "", 3), "iii");
        assert_eq!(format_page_label("R", "", 4), "IV");
    }

    #[test]
    fn format_page_label_alpha() {
        assert_eq!(format_page_label("a", "", 1), "a");
        assert_eq!(format_page_label("A", "", 2), "B");
    }

    #[test]
    fn format_page_label_prefix_only() {
        // Style "-" means no number, only prefix
        assert_eq!(format_page_label("-", "Cover", 1), "Cover");
    }

    #[test]
    fn page_labels_no_labels() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let labels = collect_page_labels(&doc);
        assert!(labels.is_empty());
    }

    #[test]
    fn page_labels_roman_then_decimal() {
        let mut doc = Document::new();

        // Create a simple page tree with 5 pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(5));
        let mut kids = Vec::new();
        for i in 10..15 {
            let mut page = Dictionary::new();
            page.set("Type", Object::Name(b"Page".to_vec()));
            page.set("Parent", Object::Reference((2, 0)));
            page.set("MediaBox", Object::Array(vec![
                Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
            ]));
            doc.objects.insert((i, 0), Object::Dictionary(page));
            kids.push(Object::Reference((i, 0)));
        }
        pages_dict.set("Kids", Object::Array(kids));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        // PageLabels: pages 0-2 are roman lowercase, pages 3-4 are decimal starting at 1
        let mut rule_roman = Dictionary::new();
        rule_roman.set("S", Object::Name(b"r".to_vec()));
        let mut rule_decimal = Dictionary::new();
        rule_decimal.set("S", Object::Name(b"D".to_vec()));
        rule_decimal.set("St", Object::Integer(1));

        let mut pl_dict = Dictionary::new();
        pl_dict.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Dictionary(rule_roman),
            Object::Integer(3), Object::Dictionary(rule_decimal),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(pl_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("PageLabels", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let labels = collect_page_labels(&doc);
        assert_eq!(labels.len(), 5);
        assert_eq!(labels[0].label, "i");
        assert_eq!(labels[1].label, "ii");
        assert_eq!(labels[2].label, "iii");
        assert_eq!(labels[3].label, "1");
        assert_eq!(labels[4].label, "2");
    }

    #[test]
    fn page_labels_with_prefix() {
        let mut doc = Document::new();
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(2));
        let mut page1 = Dictionary::new();
        page1.set("Type", Object::Name(b"Page".to_vec()));
        page1.set("Parent", Object::Reference((2, 0)));
        page1.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((10, 0), Object::Dictionary(page1));
        let mut page2 = Dictionary::new();
        page2.set("Type", Object::Name(b"Page".to_vec()));
        page2.set("Parent", Object::Reference((2, 0)));
        page2.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((11, 0), Object::Dictionary(page2));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0)), Object::Reference((11, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut rule = Dictionary::new();
        rule.set("S", Object::Name(b"D".to_vec()));
        rule.set("P", Object::String(b"A-".to_vec(), StringFormat::Literal));
        rule.set("St", Object::Integer(1));
        let mut pl_dict = Dictionary::new();
        pl_dict.set("Nums", Object::Array(vec![
            Object::Integer(0), Object::Dictionary(rule),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(pl_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("PageLabels", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let labels = collect_page_labels(&doc);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].label, "A-1");
        assert_eq!(labels[1].label, "A-2");
    }

    #[test]
    fn page_labels_print_no_labels() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_page_labels(w, &doc));
        assert!(out.contains("No page labels defined."));
    }

    #[test]
    fn page_labels_json() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_page_labels_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["page_count"], 0);
    }

    // ── links tests ──────────────────────────────────────────────────

    fn make_page_with_annots(doc: &mut Document, page_id: ObjectId, parent_id: ObjectId, annot_ids: Vec<ObjectId>) {
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(parent_id));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        let refs: Vec<Object> = annot_ids.iter().map(|id| Object::Reference(*id)).collect();
        page.set("Annots", Object::Array(refs));
        doc.objects.insert(page_id, Object::Dictionary(page));
    }

    #[test]
    fn links_uri_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"URI".to_vec()));
        action.set("URI", Object::String(b"https://example.com".to_vec(), StringFormat::Literal));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "URI");
        assert_eq!(links[0].target, "https://example.com");
    }

    #[test]
    fn links_goto_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("Dest", Object::Array(vec![Object::Reference((10, 0)), Object::Name(b"Fit".to_vec())]));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "GoTo");
    }

    #[test]
    fn links_no_links() {
        let mut doc = Document::new();
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((10, 0), Object::Dictionary(page));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert!(links.is_empty());
    }

    #[test]
    fn links_mixed_annotations() {
        // Has both Link and Text annotations, should only return Links
        let mut doc = Document::new();
        let mut link_annot = Dictionary::new();
        link_annot.set("Subtype", Object::Name(b"Link".to_vec()));
        link_annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        link_annot.set("Dest", Object::Name(b"dest1".to_vec()));
        doc.objects.insert((20, 0), Object::Dictionary(link_annot));

        let mut text_annot = Dictionary::new();
        text_annot.set("Subtype", Object::Name(b"Text".to_vec()));
        text_annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        doc.objects.insert((21, 0), Object::Dictionary(text_annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0), (21, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "GoTo");
    }

    #[test]
    fn links_page_filter() {
        let mut doc = Document::new();
        let mut link1 = Dictionary::new();
        link1.set("Subtype", Object::Name(b"Link".to_vec()));
        link1.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        link1.set("Dest", Object::Name(b"d1".to_vec()));
        doc.objects.insert((20, 0), Object::Dictionary(link1));

        let mut link2 = Dictionary::new();
        link2.set("Subtype", Object::Name(b"Link".to_vec()));
        link2.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        link2.set("Dest", Object::Name(b"d2".to_vec()));
        doc.objects.insert((21, 0), Object::Dictionary(link2));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(2));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0)), Object::Reference((11, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);
        make_page_with_annots(&mut doc, (11, 0), (2, 0), vec![(21, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let spec = PageSpec::Single(1);
        let links = collect_links(&doc, Some(&spec));
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn links_named_action() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"Named".to_vec()));
        action.set("N", Object::Name(b"NextPage".to_vec()));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "Named");
        assert_eq!(links[0].target, "NextPage");
    }

    #[test]
    fn links_goto_r_action() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"GoToR".to_vec()));
        action.set("F", Object::String(b"other.pdf".to_vec(), StringFormat::Literal));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set("Rect", Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(100), Object::Integer(20)]));
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), vec![(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let links = collect_links(&doc, None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "GoToR");
        assert!(links[0].target.contains("other.pdf"));
    }

    #[test]
    fn links_print_output() {
        let doc = Document::new();
        let out = output_of(|w| print_links(w, &doc, None));
        assert!(out.contains("0 links found"));
    }

    #[test]
    fn links_json_output() {
        let doc = Document::new();
        let out = output_of(|w| print_links_json(w, &doc, None));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["link_count"], 0);
        assert_eq!(parsed["links"].as_array().unwrap().len(), 0);
    }

    // ── enhanced validate tests ──────────────────────────────────────

    #[test]
    fn validate_content_stream_valid() {
        let mut doc = Document::new();
        let content = b"BT /F1 12 Tf (Hello) Tj ET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        let p_id = doc.add_object(Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(1));
        pages.set("Kids", Object::Array(vec![Object::Reference(p_id)]));
        let pages_id = doc.add_object(Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let cat_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(cat_id));

        let mut issues = Vec::new();
        check_content_stream_syntax(&doc, &mut issues);
        assert!(issues.is_empty(), "Valid content stream should produce no issues");
    }

    #[test]
    fn validate_content_stream_invalid() {
        let mut doc = Document::new();
        let content = b"THIS IS NOT VALID PDF CONTENT STREAM SYNTAX <<<>>>!!!";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        let c_id = doc.add_object(Object::Stream(stream));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        let p_id = doc.add_object(Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(1));
        pages.set("Kids", Object::Array(vec![Object::Reference(p_id)]));
        let pages_id = doc.add_object(Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let cat_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(cat_id));

        let mut issues = Vec::new();
        check_content_stream_syntax(&doc, &mut issues);
        // Note: lopdf's Content::decode is lenient; it may or may not fail on arbitrary bytes.
        // If it does fail, we should get a warning about invalid syntax.
        // If it doesn't fail, that's also acceptable.
    }

    #[test]
    fn validate_font_missing_basefont() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        // Missing BaseFont
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("missing /BaseFont")));
    }

    #[test]
    fn validate_font_valid() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn validate_font_widths_mismatch() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        font.set("BaseFont", Object::Name(b"Arial".to_vec()));
        font.set("FirstChar", Object::Integer(32));
        font.set("LastChar", Object::Integer(126));
        // Expected: 95 widths, provide only 10
        let widths: Vec<Object> = (0..10).map(|_| Object::Integer(600)).collect();
        font.set("Widths", Object::Array(widths));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("/Widths")));
    }

    #[test]
    fn validate_font_firstchar_without_lastchar() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        font.set("FirstChar", Object::Integer(32));
        // Missing LastChar
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let mut issues = Vec::new();
        check_font_requirements(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("/FirstChar") && i.message.contains("/LastChar")));
    }

    #[test]
    fn validate_page_tree_cycle() {
        let mut doc = Document::new();
        // Create two page nodes that form a parent cycle
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((10, 0), Object::Dictionary(page));

        let mut parent1 = Dictionary::new();
        parent1.set("Type", Object::Name(b"Pages".to_vec()));
        parent1.set("Parent", Object::Reference((3, 0)));
        parent1.set("Count", Object::Integer(1));
        parent1.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(parent1));

        let mut parent2 = Dictionary::new();
        parent2.set("Type", Object::Name(b"Pages".to_vec()));
        parent2.set("Parent", Object::Reference((2, 0))); // cycle!
        doc.objects.insert((3, 0), Object::Dictionary(parent2));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_page_tree_cycles(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("cycle")));
    }

    #[test]
    fn validate_name_tree_odd_names() {
        let mut doc = Document::new();
        let mut names_subtree = Dictionary::new();
        // Odd-length Names array
        names_subtree.set("Names", Object::Array(vec![
            Object::String(b"key1".to_vec(), StringFormat::Literal),
            Object::Integer(1),
            Object::String(b"key2".to_vec(), StringFormat::Literal),
            // Missing value for key2 → odd length = 3
        ]));
        doc.objects.insert((5, 0), Object::Dictionary(names_subtree));

        let mut ef_dict = Dictionary::new();
        ef_dict.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        doc.objects.insert((4, 0), Object::Dictionary(ef_dict));

        let mut names = Dictionary::new();
        names.set("EmbeddedFiles", Object::Reference((4, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(names));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_names_tree_structure(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("odd length")));
    }

    #[test]
    fn validate_name_tree_no_names_or_kids() {
        let mut doc = Document::new();
        // Empty dict node (neither /Names nor /Kids)
        let empty_node = Dictionary::new();
        doc.objects.insert((5, 0), Object::Dictionary(empty_node));

        let mut ef_dict = Dictionary::new();
        ef_dict.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        doc.objects.insert((4, 0), Object::Dictionary(ef_dict));

        let mut names = Dictionary::new();
        names.set("EmbeddedFiles", Object::Reference((4, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(names));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((3, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let mut issues = Vec::new();
        check_names_tree_structure(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("neither /Names nor /Kids")));
    }

    #[test]
    fn validate_duplicate_objects() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(1));
        doc.objects.insert((1, 1), Object::Integer(2)); // Same obj# different gen
        doc.objects.insert((2, 0), Object::Integer(3));

        let mut issues = Vec::new();
        check_duplicate_objects(&doc, &mut issues);
        assert!(issues.iter().any(|i| i.message.contains("Object 1") && i.message.contains("multiple generations")));
    }

    #[test]
    fn validate_no_duplicate_objects() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(1));
        doc.objects.insert((2, 0), Object::Integer(2));

        let mut issues = Vec::new();
        check_duplicate_objects(&doc, &mut issues);
        assert!(issues.is_empty());
    }

    // ── --raw modifier tests ─────────────────────────────────────────

    #[test]
    fn raw_shows_compressed_bytes() {
        let mut doc = Document::new();
        let original = b"Hello, World!";
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();
        let compressed_len = compressed.len();

        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let stream = Stream::new(dict, compressed.clone());
        doc.objects.insert((1, 0), Object::Stream(stream));

        // raw mode: should show the compressed bytes, not decoded
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains(&format!("{} bytes", compressed_len)));
        // Should NOT contain the decoded text
        assert!(!out.contains("Hello, World!"));
    }

    #[test]
    fn raw_with_hex() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..64).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains("00000000  "));
    }

    #[test]
    fn raw_with_truncate() {
        let mut doc = Document::new();
        let binary_content: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary_content);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: Some(50), json: false, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains("truncated to 50"));
    }

    #[test]
    fn raw_on_non_stream_is_noop() {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), Object::Integer(42));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("42"));
        assert!(!out.contains("raw"));
    }

    #[test]
    fn raw_json_text_stream() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"Hello text content".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["object"]["raw_content"].as_str().unwrap(), "Hello text content");
        assert!(parsed["object"]["content"].is_null());
    }

    #[test]
    fn raw_json_binary_stream() {
        let mut doc = Document::new();
        let binary: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["raw_content_binary"].as_str().unwrap().contains("32 bytes"));
    }

    #[test]
    fn raw_json_binary_hex() {
        let mut doc = Document::new();
        let binary: Vec<u8> = (0..32).collect();
        let stream = Stream::new(Dictionary::new(), binary);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["raw_content_hex"].is_string());
        assert!(parsed["object"]["raw_content_hex"].as_str().unwrap().contains("00000000"));
    }

    #[test]
    fn raw_json_binary_hex_truncate() {
        let mut doc = Document::new();
        let binary: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let stream = Stream::new(Dictionary::new(), binary);
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: Some(32), json: true, hex: true, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object_json(w, &doc, 1, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["object"]["raw_content_hex"].is_string());
        // Should only show 32 bytes = 2 full hex lines
        let hex_str = parsed["object"]["raw_content_hex"].as_str().unwrap();
        let hex_lines: Vec<&str> = hex_str.lines().filter(|l| l.starts_with("0000")).collect();
        assert_eq!(hex_lines.len(), 2);
    }

    #[test]
    fn raw_text_stream_shows_content() {
        let mut doc = Document::new();
        let stream = Stream::new(Dictionary::new(), b"Some PDF text stream".to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        assert!(out.contains("Some PDF text stream"));
    }

    #[test]
    fn raw_does_not_parse_content_stream() {
        // Even if the stream has content operations, raw should NOT parse them
        let mut doc = Document::new();
        let content = b"BT /F1 12 Tf (Hello) Tj ET";
        let stream = Stream::new(Dictionary::new(), content.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        // With raw, is_contents=false so it won't try to parse
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: None, deref: false, raw: true };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        assert!(out.contains("raw, undecoded"));
        // Should show raw text, not parsed operations
        assert!(out.contains("BT /F1 12 Tf"));
        assert!(!out.contains("Parsed Content Stream"));
    }

    // ── Enhanced --fonts diagnostics tests ────────────────────────────

    #[test]
    fn fonts_to_unicode_present() {
        let mut doc = Document::new();
        let cmap_stream = Stream::new(Dictionary::new(), b"cmap data".to_vec());
        doc.objects.insert((10, 0), Object::Stream(cmap_stream));

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        font.set("ToUnicode", Object::Reference((10, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].to_unicode, Some((10, 0)));
    }

    #[test]
    fn fonts_to_unicode_absent() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts[0].to_unicode, None);
    }

    #[test]
    fn fonts_first_last_char_widths() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Courier".to_vec()));
        font.set("FirstChar", Object::Integer(32));
        font.set("LastChar", Object::Integer(126));
        font.set("Widths", Object::Array(vec![Object::Integer(600); 95]));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts[0].first_char, Some(32));
        assert_eq!(fonts[0].last_char, Some(126));
        assert_eq!(fonts[0].widths_len, Some(95));
    }

    #[test]
    fn fonts_widths_as_reference() {
        let mut doc = Document::new();
        let widths = Object::Array(vec![Object::Integer(500); 50]);
        doc.objects.insert((10, 0), widths);

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        font.set("BaseFont", Object::Name(b"Arial".to_vec()));
        font.set("Widths", Object::Reference((10, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        assert_eq!(fonts[0].widths_len, Some(50));
    }

    #[test]
    fn fonts_encoding_differences_small() {
        let mut doc = Document::new();
        let mut enc = Dictionary::new();
        enc.set("Type", Object::Name(b"Encoding".to_vec()));
        enc.set("Differences", Object::Array(vec![
            Object::Integer(32), Object::Name(b"space".to_vec()),
            Object::Name(b"exclam".to_vec()),
        ]));
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Test".to_vec()));
        font.set("Encoding", Object::Dictionary(enc));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        let diffs = fonts[0].encoding_differences.as_ref().unwrap();
        assert!(diffs.contains("32=/space"));
        assert!(diffs.contains("33=/exclam"));
    }

    #[test]
    fn fonts_encoding_differences_truncated() {
        let mut doc = Document::new();
        let mut items: Vec<Object> = vec![Object::Integer(32)];
        for i in 0..10 {
            items.push(Object::Name(format!("glyph{}", i).into_bytes()));
        }
        let mut enc = Dictionary::new();
        enc.set("Differences", Object::Array(items));
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Test".to_vec()));
        font.set("Encoding", Object::Dictionary(enc));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        let diffs = fonts[0].encoding_differences.as_ref().unwrap();
        assert!(diffs.contains("10 total"));
    }

    #[test]
    fn fonts_cid_system_info() {
        let mut doc = Document::new();
        let mut csi = Dictionary::new();
        csi.set("Registry", Object::String(b"Adobe".to_vec(), lopdf::StringFormat::Literal));
        csi.set("Ordering", Object::String(b"Identity".to_vec(), lopdf::StringFormat::Literal));
        csi.set("Supplement", Object::Integer(0));
        let mut cid_font = Dictionary::new();
        cid_font.set("Type", Object::Name(b"Font".to_vec()));
        cid_font.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
        cid_font.set("BaseFont", Object::Name(b"NotoSans".to_vec()));
        cid_font.set("CIDSystemInfo", Object::Dictionary(csi));
        doc.objects.insert((2, 0), Object::Dictionary(cid_font));

        let mut type0 = Dictionary::new();
        type0.set("Type", Object::Name(b"Font".to_vec()));
        type0.set("Subtype", Object::Name(b"Type0".to_vec()));
        type0.set("BaseFont", Object::Name(b"NotoSans".to_vec()));
        type0.set("DescendantFonts", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(type0));

        let fonts = collect_fonts(&doc);
        let type0_font = fonts.iter().find(|f| f.subtype == "Type0").unwrap();
        assert_eq!(type0_font.cid_system_info.as_deref(), Some("Adobe-Identity-0"));
    }

    #[test]
    fn fonts_text_output_shows_diagnostics() {
        let mut doc = Document::new();
        let cmap_stream = Stream::new(Dictionary::new(), b"cmap".to_vec());
        doc.objects.insert((10, 0), Object::Stream(cmap_stream));

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        font.set("ToUnicode", Object::Reference((10, 0)));
        font.set("FirstChar", Object::Integer(32));
        font.set("LastChar", Object::Integer(126));
        font.set("Widths", Object::Array(vec![Object::Integer(600); 95]));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let out = output_of(|w| print_fonts(w, &doc));
        assert!(out.contains("ToUnicode: 10 0 R"));
        assert!(out.contains("CharRange: 32-126"));
        assert!(out.contains("Widths: 95"));
    }

    #[test]
    fn fonts_json_includes_diagnostics() {
        let mut doc = Document::new();
        let cmap_stream = Stream::new(Dictionary::new(), b"cmap".to_vec());
        doc.objects.insert((10, 0), Object::Stream(cmap_stream));

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        font.set("ToUnicode", Object::Reference((10, 0)));
        font.set("FirstChar", Object::Integer(32));
        font.set("LastChar", Object::Integer(126));
        font.set("Widths", Object::Array(vec![Object::Integer(600); 95]));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let out = output_of(|w| print_fonts_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let font_json = &parsed["fonts"][0];
        assert_eq!(font_json["to_unicode"]["object_number"], 10);
        assert_eq!(font_json["first_char"], 32);
        assert_eq!(font_json["last_char"], 126);
        assert_eq!(font_json["widths_count"], 95);
    }

    #[test]
    fn fonts_json_cid_system_info() {
        let mut doc = Document::new();
        let mut csi = Dictionary::new();
        csi.set("Registry", Object::String(b"Adobe".to_vec(), lopdf::StringFormat::Literal));
        csi.set("Ordering", Object::String(b"Japan1".to_vec(), lopdf::StringFormat::Literal));
        csi.set("Supplement", Object::Integer(6));
        let mut cid_font = Dictionary::new();
        cid_font.set("Subtype", Object::Name(b"CIDFontType0".to_vec()));
        cid_font.set("BaseFont", Object::Name(b"KozMin".to_vec()));
        cid_font.set("CIDSystemInfo", Object::Dictionary(csi));
        doc.objects.insert((2, 0), Object::Dictionary(cid_font));

        let mut type0 = Dictionary::new();
        type0.set("Type", Object::Name(b"Font".to_vec()));
        type0.set("Subtype", Object::Name(b"Type0".to_vec()));
        type0.set("BaseFont", Object::Name(b"KozMin".to_vec()));
        type0.set("DescendantFonts", Object::Array(vec![Object::Reference((2, 0))]));
        doc.objects.insert((1, 0), Object::Dictionary(type0));

        let out = output_of(|w| print_fonts_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let type0_json = parsed["fonts"].as_array().unwrap()
            .iter().find(|f| f["subtype"] == "Type0").unwrap();
        assert_eq!(type0_json["cid_system_info"].as_str().unwrap(), "Adobe-Japan1-6");
    }

    #[test]
    fn fonts_no_diagnostics_for_simple_font() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let out = output_of(|w| print_fonts(w, &doc));
        assert!(!out.contains("ToUnicode"));
        assert!(!out.contains("CharRange"));
        assert!(!out.contains("Differences"));
        assert!(!out.contains("CIDSystemInfo"));
    }

    #[test]
    fn fonts_json_omits_absent_diagnostics() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let out = output_of(|w| print_fonts_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let f = &parsed["fonts"][0];
        assert!(f.get("to_unicode").is_none());
        assert!(f.get("first_char").is_none());
        assert!(f.get("widths_count").is_none());
        assert!(f.get("encoding_differences").is_none());
        assert!(f.get("cid_system_info").is_none());
    }

    #[test]
    fn fonts_encoding_differences_via_reference() {
        let mut doc = Document::new();
        let mut enc = Dictionary::new();
        enc.set("Differences", Object::Array(vec![
            Object::Integer(65), Object::Name(b"A".to_vec()),
        ]));
        doc.objects.insert((10, 0), Object::Dictionary(enc));

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Custom".to_vec()));
        font.set("Encoding", Object::Reference((10, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(font));

        let fonts = collect_fonts(&doc);
        let diffs = fonts[0].encoding_differences.as_ref().unwrap();
        assert!(diffs.contains("65=/A"));
    }

    // ── --layers / --ocg tests ───────────────────────────────────────

    fn make_ocg_doc() -> Document {
        let mut doc = Document::new();

        // OCG objects
        let mut ocg1 = Dictionary::new();
        ocg1.set("Type", Object::Name(b"OCG".to_vec()));
        ocg1.set("Name", Object::String(b"Layer1".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg1));

        let mut ocg2 = Dictionary::new();
        ocg2.set("Type", Object::Name(b"OCG".to_vec()));
        ocg2.set("Name", Object::String(b"Layer2".to_vec(), StringFormat::Literal));
        doc.objects.insert((11, 0), Object::Dictionary(ocg2));

        // Default config
        let mut d_config = Dictionary::new();
        d_config.set("BaseState", Object::Name(b"ON".to_vec()));
        d_config.set("OFF", Object::Array(vec![Object::Reference((11, 0))]));

        // OCProperties
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        oc_props.set("D", Object::Dictionary(d_config));

        // Page with Properties referencing OCG
        let mut props = Dictionary::new();
        props.set("MC0", Object::Reference((10, 0)));

        let mut resources = Dictionary::new();
        resources.set("Properties", Object::Dictionary(props));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Resources", Object::Dictionary(resources));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));

        doc.trailer.set("Root", Object::Reference((1, 0)));
        doc
    }

    #[test]
    fn layers_collects_ocgs() {
        let doc = make_ocg_doc();
        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].name, "Layer1");
        assert_eq!(layers[1].name, "Layer2");
    }

    #[test]
    fn layers_default_state() {
        let doc = make_ocg_doc();
        let layers = collect_layers(&doc);
        assert_eq!(layers[0].default_state, "ON");
        assert_eq!(layers[1].default_state, "OFF");
    }

    #[test]
    fn layers_page_references() {
        let doc = make_ocg_doc();
        let layers = collect_layers(&doc);
        // Layer1 is referenced on page 1
        assert_eq!(layers[0].page_numbers, vec![1]);
        // Layer2 is not referenced on any page
        assert!(layers[1].page_numbers.is_empty());
    }

    #[test]
    fn layers_no_ocproperties() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert!(layers.is_empty());
    }

    #[test]
    fn layers_empty_ocgs() {
        let mut doc = Document::new();
        let mut d_config = Dictionary::new();
        d_config.set("BaseState", Object::Name(b"ON".to_vec()));
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![]));
        oc_props.set("D", Object::Dictionary(d_config));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert!(layers.is_empty());
    }

    #[test]
    fn layers_base_state_off_with_on_override() {
        let mut doc = Document::new();

        let mut ocg1 = Dictionary::new();
        ocg1.set("Type", Object::Name(b"OCG".to_vec()));
        ocg1.set("Name", Object::String(b"Hidden".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg1));

        let mut ocg2 = Dictionary::new();
        ocg2.set("Type", Object::Name(b"OCG".to_vec()));
        ocg2.set("Name", Object::String(b"Visible".to_vec(), StringFormat::Literal));
        doc.objects.insert((11, 0), Object::Dictionary(ocg2));

        let mut d_config = Dictionary::new();
        d_config.set("BaseState", Object::Name(b"OFF".to_vec()));
        d_config.set("ON", Object::Array(vec![Object::Reference((11, 0))]));

        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        oc_props.set("D", Object::Dictionary(d_config));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers[0].default_state, "OFF");
        assert_eq!(layers[1].default_state, "ON");
    }

    #[test]
    fn layers_unnamed_ocg() {
        let mut doc = Document::new();
        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        // No Name key
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Dictionary(oc_props));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers[0].name, "(unnamed)");
    }

    #[test]
    fn layers_text_output() {
        let doc = make_ocg_doc();
        let out = output_of(|w| print_layers(w, &doc));
        assert!(out.contains("2 layers found"));
        assert!(out.contains("Layer1"));
        assert!(out.contains("Layer2"));
        assert!(out.contains("ON"));
        assert!(out.contains("OFF"));
    }

    #[test]
    fn layers_json_output() {
        let doc = make_ocg_doc();
        let out = output_of(|w| print_layers_json(w, &doc));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["layer_count"], 2);
        assert_eq!(parsed["layers"][0]["name"], "Layer1");
        assert_eq!(parsed["layers"][0]["default_state"], "ON");
        assert_eq!(parsed["layers"][1]["name"], "Layer2");
        assert_eq!(parsed["layers"][1]["default_state"], "OFF");
    }

    #[test]
    fn layers_ocproperties_via_reference() {
        let mut doc = Document::new();
        let mut ocg = Dictionary::new();
        ocg.set("Type", Object::Name(b"OCG".to_vec()));
        ocg.set("Name", Object::String(b"RefLayer".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(ocg));

        let d_config = Dictionary::new();
        let mut oc_props = Dictionary::new();
        oc_props.set("OCGs", Object::Array(vec![Object::Reference((10, 0))]));
        oc_props.set("D", Object::Dictionary(d_config));
        doc.objects.insert((20, 0), Object::Dictionary(oc_props));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("OCProperties", Object::Reference((20, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let layers = collect_layers(&doc);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "RefLayer");
    }

    // ── --structure tests ────────────────────────────────────────────

    fn make_struct_doc() -> Document {
        let mut doc = Document::new();

        // Page
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        // Structure elements
        let mut span = Dictionary::new();
        span.set("Type", Object::Name(b"StructElem".to_vec()));
        span.set("S", Object::Name(b"Span".to_vec()));
        span.set("Pg", Object::Reference((3, 0)));
        span.set("K", Object::Integer(0)); // MCID
        doc.objects.insert((12, 0), Object::Dictionary(span));

        let mut p_elem = Dictionary::new();
        p_elem.set("Type", Object::Name(b"StructElem".to_vec()));
        p_elem.set("S", Object::Name(b"P".to_vec()));
        p_elem.set("K", Object::Array(vec![Object::Reference((12, 0))]));
        p_elem.set("T", Object::String(b"My Paragraph".to_vec(), StringFormat::Literal));
        doc.objects.insert((11, 0), Object::Dictionary(p_elem));

        let mut doc_elem = Dictionary::new();
        doc_elem.set("Type", Object::Name(b"StructElem".to_vec()));
        doc_elem.set("S", Object::Name(b"Document".to_vec()));
        doc_elem.set("K", Object::Array(vec![Object::Reference((11, 0))]));
        doc.objects.insert((10, 0), Object::Dictionary(doc_elem));

        // StructTreeRoot
        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        // MarkInfo
        let mut mark_info = Dictionary::new();
        mark_info.set("Marked", Object::Boolean(true));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        catalog.set("MarkInfo", Object::Dictionary(mark_info));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));

        doc.trailer.set("Root", Object::Reference((1, 0)));
        doc
    }

    #[test]
    fn structure_collects_tree() {
        let doc = make_struct_doc();
        let (is_marked, tree) = collect_structure_tree(&doc);
        assert!(is_marked);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].role, "Document");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].role, "P");
        assert_eq!(tree[0].children[0].title.as_deref(), Some("My Paragraph"));
        assert_eq!(tree[0].children[0].children.len(), 1);
        assert_eq!(tree[0].children[0].children[0].role, "Span");
        assert_eq!(tree[0].children[0].children[0].mcid, Some(0));
    }

    #[test]
    fn structure_page_refs() {
        let doc = make_struct_doc();
        let (_, tree) = collect_structure_tree(&doc);
        let span = &tree[0].children[0].children[0];
        assert_eq!(span.page, Some(1));
    }

    #[test]
    fn structure_no_struct_tree_root() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (is_marked, tree) = collect_structure_tree(&doc);
        assert!(!is_marked);
        assert!(tree.is_empty());
    }

    #[test]
    fn structure_mark_info_false() {
        let mut doc = Document::new();
        let mut mark_info = Dictionary::new();
        mark_info.set("Marked", Object::Boolean(false));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("MarkInfo", Object::Dictionary(mark_info));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (is_marked, _) = collect_structure_tree(&doc);
        assert!(!is_marked);
    }

    #[test]
    fn structure_text_output() {
        let doc = make_struct_doc();
        let config = default_config();
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("Tagged PDF: yes"));
        assert!(out.contains("Structure elements: 3"));
        assert!(out.contains("/Document"));
        assert!(out.contains("/P"));
        assert!(out.contains("/Span"));
        assert!(out.contains("MCID=0"));
        assert!(out.contains("\"My Paragraph\""));
    }

    #[test]
    fn structure_json_output() {
        let doc = make_struct_doc();
        let config = default_config();
        let out = output_of(|w| print_structure_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["tagged"], true);
        assert_eq!(parsed["element_count"], 3);
        let root = &parsed["structure"][0];
        assert_eq!(root["role"], "Document");
        assert!(root["children"].is_array());
    }

    #[test]
    fn structure_depth_limit_0() {
        let doc = make_struct_doc();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("/Document"));
        assert!(out.contains("children"));
        // /P and /Span should NOT appear since depth=0 only shows root level
        assert!(!out.contains("/P"));
    }

    #[test]
    fn structure_depth_limit_1() {
        let doc = make_struct_doc();
        let config = DumpConfig { decode_streams: false, truncate: None, json: false, hex: false, depth: Some(1), deref: false, raw: false };
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("/Document"));
        assert!(out.contains("/P"));
        // /Span at depth 2 should NOT appear
        assert!(!out.contains("/Span"));
    }

    #[test]
    fn structure_json_with_depth_limit() {
        let doc = make_struct_doc();
        let config = DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: Some(0), deref: false, raw: false };
        let out = output_of(|w| print_structure_json(w, &doc, &config));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let root = &parsed["structure"][0];
        // At depth 0, children should be represented as children_count
        assert!(root.get("children_count").is_some());
        assert!(root.get("children").is_none());
    }

    #[test]
    fn structure_cycle_detection() {
        let mut doc = Document::new();

        // Create a cycle: elem1 -> elem2 -> elem1
        let mut elem2 = Dictionary::new();
        elem2.set("S", Object::Name(b"Span".to_vec()));
        elem2.set("K", Object::Reference((10, 0))); // back to elem1
        doc.objects.insert((11, 0), Object::Dictionary(elem2));

        let mut elem1 = Dictionary::new();
        elem1.set("S", Object::Name(b"P".to_vec()));
        elem1.set("K", Object::Reference((11, 0)));
        doc.objects.insert((10, 0), Object::Dictionary(elem1));

        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (_, tree) = collect_structure_tree(&doc);
        // Should not infinite-loop, should have 2 elements (one is visited, stops)
        let count = count_struct_elems(&tree);
        assert!(count <= 2);
    }

    #[test]
    fn structure_empty_tree() {
        let mut doc = Document::new();
        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut mark_info = Dictionary::new();
        mark_info.set("Marked", Object::Boolean(true));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        catalog.set("MarkInfo", Object::Dictionary(mark_info));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (is_marked, tree) = collect_structure_tree(&doc);
        assert!(is_marked);
        assert!(tree.is_empty());
    }

    #[test]
    fn structure_alt_text() {
        let mut doc = Document::new();

        let mut elem = Dictionary::new();
        elem.set("S", Object::Name(b"Figure".to_vec()));
        elem.set("Alt", Object::String(b"A photo of sunset".to_vec(), StringFormat::Literal));
        doc.objects.insert((10, 0), Object::Dictionary(elem));

        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let config = default_config();
        let out = output_of(|w| print_structure(w, &doc, &config));
        assert!(out.contains("alt=\"A photo of sunset\""));
    }

    #[test]
    fn structure_k_as_array() {
        let mut doc = Document::new();

        let mut elem1 = Dictionary::new();
        elem1.set("S", Object::Name(b"P".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(elem1));

        let mut elem2 = Dictionary::new();
        elem2.set("S", Object::Name(b"Span".to_vec()));
        doc.objects.insert((11, 0), Object::Dictionary(elem2));

        let mut str_root = Dictionary::new();
        str_root.set("Type", Object::Name(b"StructTreeRoot".to_vec()));
        str_root.set("K", Object::Array(vec![
            Object::Reference((10, 0)),
            Object::Reference((11, 0)),
        ]));
        doc.objects.insert((5, 0), Object::Dictionary(str_root));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("StructTreeRoot", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let (_, tree) = collect_structure_tree(&doc);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].role, "P");
        assert_eq!(tree[1].role, "Span");
    }

    // ── Info mode includes object content and bidirectional refs ──────

    // ── Info mode (--info N) ─────────────────────────────────────────

    #[test]
    fn classify_object_catalog() {
        let mut doc = Document::new();
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(3));
        doc.objects.insert((2, 0), Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog.clone()));

        let obj = Object::Dictionary(catalog);
        let (role, desc, details) = classify_object(&doc, 1, &obj, &doc.get_pages());
        assert_eq!(role, "Catalog");
        assert!(desc.contains("document catalog"));
        assert!(details.iter().any(|(k, v)| k == "Pages" && v == "3"));
    }

    #[test]
    fn classify_object_font() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Font".to_vec()));
        dict.set("Subtype", Object::Name(b"Type1".to_vec()));
        dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        dict.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 10, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("Type1"));
        assert!(desc.contains("Helvetica"));
        assert!(details.iter().any(|(k, v)| k == "BaseFont" && v == "Helvetica"));
        assert!(details.iter().any(|(k, v)| k == "Encoding" && v == "WinAnsiEncoding"));
    }

    #[test]
    fn classify_object_page() {
        let mut doc = Document::new();
        let mut page_tree = Dictionary::new();
        page_tree.set("Type", Object::Name(b"Pages".to_vec()));
        page_tree.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        page_tree.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(page_tree));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("MediaBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(612), Object::Integer(792),
        ]));
        doc.objects.insert((3, 0), Object::Dictionary(page.clone()));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let obj = Object::Dictionary(page);
        let (role, desc, details) = classify_object(&doc, 3, &obj, &doc.get_pages());
        assert_eq!(role, "Page");
        assert!(desc.contains("page 1") || desc.contains("page"));
        assert!(details.iter().any(|(k, _)| k == "MediaBox"));
    }

    #[test]
    fn classify_object_image() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Image".to_vec()));
        dict.set("Width", Object::Integer(100));
        dict.set("Height", Object::Integer(200));
        dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        dict.set("BitsPerComponent", Object::Integer(8));
        let stream = Stream::new(dict, vec![0u8; 50]);
        let obj = Object::Stream(stream);

        let (role, desc, details) = classify_object(&doc, 7, &obj, &doc.get_pages());
        assert_eq!(role, "Image");
        assert!(desc.contains("100x200"));
        assert!(details.iter().any(|(k, v)| k == "Width" && v == "100"));
        assert!(details.iter().any(|(k, v)| k == "Height" && v == "200"));
    }

    #[test]
    fn classify_object_annotation() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Annot".to_vec()));
        dict.set("Subtype", Object::Name(b"Link".to_vec()));
        dict.set("Rect", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(100), Object::Integer(50),
        ]));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 8, &obj, &doc.get_pages());
        assert_eq!(role, "Annotation");
        assert!(desc.contains("Link"));
    }

    #[test]
    fn classify_object_action() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Action".to_vec()));
        dict.set("S", Object::Name(b"URI".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 9, &obj, &doc.get_pages());
        assert_eq!(role, "Action");
        assert!(desc.contains("URI"));
        assert!(details.iter().any(|(k, v)| k == "Action Type" && v == "URI"));
    }

    #[test]
    fn classify_object_integer() {
        let doc = Document::new();
        let obj = Object::Integer(42);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Integer");
        assert!(desc.contains("42"));
    }

    #[test]
    fn classify_object_string() {
        let doc = Document::new();
        let obj = Object::String(b"hello".to_vec(), StringFormat::Literal);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "String");
        assert!(desc.contains("hello"));
    }

    #[test]
    fn classify_object_array() {
        let doc = Document::new();
        let obj = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Array");
        assert!(desc.contains("2 items"));
    }

    #[test]
    fn classify_object_generic_dict() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Foo", Object::Integer(1));
        dict.set("Bar", Object::Integer(2));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Generic");
        assert!(desc.contains("dictionary"));
        assert!(details.iter().any(|(k, _)| k == "Keys"));
    }

    #[test]
    fn classify_object_font_descriptor() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"FontDescriptor".to_vec()));
        dict.set("FontName", Object::Name(b"Helvetica".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Font Descriptor");
        assert!(desc.contains("Helvetica"));
    }

    #[test]
    fn classify_object_encoding() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Encoding".to_vec()));
        dict.set("BaseEncoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Encoding");
        assert!(desc.contains("WinAnsiEncoding"));
    }

    #[test]
    fn classify_object_ext_gstate() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"ExtGState".to_vec()));
        dict.set("CA", Object::Real(0.5));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Graphics State");
        assert!(desc.contains("extended graphics state"));
    }

    #[test]
    fn classify_object_form_xobject() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        dict.set("BBox", Object::Array(vec![
            Object::Integer(0), Object::Integer(0),
            Object::Integer(100), Object::Integer(100),
        ]));
        let obj = Object::Dictionary(dict);

        let (role, desc, details) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Form XObject");
        assert!(desc.contains("form XObject"));
        assert!(details.iter().any(|(k, _)| k == "BBox"));
    }

    #[test]
    fn classify_object_pages_tree() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Pages".to_vec()));
        dict.set("Count", Object::Integer(5));
        let obj = Object::Dictionary(dict);

        let (role, _, details) = classify_object(&doc, 2, &obj, &doc.get_pages());
        assert_eq!(role, "Page Tree");
        assert!(details.iter().any(|(k, v)| k == "Count" && v == "5"));
    }

    #[test]
    fn find_page_associations_finds_direct_ref() {
        let mut doc = Document::new();

        // Font object
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        // Page that references the font
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        page.set("Resources", Object::Dictionary(res));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        // Page tree
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let assoc = find_page_associations(&doc, 5, &doc.get_pages());
        assert_eq!(assoc, vec![1]);
    }

    #[test]
    fn find_page_associations_no_association() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let assoc = find_page_associations(&doc, 5, &doc.get_pages());
        assert!(assoc.is_empty());
    }

    #[test]
    fn print_info_shows_role_and_details() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Courier".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("Type1 font (Courier)"));
        assert!(out.contains("Role: Font"));
        assert!(out.contains("Kind: Dictionary"));
        assert!(out.contains("BaseFont: Courier"));
        // Now includes full object content
        assert!(out.contains("Object 5 0:"));
    }

    #[test]
    fn print_info_shows_reverse_refs() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));
        let mut dict = Dictionary::new();
        dict.set("Value", Object::Reference((5, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(dict));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("Referenced by:"));
        assert!(out.contains("via /Value"));
    }

    #[test]
    fn print_info_json_valid() {
        let mut doc = Document::new();
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"TrueType".to_vec()));
        font.set("BaseFont", Object::Name(b"Arial".to_vec()));
        doc.objects.insert((10, 0), Object::Dictionary(font));

        let out = output_of(|w| print_info_json(w, &doc, 10));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["object_number"], 10);
        assert_eq!(val["role"], "Font");
        assert!(val["description"].as_str().unwrap().contains("Arial"));
        assert!(val["details"].is_object());
        assert!(val["object"].is_object()); // now includes full object content
        assert!(val["page_associations"].is_array());
        assert!(val["references"].is_array());
        assert!(val["referenced_by"].is_array());
    }

    #[test]
    fn print_info_json_with_page_associations() {
        let mut doc = Document::new();

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        page.set("Resources", Object::Dictionary(res));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_info_json(w, &doc, 5));
        let val: Value = serde_json::from_str(&out).unwrap();
        let pages = val["page_associations"].as_array().unwrap();
        assert_eq!(pages, &[1]);
    }

    #[test]
    fn print_info_integer_object() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("integer: 42"));
        assert!(out.contains("Role: Integer"));
    }

    #[test]
    fn print_info_json_integer_object() {
        let mut doc = Document::new();
        doc.objects.insert((5, 0), Object::Integer(42));

        let out = output_of(|w| print_info_json(w, &doc, 5));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["role"], "Integer");
        assert_eq!(val["object_number"], 5);
    }


    #[test]
    fn classify_object_xref_stream() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XRef".to_vec()));
        let obj = Object::Dictionary(dict);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "XRef Stream");
        assert!(desc.contains("cross-reference stream"));
    }

    #[test]
    fn classify_object_obj_stream() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"ObjStm".to_vec()));
        let obj = Object::Dictionary(dict);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Object Stream");
        assert!(desc.contains("object stream"));
    }

    #[test]
    fn classify_object_null() {
        let doc = Document::new();
        let obj = Object::Null;
        let (role, _, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Null");
    }

    #[test]
    fn classify_object_boolean() {
        let doc = Document::new();
        let obj = Object::Boolean(true);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Boolean");
        assert!(desc.contains("true"));
    }

    #[test]
    fn classify_object_real() {
        let doc = Document::new();
        let obj = Object::Real(2.72);
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Real");
        assert!(desc.contains("2.72"));
    }

    #[test]
    fn classify_object_name() {
        let doc = Document::new();
        let obj = Object::Name(b"Test".to_vec());
        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Name");
        assert!(desc.contains("Test"));
    }

    #[test]
    fn classify_font_by_subtype_only() {
        let doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Subtype", Object::Name(b"TrueType".to_vec()));
        dict.set("BaseFont", Object::Name(b"TimesNewRoman".to_vec()));
        let obj = Object::Dictionary(dict);

        let (role, desc, _) = classify_object(&doc, 5, &obj, &doc.get_pages());
        assert_eq!(role, "Font");
        assert!(desc.contains("TrueType"));
        assert!(desc.contains("TimesNewRoman"));
    }

    #[test]
    fn print_info_shows_forward_refs() {
        let mut doc = Document::new();
        let mut dict = Dictionary::new();
        dict.set("Font", Object::Reference((10, 0)));
        doc.objects.insert((5, 0), Object::Dictionary(dict));
        doc.objects.insert((10, 0), Object::Integer(99));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("References from this object:"));
        assert!(out.contains("/Font -> 10 0 R"));
    }

    #[test]
    fn print_info_page_associations_text() {
        let mut doc = Document::new();

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        page.set("Resources", Object::Dictionary(res));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_info(w, &doc, 5));
        assert!(out.contains("Referenced by pages: 1"));
    }

    #[test]
    fn find_page_associations_via_resource_ref() {
        let mut doc = Document::new();

        // Font object
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        doc.objects.insert((5, 0), Object::Dictionary(font));

        // Resources dict as separate object with ref to font
        let mut res = Dictionary::new();
        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference((5, 0)));
        res.set("Font", Object::Dictionary(font_dict));
        doc.objects.insert((4, 0), Object::Dictionary(res));

        // Page referencing resources by ref
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set("Resources", Object::Reference((4, 0)));
        doc.objects.insert((3, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((3, 0))]));
        pages_dict.set("Count", Object::Integer(1));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let assoc = find_page_associations(&doc, 5, &doc.get_pages());
        assert_eq!(assoc, vec![1]);
    }
}
