use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::io::Write;

use crate::helpers::{format_color_space, format_filter};

pub(crate) struct ImageInfo {
    pub object_id: ObjectId,
    pub width: i64,
    pub height: i64,
    pub color_space: String,
    pub bits_per_component: i64,
    pub filter: String,
    pub size: usize,
}

pub(crate) fn collect_images(doc: &Document) -> Vec<ImageInfo> {
    let mut images = Vec::new();
    for (&obj_id, object) in &doc.objects {
        let (dict, content_len) = match object {
            Object::Stream(s) => (&s.dict, s.content.len()),
            _ => continue,
        };

        let is_image = dict
            .get(b"Subtype")
            .ok()
            .is_some_and(|v| v.as_name().ok().is_some_and(|n| n == b"Image"));
        if !is_image {
            continue;
        }

        let width = dict
            .get(b"Width")
            .ok()
            .and_then(|v| v.as_i64().ok())
            .unwrap_or(0);
        let height = dict
            .get(b"Height")
            .ok()
            .and_then(|v| v.as_i64().ok())
            .unwrap_or(0);
        let color_space = dict
            .get(b"ColorSpace")
            .ok()
            .map(|v| format_color_space(v, doc))
            .unwrap_or_else(|| "-".to_string());
        let bits_per_component = dict
            .get(b"BitsPerComponent")
            .ok()
            .and_then(|v| v.as_i64().ok())
            .unwrap_or(0);
        let filter = dict
            .get(b"Filter")
            .ok()
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

pub(crate) fn print_images(writer: &mut impl Write, doc: &Document) {
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
}

pub(crate) fn images_json_value(doc: &Document) -> Value {
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
            })
        })
        .collect();
    json!({
        "image_count": items.len(),
        "images": items,
    })
}

#[cfg(test)]
pub(crate) fn print_images_json(writer: &mut impl Write, doc: &Document) {
    use crate::helpers::json_pretty;
    let output = images_json_value(doc);
    writeln!(writer, "{}", json_pretty(&output)).unwrap();
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

        let out = output_of(|w| print_images_json(w, &doc));
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
}
