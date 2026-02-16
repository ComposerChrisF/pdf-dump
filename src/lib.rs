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
pub(crate) mod diff;
pub(crate) mod fonts;
pub(crate) mod images;
pub(crate) mod validate;
pub(crate) mod stats;
pub(crate) mod bookmarks;
pub(crate) mod annotations;
pub(crate) mod security;
pub(crate) mod embedded;
pub(crate) mod page_labels;
pub(crate) mod tree;
pub(crate) mod layers;
pub(crate) mod structure;
pub(crate) mod info;
pub(crate) mod page_info;

use clap::Parser;
use lopdf::{Document, Object};
use std::collections::BTreeSet;
use std::io::{self, Write};

use types::{Args, DumpConfig, PageSpec, parse_object_spec};

pub fn run() {
    let args = Args::parse();

    // Mutual exclusivity check
    // --list alone is a mode; with --search it becomes a modifier
    // --page alone is a mode; with --text it becomes a filter
    let mode_count = [
        args.extract_stream.is_some(),
        args.object.is_some(),
        args.list && args.search.is_none(),
        args.page.is_some() && !args.text && !args.annotations && !args.operators && !args.resources,
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
        args.layers,
        args.tags,
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
            || (args.list && args.search.is_none())
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
            || args.layers
            || args.tags
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

    let config = DumpConfig {
        decode_streams: args.decode_streams,
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
        let result = diff::compare_pdfs(&doc, &doc2, page_spec.as_ref());
        let mut out = io::stdout().lock();
        if config.json {
            diff::print_diff_json(&mut out, &result, &args.file, diff_path);
        } else {
            diff::print_diff(&mut out, &result, &args.file, diff_path);
        }
        return;
    }

    if let Some(object_id) = args.extract_stream {
        let output_path = args.output.as_ref().unwrap();
        let object_id = (object_id, 0);
        match doc.get_object(object_id) {
            Ok(Object::Stream(s)) => {
                let (decoded_content, warning) = stream::decode_stream(s);
                if let Some(warn) = &warning {
                    eprintln!("Warning: {}", warn);
                }
                if let Err(e) = std::fs::write(output_path, &*decoded_content) {
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
        let conditions = match search::parse_search_expr(search_expr) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error: Invalid search expression: {}", e);
                std::process::exit(1);
            }
        };
        let mut out = io::stdout().lock();
        if config.json {
            search::search_objects_json(&mut out, &doc, search_expr, &conditions, &config);
        } else {
            search::search_objects(&mut out, &doc, &conditions, &config, args.list);
        }
    } else if args.text {
        let mut out = io::stdout().lock();
        if config.json {
            text::print_text_json(&mut out, &doc, page_spec.as_ref());
        } else {
            text::print_text(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.operators {
        let mut out = io::stdout().lock();
        if config.json {
            operators::print_operators_json(&mut out, &doc, page_spec.as_ref());
        } else {
            operators::print_operators(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.resources {
        let mut out = io::stdout().lock();
        if config.json {
            resources::print_resources_json(&mut out, &doc, page_spec.as_ref());
        } else {
            resources::print_resources(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.forms {
        let mut out = io::stdout().lock();
        if config.json {
            forms::print_forms_json(&mut out, &doc);
        } else {
            forms::print_forms(&mut out, &doc);
        }
    } else if let Some(info_num) = args.info {
        let mut out = io::stdout().lock();
        if config.json {
            info::print_info_json(&mut out, &doc, info_num);
        } else {
            info::print_info(&mut out, &doc, info_num);
        }
    } else if let Some(target) = args.refs_to {
        let mut out = io::stdout().lock();
        if config.json {
            refs::print_refs_to_json(&mut out, &doc, target);
        } else {
            refs::print_refs_to(&mut out, &doc, target);
        }
    } else if args.fonts {
        let mut out = io::stdout().lock();
        if config.json {
            fonts::print_fonts_json(&mut out, &doc);
        } else {
            fonts::print_fonts(&mut out, &doc);
        }
    } else if args.images {
        let mut out = io::stdout().lock();
        if config.json {
            images::print_images_json(&mut out, &doc);
        } else {
            images::print_images(&mut out, &doc);
        }
    } else if args.validate {
        let mut out = io::stdout().lock();
        if config.json {
            validate::print_validation_json(&mut out, &doc);
        } else {
            validate::print_validation(&mut out, &doc);
        }
    } else if args.stats {
        let mut out = io::stdout().lock();
        if config.json {
            stats::print_stats_json(&mut out, &doc);
        } else {
            stats::print_stats(&mut out, &doc);
        }
    } else if args.bookmarks {
        let mut out = io::stdout().lock();
        if config.json {
            bookmarks::print_bookmarks_json(&mut out, &doc);
        } else {
            bookmarks::print_bookmarks(&mut out, &doc);
        }
    } else if args.annotations {
        let mut out = io::stdout().lock();
        if config.json {
            annotations::print_annotations_json(&mut out, &doc, page_spec.as_ref());
        } else {
            annotations::print_annotations(&mut out, &doc, page_spec.as_ref());
        }
    } else if args.security {
        let mut out = io::stdout().lock();
        if config.json {
            security::print_security_json(&mut out, &doc, &args.file);
        } else {
            security::print_security(&mut out, &doc, &args.file);
        }
    } else if args.embedded_files {
        let mut out = io::stdout().lock();
        if config.json {
            embedded::print_embedded_files_json(&mut out, &doc);
        } else {
            embedded::print_embedded_files(&mut out, &doc);
        }
    } else if args.page_labels {
        let mut out = io::stdout().lock();
        if config.json {
            page_labels::print_page_labels_json(&mut out, &doc);
        } else {
            page_labels::print_page_labels(&mut out, &doc);
        }
    } else if args.layers {
        let mut out = io::stdout().lock();
        if config.json {
            layers::print_layers_json(&mut out, &doc);
        } else {
            layers::print_layers(&mut out, &doc);
        }
    } else if args.tags {
        let mut out = io::stdout().lock();
        if config.json {
            structure::print_structure_json(&mut out, &doc, &config);
        } else {
            structure::print_structure(&mut out, &doc, &config);
        }
    } else if args.tree {
        let mut out = io::stdout().lock();
        if args.dot {
            tree::print_tree_dot(&mut out, &doc, &config);
        } else if config.json {
            tree::print_tree_json(&mut out, &doc, &config);
        } else {
            tree::print_tree(&mut out, &doc, &config);
        }
    } else if let Some(ref nums) = object_nums {
        let mut out = io::stdout().lock();
        if config.json {
            object::print_objects_json(&mut out, &doc, nums, &config);
        } else {
            object::print_objects(&mut out, &doc, nums, &config);
        }
    } else if args.list {
        let mut out = io::stdout().lock();
        if config.json {
            summary::print_summary_json(&mut out, &doc);
        } else {
            summary::print_summary(&mut out, &doc);
        }
    } else if let Some(ref spec) = page_spec {
        let mut out = io::stdout().lock();
        if config.json {
            page_info::print_page_info_json(&mut out, &doc, spec);
        } else {
            page_info::print_page_info(&mut out, &doc, spec);
        }
    } else if args.dump {
        let mut out = io::stdout().lock();
        if config.json {
            object::dump_json(&mut out, &doc, &config);
        } else {
            writeln!(out, "Trailer:").unwrap();
            let visited_for_print = BTreeSet::new();
            let mut trailer_refs = BTreeSet::new();
            object::print_object(
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
                object::dump_object_and_children(&mut out, root_id, &doc, &mut visited_for_traverse, &config, false, 0);
            } else {
                eprintln!("Warning: /Root not found or not a reference in trailer.");
            }
        }
    } else {
        // Default: overview mode
        let mut out = io::stdout().lock();
        if config.json {
            summary::print_overview_json(&mut out, &doc);
        } else {
            summary::print_overview(&mut out, &doc);
        }
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
            decode_streams: false,
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
        DumpConfig { decode_streams: false, truncate: None, json: true, hex: false, depth: None, deref: false, raw: false }
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
