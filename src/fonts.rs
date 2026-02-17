use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::io::Write;

use crate::helpers::{resolve_dict, resolve_array, obj_to_string_lossy, name_to_string};

pub(crate) struct FontInfo {
    pub object_id: ObjectId,
    pub base_font: String,
    pub subtype: String,
    pub encoding: String,
    pub embedded: Option<ObjectId>,
    pub to_unicode: Option<ObjectId>,
    pub first_char: Option<i64>,
    pub last_char: Option<i64>,
    pub widths_len: Option<usize>,
    pub encoding_differences: Option<String>,
    pub cid_system_info: Option<String>,
}

pub(crate) fn collect_fonts(doc: &Document) -> Vec<FontInfo> {
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

pub(crate) fn extract_encoding_differences(doc: &Document, dict: &lopdf::Dictionary) -> Option<String> {
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

pub(crate) fn extract_cid_system_info(doc: &Document, dict: &lopdf::Dictionary) -> Option<String> {
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

pub(crate) fn print_fonts(writer: &mut impl Write, doc: &Document) {
    let fonts = collect_fonts(doc);
    wln!(writer, "{} fonts found\n", fonts.len());
    wln!(writer, "  {:>4}  {:<30} {:<14} {:<18} Embedded", "Obj#", "BaseFont", "Subtype", "Encoding");
    for f in &fonts {
        let embedded_str = match f.embedded {
            Some(id) => format!("yes ({})", id.0),
            None => "no".to_string(),
        };
        wln!(writer, "  {:>4}  {:<30} {:<14} {:<18} {}", f.object_id.0, f.base_font, f.subtype, f.encoding, embedded_str);
        // Diagnostic details
        if let Some(id) = f.to_unicode {
            wln!(writer, "          ToUnicode: {} 0 R", id.0);
        }
        if f.first_char.is_some() || f.last_char.is_some() || f.widths_len.is_some() {
            let fc = f.first_char.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
            let lc = f.last_char.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
            let wl = f.widths_len.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
            wln!(writer, "          CharRange: {}-{}, Widths: {}", fc, lc, wl);
        }
        if let Some(ref diffs) = f.encoding_differences {
            wln!(writer, "          Differences: {}", diffs);
        }
        if let Some(ref csi) = f.cid_system_info {
            wln!(writer, "          CIDSystemInfo: {}", csi);
        }
    }
}

pub(crate) fn fonts_json_value(doc: &Document) -> Value {
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
    json!({
        "font_count": items.len(),
        "fonts": items,
    })
}

#[cfg(test)]
pub(crate) fn print_fonts_json(writer: &mut impl Write, doc: &Document) {
    use crate::helpers::json_pretty;
    let output = fonts_json_value(doc);
    writeln!(writer, "{}", json_pretty(&output)).unwrap();
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::{Dictionary, Stream};
    use pretty_assertions::assert_eq;
    use serde_json::{Value};
    use lopdf::Object;

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

}
