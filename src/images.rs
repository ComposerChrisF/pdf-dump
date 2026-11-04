use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;

use crate::helpers::{format_color_space, format_filter};
use crate::image_export::{ImageSource, export_format, full_filter_name};
use crate::inline_image::{InlineImage, InlineScan, scan_document};

pub(crate) struct ImageInfo {
    pub object_id: ObjectId,
    pub width: i64,
    pub height: i64,
    pub color_space: String,
    pub bits_per_component: i64,
    pub filter: String,
    pub size: usize,
    /// What `--extract-image` would write; `None` when the image's filters cannot be read.
    pub export_format: Option<&'static str>,
}

pub(crate) fn collect_images(doc: &Document) -> Vec<ImageInfo> {
    let is_image = |dict: &lopdf::Dictionary| {
        let subtype = dict.get(b"Subtype").ok().map(|v| match v {
            Object::Reference(id) => doc.get_object(*id).ok(),
            other => Some(other),
        });
        matches!(subtype, Some(Some(Object::Name(n))) if n == b"Image")
    };
    let mut wanted: BTreeSet<ObjectId> = doc
        .objects
        .iter()
        .filter(|(_, object)| matches!(object, Object::Stream(s) if is_image(&s.dict)))
        .map(|(&id, _)| id)
        .collect();
    // A mask is an image whatever its /Subtype says, and poppler draws one that has none.  A caller
    // reading every image must get it, since a scan may keep its words there.
    let masks: Vec<ObjectId> = wanted
        .iter()
        .filter_map(|id| match doc.objects.get(id) {
            Some(Object::Stream(s)) => Some(s),
            _ => None,
        })
        .flat_map(|s| [s.dict.get(b"SMask"), s.dict.get(b"Mask")])
        .filter_map(|mask| mask.ok()?.as_reference().ok())
        .filter(|id| matches!(doc.get_object(*id), Ok(Object::Stream(_))))
        .collect();
    wanted.extend(masks);

    let mut images = Vec::new();
    for obj_id in wanted {
        let Some(Object::Stream(stream)) = doc.objects.get(&obj_id) else {
            continue;
        };
        let dict = &stream.dict;
        let number = |key: &[u8]| {
            dict.get(key)
                .ok()
                .and_then(|v| match v {
                    Object::Reference(id) => doc.get_object(*id).ok(),
                    other => Some(other),
                })
                .and_then(|v| v.as_i64().ok())
                .unwrap_or(0)
        };
        let color_space = dict
            .get(b"ColorSpace")
            .ok()
            .map(|v| format_color_space(v, doc))
            .unwrap_or_else(|| "-".to_string());
        let filter = dict
            .get(b"Filter")
            .ok()
            .map(format_filter)
            .unwrap_or_else(|| "-".to_string());
        images.push(ImageInfo {
            object_id: obj_id,
            width: number(b"Width"),
            height: number(b"Height"),
            color_space,
            bits_per_component: number(b"BitsPerComponent"),
            filter,
            size: stream.content.len(),
            export_format: export_format(
                doc,
                &ImageSource {
                    dict,
                    data: &[],
                    resources: None,
                    inline: false,
                },
            )
            .map(|f| f.name()),
        });
    }
    images
}

/// An inline image's dictionary entry under either spelling of its key.
fn inline_entry<'a>(image: &'a InlineImage, long: &[u8], short: &[u8]) -> Option<&'a Object> {
    image.dict.get(long).or_else(|_| image.dict.get(short)).ok()
}

fn inline_filter(image: &InlineImage) -> String {
    match inline_entry(image, b"Filter", b"F") {
        Some(Object::Name(n)) => full_filter_name(n),
        Some(Object::Array(items)) => items
            .iter()
            .map(|item| match item {
                Object::Name(n) => full_filter_name(n),
                _ => "?".to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => "-".to_string(),
    }
}

/// The dictionary's own text, abbreviations spelled out.  A name from the resources stays a name.
fn inline_color_space(doc: &Document, image: &InlineImage) -> String {
    let Some(space) = inline_entry(image, b"ColorSpace", b"CS") else {
        return "-".to_string();
    };
    match space {
        Object::Name(n) if n == b"G" => "DeviceGray".to_string(),
        Object::Name(n) if n == b"RGB" => "DeviceRGB".to_string(),
        Object::Name(n) if n == b"CMYK" => "DeviceCMYK".to_string(),
        other => format_color_space(other, doc),
    }
}

fn inline_number(image: &InlineImage, long: &[u8], short: &[u8]) -> Option<i64> {
    inline_entry(image, long, short)?.as_i64().ok()
}

fn warn_unwalked(scan: &InlineScan) -> bool {
    for warning in &scan.warnings {
        eprintln!("Warning: looking for inline images, {warning}");
    }
    !scan.warnings.is_empty()
}

/// Returns `true` when part of a page could not be walked for inline images (a finding, exit 3).
pub(crate) fn print_images(writer: &mut impl Write, doc: &Document) -> bool {
    let images = collect_images(doc);
    wln!(writer, "{} images found\n", images.len());
    wln!(
        writer,
        "  {:>4}  {:>5}  {:>6}  {:<18} {:>3}  {:<18} {:>8}",
        "Obj#",
        "Width",
        "Height",
        "ColorSpace",
        "BPC",
        "Filter",
        "Size"
    );
    for img in &images {
        wln!(
            writer,
            "  {:>4}  {:>5}  {:>6}  {:<18} {:>3}  {:<18} {:>8}",
            img.object_id.0,
            img.width,
            img.height,
            img.color_space,
            img.bits_per_component,
            img.filter,
            img.size
        );
    }

    let scan = scan_document(doc);
    if !scan.images.is_empty() {
        wln!(writer, "\n{} inline images found\n", scan.images.len());
        wln!(
            writer,
            "  {:>10}  {:<10} {:>8}  {:>5}  {:>6}  {:<18} {:>3}  {:<18} {:>8}",
            "PAGE:INDEX",
            "Source",
            "Offset",
            "Width",
            "Height",
            "ColorSpace",
            "BPC",
            "Filter",
            "Size"
        );
        for image in &scan.images {
            wln!(
                writer,
                "  {:>10}  {:<10} {:>8}  {:>5}  {:>6}  {:<18} {:>3}  {:<18} {:>8}",
                format!("{}:{}", image.page, image.index),
                image.source,
                image.offset,
                inline_number(image, b"Width", b"W").unwrap_or(0),
                inline_number(image, b"Height", b"H").unwrap_or(0),
                inline_color_space(doc, image),
                inline_number(image, b"BitsPerComponent", b"BPC").unwrap_or(0),
                inline_filter(image),
                image.size
            );
        }
    }
    warn_unwalked(&scan)
}

#[cfg(test)]
pub(crate) fn images_json_value(doc: &Document) -> Value {
    images_json_value_with_status(doc).0
}

/// Returns the JSON and whether part of a page could not be walked for inline images.
pub(crate) fn images_json_value_with_status(doc: &Document) -> (Value, bool) {
    let images = collect_images(doc);
    let items: Vec<Value> = images
        .iter()
        .map(|img| {
            json!({
                "object_number": img.object_id.0,
                "generation": img.object_id.1,
                "width": img.width,
                "height": img.height,
                "color_space": img.color_space,
                "bits_per_component": img.bits_per_component,
                "filter": img.filter,
                "size": img.size,
                "export_format": img.export_format,
            })
        })
        .collect();
    let scan = scan_document(doc);
    let inline: Vec<Value> = scan
        .images
        .iter()
        .map(|image| {
            json!({
                "page_number": image.page,
                "index": image.index,
                "source": image.source,
                "source_object": image.source_object.map(|id| id.0),
                "offset": image.offset,
                "width": inline_number(image, b"Width", b"W"),
                "height": inline_number(image, b"Height", b"H"),
                "color_space": inline_color_space(doc, image),
                "bits_per_component": inline_number(image, b"BitsPerComponent", b"BPC"),
                "filter": inline_filter(image),
                "size": image.size,
                "export_format": export_format(doc, &image.as_source()).map(|f| f.name()),
            })
        })
        .collect();
    let had_issues = warn_unwalked(&scan);
    (
        json!({
            "image_count": items.len(),
            "images": items,
            "inline_image_count": inline.len(),
            "inline_images": inline,
            "inline_image_warnings": scan.warnings,
        }),
        had_issues,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    #[test]
    fn a_mask_with_no_subtype_is_listed_and_an_indirect_subtype_is_followed() {
        let mut doc = Document::new();
        doc.objects.insert((8, 0), Object::Name(b"Image".to_vec()));
        let mut image = Dictionary::new();
        image.set("Subtype", Object::Reference((8, 0)));
        image.set("SMask", Object::Reference((2, 0)));
        doc.objects
            .insert((1, 0), Object::Stream(Stream::new(image, vec![0])));
        doc.objects.insert(
            (2, 0),
            Object::Stream(Stream::new(Dictionary::new(), vec![0])),
        );
        let ids: Vec<u32> = collect_images(&doc).iter().map(|i| i.object_id.0).collect();
        assert_eq!(ids, vec![1, 2]);
    }

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
        dict.set(
            b"ColorSpace",
            Object::Array(vec![
                Object::Name(b"ICCBased".to_vec()),
                Object::Reference((2, 0)),
            ]),
        );
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
        dict.set(
            b"Filter",
            Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"DCTDecode".to_vec()),
            ]),
        );
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

        let out = output_of(|w| render_json(w, &images_json_value(&doc)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["image_count"], 1);
        assert_eq!(parsed["images"][0]["width"], 100);
        assert_eq!(parsed["images"][0]["height"], 200);
    }

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
        doc.objects
            .insert((2, 0), Object::Name(b"DeviceGray".to_vec()));

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

        let out = output_of(|w| render_json(w, &images_json_value(&doc)));
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
}
