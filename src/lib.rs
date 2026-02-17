pub(crate) mod types;
pub(crate) mod stream;
pub(crate) mod helpers;
pub(crate) mod object;
pub(crate) mod refs;
pub(crate) mod summary;
pub(crate) mod search;
pub(crate) mod text;
pub(crate) mod operators;
pub(crate) mod resources;
pub(crate) mod forms;
pub(crate) mod fonts;
pub(crate) mod images;
pub(crate) mod validate;
pub(crate) mod bookmarks;
pub(crate) mod annotations;
pub(crate) mod security;
pub(crate) mod embedded;
pub(crate) mod page_labels;
pub(crate) mod tree;
pub(crate) mod layers;
pub(crate) mod structure;
pub(crate) mod inspect;
pub(crate) mod page_info;
pub(crate) mod find_text;

use clap::Parser;
use lopdf::{Document, Object};
use serde_json::Value;
use std::io::{self, Write};

use types::{Args, DocMode, DumpConfig, PageSpec, ResolvedMode, StandaloneMode};

pub fn run() {
    let args = Args::parse();

    let resolved = args.resolve_mode().unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    });

    // Modifier validation
    if args.raw {
        if !matches!(resolved, ResolvedMode::Standalone(StandaloneMode::Object { .. })) {
            eprintln!("Error: --raw requires --object.");
            std::process::exit(1);
        }
        if args.decode {
            eprintln!("Error: --raw and --decode cannot be used together.");
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
        decode: args.decode,
        truncate: args.truncate,
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

    let mut out = io::stdout().lock();

    match resolved {
        ResolvedMode::Default => {
            dispatch_default(&mut out, &doc, &config, page_spec.as_ref());
        }
        ResolvedMode::Standalone(mode) => {
            dispatch_standalone(&mut out, &doc, &config, page_spec.as_ref(), &args, mode);
        }
        ResolvedMode::Combined(modes) => {
            dispatch_combined(&mut out, &doc, &config, page_spec.as_ref(), &args, &modes);
        }
    }
}

fn dispatch_default(
    out: &mut impl Write,
    doc: &Document,
    config: &DumpConfig,
    page_spec: Option<&PageSpec>,
) {
    if let Some(spec) = page_spec {
        if config.json {
            page_info::print_page_info_json(out, doc, spec);
        } else {
            page_info::print_page_info(out, doc, spec);
        }
    } else if config.json {
        summary::print_overview_json(out, doc);
    } else {
        summary::print_overview(out, doc);
    }
}

fn dispatch_standalone(
    out: &mut impl Write,
    doc: &Document,
    config: &DumpConfig,
    page_spec: Option<&PageSpec>,
    args: &Args,
    mode: StandaloneMode,
) {
    match mode {
        StandaloneMode::ExtractStream { obj_num, ref output } => {
            let object_id = (obj_num, 0);
            match doc.get_object(object_id) {
                Ok(Object::Stream(s)) => {
                    let (decoded_content, warning) = stream::decode_stream(s);
                    if let Some(warn) = &warning {
                        eprintln!("Warning: {}", warn);
                    }
                    if let Err(e) = std::fs::write(output, &*decoded_content) {
                        eprintln!("Error writing to output file: {}", e);
                        std::process::exit(1);
                    }
                    writeln!(out, "Successfully extracted object {} to '{}'.", obj_num, output.display()).unwrap();
                }
                Ok(_) => {
                    eprintln!("Error: Object {} is not a stream and cannot be extracted to a file.", obj_num);
                    std::process::exit(1);
                }
                Err(_) => {
                    eprintln!("Error: Object {} not found in the document.", obj_num);
                    std::process::exit(1);
                }
            }
        }
        StandaloneMode::Object { ref nums } => {
            if config.json {
                object::print_objects_json(out, doc, nums, config);
            } else {
                object::print_objects(out, doc, nums, config);
            }
        }
        StandaloneMode::Inspect { obj_num } => {
            if config.json {
                inspect::print_info_json(out, doc, obj_num);
            } else {
                inspect::print_info(out, doc, obj_num);
            }
        }
        StandaloneMode::Search { ref expr, list_modifier } => {
            let conditions = match search::parse_search_expr(expr) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Invalid search expression: {}", e);
                    std::process::exit(1);
                }
            };
            if config.json {
                search::search_objects_json(out, doc, expr, &conditions, config);
            } else {
                search::search_objects(out, doc, &conditions, config, list_modifier);
            }
        }
    }
    let _ = (page_spec, args); // acknowledge unused params for future use
}

fn dispatch_combined(
    out: &mut impl Write,
    doc: &Document,
    config: &DumpConfig,
    page_spec: Option<&PageSpec>,
    args: &Args,
    modes: &[DocMode],
) {
    let multi = modes.len() > 1;

    if config.json {
        if multi {
            // Multiple modes: wrap in { "key": value, ... }
            let mut map = serde_json::Map::new();
            for mode in modes {
                let value = build_mode_json_value(mode, doc, config, page_spec, args);
                map.insert(mode.json_key().to_string(), value);
            }
            let output = Value::Object(map);
            writeln!(out, "{}", serde_json::to_string_pretty(&output).unwrap()).unwrap();
        } else {
            // Single mode: output directly (unchanged schema)
            let value = build_mode_json_value(&modes[0], doc, config, page_spec, args);
            writeln!(out, "{}", serde_json::to_string_pretty(&value).unwrap()).unwrap();
        }
    } else {
        for (i, mode) in modes.iter().enumerate() {
            if multi {
                if i > 0 {
                    writeln!(out).unwrap();
                }
                writeln!(out, "=== {} ===", mode.label()).unwrap();
            }
            dispatch_mode_text(out, mode, doc, config, page_spec, args);
        }
    }
}

fn build_mode_json_value(
    mode: &DocMode,
    doc: &Document,
    config: &DumpConfig,
    page_spec: Option<&PageSpec>,
    args: &Args,
) -> Value {
    match mode {
        DocMode::List => summary::list_json_value(doc),
        DocMode::Validate => validate::validation_json_value(doc),
        DocMode::Fonts => fonts::fonts_json_value(doc),
        DocMode::Images => images::images_json_value(doc),
        DocMode::Forms => forms::forms_json_value(doc),
        DocMode::Bookmarks => bookmarks::bookmarks_json_value(doc),
        DocMode::Annotations => annotations::annotations_json_value(doc, page_spec),
        DocMode::Text => text::text_json_value(doc, page_spec),
        DocMode::Operators => operators::operators_json_value(doc, page_spec),
        DocMode::Tags => structure::structure_json_value(doc, config),
        DocMode::Tree => tree::tree_json_value(doc, config),
        DocMode::FindText => find_text::find_text_json_value(doc, args.find_text.as_deref().unwrap_or(""), page_spec),
        DocMode::Detail(sub) => match sub {
            types::DetailSub::Security => security::security_json_value(doc, &args.file),
            types::DetailSub::Embedded => embedded::embedded_json_value(doc),
            types::DetailSub::Labels => page_labels::labels_json_value(doc),
            types::DetailSub::Layers => layers::layers_json_value(doc),
        },
    }
}

fn dispatch_mode_text(
    out: &mut impl Write,
    mode: &DocMode,
    doc: &Document,
    config: &DumpConfig,
    page_spec: Option<&PageSpec>,
    args: &Args,
) {
    match mode {
        DocMode::List => summary::print_summary(out, doc),
        DocMode::Validate => validate::print_validation(out, doc),
        DocMode::Fonts => fonts::print_fonts(out, doc),
        DocMode::Images => images::print_images(out, doc),
        DocMode::Forms => forms::print_forms(out, doc),
        DocMode::Bookmarks => bookmarks::print_bookmarks(out, doc),
        DocMode::Annotations => annotations::print_annotations(out, doc, page_spec),
        DocMode::Text => text::print_text(out, doc, page_spec),
        DocMode::Operators => operators::print_operators(out, doc, page_spec),
        DocMode::Tags => structure::print_structure(out, doc, config),
        DocMode::FindText => find_text::print_find_text(out, doc, args.find_text.as_deref().unwrap_or(""), page_spec),
        DocMode::Tree => {
            if args.dot {
                tree::print_tree_dot(out, doc, config);
            } else {
                tree::print_tree(out, doc, config);
            }
        }
        DocMode::Detail(sub) => match sub {
            types::DetailSub::Security => security::print_security(out, doc, &args.file),
            types::DetailSub::Embedded => embedded::print_embedded_files(out, doc),
            types::DetailSub::Labels => page_labels::print_page_labels(out, doc),
            types::DetailSub::Layers => layers::print_layers(out, doc),
        },
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use lopdf::{Document, Object, Stream};
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;
    use crate::types::DumpConfig;

    pub fn output_of(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut buf = Vec::new();
        f(&mut buf);
        String::from_utf8(buf).unwrap()
    }

    pub fn empty_doc() -> Document {
        let mut doc = Document::new();
        doc.version = "1.5".to_string();
        doc
    }

    pub fn default_config() -> DumpConfig {
        DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        }
    }

    pub fn make_stream(filter: Option<Object>, content: Vec<u8>) -> Stream {
        let mut dict = lopdf::Dictionary::new();
        if let Some(f) = filter {
            dict.set("Filter", f);
        }
        Stream::new(dict, content)
    }

    pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    pub fn json_config() -> DumpConfig {
        DumpConfig { decode: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false }
    }

    pub fn build_two_page_doc() -> Document {
        use lopdf::Dictionary;

        let mut doc = Document::new();

        let c1 = Stream::new(Dictionary::new(), b"BT /F1 12 Tf (Page1) Tj ET".to_vec());
        let c1_id = doc.add_object(Object::Stream(c1));
        let c2 = Stream::new(Dictionary::new(), b"BT /F1 12 Tf (Page2) Tj ET".to_vec());
        let c2_id = doc.add_object(Object::Stream(c2));

        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let font_id = doc.add_object(Object::Dictionary(font));

        let mut f1 = Dictionary::new();
        f1.set("F1", Object::Reference(font_id));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(f1));
        let resources_id = doc.add_object(Object::Dictionary(resources));

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(2));
        pages.set("Kids", Object::Array(vec![]));
        let pages_id = doc.add_object(Object::Dictionary(pages));

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

        doc
    }

    pub fn build_page_doc_with_content(content: &[u8]) -> Document {
        use lopdf::Dictionary;

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

    pub fn make_page_with_annots(doc: &mut Document, page_id: lopdf::ObjectId, parent_id: lopdf::ObjectId, annot_ids: Vec<lopdf::ObjectId>) {
        use lopdf::Dictionary;

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
}
