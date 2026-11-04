//! Writes one image as a file an ordinary viewer or OCR engine opens: PNG for decoded pixels, and
//! the stream as it is for `DCTDecode` (`.jpg`) and `JPXDecode` (`.jp2`).
//!
//! The written image is the picture as drawn.  Predictors and `/Decode` are applied, palettes
//! expanded, CMYK converted naively, 16-bit samples reduced to 8.  Masks are composited onto white,
//! the colour of paper: an alpha channel would send an OCR engine a page of black where a mask held
//! the words, and a stencil mask has no colour of its own to keep.  So light ink through a mask
//! would come out as nothing, and that is refused rather than written.  A soft mask's `/Matte` is
//! not undone, which only tints the edges.
//!
//! Every failure is a refusal that names what it met, and nothing is written.  A partial or wrong
//! image is worse than none, since a caller would read it as the picture.

use lopdf::{Dictionary, Document, Object};
use std::borrow::Cow;
use std::path::Path;

use crate::stream::{
    PredictorParams, decode_ascii85, decode_asciihex, decode_flate, decode_lzw_early,
    decode_run_length, deref, follow, undo_predictor,
};

/// Refuses an image whose decoded pixels would pass this size, before allocating them.  The same as
/// the filters' own limit, and room for a 600 dpi colour page several times over.
const MAX_PIXEL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportFormat {
    Png,
    Jpeg,
    Jp2,
}

impl ExportFormat {
    pub(crate) fn name(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpeg",
            ExportFormat::Jp2 => "jp2",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Png => ".png",
            ExportFormat::Jpeg => ".jpg",
            ExportFormat::Jp2 => ".jp2",
        }
    }

    /// The format an output path names, by extension in any case.  `None` for anything else, which
    /// is a usage error: no document could make it right.
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(ExportFormat::Png),
            "jpg" | "jpeg" => Some(ExportFormat::Jpeg),
            "jp2" => Some(ExportFormat::Jp2),
            _ => None,
        }
    }
}

/// An image to export: an image XObject, or an inline image, whose dictionary may abbreviate its
/// keys and whose named colour spaces come from `resources`.
pub(crate) struct ImageSource<'a> {
    pub dict: &'a Dictionary,
    pub data: &'a [u8],
    pub resources: Option<&'a Dictionary>,
    pub inline: bool,
}

impl<'a> ImageSource<'a> {
    /// `/F` on a stream names an external file, so the short keys count only inline.
    fn entry(&self, doc: &'a Document, long: &[u8], short: &[u8]) -> Option<&'a Object> {
        let value = self
            .dict
            .get(long)
            .ok()
            .or_else(|| self.inline.then(|| self.dict.get(short).ok()).flatten())?;
        deref(doc, value)
    }

    fn number(&self, doc: &'a Document, long: &[u8], short: &[u8]) -> Option<i64> {
        self.entry(doc, long, short)?.as_i64().ok()
    }
}

/// A successful export, ready to write.
pub(crate) struct ExportedImage {
    pub bytes: Vec<u8>,
    pub format: ExportFormat,
    pub width: u32,
    pub height: u32,
    pub color_space: String,
    pub bits_per_component: Option<i64>,
    pub filter: String,
    /// `none`, `smask`, `explicit` or `color_key`.
    pub mask: &'static str,
    /// False for a JPEG or JPEG 2000 written as it is, whose mask stays unapplied.
    pub mask_applied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    Dct,
    Jpx,
    Ccitt,
    Jbig2,
}

struct FilterStep<'a> {
    name: String,
    parms: Option<&'a Dictionary>,
}

fn filter_chain<'a>(
    doc: &'a Document,
    src: &ImageSource<'a>,
) -> Result<Vec<FilterStep<'a>>, String> {
    let Some(filter) = src.entry(doc, b"Filter", b"F") else {
        return Ok(vec![]);
    };
    let names: Vec<String> = match filter {
        Object::Name(n) => vec![full_filter_name(n)],
        Object::Array(items) => items
            .iter()
            .map(|item| match deref(doc, item) {
                Some(Object::Name(n)) => Ok(full_filter_name(n)),
                _ => Err("a /Filter array entry that is not a name".to_string()),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("a /Filter that is neither a name nor an array".to_string()),
    };
    let raw_parms = src
        .dict
        .get(b"DecodeParms")
        .ok()
        .or_else(|| src.inline.then(|| src.dict.get(b"DP").ok()).flatten());
    let parms = match raw_parms {
        Some(value) => follow(doc, value).map_err(|reason| format!("a /DecodeParms: {reason}"))?,
        None => None,
    };
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let step = match parms {
                None => None,
                Some(Object::Dictionary(d)) => (i == 0).then_some(d),
                Some(Object::Array(items)) => {
                    let item = match items.get(i) {
                        Some(item) => follow(doc, item)
                            .map_err(|reason| format!("a /DecodeParms entry: {reason}"))?,
                        None => None,
                    };
                    match item {
                        None => None,
                        Some(Object::Dictionary(d)) => Some(d),
                        Some(_) => {
                            return Err("a /DecodeParms entry that is not a dictionary".to_string());
                        }
                    }
                }
                Some(_) => {
                    return Err(
                        "a /DecodeParms that is neither a dictionary nor an array".to_string()
                    );
                }
            };
            Ok(FilterStep { name, parms: step })
        })
        .collect()
}

/// Inline images may abbreviate filter names (§ 8.9.7, Table 94).
pub(crate) fn full_filter_name(name: &[u8]) -> String {
    match name {
        b"AHx" => "ASCIIHexDecode".to_string(),
        b"A85" => "ASCII85Decode".to_string(),
        b"LZW" => "LZWDecode".to_string(),
        b"Fl" => "FlateDecode".to_string(),
        b"RL" => "RunLengthDecode".to_string(),
        b"CCF" => "CCITTFaxDecode".to_string(),
        b"DCT" => "DCTDecode".to_string(),
        other => String::from_utf8_lossy(other).into_owned(),
    }
}

fn codec_of(name: &str) -> Option<Codec> {
    match name {
        "DCTDecode" => Some(Codec::Dct),
        "JPXDecode" => Some(Codec::Jpx),
        "CCITTFaxDecode" => Some(Codec::Ccitt),
        "JBIG2Decode" => Some(Codec::Jbig2),
        _ => None,
    }
}

/// The format `--extract-image` would write for this image, from its last filter; `None` when its
/// filters cannot be read.
pub(crate) fn export_format(doc: &Document, src: &ImageSource) -> Option<ExportFormat> {
    let chain = filter_chain(doc, src).ok()?;
    Some(match chain.last().and_then(|s| codec_of(&s.name)) {
        Some(Codec::Dct) => ExportFormat::Jpeg,
        Some(Codec::Jpx) => ExportFormat::Jp2,
        _ => ExportFormat::Png,
    })
}

/// One `/DecodeParms` integer.  A key that is absent takes its default; one that is present and
/// unusable is refused, since a default there would decode a different picture.
fn parm_int(
    doc: &Document,
    parms: Option<&Dictionary>,
    key: &[u8],
    default: i64,
) -> Result<i64, String> {
    let Some(value) = parms
        .and_then(|p| p.get(key).ok())
        .and_then(|v| deref(doc, v))
    else {
        return Ok(default);
    };
    value
        .as_i64()
        .map_err(|_| format!("a /{} that is not an integer", String::from_utf8_lossy(key)))
}

fn parm_bool(
    doc: &Document,
    parms: Option<&Dictionary>,
    key: &[u8],
    default: bool,
) -> Result<bool, String> {
    let Some(value) = parms
        .and_then(|p| p.get(key).ok())
        .and_then(|v| deref(doc, v))
    else {
        return Ok(default);
    };
    value
        .as_bool()
        .map_err(|_| format!("a /{} that is not a boolean", String::from_utf8_lossy(key)))
}

/// An image codec reached at the end of the filter chain, with its parameters.
type PendingCodec<'a> = (Codec, Option<&'a Dictionary>);

/// Undoes every filter up to an image codec, which must come last.
fn undo_filters<'a>(
    doc: &Document,
    src: &ImageSource<'a>,
    chain: &[FilterStep<'a>],
) -> Result<(Cow<'a, [u8]>, Option<PendingCodec<'a>>), String> {
    if !src.inline && src.dict.has(b"F") {
        return Err("data held in an external file (/F)".to_string());
    }
    let mut data: Cow<'a, [u8]> = Cow::Borrowed(src.data);
    for (i, step) in chain.iter().enumerate() {
        if let Some(codec) = codec_of(&step.name) {
            if i + 1 != chain.len() {
                return Err(format!(
                    "{} is followed by another filter ({})",
                    step.name,
                    chain[i + 1].name
                ));
            }
            return Ok((data, Some((codec, step.parms))));
        }
        let decoded = match step.name.as_str() {
            "FlateDecode" => apply_predictor(doc, decode_flate(&data)?, step.parms)?,
            "LZWDecode" => {
                let early = parm_int(doc, step.parms, b"EarlyChange", 1)?;
                apply_predictor(doc, decode_lzw_early(&data, early != 0)?, step.parms)?
            }
            "ASCII85Decode" => decode_ascii85(&data)?,
            "ASCIIHexDecode" => decode_asciihex(&data)?,
            "RunLengthDecode" => decode_run_length(&data)?,
            other => return Err(format!("the filter {other}, which pdf-dump cannot decode")),
        };
        data = Cow::Owned(decoded);
    }
    Ok((data, None))
}

fn apply_predictor(
    doc: &Document,
    data: Vec<u8>,
    parms: Option<&Dictionary>,
) -> Result<Vec<u8>, String> {
    undo_predictor(
        &data,
        PredictorParams {
            predictor: parm_int(doc, parms, b"Predictor", 1)?,
            colors: parm_int(doc, parms, b"Colors", 1)?,
            bits_per_component: parm_int(doc, parms, b"BitsPerComponent", 8)?,
            columns: parm_int(doc, parms, b"Columns", 1)?,
        },
    )
}

/// A whole stream's bytes, every filter undone, by the walk this module trusts: it follows an indirect
/// `/Filter`, as `stream::decode_stream` does not.  For
/// streams that are data rather than pictures: a palette, JBIG2 globals, a content stream.  An image
/// codec in the chain is refused.
pub(crate) fn decode_stream_fully(
    doc: &Document,
    stream: &lopdf::Stream,
) -> Result<Vec<u8>, String> {
    let src = ImageSource {
        dict: &stream.dict,
        data: &stream.content,
        resources: None,
        inline: false,
    };
    let chain = filter_chain(doc, &src)?;
    match undo_filters(doc, &src, &chain)? {
        (data, None) => Ok(data.into_owned()),
        (_, Some(_)) => Err(format!(
            "image data ({}) where data was expected",
            chain.last().map_or("", |s| s.name.as_str())
        )),
    }
}

/// A colour space pdf-dump can turn into grey or RGB.  Separation, DeviceN, Lab and Pattern need a
/// tint transform or a real colour conversion, and are refused by name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    Indexed {
        base: Box<ColorSpace>,
        hival: u32,
        table: Vec<u8>,
    },
}

impl ColorSpace {
    pub(crate) fn components(&self) -> usize {
        match self {
            ColorSpace::Gray | ColorSpace::Indexed { .. } => 1,
            ColorSpace::Rgb => 3,
            ColorSpace::Cmyk => 4,
        }
    }

    fn output_channels(&self) -> usize {
        match self {
            ColorSpace::Gray => 1,
            ColorSpace::Indexed { base, .. } => base.output_channels(),
            ColorSpace::Rgb | ColorSpace::Cmyk => 3,
        }
    }
}

const MAX_COLOR_SPACE_DEPTH: u32 = 4;

pub(crate) fn parse_color_space(
    doc: &Document,
    obj: &Object,
    resources: Option<&Dictionary>,
    depth: u32,
) -> Result<ColorSpace, String> {
    if depth > MAX_COLOR_SPACE_DEPTH {
        return Err("a colour space nested too deeply".to_string());
    }
    let refused = |name: &[u8]| {
        Err(format!(
            "the colour space {}, which pdf-dump cannot convert",
            String::from_utf8_lossy(name)
        ))
    };
    let obj = deref(doc, obj).ok_or("a /ColorSpace that refers to nothing")?;
    match obj {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Ok(ColorSpace::Gray),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Ok(ColorSpace::Rgb),
            b"DeviceCMYK" | b"CMYK" => Ok(ColorSpace::Cmyk),
            name => {
                let named = resources
                    .and_then(|r| r.get(b"ColorSpace").ok())
                    .and_then(|o| deref(doc, o))
                    .and_then(|o| o.as_dict().ok())
                    .and_then(|d| d.get(name).ok());
                match named {
                    Some(found) => parse_color_space(doc, found, resources, depth + 1),
                    None => refused(name),
                }
            }
        },
        Object::Array(items) => {
            let family = items
                .first()
                .and_then(|o| deref(doc, o))
                .and_then(|o| o.as_name().ok())
                .ok_or("a /ColorSpace array with no family name")?;
            match family {
                b"DeviceGray" | b"G" | b"CalGray" => Ok(ColorSpace::Gray),
                b"DeviceRGB" | b"RGB" | b"CalRGB" => Ok(ColorSpace::Rgb),
                b"DeviceCMYK" | b"CMYK" => Ok(ColorSpace::Cmyk),
                b"ICCBased" => {
                    let profile = items.get(1).and_then(|o| deref(doc, o));
                    let Some(Object::Stream(profile)) = profile else {
                        return Err("an ICCBased colour space with no profile stream".to_string());
                    };
                    match profile.dict.get(b"N").ok().and_then(|n| deref(doc, n)) {
                        Some(n) => {
                            match n.as_i64() {
                                Ok(1) => Ok(ColorSpace::Gray),
                                Ok(3) => Ok(ColorSpace::Rgb),
                                Ok(4) => Ok(ColorSpace::Cmyk),
                                _ => Err("an ICCBased colour space whose /N is not 1, 3 or 4"
                                    .to_string()),
                            }
                        }
                        None => match profile.dict.get(b"Alternate") {
                            Ok(alt) => parse_color_space(doc, alt, resources, depth + 1),
                            Err(_) => Err("an ICCBased colour space with no /N".to_string()),
                        },
                    }
                }
                b"Indexed" | b"I" => parse_indexed(doc, items, resources, depth),
                name => refused(name),
            }
        }
        _ => Err("a /ColorSpace that is neither a name nor an array".to_string()),
    }
}

fn parse_indexed(
    doc: &Document,
    items: &[Object],
    resources: Option<&Dictionary>,
    depth: u32,
) -> Result<ColorSpace, String> {
    let [_, base, hival, lookup] = items else {
        return Err("an Indexed colour space without its base, hival and lookup".to_string());
    };
    let base = parse_color_space(doc, base, resources, depth + 1)?;
    if matches!(base, ColorSpace::Indexed { .. }) {
        return Err("an Indexed colour space whose base is Indexed".to_string());
    }
    let hival = deref(doc, hival)
        .and_then(|o| o.as_i64().ok())
        .filter(|h| (0..=255).contains(h))
        .ok_or("an Indexed colour space whose hival is not 0 to 255")? as u32;
    let table = match deref(doc, lookup) {
        Some(Object::String(bytes, _)) => bytes.clone(),
        Some(Object::Stream(stream)) => decode_stream_fully(doc, stream)
            .map_err(|e| format!("an Indexed palette that will not decode ({e})"))?,
        _ => return Err("an Indexed colour space with no palette".to_string()),
    };
    Ok(ColorSpace::Indexed {
        base: Box::new(base),
        hival,
        table,
    })
}

/// Decoded pixels, grey (1 channel) or RGB (3), 8 bits each.
struct Pixels {
    width: u32,
    height: u32,
    channels: usize,
    data: Vec<u8>,
}

fn dimension(src: &ImageSource, doc: &Document, long: &[u8], short: &[u8]) -> Result<u32, String> {
    let value = src
        .number(doc, long, short)
        .ok_or_else(|| format!("no /{}", String::from_utf8_lossy(long)))?;
    u32::try_from(value)
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| format!("a /{} of {value}", String::from_utf8_lossy(long)))
}

/// The bytes `width` × `height` × `channels` come to, refused past the limit, so a tiny hostile
/// dictionary cannot demand a huge allocation.
fn checked_size(width: u32, height: u32, channels: usize) -> Result<usize, String> {
    let size = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|p| p.checked_mul(channels as u64))
        .filter(|s| *s <= MAX_PIXEL_BYTES);
    size.map(|s| s as usize).ok_or_else(|| {
        format!("an image of {width}×{height}, past the {MAX_PIXEL_BYTES} byte limit on pixels")
    })
}

/// Bytes per row of packed samples, rows being padded to a byte (§ 8.9.5.2).
fn row_stride(width: u32, components: usize, bits: u32) -> Result<usize, String> {
    let bits_in_row = u64::from(width)
        .checked_mul(components as u64)
        .and_then(|b| b.checked_mul(u64::from(bits)))
        .ok_or_else(|| format!("a row of {width} samples, too wide to address"))?;
    Ok(bits_in_row.div_ceil(8) as usize)
}

fn read_sample(row: &[u8], index: usize, bits: u32) -> u32 {
    match bits {
        8 => u32::from(row[index]),
        16 => u32::from(u16::from_be_bytes([row[index * 2], row[index * 2 + 1]])),
        _ => {
            let bit = index * bits as usize;
            let byte = row[bit / 8];
            let shift = 8 - bits as usize - bit % 8;
            (u32::from(byte) >> shift) & ((1 << bits) - 1)
        }
    }
}

fn to_byte(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn number_array(doc: &Document, obj: &Object) -> Option<Vec<f64>> {
    let Object::Array(items) = obj else {
        return None;
    };
    items
        .iter()
        .map(|o| match deref(doc, o)? {
            Object::Integer(i) => Some(*i as f64),
            Object::Real(r) => Some(f64::from(*r)),
            _ => None,
        })
        .collect()
}

fn read_decode_array(
    doc: &Document,
    src: &ImageSource,
    components: usize,
) -> Result<Option<Vec<f64>>, String> {
    let Some(obj) = src.entry(doc, b"Decode", b"D") else {
        return Ok(None);
    };
    let decode = number_array(doc, obj).ok_or("a /Decode that is not an array of numbers")?;
    if decode.len() != components * 2 {
        return Err(format!(
            "a /Decode of {} numbers for {components} component(s)",
            decode.len()
        ));
    }
    Ok(Some(decode))
}

/// Turns packed samples into pixels: `/Decode`, the palette, CMYK to RGB, and a colour-key mask.
fn convert_samples(
    samples: &[u8],
    width: u32,
    height: u32,
    bits: u32,
    space: &ColorSpace,
    decode: Option<&[f64]>,
    color_key: Option<&[f64]>,
) -> Result<Pixels, String> {
    let components = space.components();
    let stride = row_stride(width, components, bits)?;
    let channels = space.output_channels();
    let size = checked_size(width, height, channels)?;
    let need = (stride as u64) * u64::from(height);
    if (samples.len() as u64) < need {
        return Err(format!(
            "image data of {} bytes where {width}×{height} at {bits} bits needs {need}",
            samples.len()
        ));
    }

    let mut any_masked = false;
    let mut any_shown = false;
    let max = f64::from((1u32 << bits) - 1);
    let indexed = matches!(space, ColorSpace::Indexed { .. });
    let (default_low, default_high) = if indexed { (0.0, max) } else { (0.0, 1.0) };
    let range = |c: usize| match decode {
        Some(d) => (d[c * 2], d[c * 2 + 1]),
        None => (default_low, default_high),
    };
    // A table per component for 8 bits and under; 16-bit samples compute each value.
    let tables: Vec<Vec<f64>> = if bits <= 8 {
        (0..components)
            .map(|c| {
                let (low, high) = range(c);
                (0..=(max as u32))
                    .map(|s| low + f64::from(s) * (high - low) / max)
                    .collect()
            })
            .collect()
    } else {
        vec![]
    };
    let value = |c: usize, sample: u32| -> f64 {
        if bits <= 8 {
            tables[c][sample as usize]
        } else {
            let (low, high) = range(c);
            low + f64::from(sample) * (high - low) / max
        }
    };

    let mut data = Vec::with_capacity(size);
    let mut raw = vec![0u32; components];
    for row in samples.chunks_exact(stride).take(height as usize) {
        for x in 0..width as usize {
            for (c, slot) in raw.iter_mut().enumerate() {
                *slot = read_sample(row, x * components + c, bits);
            }
            let masked = color_key.is_some_and(|key| {
                raw.iter().enumerate().all(|(c, s)| {
                    let s = f64::from(*s);
                    key[c * 2] <= s && s <= key[c * 2 + 1]
                })
            });
            if masked {
                data.extend(std::iter::repeat_n(255u8, channels));
                any_masked = true;
                continue;
            }
            any_shown = true;
            match space {
                ColorSpace::Gray => data.push(to_byte(value(0, raw[0]))),
                ColorSpace::Rgb => {
                    for (c, sample) in raw.iter().enumerate() {
                        data.push(to_byte(value(c, *sample)));
                    }
                }
                ColorSpace::Cmyk => {
                    let cmyk = [
                        value(0, raw[0]),
                        value(1, raw[1]),
                        value(2, raw[2]),
                        value(3, raw[3]),
                    ];
                    push_cmyk(&mut data, &cmyk);
                }
                ColorSpace::Indexed { base, hival, table } => {
                    let index = value(0, raw[0]).round();
                    let n = base.components();
                    let entry = (index >= 0.0 && index <= f64::from(*hival))
                        .then(|| index as usize * n)
                        .and_then(|at| table.get(at..at + n))
                        .ok_or_else(|| {
                            format!(
                                "a palette index of {index} past the palette's hival of {hival} or its end"
                            )
                        })?;
                    match base.as_ref() {
                        ColorSpace::Cmyk => {
                            let cmyk = [
                                f64::from(entry[0]) / 255.0,
                                f64::from(entry[1]) / 255.0,
                                f64::from(entry[2]) / 255.0,
                                f64::from(entry[3]) / 255.0,
                            ];
                            push_cmyk(&mut data, &cmyk);
                        }
                        _ => data.extend_from_slice(entry),
                    }
                }
            }
        }
    }
    // A key with a shape, leaving something light on paper, over a picture that had more in it.
    if any_masked && any_shown && spread(&data) < NO_PICTURE_SPREAD {
        let unkeyed = convert_samples(samples, width, height, bits, space, decode, None)?;
        if spread(&unkeyed.data) >= NO_PICTURE_SPREAD {
            return Err(LIGHT_THROUGH_A_MASK.to_string());
        }
    }
    Ok(Pixels {
        width,
        height,
        channels,
        data,
    })
}

/// The naive conversion: enough for viewing and OCR, and no colour management is claimed.
fn push_cmyk(data: &mut Vec<u8>, cmyk: &[f64; 4]) {
    let k = 1.0 - cmyk[3].clamp(0.0, 1.0);
    for c in &cmyk[..3] {
        data.push(to_byte((1.0 - c.clamp(0.0, 1.0)) * k));
    }
}

/// A stencil mask as grey: painted samples black, the rest white.  `[0 1]`, the default, paints
/// where a sample is 0.
fn convert_stencil(
    samples: &[u8],
    width: u32,
    height: u32,
    decode: Option<&[f64]>,
) -> Result<Pixels, String> {
    let stride = row_stride(width, 1, 1)?;
    let size = checked_size(width, height, 1)?;
    let need = (stride as u64) * u64::from(height);
    if (samples.len() as u64) < need {
        return Err(format!(
            "image data of {} bytes where a {width}×{height} stencil mask needs {need}",
            samples.len()
        ));
    }
    let paints_on_one = decode.is_some_and(|d| d[0] > d[1]);
    let mut data = Vec::with_capacity(size);
    for row in samples.chunks_exact(stride).take(height as usize) {
        for x in 0..width as usize {
            let sample_is_one = read_sample(row, x, 1) == 1;
            data.push(if sample_is_one == paints_on_one {
                0
            } else {
                255
            });
        }
    }
    Ok(Pixels {
        width,
        height,
        channels: 1,
        data,
    })
}

struct CcittRows {
    out: Vec<u8>,
    stride: usize,
    columns: u32,
    rows_wanted: u32,
    rows_seen: u32,
    black_is_1: bool,
}

impl hayro_ccitt::Decoder for CcittRows {
    fn push_row(&mut self, changes: &[u32]) {
        self.rows_seen += 1;
        if self.rows_seen > self.rows_wanted {
            return;
        }
        let start = self.out.len();
        // PDF's default has black as 0: rows begin white, which is all ones.
        let white = if self.black_is_1 { 0x00 } else { 0xFF };
        self.out.resize(start + self.stride, white);
        let row = &mut self.out[start..];
        let mut runs = changes.chunks(2);
        for run in &mut runs {
            let from = run[0];
            let to = run
                .get(1)
                .copied()
                .unwrap_or(self.columns)
                .min(self.columns);
            for x in from..to {
                let (byte, bit) = ((x / 8) as usize, 7 - x % 8);
                if self.black_is_1 {
                    row[byte] |= 1 << bit;
                } else {
                    row[byte] &= !(1 << bit);
                }
            }
        }
    }
}

/// Strict, because a lenient decode can paint a damaged line white and still report success.
fn decode_ccitt(
    doc: &Document,
    data: &[u8],
    parms: Option<&Dictionary>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let columns = parm_int(doc, parms, b"Columns", 1728)?;
    if columns != i64::from(width) {
        return Err(format!(
            "a CCITTFaxDecode /Columns of {columns} on an image {width} wide"
        ));
    }
    let k = parm_int(doc, parms, b"K", 0)?;
    let settings = hayro_ccitt::DecodeSettings {
        columns: width,
        rows: height,
        end_of_block: parm_bool(doc, parms, b"EndOfBlock", true)?,
        end_of_line: parm_bool(doc, parms, b"EndOfLine", false)?,
        rows_are_byte_aligned: parm_bool(doc, parms, b"EncodedByteAlign", false)?,
        encoding: match k {
            k if k < 0 => hayro_ccitt::EncodingMode::Group4,
            0 => hayro_ccitt::EncodingMode::Group3_1D,
            _ => hayro_ccitt::EncodingMode::Group3_2D,
        },
        strict: true,
    };
    checked_size(width, height, 1)?;
    let stride = row_stride(width, 1, 1)?;
    let mut rows = CcittRows {
        out: Vec::with_capacity(stride * height as usize),
        stride,
        columns: width,
        rows_wanted: height,
        rows_seen: 0,
        black_is_1: parm_bool(doc, parms, b"BlackIs1", false)?,
    };
    let mut context = hayro_ccitt::DecoderContext::new(settings);
    hayro_ccitt::decode(data, &mut rows, &mut context)
        .map_err(|e| format!("CCITTFaxDecode data that will not decode ({e})"))?;
    if rows.rows_seen < height {
        return Err(format!(
            "CCITTFaxDecode data of {} rows on an image {height} high",
            rows.rows_seen
        ));
    }
    Ok(rows.out)
}

#[derive(Default)]
struct Jbig2Rows {
    out: Vec<u8>,
    current: u8,
    filled: u8,
    rows: u32,
}

impl hayro_jbig2::Decoder for Jbig2Rows {
    fn push_pixel(&mut self, black: bool) {
        // JBIG2 has a set bit mean black; a PDF sample has black as 0 (§ 7.4.7).
        self.current = (self.current << 1) | u8::from(!black);
        self.filled += 1;
        if self.filled == 8 {
            self.out.push(self.current);
            self.current = 0;
            self.filled = 0;
        }
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        let byte = if black { 0x00 } else { 0xFF };
        self.out
            .extend(std::iter::repeat_n(byte, chunk_count as usize));
    }

    fn next_line(&mut self) {
        if self.filled > 0 {
            self.out.push(self.current << (8 - self.filled));
            self.current = 0;
            self.filled = 0;
        }
        self.rows += 1;
    }
}

/// Strict, because a lenient decode of a page missing its globals reports success and a blank page.
fn decode_jbig2(
    doc: &Document,
    data: &[u8],
    parms: Option<&Dictionary>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let settings = hayro_jbig2::DecodeSettings { strict: true };
    let globals = match parms.and_then(|p| p.get(b"JBIG2Globals").ok()) {
        None => None,
        Some(g) => {
            let Some(Object::Stream(stream)) = deref(doc, g) else {
                return Err("a /JBIG2Globals that is not a stream".to_string());
            };
            let bytes = decode_stream_fully(doc, stream)
                .map_err(|e| format!("/JBIG2Globals that will not decode ({e})"))?;
            Some(
                hayro_jbig2::Globals::new(&bytes, &settings)
                    .map_err(|e| format!("/JBIG2Globals that will not decode ({e})"))?,
            )
        }
    };
    let page = hayro_jbig2::Page::new_embedded(data, globals.as_ref(), &settings)
        .map_err(|e| format!("JBIG2Decode data that will not decode ({e})"))?;
    // Compared first: the page header can claim 65535×65535 from a tiny stream.
    if (page.width(), page.height()) != (width, height) {
        return Err(format!(
            "a JBIG2 page of {}×{} on an image of {width}×{height}",
            page.width(),
            page.height()
        ));
    }
    checked_size(width, height, 1)?;
    let mut rows = Jbig2Rows::default();
    page.decode(&mut rows)
        .map_err(|e| format!("JBIG2Decode data that will not decode ({e})"))?;
    if rows.rows != height {
        return Err(format!(
            "JBIG2Decode data of {} rows on an image {height} high",
            rows.rows
        ));
    }
    Ok(rows.out)
}

/// Decodes an image to pixels.  A codec that is passed through has been dealt with by the caller, so
/// here one can only be a mask's.
fn decode_pixels(doc: &Document, src: &ImageSource) -> Result<Pixels, String> {
    let width = dimension(src, doc, b"Width", b"W")?;
    let height = dimension(src, doc, b"Height", b"H")?;
    let chain = filter_chain(doc, src)?;
    let (data, codec) = undo_filters(doc, src, &chain)?;
    let image_mask = matches!(
        src.entry(doc, b"ImageMask", b"IM"),
        Some(Object::Boolean(true))
    );
    let bilevel_codec = matches!(codec, Some((Codec::Ccitt | Codec::Jbig2, _)));

    let bits = if image_mask || bilevel_codec {
        match src.number(doc, b"BitsPerComponent", b"BPC") {
            None | Some(1) => 1,
            Some(other) => return Err(format!("{other} bits per component on a 1-bit image")),
        }
    } else {
        match src.number(doc, b"BitsPerComponent", b"BPC") {
            Some(b @ (1 | 2 | 4 | 8 | 16)) => b as u32,
            Some(other) => return Err(format!("{other} bits per component")),
            None => return Err("no /BitsPerComponent".to_string()),
        }
    };

    let samples: Cow<[u8]> = match codec {
        None => data,
        Some((Codec::Ccitt, parms)) => Cow::Owned(decode_ccitt(doc, &data, parms, width, height)?),
        Some((Codec::Jbig2, parms)) => Cow::Owned(decode_jbig2(doc, &data, parms, width, height)?),
        Some((Codec::Dct, _)) => {
            return Err("a DCTDecode mask, which cannot be applied".to_string());
        }
        Some((Codec::Jpx, _)) => {
            return Err("a JPXDecode mask, which cannot be applied".to_string());
        }
    };

    if image_mask {
        let decode = read_decode_array(doc, src, 1)?;
        return convert_stencil(&samples, width, height, decode.as_deref());
    }
    let space = match src.entry(doc, b"ColorSpace", b"CS") {
        Some(obj) => parse_color_space(doc, obj, src.resources, 0)?,
        // A 1-bit codec has only one reading without a colour space.
        None if bilevel_codec => ColorSpace::Gray,
        None => return Err("no /ColorSpace".to_string()),
    };
    let decode = read_decode_array(doc, src, space.components())?;
    // A soft mask overrides /Mask (§ 8.9.5, Table 89), the colour-key form included.
    let color_key = match src.entry(doc, b"Mask", b"Mask") {
        Some(obj @ Object::Array(_)) if mask_kind(doc, src)? == "color_key" => {
            let key = number_array(doc, obj).ok_or("a /Mask array that is not numbers")?;
            if key.len() != space.components() * 2 {
                return Err(format!(
                    "a /Mask array of {} numbers for {} component(s)",
                    key.len(),
                    space.components()
                ));
            }
            Some(key)
        }
        _ => None,
    };
    convert_samples(
        &samples,
        width,
        height,
        bits,
        &space,
        decode.as_deref(),
        color_key.as_deref(),
    )
}

/// Composites `base` onto white through `alpha` (grey, 255 opaque), nearest-neighbour, which keeps a
/// mask's edges hard.  The result takes the larger of each dimension: a scan's mask often holds the
/// words at full resolution over a tiny colour image.
fn composite(base: Pixels, alpha: &Pixels) -> Result<Pixels, String> {
    let width = base.width.max(alpha.width);
    let height = base.height.max(alpha.height);
    let size = checked_size(width, height, base.channels)?;
    let scaled =
        |x: u32, from: u32, to: u32| (u64::from(x) * u64::from(from) / u64::from(to)) as usize;
    let mut data = Vec::with_capacity(size);
    for y in 0..height {
        let base_row = scaled(y, base.height, height) * base.width as usize;
        let alpha_row = scaled(y, alpha.height, height) * alpha.width as usize;
        for x in 0..width {
            let color = (base_row + scaled(x, base.width, width)) * base.channels;
            let a = u32::from(alpha.data[alpha_row + scaled(x, alpha.width, width)]);
            for c in &base.data[color..color + base.channels] {
                data.push(((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8);
            }
        }
    }
    let pixels = Pixels {
        width,
        height,
        channels: base.channels,
        data,
    };
    if masks_a_picture_away(&pixels, alpha) {
        return Err(LIGHT_THROUGH_A_MASK.to_string());
    }
    Ok(pixels)
}

/// The spread below which a composite counts as having no picture in it.
const NO_PICTURE_SPREAD: u8 = 8;

const LIGHT_THROUGH_A_MASK: &str =
    "a mask holding a picture in ink too light to show on white, so the file would be blank";

fn spread(data: &[u8]) -> u8 {
    let (low, high) = data
        .iter()
        .fold((255u8, 0u8), |(l, h), v| (l.min(*v), h.max(*v)));
    high.saturating_sub(low)
}

/// True when the mask has a shape, showing some of the image and not the rest, but the result is
/// one flat tone.  That is light ink on paper: white words through a mask over a dark panel, say.
fn masks_a_picture_away(result: &Pixels, alpha: &Pixels) -> bool {
    spread(&alpha.data) >= 128 && spread(&result.data) < NO_PICTURE_SPREAD
}

/// Which mask an image has, the soft mask taking precedence (§ 8.9.5, Table 89).  A present entry
/// that is not usable is refused rather than read as no mask.
fn mask_kind(doc: &Document, src: &ImageSource) -> Result<&'static str, String> {
    if src.inline {
        return Ok(match src.entry(doc, b"Mask", b"Mask") {
            Some(Object::Array(_)) => "color_key",
            _ => "none",
        });
    }
    match src.entry(doc, b"SMask", b"SMask") {
        Some(Object::Stream(_)) => return Ok("smask"),
        Some(_) => return Err("a /SMask that is not a stream".to_string()),
        None => {}
    }
    match src.entry(doc, b"Mask", b"Mask") {
        Some(Object::Stream(_)) => Ok("explicit"),
        Some(Object::Array(_)) => Ok("color_key"),
        Some(_) => Err("a /Mask that is neither a stream nor an array".to_string()),
        None => Ok("none"),
    }
}

fn mask_source<'a>(
    doc: &'a Document,
    src: &ImageSource<'a>,
    key: &[u8],
) -> Result<ImageSource<'a>, String> {
    match src.entry(doc, key, key) {
        Some(Object::Stream(s)) => Ok(ImageSource {
            dict: &s.dict,
            data: &s.content,
            resources: src.resources,
            inline: false,
        }),
        _ => Err(format!(
            "a /{} that is not a stream",
            String::from_utf8_lossy(key)
        )),
    }
}

fn encode_png(p: &Pixels) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, p.width, p.height);
    encoder.set_color(if p.channels == 1 {
        png::ColorType::Grayscale
    } else {
        png::ColorType::Rgb
    });
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("a PNG that could not be encoded ({e})"))?;
    writer
        .write_image_data(&p.data)
        .map_err(|e| format!("a PNG that could not be encoded ({e})"))?;
    writer
        .finish()
        .map_err(|e| format!("a PNG that could not be encoded ({e})"))?;
    Ok(bytes)
}

/// The file bytes for one image.  The `Err` names what stopped it.
pub(crate) fn export_image(doc: &Document, src: &ImageSource) -> Result<ExportedImage, String> {
    let chain = filter_chain(doc, src)?;
    let filter = chain
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let color_space = src.entry(doc, b"ColorSpace", b"CS").map_or_else(
        || "-".to_string(),
        |o| crate::helpers::format_color_space(o, doc),
    );
    let bits_per_component = src.number(doc, b"BitsPerComponent", b"BPC");
    let mask = mask_kind(doc, src)?;

    let passthrough = match chain.last().and_then(|s| codec_of(&s.name)) {
        Some(Codec::Dct) => Some(ExportFormat::Jpeg),
        Some(Codec::Jpx) => Some(ExportFormat::Jp2),
        _ => None,
    };
    if let Some(format) = passthrough {
        let width = dimension(src, doc, b"Width", b"W")?;
        let height = dimension(src, doc, b"Height", b"H")?;
        // A /Decode that changes the picture cannot be applied to bytes passed through.
        if let Some(obj) = src.entry(doc, b"Decode", b"D") {
            let decode =
                number_array(doc, obj).ok_or("a /Decode that is not an array of numbers")?;
            let is_default = decode.chunks(2).all(|pair| pair == [0.0, 1.0]);
            if !is_default {
                return Err(format!(
                    "a {} image with a /Decode array, which cannot be applied to its bytes as they stand",
                    chain.last().map_or("", |s| s.name.as_str())
                ));
            }
        }
        // Bytes passed through are the picture only in a colour space that reads them directly.
        // A separation's or a palette's samples would come out as a negative or an index map.
        if let Some(obj) = src.entry(doc, b"ColorSpace", b"CS")
            && matches!(
                parse_color_space(doc, obj, src.resources, 0)?,
                ColorSpace::Indexed { .. }
            )
        {
            return Err(format!(
                "an Indexed {} image, whose bytes as they stand are palette indices",
                chain.last().map_or("", |s| s.name.as_str())
            ));
        }
        let (data, _) = undo_filters(doc, src, &chain)?;
        match format {
            ExportFormat::Jpeg => check_jpeg(&data, width, height)?,
            _ => check_jp2(&data)?,
        }
        return Ok(ExportedImage {
            bytes: data.into_owned(),
            format,
            width,
            height,
            color_space,
            bits_per_component,
            filter,
            mask,
            mask_applied: mask == "none",
        });
    }

    let mut pixels = decode_pixels(doc, src)?;
    match mask {
        "smask" => {
            let alpha = decode_pixels(doc, &mask_source(doc, src, b"SMask")?)?;
            if alpha.channels != 1 {
                return Err("a /SMask that is not grey".to_string());
            }
            pixels = composite(pixels, &alpha)?;
        }
        "explicit" => {
            let stencil = decode_pixels(doc, &mask_source(doc, src, b"Mask")?)?;
            if stencil.channels != 1 {
                return Err("a /Mask image that is not a stencil".to_string());
            }
            // Painted is black in a stencil's pixels, and painted is where the image shows.
            let opacity = Pixels {
                data: stencil.data.iter().map(|v| 255 - v).collect(),
                ..stencil
            };
            pixels = composite(pixels, &opacity)?;
        }
        _ => {}
    }
    Ok(ExportedImage {
        bytes: encode_png(&pixels)?,
        format: ExportFormat::Png,
        width: pixels.width,
        height: pixels.height,
        color_space,
        bits_per_component,
        filter,
        mask,
        mask_applied: true,
    })
}

/// A JPEG's frame must be there and be the size the image claims, or the bytes are not this picture.
fn check_jpeg(data: &[u8], width: u32, height: u32) -> Result<(), String> {
    let not_jpeg = || "a DCTDecode stream that does not hold jpeg data".to_string();
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err(not_jpeg());
    }
    let mut at = 2;
    loop {
        // A marker: fill bytes of 0xFF, then its code.
        while data.get(at) == Some(&0xFF) && data.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        if data.get(at) != Some(&0xFF) {
            return Err(not_jpeg());
        }
        let marker = *data.get(at + 1).ok_or_else(not_jpeg)?;
        let length = data
            .get(at + 2..at + 4)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
            .ok_or_else(not_jpeg)?;
        let is_frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_frame {
            let frame = data.get(at + 5..at + 9).ok_or_else(not_jpeg)?;
            let frame_height = u32::from(u16::from_be_bytes([frame[0], frame[1]]));
            let frame_width = u32::from(u16::from_be_bytes([frame[2], frame[3]]));
            // A height of 0 means a later marker gives it (DNL), so only the width can be compared.
            if frame_width != width || (frame_height != 0 && frame_height != height) {
                return Err(format!(
                    "a JPEG of {frame_width}×{frame_height} on an image of {width}×{height}"
                ));
            }
            return Ok(());
        }
        if marker == 0xDA {
            return Err(not_jpeg());
        }
        at += 2 + length;
    }
}

/// A JPEG 2000 file must carry its codestream box; a bare codestream, its size marker straight after
/// its start.
fn check_jp2(data: &[u8]) -> Result<(), String> {
    let not_jp2 = || "a JPXDecode stream that does not hold jp2 data".to_string();
    if data.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        return Ok(());
    }
    if !data.starts_with(&[0, 0, 0, 0x0C, 0x6A, 0x50, 0x20, 0x20]) {
        return Err(not_jp2());
    }
    let mut at = 0usize;
    while let Some(header) = data.get(at..at + 8) {
        if &header[4..] == b"jp2c" {
            return Ok(());
        }
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        // 0 runs to the end of the file, and 1 has a 64-bit length; neither can precede jp2c usefully
        // here without one, and a box shorter than its header is malformed.
        if length < 8 {
            return Err(not_jp2());
        }
        at = at.checked_add(length).ok_or_else(not_jp2)?;
    }
    Err(not_jp2())
}

/// The refusal when an output's extension does not match what would be written.
pub(crate) fn extension_mismatch(wanted: ExportFormat, actual: ExportFormat) -> String {
    format!(
        "this image is written as {}, but the output is named for {}; name it {}",
        actual.name(),
        wanted.name(),
        actual.extension()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::zlib_compress;
    use lopdf::{Stream, StringFormat};
    use pretty_assertions::assert_eq;

    fn name(n: &str) -> Object {
        Object::Name(n.as_bytes().to_vec())
    }

    fn image_dict(w: i64, h: i64, bits: i64, space: Object) -> Dictionary {
        let mut d = Dictionary::new();
        d.set("Subtype", name("Image"));
        d.set("Width", w);
        d.set("Height", h);
        d.set("BitsPerComponent", bits);
        d.set("ColorSpace", space);
        d
    }

    fn numbers(values: &[f32]) -> Object {
        Object::Array(values.iter().map(|v| Object::Real(*v)).collect())
    }

    fn export(doc: &Document, dict: &Dictionary, data: &[u8]) -> Result<ExportedImage, String> {
        export_image(
            doc,
            &ImageSource {
                dict,
                data,
                resources: None,
                inline: false,
            },
        )
    }

    /// Exports and decodes the PNG again, so an assertion sees the file's pixels: width, height,
    /// channels, samples.
    fn pixels_in(doc: &Document, dict: &Dictionary, data: &[u8]) -> (u32, u32, usize, Vec<u8>) {
        let image = export(doc, dict, data).unwrap();
        assert_eq!(image.format, ExportFormat::Png);
        let decoder = png::Decoder::new(std::io::Cursor::new(image.bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(frame.bit_depth, png::BitDepth::Eight);
        let channels = match frame.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::Rgb => 3,
            other => panic!("unexpected {other:?}"),
        };
        buffer.truncate(frame.buffer_size());
        (frame.width, frame.height, channels, buffer)
    }

    fn pixels(dict: &Dictionary, data: &[u8]) -> Vec<u8> {
        pixels_in(&Document::new(), dict, data).3
    }

    fn refusal(dict: &Dictionary, data: &[u8]) -> String {
        match export(&Document::new(), dict, data) {
            Ok(_) => panic!("expected a refusal"),
            Err(why) => why,
        }
    }

    // ── Colour spaces and bit depths ────────────────────────────────────

    #[test]
    fn eight_bit_grey_and_rgb_are_the_samples() {
        let d = image_dict(2, 1, 8, name("DeviceGray"));
        assert_eq!(
            pixels_in(&Document::new(), &d, &[7, 200]),
            (2, 1, 1, vec![7, 200])
        );
        let d = image_dict(1, 2, 8, name("DeviceRGB"));
        assert_eq!(
            pixels_in(&Document::new(), &d, &[1, 2, 3, 4, 5, 6]),
            (1, 2, 3, vec![1, 2, 3, 4, 5, 6])
        );
    }

    #[test]
    fn extra_data_is_ignored_and_short_data_is_refused() {
        let d = image_dict(1, 1, 8, name("DeviceGray"));
        assert_eq!(pixels(&d, &[9, 1, 2, 3]), vec![9]);
        let d = image_dict(2, 2, 8, name("DeviceGray"));
        let why = refusal(&d, &[1, 2, 3]);
        assert!(why.contains("3 bytes") && why.contains("needs 4"), "{why}");
    }

    #[test]
    fn one_bit_rows_are_padded_to_a_byte() {
        let d = image_dict(3, 2, 1, name("DeviceGray"));
        assert_eq!(
            pixels(&d, &[0b1010_0000, 0b0101_1111]),
            vec![255, 0, 255, 0, 255, 0]
        );
    }

    #[test]
    fn two_four_and_sixteen_bit_samples_scale_to_eight() {
        let d = image_dict(4, 1, 2, name("DeviceGray"));
        assert_eq!(pixels(&d, &[0b00_01_10_11]), vec![0, 85, 170, 255]);
        let d = image_dict(2, 1, 4, name("DeviceGray"));
        assert_eq!(pixels(&d, &[0x0F]), vec![0, 255]);
        let d = image_dict(2, 1, 16, name("DeviceGray"));
        assert_eq!(pixels(&d, &[0x00, 0x00, 0x80, 0x00]), vec![0, 128]);
    }

    #[test]
    fn decode_array_inverts_and_remaps() {
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Decode", numbers(&[1.0, 0.0]));
        assert_eq!(pixels(&d, &[0, 255]), vec![255, 0]);

        let mut d = image_dict(2, 1, 1, name("DeviceGray"));
        d.set("Decode", numbers(&[1.0, 0.0]));
        assert_eq!(pixels(&d, &[0b1000_0000]), vec![0, 255]);

        let mut d = image_dict(1, 1, 8, name("DeviceRGB"));
        d.set("Decode", numbers(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.5]));
        assert_eq!(pixels(&d, &[0, 255, 255]), vec![255, 255, 128]);
    }

    #[test]
    fn a_decode_array_of_the_wrong_length_is_refused() {
        let mut d = image_dict(1, 1, 8, name("DeviceRGB"));
        d.set("Decode", numbers(&[1.0, 0.0]));
        assert!(refusal(&d, &[0, 0, 0]).contains("/Decode"));
    }

    #[test]
    fn cmyk_converts_naively() {
        let d = image_dict(3, 1, 8, name("DeviceCMYK"));
        assert_eq!(
            pixels(&d, &[255, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0]),
            vec![0, 255, 255, 0, 0, 0, 255, 255, 255]
        );
    }

    fn indexed(base: Object, hival: i64, lookup: Object) -> Object {
        Object::Array(vec![name("Indexed"), base, Object::Integer(hival), lookup])
    }

    fn string(bytes: &[u8]) -> Object {
        Object::String(bytes.to_vec(), StringFormat::Hexadecimal)
    }

    #[test]
    fn indexed_expands_through_its_palette() {
        let d = image_dict(
            2,
            1,
            8,
            indexed(name("DeviceRGB"), 1, string(&[10, 20, 30, 40, 50, 60])),
        );
        assert_eq!(pixels(&d, &[1, 0]), vec![40, 50, 60, 10, 20, 30]);
    }

    #[test]
    fn indexed_at_low_bits_on_a_grey_base_stays_grey() {
        let d = image_dict(
            2,
            1,
            4,
            indexed(name("DeviceGray"), 15, string(&[0, 17, 34])),
        );
        assert_eq!(
            pixels_in(&Document::new(), &d, &[0x21]),
            (2, 1, 1, vec![34, 17])
        );
    }

    #[test]
    fn indexed_over_cmyk_converts_the_palette() {
        let d = image_dict(
            1,
            1,
            8,
            indexed(name("DeviceCMYK"), 0, string(&[255, 0, 0, 0])),
        );
        assert_eq!(pixels(&d, &[0]), vec![0, 255, 255]);
    }

    #[test]
    fn indexed_palette_in_a_stream_is_read() {
        let mut doc = Document::new();
        let mut palette = Dictionary::new();
        palette.set("Filter", name("FlateDecode"));
        doc.objects.insert(
            (9, 0),
            Object::Stream(Stream::new(palette, zlib_compress(&[1, 2, 3]))),
        );
        let d = image_dict(
            1,
            1,
            8,
            indexed(name("DeviceRGB"), 0, Object::Reference((9, 0))),
        );
        assert_eq!(pixels_in(&doc, &d, &[0]).3, vec![1, 2, 3]);
    }

    #[test]
    fn an_index_past_hival_or_the_palette_is_refused() {
        let d = image_dict(1, 1, 8, indexed(name("DeviceRGB"), 0, string(&[1, 2, 3])));
        assert!(refusal(&d, &[1]).contains("palette index"));
        let d = image_dict(1, 1, 8, indexed(name("DeviceRGB"), 5, string(&[1, 2, 3])));
        assert!(refusal(&d, &[3]).contains("palette index"));
    }

    fn icc_doc(entries: Vec<(&str, Object)>) -> (Document, Object) {
        let mut doc = Document::new();
        let mut profile = Dictionary::new();
        for (k, v) in entries {
            profile.set(k, v);
        }
        doc.objects
            .insert((9, 0), Object::Stream(Stream::new(profile, vec![])));
        (
            doc,
            Object::Array(vec![name("ICCBased"), Object::Reference((9, 0))]),
        )
    }

    #[test]
    fn icc_based_goes_by_n_then_alternate() {
        let (doc, space) = icc_doc(vec![("N", Object::Integer(1))]);
        assert_eq!(pixels_in(&doc, &image_dict(1, 1, 8, space), &[9]).2, 1);
        let (doc, space) = icc_doc(vec![("N", Object::Integer(3))]);
        assert_eq!(
            pixels_in(&doc, &image_dict(1, 1, 8, space), &[1, 2, 3]).3,
            vec![1, 2, 3]
        );
        let (doc, space) = icc_doc(vec![("N", Object::Integer(4))]);
        assert_eq!(
            pixels_in(&doc, &image_dict(1, 1, 8, space), &[0, 0, 0, 255]).3,
            vec![0, 0, 0]
        );
        let (doc, space) = icc_doc(vec![("Alternate", name("DeviceRGB"))]);
        assert_eq!(
            pixels_in(&doc, &image_dict(1, 1, 8, space), &[1, 2, 3]).2,
            3
        );
        let (doc, space) = icc_doc(vec![]);
        let d = image_dict(1, 1, 8, space);
        assert!(export(&doc, &d, &[0]).err().unwrap().contains("/N"));
    }

    #[test]
    fn colour_spaces_it_cannot_convert_are_refused_by_name() {
        for family in ["Separation", "DeviceN", "Lab", "Pattern"] {
            let d = image_dict(1, 1, 8, Object::Array(vec![name(family)]));
            assert!(refusal(&d, &[0]).contains(family), "{family}");
        }
        let d = image_dict(1, 1, 8, name("NoSuchSpace"));
        assert!(refusal(&d, &[0]).contains("NoSuchSpace"));
    }

    #[test]
    fn a_named_colour_space_resolves_through_resources() {
        let mut spaces = Dictionary::new();
        spaces.set("Cs1", name("DeviceRGB"));
        let mut resources = Dictionary::new();
        resources.set("ColorSpace", Object::Dictionary(spaces));
        let d = image_dict(1, 1, 8, name("Cs1"));
        let doc = Document::new();
        let image = export_image(
            &doc,
            &ImageSource {
                dict: &d,
                data: &[1, 2, 3],
                resources: Some(&resources),
                inline: false,
            },
        )
        .unwrap();
        assert_eq!(image.width, 1);
    }

    // ── Stencils and masks ──────────────────────────────────────────────

    fn stencil(w: i64, h: i64) -> Dictionary {
        let mut d = Dictionary::new();
        d.set("Subtype", name("Image"));
        d.set("Width", w);
        d.set("Height", h);
        d.set("ImageMask", true);
        d
    }

    #[test]
    fn a_stencil_paints_black_where_its_decode_says() {
        let d = stencil(2, 1);
        assert_eq!(pixels(&d, &[0b0100_0000]), vec![0, 255]);
        let mut d = stencil(2, 1);
        d.set("Decode", numbers(&[1.0, 0.0]));
        assert_eq!(pixels(&d, &[0b0100_0000]), vec![255, 0]);
    }

    #[test]
    fn a_stencil_of_more_than_one_bit_is_refused() {
        let mut d = stencil(1, 1);
        d.set("BitsPerComponent", 8);
        assert!(refusal(&d, &[0]).contains("8 bits"));
    }

    fn doc_with(id: u32, stream_dict: Dictionary, data: &[u8]) -> Document {
        let mut doc = Document::new();
        doc.objects.insert(
            (id, 0),
            Object::Stream(Stream::new(stream_dict, data.to_vec())),
        );
        doc
    }

    #[test]
    fn a_soft_mask_composites_onto_white() {
        let doc = doc_with(9, image_dict(3, 1, 8, name("DeviceGray")), &[255, 128, 0]);
        let mut d = image_dict(3, 1, 8, name("DeviceGray"));
        d.set("SMask", Object::Reference((9, 0)));
        // Black shown through opaque, half and transparent.
        assert_eq!(pixels_in(&doc, &d, &[0, 0, 0]).3, vec![0, 127, 255]);
        assert_eq!(export(&doc, &d, &[0, 0, 0]).unwrap().mask, "smask");
    }

    #[test]
    fn a_larger_mask_sets_the_size_so_its_words_survive() {
        // The shape of a mixed-raster scan: one colour pixel, and the words in a full-size mask.
        let mut mask = image_dict(2, 2, 8, name("DeviceGray"));
        mask.remove(b"Subtype");
        let doc = doc_with(9, mask, &[255, 0, 0, 255]);
        let mut d = image_dict(1, 1, 8, name("DeviceRGB"));
        d.set("SMask", Object::Reference((9, 0)));
        assert_eq!(
            pixels_in(&doc, &d, &[10, 20, 30]),
            (
                2,
                2,
                3,
                vec![10, 20, 30, 255, 255, 255, 255, 255, 255, 10, 20, 30]
            )
        );
    }

    #[test]
    fn a_smaller_mask_stretches_to_the_image() {
        let doc = doc_with(9, image_dict(1, 1, 8, name("DeviceGray")), &[0]);
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("SMask", Object::Reference((9, 0)));
        assert_eq!(pixels_in(&doc, &d, &[0, 0]).3, vec![255, 255]);
    }

    #[test]
    fn an_explicit_mask_shows_the_image_where_it_paints() {
        let doc = doc_with(9, stencil(2, 1), &[0b0100_0000]);
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Mask", Object::Reference((9, 0)));
        assert_eq!(pixels_in(&doc, &d, &[50, 50]).3, vec![50, 255]);
        assert_eq!(export(&doc, &d, &[50, 50]).unwrap().mask, "explicit");
    }

    #[test]
    fn a_soft_mask_takes_precedence_over_a_mask() {
        let mut doc = doc_with(9, stencil(1, 1), &[0x80]); // masks everything out
        doc.objects.insert(
            (10, 0),
            Object::Stream(Stream::new(
                image_dict(1, 1, 8, name("DeviceGray")),
                vec![255],
            )),
        );
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("Mask", Object::Reference((9, 0)));
        d.set("SMask", Object::Reference((10, 0)));
        assert_eq!(pixels_in(&doc, &d, &[50]).3, vec![50]);
    }

    #[test]
    fn a_colour_key_turns_its_range_white_on_raw_samples() {
        let mut d = image_dict(3, 1, 8, name("DeviceGray"));
        d.set("Mask", numbers(&[10.0, 20.0]));
        // A /Decode must not move what the key matches: the key reads samples, not colours.
        d.set("Decode", numbers(&[1.0, 0.0]));
        assert_eq!(pixels(&d, &[15, 100, 21]), vec![255, 155, 234]);
    }

    #[test]
    fn a_mask_it_cannot_decode_refuses_the_image() {
        let mut mask = image_dict(1, 1, 8, name("DeviceGray"));
        mask.set("Filter", name("DCTDecode"));
        let doc = doc_with(9, mask, &[0xFF, 0xD8]);
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("SMask", Object::Reference((9, 0)));
        assert!(export(&doc, &d, &[0]).err().unwrap().contains("DCTDecode"));
    }

    // ── Filters and predictors ──────────────────────────────────────────

    fn parms(entries: Vec<(&str, Object)>) -> Object {
        let mut d = Dictionary::new();
        for (k, v) in entries {
            d.set(k, v);
        }
        Object::Dictionary(d)
    }

    #[test]
    fn a_png_predictor_is_undone_so_the_samples_are_the_picture() {
        let mut d = image_dict(2, 2, 8, name("DeviceGray"));
        d.set("Filter", name("FlateDecode"));
        d.set(
            "DecodeParms",
            parms(vec![
                ("Predictor", Object::Integer(12)),
                ("Columns", Object::Integer(2)),
            ]),
        );
        let data = zlib_compress(&[2, 1, 2, 2, 1, 1]);
        assert_eq!(pixels(&d, &data), vec![1, 2, 2, 3]);
    }

    #[test]
    fn a_tiff_predictor_is_undone() {
        let mut d = image_dict(3, 1, 8, name("DeviceGray"));
        d.set("Filter", name("FlateDecode"));
        d.set(
            "DecodeParms",
            parms(vec![
                ("Predictor", Object::Integer(2)),
                ("Columns", Object::Integer(3)),
            ]),
        );
        assert_eq!(pixels(&d, &zlib_compress(&[10, 5, 5])), vec![10, 15, 20]);
    }

    #[test]
    fn decode_parms_in_an_array_pair_with_their_filter() {
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set(
            "Filter",
            Object::Array(vec![name("ASCIIHexDecode"), name("FlateDecode")]),
        );
        d.set(
            "DecodeParms",
            Object::Array(vec![
                Object::Null,
                parms(vec![
                    ("Predictor", Object::Integer(12)),
                    ("Columns", Object::Integer(2)),
                ]),
            ]),
        );
        let compressed = zlib_compress(&[1, 7, 1]);
        let hex: String = compressed.iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(pixels(&d, format!("{hex}>").as_bytes()), vec![7, 8]);
    }

    #[test]
    fn an_indirect_filter_and_indirect_parms_are_followed() {
        let mut doc = Document::new();
        doc.objects.insert((8, 0), name("FlateDecode"));
        doc.objects.insert(
            (9, 0),
            parms(vec![
                ("Predictor", Object::Integer(2)),
                ("Columns", Object::Integer(2)),
            ]),
        );
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Filter", Object::Reference((8, 0)));
        d.set("DecodeParms", Object::Reference((9, 0)));
        assert_eq!(pixels_in(&doc, &d, &zlib_compress(&[3, 4])).3, vec![3, 7]);
    }

    #[test]
    fn lzw_and_run_length_decode() {
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let lzw = encoder.encode(&[1, 2]).unwrap();
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Filter", name("LZWDecode"));
        assert_eq!(pixels(&d, &lzw), vec![1, 2]);

        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Filter", name("RunLengthDecode"));
        assert_eq!(pixels(&d, &[255, 9, 128]), vec![9, 9]);
    }

    #[test]
    fn filters_it_cannot_decode_and_bad_chains_are_refused() {
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("Filter", name("Crypt"));
        assert!(refusal(&d, &[0]).contains("Crypt"));

        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set(
            "Filter",
            Object::Array(vec![name("DCTDecode"), name("FlateDecode")]),
        );
        assert!(refusal(&d, &[0]).contains("followed"));

        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("Filter", name("FlateDecode"));
        assert!(refusal(&d, b"not zlib").contains("FlateDecode"));

        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("Filter", Object::Integer(1));
        assert!(refusal(&d, &[0]).contains("/Filter"));
    }

    #[test]
    fn bad_dimensions_and_depths_are_refused_before_any_allocation() {
        let d = image_dict(0, 1, 8, name("DeviceGray"));
        assert!(refusal(&d, &[]).contains("/Width of 0"));
        let d = image_dict(1, -1, 8, name("DeviceGray"));
        assert!(refusal(&d, &[]).contains("/Height of -1"));
        let d = image_dict(1, 1, 3, name("DeviceGray"));
        assert!(refusal(&d, &[0]).contains("3 bits"));
        let d = image_dict(100_000, 100_000, 8, name("DeviceGray"));
        assert!(refusal(&d, &[0]).contains("limit"));
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.remove(b"ColorSpace");
        assert!(refusal(&d, &[0]).contains("/ColorSpace"));
    }

    // ── What a review found ─────────────────────────────────────────────

    #[test]
    fn indirect_decode_parms_are_followed_not_defaulted() {
        let mut doc = Document::new();
        doc.objects.insert((8, 0), Object::Integer(12));
        doc.objects.insert((9, 0), Object::Integer(1));
        let mut d = image_dict(1, 2, 8, name("DeviceGray"));
        d.set("Filter", name("FlateDecode"));
        d.set(
            "DecodeParms",
            parms(vec![
                ("Predictor", Object::Reference((8, 0))),
                ("Columns", Object::Reference((9, 0))),
            ]),
        );
        let data = zlib_compress(&[2, 100, 2, 5]);
        assert_eq!(pixels_in(&doc, &d, &data).3, vec![100, 105]);
    }

    #[test]
    fn a_decode_parm_of_the_wrong_type_is_refused_not_defaulted() {
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("Filter", name("FlateDecode"));
        d.set(
            "DecodeParms",
            parms(vec![("Predictor", Object::Real(12.0))]),
        );
        assert!(refusal(&d, &zlib_compress(&[0])).contains("/Predictor"));

        let mut d = ccitt_dict(16, 1, vec![("BlackIs1", Object::Integer(1))]);
        d.set("Filter", name("CCITTFaxDecode"));
        assert!(refusal(&d, &[0]).contains("/BlackIs1"));
    }

    #[test]
    fn an_indirect_black_is_1_is_followed() {
        let mut doc = Document::new();
        doc.objects.insert((9, 0), Object::Boolean(true));
        let d = ccitt_dict(
            16,
            1,
            vec![
                ("K", Object::Integer(-1)),
                ("BlackIs1", Object::Reference((9, 0))),
            ],
        );
        let inverted: Vec<u8> = expected_row().iter().map(|v| 255 - v).collect();
        assert_eq!(pixels_in(&doc, &d, &bits("001 1011 011 1")).3, inverted);
    }

    #[test]
    fn an_indirect_icc_n_beats_the_alternate() {
        let mut doc = Document::new();
        doc.objects.insert((8, 0), Object::Integer(3));
        let mut profile = Dictionary::new();
        profile.set("N", Object::Reference((8, 0)));
        profile.set("Alternate", name("DeviceGray"));
        doc.objects
            .insert((9, 0), Object::Stream(Stream::new(profile, vec![])));
        let d = image_dict(
            1,
            1,
            8,
            Object::Array(vec![name("ICCBased"), Object::Reference((9, 0))]),
        );
        assert_eq!(pixels_in(&doc, &d, &[1, 2, 3]).2, 3);
    }

    #[test]
    fn a_palette_stream_gets_the_full_filter_walk() {
        // An indirect /Filter, which decode_stream would take for no filter at all.
        let mut doc = Document::new();
        doc.objects.insert((8, 0), name("FlateDecode"));
        let mut palette = Dictionary::new();
        palette.set("Filter", Object::Reference((8, 0)));
        doc.objects.insert(
            (9, 0),
            Object::Stream(Stream::new(palette, zlib_compress(&[10, 20, 30]))),
        );
        let d = image_dict(
            1,
            1,
            8,
            indexed(name("DeviceRGB"), 0, Object::Reference((9, 0))),
        );
        assert_eq!(pixels_in(&doc, &d, &[0]).3, vec![10, 20, 30]);

        // A predictor, whose row tags would otherwise land in the colours.
        let mut doc = Document::new();
        let mut palette = Dictionary::new();
        palette.set("Filter", name("FlateDecode"));
        palette.set(
            "DecodeParms",
            parms(vec![
                ("Predictor", Object::Integer(12)),
                ("Columns", Object::Integer(3)),
            ]),
        );
        doc.objects.insert(
            (9, 0),
            Object::Stream(Stream::new(palette, zlib_compress(&[0, 10, 20, 30]))),
        );
        let d = image_dict(
            1,
            1,
            8,
            indexed(name("DeviceRGB"), 0, Object::Reference((9, 0))),
        );
        assert_eq!(pixels_in(&doc, &d, &[0]).3, vec![10, 20, 30]);
    }

    #[test]
    fn a_soft_mask_overrides_a_colour_key() {
        let doc = doc_with(9, image_dict(2, 1, 8, name("DeviceGray")), &[255, 255]);
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Mask", numbers(&[10.0, 20.0]));
        d.set("SMask", Object::Reference((9, 0)));
        assert_eq!(pixels_in(&doc, &d, &[15, 100]).3, vec![15, 100]);
    }

    #[test]
    fn light_ink_through_a_mask_is_refused_rather_than_written_blank() {
        let doc = doc_with(9, image_dict(2, 1, 8, name("DeviceGray")), &[255, 0]);
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("SMask", Object::Reference((9, 0)));
        assert!(export(&doc, &d, &[255]).err().unwrap().contains("blank"));
        // Dark ink through the same mask is the ordinary case.
        assert_eq!(pixels_in(&doc, &d, &[0]).3, vec![0, 255]);
        // A uniform opaque mask has no shape to lose.
        let doc = doc_with(9, image_dict(2, 1, 8, name("DeviceGray")), &[255, 255]);
        assert_eq!(pixels_in(&doc, &d, &[255]).3, vec![255, 255]);
    }

    #[test]
    fn a_mask_entry_that_is_no_mask_is_refused_rather_than_ignored() {
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("SMask", Object::Integer(1));
        assert!(refusal(&d, &[0]).contains("/SMask"));
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("Mask", Object::Integer(1));
        assert!(refusal(&d, &[0]).contains("/Mask"));
    }

    #[test]
    fn a_big_image_and_mask_crossed_are_refused_from_almost_no_data() {
        // Each is one row or column, and together they demand 900 million pixels.
        let mut mask = image_dict(1, 30_000, 1, name("DeviceGray"));
        mask.remove(b"Subtype");
        let doc = doc_with(9, mask, &vec![0u8; 30_000]);
        let mut d = image_dict(30_000, 1, 1, name("DeviceGray"));
        d.set("SMask", Object::Reference((9, 0)));
        assert!(
            export(&doc, &d, &[0u8; 3750])
                .err()
                .unwrap()
                .contains("limit")
        );
    }

    #[test]
    fn data_in_an_external_file_is_refused() {
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set(
            "F",
            Object::String(b"other.bin".to_vec(), StringFormat::Literal),
        );
        assert!(refusal(&d, &[9]).contains("external"));
    }

    #[test]
    fn jpeg_in_a_colour_space_its_bytes_cannot_show_is_refused() {
        let mut d = jpeg_dict();
        d.set(
            "ColorSpace",
            Object::Array(vec![name("Separation"), name("Black")]),
        );
        assert!(refusal(&d, JPEG).contains("Separation"));

        let mut d = jpeg_dict();
        d.set(
            "ColorSpace",
            indexed(name("DeviceRGB"), 0, string(&[1, 2, 3])),
        );
        assert!(refusal(&d, JPEG).contains("palette indices"));
    }

    #[test]
    fn decode_parms_of_the_wrong_kind_are_refused_not_read_as_none() {
        let flated = crate::test_utils::zlib_compress(&[127]);
        for parms in [Object::Integer(3), Object::Array(vec![Object::Integer(3)])] {
            let mut d = image_dict(1, 1, 8, name("DeviceGray"));
            d.set("Filter", name("FlateDecode"));
            d.set("DecodeParms", parms.clone());
            assert!(refusal(&d, &flated).contains("/DecodeParms"), "{parms:?}");
        }
    }

    #[test]
    fn decode_parms_that_are_null_are_none() {
        let mut doc = Document::new();
        doc.objects.insert((9, 0), Object::Null);
        for parms in [
            Object::Null,
            Object::Reference((9, 0)),
            Object::Reference((99, 0)),
            Object::Array(vec![Object::Reference((9, 0))]),
        ] {
            let mut d = image_dict(1, 1, 8, name("DeviceGray"));
            d.set("Filter", name("FlateDecode"));
            d.set("DecodeParms", parms.clone());
            let flated = crate::test_utils::zlib_compress(&[127]);
            assert_eq!(pixels_in(&doc, &d, &flated).3, vec![127], "{parms:?}");
        }
    }

    #[test]
    fn a_colour_key_over_a_picture_it_would_leave_blank_is_refused() {
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Mask", numbers(&[0.0, 0.0]));
        assert!(refusal(&d, &[255, 0]).contains("too light"));
    }

    #[test]
    fn a_colour_key_with_nothing_to_lose_is_written() {
        let mut d = image_dict(2, 1, 8, name("DeviceGray"));
        d.set("Mask", numbers(&[0.0, 0.0]));
        // Something still shows, or there was no picture.
        assert_eq!(pixels_in(&Document::new(), &d, &[100, 0]).3, vec![100, 255]);
        assert_eq!(pixels_in(&Document::new(), &d, &[0, 0]).3, vec![255, 255]);
        assert_eq!(
            pixels_in(&Document::new(), &d, &[255, 255]).3,
            vec![255, 255]
        );
        // A blank scan whose key removes white: faithfully near-blank.
        let mut d = image_dict(4, 1, 8, name("DeviceGray"));
        d.set("Mask", numbers(&[255.0, 255.0]));
        assert_eq!(
            pixels_in(&Document::new(), &d, &[255, 252, 250, 255]).3,
            vec![255, 252, 250, 255]
        );
    }

    #[test]
    fn a_jpeg_must_have_a_frame_of_the_images_size() {
        // Just the two magic bytes, which no viewer can open.
        assert!(refusal(&jpeg_dict(), &[0xFF, 0xD8]).contains("does not hold jpeg data"));
        // A scan with no frame before it.
        assert!(
            refusal(&jpeg_dict(), &[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]).contains("does not hold jpeg")
        );
        // The dictionary says 4×4; this frame says 1×1.
        let wrong = [0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 0, 1, 0, 1, 1, 1, 0x11, 0];
        assert!(refusal(&jpeg_dict(), &wrong).contains("1×1"));
        // Other markers before the frame are stepped over, with fill bytes.
        let mut with_app = vec![0xFF, 0xD8, 0xFF, 0xFF, 0xE0, 0, 4, 1, 2];
        with_app.extend(&JPEG[2..]);
        assert_eq!(
            export(&Document::new(), &jpeg_dict(), &with_app)
                .unwrap()
                .bytes,
            with_app
        );
    }

    #[test]
    fn a_hostile_predictor_row_is_refused_without_overflow() {
        let mut d = image_dict(1, 1, 8, name("DeviceGray"));
        d.set("Filter", name("FlateDecode"));
        d.set(
            "DecodeParms",
            parms(vec![
                ("Predictor", Object::Integer(12)),
                ("Colors", Object::Integer(1_152_921_504_606_846_976)),
            ]),
        );
        assert!(refusal(&d, &zlib_compress(&[0])).contains("too wide"));
    }

    // ── CCITT ───────────────────────────────────────────────────────────

    /// Packs a string of 0s and 1s (spaces ignored) into bytes.
    fn bits(s: &str) -> Vec<u8> {
        let digits: Vec<u8> = s.bytes().filter(|b| *b != b' ').map(|b| b - b'0').collect();
        digits
            .chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, d)| acc | d << (7 - i))
            })
            .collect()
    }

    fn ccitt_dict(w: i64, h: i64, entries: Vec<(&str, Object)>) -> Dictionary {
        let mut d = image_dict(w, h, 1, name("DeviceGray"));
        d.set("Filter", name("CCITTFaxDecode"));
        let mut all = vec![("Columns", Object::Integer(w))];
        all.extend(entries);
        d.set("DecodeParms", parms(all));
        d
    }

    /// A row of 4 white, 4 black, 8 white: what the vectors below encode.
    fn expected_row() -> Vec<u8> {
        let mut out = vec![255; 4];
        out.extend([0; 4]);
        out.extend([255; 8]);
        out
    }

    #[test]
    fn group_4_decodes() {
        // Horizontal mode, 4 white, 4 black, then vertical to the end.
        let d = ccitt_dict(16, 1, vec![("K", Object::Integer(-1))]);
        let data = bits("001 1011 011 1");
        assert_eq!(pixels(&d, &data), expected_row());
    }

    #[test]
    fn group_3_1d_with_and_without_end_of_line_decodes() {
        let row = "1011 011 10011";
        let d = ccitt_dict(16, 2, vec![]);
        let data = bits(&format!("{row}{row}"));
        assert_eq!(pixels(&d, &data), [expected_row(), expected_row()].concat());

        let eol = "000000000001";
        let d = ccitt_dict(16, 2, vec![("EndOfLine", Object::Boolean(true))]);
        let data = bits(&format!("{eol}{row}{eol}{row}"));
        assert_eq!(pixels(&d, &data), [expected_row(), expected_row()].concat());
    }

    #[test]
    fn group_3_1d_with_rows_on_byte_boundaries_decodes() {
        let d = ccitt_dict(16, 2, vec![("EncodedByteAlign", Object::Boolean(true))]);
        let data = bits("1011 011 10011 0000 1011 011 10011 0000");
        assert_eq!(pixels(&d, &data), [expected_row(), expected_row()].concat());
    }

    #[test]
    fn group_3_2d_decodes() {
        // K > 0: a tag bit before each row, 1 for one-dimensional, 0 for two.  The second row's
        // `1` is vertical mode, V0, repeating the first row's changes.
        let d = ccitt_dict(16, 2, vec![("K", Object::Integer(4))]);
        let data = bits("1 1011 011 10011 0 1 1 1");
        assert_eq!(pixels(&d, &data), [expected_row(), expected_row()].concat());
    }

    #[test]
    fn black_is_1_and_decode_are_both_honoured() {
        let data = bits("001 1011 011 1");
        let flipped = ccitt_dict(
            16,
            1,
            vec![
                ("K", Object::Integer(-1)),
                ("BlackIs1", Object::Boolean(true)),
            ],
        );
        // With BlackIs1 the black run comes out as 1s, which DeviceGray shows as white.
        let inverted: Vec<u8> = expected_row().iter().map(|v| 255 - v).collect();
        assert_eq!(pixels(&flipped, &data), inverted);

        let mut decoded = ccitt_dict(16, 1, vec![("K", Object::Integer(-1))]);
        decoded.set("Decode", numbers(&[1.0, 0.0]));
        assert_eq!(pixels(&decoded, &data), inverted);
    }

    #[test]
    fn a_fax_stencil_works() {
        let mut d = ccitt_dict(16, 1, vec![("K", Object::Integer(-1))]);
        d.remove(b"ColorSpace");
        d.remove(b"BitsPerComponent");
        d.set("ImageMask", true);
        // Black samples are 0, which is what a stencil paints by default: black again.
        assert_eq!(pixels(&d, &bits("001 1011 011 1")), expected_row());
    }

    #[test]
    fn too_few_rows_and_a_columns_mismatch_are_refused() {
        let d = ccitt_dict(16, 5, vec![("K", Object::Integer(-1))]);
        let short = bits(&format!("001 1011 011 1 {}", "000000000001".repeat(2)));
        assert!(refusal(&d, &short).contains("rows"));

        let mut d = image_dict(16, 1, 1, name("DeviceGray"));
        d.set("Filter", name("CCITTFaxDecode"));
        d.set("DecodeParms", parms(vec![("Columns", Object::Integer(8))]));
        assert!(refusal(&d, &[0]).contains("/Columns of 8"));

        // No /Columns means 1728, the default, which this image is not.
        let mut d = image_dict(16, 1, 1, name("DeviceGray"));
        d.set("Filter", name("CCITTFaxDecode"));
        assert!(refusal(&d, &[0]).contains("/Columns of 1728"));
    }

    // ── JBIG2 ───────────────────────────────────────────────────────────

    /// One embedded page, 8×2: a page-information segment and an immediate generic region coded
    /// with MMR, both of whose rows are 4 white, then 4 black.
    fn jbig2_page() -> Vec<u8> {
        let be = |v: u32| v.to_be_bytes();
        let mut o = vec![];
        o.extend(be(0));
        o.extend([48, 0, 1]);
        o.extend(be(19));
        o.extend(be(8));
        o.extend(be(2));
        o.extend(be(0));
        o.extend(be(0));
        o.extend([0, 0, 0]);
        let mmr = bits("001 1011 011 1 111");
        o.extend(be(1));
        o.extend([38, 0, 1]);
        o.extend(be(18 + mmr.len() as u32));
        o.extend(be(8));
        o.extend(be(2));
        o.extend(be(0));
        o.extend(be(0));
        o.extend([0, 1]);
        o.extend(&mmr);
        o
    }

    fn jbig2_dict(w: i64, h: i64) -> Dictionary {
        let mut d = image_dict(w, h, 1, name("DeviceGray"));
        d.set("Filter", name("JBIG2Decode"));
        d
    }

    #[test]
    fn jbig2_decodes_with_black_as_0() {
        assert_eq!(
            pixels(&jbig2_dict(8, 2), &jbig2_page()),
            vec![
                255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn a_jbig2_page_of_another_size_is_refused() {
        let why = refusal(&jbig2_dict(9, 2), &jbig2_page());
        assert!(why.contains("8×2") && why.contains("9×2"), "{why}");
    }

    #[test]
    fn jbig2_garbage_is_refused() {
        assert!(refusal(&jbig2_dict(8, 2), b"garbage").contains("JBIG2Decode"));
    }

    #[test]
    fn unreadable_jbig2_globals_are_refused_never_taken_for_none() {
        let mut d = jbig2_dict(8, 2);
        d.set(
            "DecodeParms",
            parms(vec![("JBIG2Globals", Object::Reference((9, 0)))]),
        );
        // Not a stream at all: a reference to nothing.
        assert!(refusal(&d, &jbig2_page()).contains("/JBIG2Globals"));
        // A stream that will not decode.
        let mut broken = Dictionary::new();
        broken.set("Filter", name("FlateDecode"));
        let doc = doc_with(9, broken, b"nope");
        assert!(
            export(&doc, &d, &jbig2_page())
                .err()
                .unwrap()
                .contains("/JBIG2Globals")
        );
        // A stream that decodes to garbage.
        let doc = doc_with(9, Dictionary::new(), b"garbage garbage garbage");
        assert!(
            export(&doc, &d, &jbig2_page())
                .err()
                .unwrap()
                .contains("/JBIG2Globals")
        );
    }

    #[test]
    fn empty_jbig2_globals_decode() {
        let doc = doc_with(9, Dictionary::new(), b"");
        let mut d = jbig2_dict(8, 2);
        d.set(
            "DecodeParms",
            parms(vec![("JBIG2Globals", Object::Reference((9, 0)))]),
        );
        assert_eq!(pixels_in(&doc, &d, &jbig2_page()).0, 8);
    }

    // ── Passed through ──────────────────────────────────────────────────

    fn jpeg_dict() -> Dictionary {
        let mut d = image_dict(4, 4, 8, name("DeviceRGB"));
        d.set("Filter", name("DCTDecode"));
        d
    }

    /// SOI, then a baseline frame of 4×4; enough for what the export checks.
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 0, 4, 0, 4, 1, 1, 0x11, 0];

    #[test]
    fn a_jpeg_is_written_as_it_is() {
        let image = export(&Document::new(), &jpeg_dict(), JPEG).unwrap();
        assert_eq!(image.format, ExportFormat::Jpeg);
        assert_eq!(image.bytes, JPEG);
        assert_eq!((image.width, image.height), (4, 4));
        assert_eq!(image.mask_applied, true);
    }

    #[test]
    fn a_jpeg_under_another_filter_has_that_filter_undone() {
        let mut d = jpeg_dict();
        d.set(
            "Filter",
            Object::Array(vec![name("FlateDecode"), name("DCTDecode")]),
        );
        let image = export(&Document::new(), &d, &zlib_compress(JPEG)).unwrap();
        assert_eq!(image.bytes, JPEG);
        assert_eq!(image.filter, "FlateDecode, DCTDecode");
    }

    #[test]
    fn a_jp2_and_a_raw_codestream_are_written_as_they_are() {
        let mut d = image_dict(1, 1, 8, name("DeviceRGB"));
        d.set("Filter", name("JPXDecode"));
        let mut jp2 = vec![
            0, 0, 0, 0x0C, 0x6A, 0x50, 0x20, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ];
        jp2.extend([0, 0, 0, 8]);
        jp2.extend(b"jp2c");
        let image = export(&Document::new(), &d, &jp2).unwrap();
        assert_eq!(
            (image.format, image.bytes),
            (ExportFormat::Jp2, jp2.clone())
        );
        // The signature alone, with no codestream, is not an image.
        assert!(refusal(&d, &jp2[..12]).contains("does not hold jp2 data"));
        let codestream = [0xFF, 0x4F, 0xFF, 0x51];
        assert_eq!(
            export(&Document::new(), &d, &codestream).unwrap().bytes,
            codestream
        );
    }

    #[test]
    fn a_passed_through_stream_that_is_not_what_it_claims_is_refused() {
        assert!(refusal(&jpeg_dict(), b"not a jpeg").contains("does not hold jpeg data"));
        let mut d = image_dict(1, 1, 8, name("DeviceRGB"));
        d.set("Filter", name("JPXDecode"));
        assert!(refusal(&d, b"not a jp2").contains("does not hold jp2 data"));
    }

    #[test]
    fn a_decode_on_a_jpeg_is_refused_unless_it_changes_nothing() {
        let mut d = jpeg_dict();
        d.set("Decode", numbers(&[1.0, 0.0, 1.0, 0.0, 1.0, 0.0]));
        assert!(refusal(&d, JPEG).contains("/Decode"));
        let mut d = jpeg_dict();
        d.set("Decode", numbers(&[0.0, 1.0, 0.0, 1.0, 0.0, 1.0]));
        assert_eq!(export(&Document::new(), &d, JPEG).unwrap().bytes, JPEG);
    }

    #[test]
    fn a_jpegs_mask_is_reported_as_not_applied() {
        let doc = doc_with(9, image_dict(1, 1, 8, name("DeviceGray")), &[0]);
        let mut d = jpeg_dict();
        d.set("SMask", Object::Reference((9, 0)));
        let image = export(&doc, &d, JPEG).unwrap();
        assert_eq!((image.mask, image.mask_applied), ("smask", false));
    }

    // ── Inline dictionaries and formats ─────────────────────────────────

    #[test]
    fn inline_keys_and_names_may_be_abbreviated() {
        let mut d = Dictionary::new();
        d.set("W", 2);
        d.set("H", 1);
        d.set("BPC", 8);
        d.set("CS", name("G"));
        d.set("F", name("AHx"));
        d.set("D", numbers(&[1.0, 0.0]));
        let doc = Document::new();
        let image = export_image(
            &doc,
            &ImageSource {
                dict: &d,
                data: b"00FF>",
                resources: None,
                inline: true,
            },
        )
        .unwrap();
        assert_eq!(image.filter, "ASCIIHexDecode");
        let decoder = png::Decoder::new(std::io::Cursor::new(image.bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buffer).unwrap();
        assert_eq!(buffer, vec![255, 0]);
    }

    #[test]
    fn export_format_goes_by_the_last_filter_and_the_extension_by_the_path() {
        let doc = Document::new();
        for (dict, format) in [
            (jpeg_dict(), ExportFormat::Jpeg),
            (jbig2_dict(1, 1), ExportFormat::Png),
            (image_dict(1, 1, 8, name("G")), ExportFormat::Png),
        ] {
            let source = ImageSource {
                dict: &dict,
                data: &[],
                resources: None,
                inline: false,
            };
            assert_eq!(export_format(&doc, &source), Some(format));
        }
        for (path, format) in [
            ("a.PNG", Some(ExportFormat::Png)),
            ("a.jpg", Some(ExportFormat::Jpeg)),
            ("a.JPEG", Some(ExportFormat::Jpeg)),
            ("a.jp2", Some(ExportFormat::Jp2)),
            ("a.tiff", None),
            ("a", None),
        ] {
            assert_eq!(ExportFormat::from_path(Path::new(path)), format, "{path}");
        }
    }
}
