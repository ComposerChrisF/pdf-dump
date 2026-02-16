use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;

use crate::stream::{get_filter_names, decode_stream};

pub(crate) struct PdfStats {
    pub page_count: usize,
    pub object_count: usize,
    pub type_counts: BTreeMap<String, usize>,
    pub total_stream_bytes: usize,
    pub total_decoded_bytes: usize,
    pub filter_counts: BTreeMap<String, usize>,
    pub largest_streams: Vec<(ObjectId, usize)>,
}

pub(crate) fn collect_stats(doc: &Document) -> PdfStats {
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

pub(crate) fn print_stats(writer: &mut impl Write, doc: &Document) {
    let stats = collect_stats(doc);

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

pub(crate) fn print_stats_json(writer: &mut impl Write, doc: &Document) {
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

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

}
