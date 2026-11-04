use lopdf::{Dictionary, Document, Object, ObjectId};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;

use crate::helpers::{get_catalog, name_to_string, resolve_array, resolve_dict, walk_name_tree};
use crate::text_string::decode_text_string;

pub(crate) struct EmbeddedFileInfo {
    pub name: String,
    pub filename: String,
    pub filename_decoded: bool,
    pub mime_type: String,
    pub size: Option<i64>,
    pub object_number: u32,
    /// `None` for a filespec written directly inside another object, or for no filespec at all.
    pub filespec_object: Option<ObjectId>,
    pub reached_from: &'static str,
}

/// How a file was found, as `reached_from`: the document's `EmbeddedFiles` name tree; a filespec
/// elsewhere (an action, a `FileAttachment` annotation, an associated-files array, …); or a stream
/// marked `/Type /EmbeddedFile` that no `/EF` names.
const FROM_NAME_TREE: &str = "name_tree";
const FROM_FILESPEC: &str = "filespec";
const FROM_ORPHAN: &str = "orphan";

/// Every embedded file in the document, however it is reached.  The name tree comes first, for its
/// names; then a walk of every object, which is what finds a filespec behind a `Launch` or `GoToE`
/// action or an annotation's `/FS`, and any filespec the tree cannot show.  A stream reached twice
/// is one file.
pub(crate) fn collect_embedded_files(doc: &Document) -> Vec<EmbeddedFileInfo> {
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();

    let tree = get_catalog(doc)
        .and_then(|catalog| catalog.get(b"Names").ok())
        .and_then(|names| resolve_dict(doc, names))
        .and_then(|names| names.get(b"EmbeddedFiles").ok())
        .and_then(|ef_tree| resolve_dict(doc, ef_tree));
    if let Some(tree) = tree {
        for (name, value) in walk_name_tree(doc, tree) {
            let (filespec_id, filespec) = match &value {
                Object::Reference(id) => match doc.get_object(*id) {
                    Ok(Object::Dictionary(d)) => (Some(*id), d),
                    _ => continue,
                },
                Object::Dictionary(d) => (None, d),
                _ => continue,
            };
            add_filespec(
                doc,
                filespec,
                filespec_id,
                Some(&name),
                FROM_NAME_TREE,
                &mut files,
                &mut seen,
            );
        }
    }

    for (&id, object) in &doc.objects {
        scan_for_filespecs(doc, object, Some(id), &mut files, &mut seen);
    }

    for (&id, object) in &doc.objects {
        if let Object::Stream(stream) = object
            && !seen.contains(&id)
            && stream
                .dict
                .get(b"Type")
                .ok()
                .and_then(|t| t.as_name().ok())
                .is_some_and(|t| t == b"EmbeddedFile")
        {
            seen.insert(id);
            files.push(file_info(
                doc,
                id,
                "-".to_string(),
                "-".to_string(),
                true,
                None,
                FROM_ORPHAN,
            ));
        }
    }

    files
}

/// Walks one object for dictionaries holding `/EF`.  Direct values are walked here; a reference
/// need not be followed, since every indirect object gets its own turn.
fn scan_for_filespecs(
    doc: &Document,
    object: &Object,
    own_id: Option<ObjectId>,
    files: &mut Vec<EmbeddedFileInfo>,
    seen: &mut BTreeSet<ObjectId>,
) {
    let dict = match object {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &s.dict,
        Object::Array(items) => {
            for item in items {
                scan_for_filespecs(doc, item, None, files, seen);
            }
            return;
        }
        _ => return,
    };
    if dict.has(b"EF") {
        add_filespec(doc, dict, own_id, None, FROM_FILESPEC, files, seen);
    }
    for (_, value) in dict.iter() {
        scan_for_filespecs(doc, value, None, files, seen);
    }
}

fn add_filespec(
    doc: &Document,
    filespec: &Dictionary,
    filespec_id: Option<ObjectId>,
    tree_name: Option<&str>,
    reached_from: &'static str,
    files: &mut Vec<EmbeddedFileInfo>,
    seen: &mut BTreeSet<ObjectId>,
) {
    let Some(ef) = filespec.get(b"EF").ok().and_then(|v| resolve_dict(doc, v)) else {
        return;
    };
    let declared = filespec
        .get(b"UF")
        .ok()
        .or_else(|| filespec.get(b"F").ok())
        .and_then(|v| match v {
            Object::String(bytes, _) => Some(decode_text_string(bytes)),
            _ => None,
        });
    let (filename, filename_decoded) = match declared {
        Some(d) => (d.text, d.decoded),
        None => (tree_name.unwrap_or("-").to_string(), true),
    };
    let name = tree_name.unwrap_or(&filename).to_string();
    // Every entry, not only the five keys Table 46 names, each of which may be a different stream.
    let mut streams: Vec<ObjectId> = ef
        .iter()
        .filter_map(|(_, value)| value.as_reference().ok())
        .collect();
    // /RF names related files: for each /EF key, an array of pairs, a name and a stream.
    if let Some(related) = filespec.get(b"RF").ok().and_then(|v| resolve_dict(doc, v)) {
        for (_, pairs) in related.iter() {
            if let Some(pairs) = resolve_array(doc, pairs) {
                streams.extend(pairs.iter().filter_map(|item| item.as_reference().ok()));
            }
        }
    }
    for stream_id in streams {
        if seen.insert(stream_id) {
            files.push(file_info(
                doc,
                stream_id,
                name.clone(),
                filename.clone(),
                filename_decoded,
                filespec_id,
                reached_from,
            ));
        }
    }
}

fn file_info(
    doc: &Document,
    stream_id: ObjectId,
    name: String,
    filename: String,
    filename_decoded: bool,
    filespec_object: Option<ObjectId>,
    reached_from: &'static str,
) -> EmbeddedFileInfo {
    let stream_dict = match doc.get_object(stream_id) {
        Ok(Object::Stream(s)) => Some(&s.dict),
        _ => None,
    };
    let mime_type = stream_dict
        .and_then(|d| d.get(b"Subtype").ok())
        .and_then(name_to_string)
        .unwrap_or_else(|| "-".to_string());
    // /Params /Size is the uncompressed size.
    let size = stream_dict
        .and_then(|d| d.get(b"Params").ok())
        .and_then(|v| resolve_dict(doc, v))
        .and_then(|d| d.get(b"Size").ok())
        .and_then(|v| v.as_i64().ok());
    EmbeddedFileInfo {
        name,
        filename,
        filename_decoded,
        mime_type,
        size,
        object_number: stream_id.0,
        filespec_object,
        reached_from,
    }
}

pub(crate) fn print_embedded_files(writer: &mut impl Write, doc: &Document) {
    let files = collect_embedded_files(doc);
    wln!(writer, "{} embedded files\n", files.len());
    if files.is_empty() {
        return;
    }
    wln!(
        writer,
        "  {:>4}  {:<30} {:<24} {:>8}  Reached from",
        "Obj#",
        "Filename",
        "MIME Type",
        "Size"
    );
    for f in &files {
        let size_str = f
            .size
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());
        wln!(
            writer,
            "  {:>4}  {:<30} {:<24} {:>8}  {}",
            f.object_number,
            f.filename,
            f.mime_type,
            size_str,
            f.reached_from
        );
    }
}

pub(crate) fn embedded_json_value(doc: &Document) -> Value {
    let files = collect_embedded_files(doc);
    let items: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "filename": f.filename,
                "filename_decoded": f.filename_decoded,
                "mime_type": f.mime_type,
                "size": f.size,
                "object_number": f.object_number,
                "filespec_object": f.filespec_object.map(|id| id.0),
                "filespec_generation": f.filespec_object.map(|id| id.1),
                "reached_from": f.reached_from,
            })
        })
        .collect();
    json!({
        "embedded_file_count": items.len(),
        "embedded_files": items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    // ── plan-0006: every attachment, however it is reached ─────────────

    fn file_stream() -> Object {
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"EmbeddedFile".to_vec()));
        Object::Stream(Stream::new(d, b"Lockbox code: 7301".to_vec()))
    }

    fn filespec(name: &[u8], stream: ObjectId) -> Dictionary {
        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference(stream));
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"Filespec".to_vec()));
        d.set("F", Object::String(name.to_vec(), StringFormat::Literal));
        d.set("EF", Object::Dictionary(ef));
        d
    }

    /// A catalog with `extra` entries, and `objs` added as they stand.
    fn catalog_doc(extra: Vec<(&str, Object)>, objs: Vec<(ObjectId, Object)>) -> Document {
        let mut doc = Document::new();
        for (id, o) in objs {
            doc.objects.insert(id, o);
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        for (k, v) in extra {
            catalog.set(k, v);
        }
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        doc
    }

    fn tree_of(names: Vec<Object>) -> Object {
        let mut tree = Dictionary::new();
        tree.set("Names", Object::Array(names));
        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Dictionary(tree));
        Object::Dictionary(names_dict)
    }

    #[test]
    fn a_direct_filespec_in_the_name_tree_is_counted() {
        let doc = catalog_doc(
            vec![(
                "Names",
                tree_of(vec![
                    Object::String(b"code.txt".to_vec(), StringFormat::Literal),
                    Object::Dictionary(filespec(b"code.txt", (6, 0))),
                ]),
            )],
            vec![((6, 0), file_stream())],
        );
        let files = collect_embedded_files(&doc);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "code.txt");
        assert_eq!(files[0].object_number, 6);
        assert_eq!(files[0].filespec_object, None);
        assert_eq!(files[0].reached_from, FROM_NAME_TREE);
    }

    #[test]
    fn a_filespec_behind_an_action_is_counted() {
        for action in [&b"Launch"[..], b"GoToE"] {
            let mut a = Dictionary::new();
            a.set("S", Object::Name(action.to_vec()));
            a.set("F", Object::Dictionary(filespec(b"run.exe", (6, 0))));
            let doc = catalog_doc(
                vec![("OpenAction", Object::Dictionary(a))],
                vec![((6, 0), file_stream())],
            );
            let files = collect_embedded_files(&doc);
            assert_eq!(files.len(), 1, "{}", String::from_utf8_lossy(action));
            assert_eq!(files[0].filename, "run.exe");
            assert_eq!(files[0].reached_from, FROM_FILESPEC);
        }
    }

    #[test]
    fn an_indirect_filespec_outside_the_tree_is_counted_with_its_object() {
        let doc = catalog_doc(
            vec![],
            vec![
                ((6, 0), file_stream()),
                ((7, 0), Object::Dictionary(filespec(b"fs.bin", (6, 0)))),
            ],
        );
        let files = collect_embedded_files(&doc);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filespec_object, Some((7, 0)));
    }

    #[test]
    fn a_stream_reached_twice_is_one_file_named_by_the_tree() {
        let doc = catalog_doc(
            vec![(
                "Names",
                tree_of(vec![
                    Object::String(b"tree-name".to_vec(), StringFormat::Literal),
                    Object::Reference((7, 0)),
                ]),
            )],
            vec![
                ((6, 0), file_stream()),
                ((7, 0), Object::Dictionary(filespec(b"f.txt", (6, 0)))),
                ((8, 0), Object::Dictionary(filespec(b"g.txt", (6, 0)))),
            ],
        );
        let files = collect_embedded_files(&doc);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "tree-name");
        assert_eq!(files[0].reached_from, FROM_NAME_TREE);
    }

    #[test]
    fn each_distinct_ef_stream_of_one_filespec_is_a_file() {
        let mut spec = filespec(b"a.txt", (6, 0));
        if let Ok(Object::Dictionary(ef)) = spec.get_mut(b"EF") {
            ef.set("Unix", Object::Reference((8, 0)));
        }
        let doc = catalog_doc(
            vec![],
            vec![
                ((6, 0), file_stream()),
                ((8, 0), file_stream()),
                ((7, 0), Object::Dictionary(spec)),
            ],
        );
        let numbers: Vec<u32> = collect_embedded_files(&doc)
            .iter()
            .map(|f| f.object_number)
            .collect();
        assert_eq!(numbers, vec![6, 8]);
    }

    #[test]
    fn an_ef_entry_under_any_key_and_a_related_file_are_counted() {
        let untyped = || Object::Stream(Stream::new(Dictionary::new(), b"x".to_vec()));
        let mut ef = Dictionary::new();
        ef.set("Foo", Object::Reference((6, 0)));
        let mut rf = Dictionary::new();
        rf.set(
            "F",
            Object::Array(vec![
                Object::String(b"other".to_vec(), StringFormat::Literal),
                Object::Reference((8, 0)),
            ]),
        );
        let mut spec = Dictionary::new();
        spec.set(
            "F",
            Object::String(b"a.txt".to_vec(), StringFormat::Literal),
        );
        spec.set("EF", Object::Dictionary(ef));
        spec.set("RF", Object::Dictionary(rf));
        let doc = catalog_doc(
            vec![],
            vec![
                ((6, 0), untyped()),
                ((8, 0), untyped()),
                ((7, 0), Object::Dictionary(spec)),
            ],
        );
        let numbers: Vec<u32> = collect_embedded_files(&doc)
            .iter()
            .map(|f| f.object_number)
            .collect();
        assert_eq!(numbers, vec![6, 8]);
    }

    #[test]
    fn an_embedded_file_stream_no_filespec_names_is_counted() {
        let doc = catalog_doc(vec![], vec![((6, 0), file_stream())]);
        let files = collect_embedded_files(&doc);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].reached_from, FROM_ORPHAN);
        assert_eq!(files[0].filespec_object, None);
    }

    #[test]
    fn utf16_filename_is_decoded_and_an_undecodable_one_is_flagged() {
        let mut uf = vec![0xFE, 0xFF];
        uf.extend("Mākaha.txt".encode_utf16().flat_map(u16::to_be_bytes));
        let mut good = filespec(b"x", (6, 0));
        good.set("UF", Object::String(uf, StringFormat::Hexadecimal));
        let mut bad = filespec(b"y", (8, 0));
        bad.set(
            "UF",
            Object::String(vec![0xFE, 0xFF, 0x00], StringFormat::Hexadecimal),
        );
        let doc = catalog_doc(
            vec![],
            vec![
                ((6, 0), file_stream()),
                ((8, 0), file_stream()),
                ((7, 0), Object::Dictionary(good)),
                ((9, 0), Object::Dictionary(bad)),
            ],
        );
        let v = embedded_json_value(&doc);
        assert_eq!(v["embedded_files"][0]["filename"], "Mākaha.txt");
        assert_eq!(v["embedded_files"][0]["filename_decoded"], true);
        assert_eq!(v["embedded_files"][1]["filename"], "<FEFF00>");
        assert_eq!(v["embedded_files"][1]["filename_decoded"], false);
    }

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
        filespec.set(
            "F",
            Object::String(b"invoice.pdf".to_vec(), StringFormat::Literal),
        );
        filespec.set("EF", Object::Dictionary(ef));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        // Names tree (leaf)
        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"invoice.pdf".to_vec(), StringFormat::Literal),
                Object::Reference((11, 0)),
            ]),
        );
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
        filespec.set(
            "F",
            Object::String(b"missing.pdf".to_vec(), StringFormat::Literal),
        );
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"missing.pdf".to_vec(), StringFormat::Literal),
                Object::Reference((11, 0)),
            ]),
        );
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
        let out = output_of(|w| render_json(w, &embedded_json_value(&doc)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["embedded_file_count"], 0);
        assert_eq!(parsed["embedded_files"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn embedded_files_uf_fallback_to_f() {
        // Arrange: filespec has /F but no /UF — should use /F for filename
        let mut doc = Document::new();
        let mut ef_stream_dict = Dictionary::new();
        ef_stream_dict.set("Type", Object::Name(b"EmbeddedFile".to_vec()));
        let ef_stream = Stream::new(ef_stream_dict, b"data".to_vec());
        doc.objects.insert((10, 0), Object::Stream(ef_stream));

        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference((10, 0)));
        let mut filespec = Dictionary::new();
        filespec.set("Type", Object::Name(b"Filespec".to_vec()));
        filespec.set(
            "F",
            Object::String(b"readme.txt".to_vec(), StringFormat::Literal),
        );
        // No /UF key
        filespec.set("EF", Object::Dictionary(ef));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"readme.txt".to_vec(), StringFormat::Literal),
                Object::Reference((11, 0)),
            ]),
        );
        doc.objects.insert((12, 0), Object::Dictionary(ef_tree));

        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Reference((12, 0)));
        doc.objects.insert((13, 0), Object::Dictionary(names_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((13, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // Act
        let files = collect_embedded_files(&doc);

        // Assert
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "readme.txt");
    }

    #[test]
    fn embedded_files_missing_params_size() {
        // Arrange: stream without /Params/Size
        let mut doc = Document::new();
        let ef_stream_dict = Dictionary::new(); // No Params
        let ef_stream = Stream::new(ef_stream_dict, b"data".to_vec());
        doc.objects.insert((10, 0), Object::Stream(ef_stream));

        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference((10, 0)));
        let mut filespec = Dictionary::new();
        filespec.set("Type", Object::Name(b"Filespec".to_vec()));
        filespec.set(
            "F",
            Object::String(b"test.bin".to_vec(), StringFormat::Literal),
        );
        filespec.set("EF", Object::Dictionary(ef));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"test.bin".to_vec(), StringFormat::Literal),
                Object::Reference((11, 0)),
            ]),
        );
        doc.objects.insert((12, 0), Object::Dictionary(ef_tree));

        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Reference((12, 0)));
        doc.objects.insert((13, 0), Object::Dictionary(names_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((13, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // Act
        let files = collect_embedded_files(&doc);

        // Assert
        assert_eq!(files.len(), 1);
        assert!(files[0].size.is_none());
    }

    #[test]
    fn embedded_files_missing_mime_type() {
        // Arrange: stream without /Subtype
        let mut doc = Document::new();
        let ef_stream_dict = Dictionary::new(); // No Subtype
        let ef_stream = Stream::new(ef_stream_dict, b"data".to_vec());
        doc.objects.insert((10, 0), Object::Stream(ef_stream));

        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference((10, 0)));
        let mut filespec = Dictionary::new();
        filespec.set("Type", Object::Name(b"Filespec".to_vec()));
        filespec.set(
            "F",
            Object::String(b"test.bin".to_vec(), StringFormat::Literal),
        );
        filespec.set("EF", Object::Dictionary(ef));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"test.bin".to_vec(), StringFormat::Literal),
                Object::Reference((11, 0)),
            ]),
        );
        doc.objects.insert((12, 0), Object::Dictionary(ef_tree));

        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Reference((12, 0)));
        doc.objects.insert((13, 0), Object::Dictionary(names_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((13, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        // Act
        let files = collect_embedded_files(&doc);

        // Assert
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].mime_type, "-");
    }

    #[test]
    fn embedded_files_json_structure_with_file() {
        let mut doc = Document::new();
        let mut ef_stream_dict = Dictionary::new();
        ef_stream_dict.set("Subtype", Object::Name(b"text#2Fplain".to_vec()));
        let mut params = Dictionary::new();
        params.set("Size", Object::Integer(100));
        ef_stream_dict.set("Params", Object::Dictionary(params));
        let ef_stream = Stream::new(ef_stream_dict, b"hello".to_vec());
        doc.objects.insert((10, 0), Object::Stream(ef_stream));

        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference((10, 0)));
        let mut filespec = Dictionary::new();
        filespec.set("Type", Object::Name(b"Filespec".to_vec()));
        filespec.set(
            "UF",
            Object::String(b"notes.txt".to_vec(), StringFormat::Literal),
        );
        filespec.set("EF", Object::Dictionary(ef));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"notes.txt".to_vec(), StringFormat::Literal),
                Object::Reference((11, 0)),
            ]),
        );
        doc.objects.insert((12, 0), Object::Dictionary(ef_tree));

        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Reference((12, 0)));
        doc.objects.insert((13, 0), Object::Dictionary(names_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((13, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| render_json(w, &embedded_json_value(&doc)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["embedded_file_count"], 1);
        let file = &parsed["embedded_files"][0];
        assert_eq!(file["filename"], "notes.txt");
        assert_eq!(file["size"], 100);
        assert_eq!(file["object_number"], 10);
        assert!(file["filespec_object"].as_u64().is_some());
    }

    #[test]
    fn embedded_files_non_reference_value_skipped() {
        // Arrange: name tree entry value is not a Reference (should be skipped)
        let mut doc = Document::new();
        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"bad.pdf".to_vec(), StringFormat::Literal),
                Object::Integer(42), // not a reference — should be skipped
            ]),
        );
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
    fn embedded_files_print_with_file() {
        // Build doc with one embedded file
        let mut doc = Document::new();
        let mut ef_stream_dict = Dictionary::new();
        ef_stream_dict.set("Subtype", Object::Name(b"application#2Fpdf".to_vec()));
        let mut params = Dictionary::new();
        params.set("Size", Object::Integer(5000));
        ef_stream_dict.set("Params", Object::Dictionary(params));
        let ef_stream = Stream::new(ef_stream_dict, b"pdf".to_vec());
        doc.objects.insert((10, 0), Object::Stream(ef_stream));

        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference((10, 0)));
        let mut filespec = Dictionary::new();
        filespec.set("Type", Object::Name(b"Filespec".to_vec()));
        filespec.set(
            "F",
            Object::String(b"report.pdf".to_vec(), StringFormat::Literal),
        );
        filespec.set("EF", Object::Dictionary(ef));
        doc.objects.insert((11, 0), Object::Dictionary(filespec));

        let mut ef_tree = Dictionary::new();
        ef_tree.set(
            "Names",
            Object::Array(vec![
                Object::String(b"report.pdf".to_vec(), StringFormat::Literal),
                Object::Reference((11, 0)),
            ]),
        );
        doc.objects.insert((12, 0), Object::Dictionary(ef_tree));
        let mut names_dict = Dictionary::new();
        names_dict.set("EmbeddedFiles", Object::Reference((12, 0)));
        doc.objects.insert((13, 0), Object::Dictionary(names_dict));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Names", Object::Reference((13, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let out = output_of(|w| print_embedded_files(w, &doc));
        assert!(out.contains("1 embedded files"));
        assert!(out.contains("report.pdf"));
        assert!(out.contains("5000"));
    }
}
