use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::helpers::{resolve_dict, obj_to_string_lossy, name_to_string, walk_name_tree};

pub(crate) struct EmbeddedFileInfo {
    pub name: String,
    pub filename: String,
    pub mime_type: String,
    pub size: Option<i64>,
    pub object_number: u32,
    pub filespec_object: ObjectId,
}

pub(crate) fn collect_embedded_files(doc: &Document) -> Vec<EmbeddedFileInfo> {
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

pub(crate) fn print_embedded_files(writer: &mut impl Write, doc: &Document) {
    let files = collect_embedded_files(doc);
    writeln!(writer, "{} embedded files\n", files.len()).unwrap();
    if files.is_empty() { return; }
    writeln!(writer, "  {:>4}  {:<30} {:<24} {:>8}", "Obj#", "Filename", "MIME Type", "Size").unwrap();
    for f in &files {
        let size_str = f.size.map(|s| s.to_string()).unwrap_or_else(|| "-".to_string());
        writeln!(writer, "  {:>4}  {:<30} {:<24} {:>8}", f.object_number, f.filename, f.mime_type, size_str).unwrap();
    }
}

pub(crate) fn print_embedded_files_json(writer: &mut impl Write, doc: &Document) {
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

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

}
