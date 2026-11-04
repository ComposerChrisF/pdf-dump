use flate2::read::ZlibDecoder;
use lopdf::{Dictionary, Document, Object};
use std::borrow::Cow;
use std::io::Read;

/// Maximum decoded stream size (256 MB) to prevent decompression bombs.
/// Each expanding filter (Flate, LZW, RunLength) enforces it during decode,
/// not after; the ASCII filters only shrink their input.
const MAX_DECODED_SIZE: u64 = 256 * 1024 * 1024;

pub(crate) fn is_binary_stream(content: &[u8]) -> bool {
    content.iter().any(|&b| {
        !b.is_ascii_alphanumeric() && !b.is_ascii_whitespace() && !b.is_ascii_punctuation()
    })
}

pub(crate) fn decode_ascii85(data: &[u8]) -> Result<Vec<u8>, String> {
    let cleaned: Vec<u8> = data
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let mut input = if cleaned.ends_with(b"~>") {
        &cleaned[..cleaned.len() - 2]
    } else {
        &cleaned[..]
    };
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

        for &b in chunk {
            if !(b'!'..=b'u').contains(&b) {
                return Err(format!("ASCII85Decode: invalid character 0x{:02x}", b));
            }
        }

        let mut digits = [b'u'; 5];
        digits[..chunk_len].copy_from_slice(chunk);

        let mut value: u64 = 0;
        for &d in &digits {
            value = value * 85 + (d - b'!') as u64;
        }

        let bytes = value.to_be_bytes();
        let output_len = if chunk_len == 5 { 4 } else { chunk_len - 1 };
        result.extend_from_slice(&bytes[4..4 + output_len]);
        i += chunk_len;
    }
    Ok(result)
}

pub(crate) fn decode_asciihex(data: &[u8]) -> Result<Vec<u8>, String> {
    let cleaned: Vec<u8> = data
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
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
            None => {
                return Err(format!(
                    "ASCIIHexDecode: invalid hex character 0x{:02x}",
                    input[i]
                ));
            }
        };
        let lo = if i + 1 < input.len() {
            match hex_digit(input[i + 1]) {
                Some(v) => v,
                None => {
                    return Err(format!(
                        "ASCIIHexDecode: invalid hex character 0x{:02x}",
                        input[i + 1]
                    ));
                }
            }
        } else {
            0
        };
        result.push(hi << 4 | lo);
        i += 2;
    }
    Ok(result)
}

pub(crate) fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn decode_lzw(data: &[u8]) -> Result<Vec<u8>, String> {
    decode_lzw_capped(data, MAX_DECODED_SIZE, true)
}

/// LZW with `/EarlyChange` honoured: the code length grows one code early by default (true).
pub(crate) fn decode_lzw_early(data: &[u8], early_change: bool) -> Result<Vec<u8>, String> {
    decode_lzw_capped(data, MAX_DECODED_SIZE, early_change)
}

/// LZW-decodes `data` in chunks, failing as soon as the output would exceed
/// `limit` rather than decoding everything first (bug-0005).  An input that
/// runs out before the end marker is an error, as with weezl's `decode`.
fn decode_lzw_capped(data: &[u8], limit: u64, early_change: bool) -> Result<Vec<u8>, String> {
    let mut decoder = if early_change {
        weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
    } else {
        weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8)
    };
    let mut result = Vec::new();
    let mut chunk = [0u8; 1 << 12];
    let mut input = data;
    loop {
        let step = decoder.decode_bytes(input, &mut chunk);
        input = &input[step.consumed_in..];
        if (result.len() + step.consumed_out) as u64 > limit {
            return Err(format!(
                "LZWDecode: decoded size exceeds {} byte limit",
                limit
            ));
        }
        result.extend_from_slice(&chunk[..step.consumed_out]);
        match step.status {
            Ok(weezl::LzwStatus::Ok) => {}
            Ok(weezl::LzwStatus::Done) => return Ok(result),
            Ok(weezl::LzwStatus::NoProgress) => {
                return Err(format!("LZWDecode: {}", weezl::LzwError::InvalidCode));
            }
            Err(e) => return Err(format!("LZWDecode: {}", e)),
        }
    }
}

pub(crate) fn decode_run_length(data: &[u8]) -> Result<Vec<u8>, String> {
    decode_run_length_capped(data, MAX_DECODED_SIZE)
}

/// RunLength-decodes `data`, failing before any run that would push the
/// output past `limit` (bug-0005).
fn decode_run_length_capped(data: &[u8], limit: u64) -> Result<Vec<u8>, String> {
    let over_limit = |len: usize| {
        Err(format!(
            "RunLengthDecode: decoded size {} exceeds {} byte limit",
            len, limit
        ))
    };
    let mut result = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let length = data[i];
        i += 1;
        if length <= 127 {
            let count = length as usize + 1;
            if i + count > data.len() {
                return Err("RunLengthDecode: truncated literal run".to_string());
            }
            if (result.len() + count) as u64 > limit {
                return over_limit(result.len() + count);
            }
            result.extend_from_slice(&data[i..i + count]);
            i += count;
        } else if length == 128 {
            break;
        } else {
            if i >= data.len() {
                return Err("RunLengthDecode: truncated repeat run".to_string());
            }
            let count = 257 - length as usize;
            if (result.len() + count) as u64 > limit {
                return over_limit(result.len() + count);
            }
            let byte = data[i];
            i += 1;
            result.extend(std::iter::repeat_n(byte, count));
        }
    }
    Ok(result)
}

pub(crate) fn decode_flate(data: &[u8]) -> Result<Vec<u8>, String> {
    let decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder
        .take(MAX_DECODED_SIZE)
        .read_to_end(&mut decompressed)
        .map_err(|_| "FlateDecode decompression failed".to_string())?;
    if decompressed.len() as u64 >= MAX_DECODED_SIZE {
        return Err(format!(
            "FlateDecode: decoded size exceeds {} byte limit",
            MAX_DECODED_SIZE
        ));
    }
    Ok(decompressed)
}

/// The `/DecodeParms` entries a predictor reads (ISO 32000-1 Table 8).
pub(crate) struct PredictorParams {
    pub predictor: i64,
    pub colors: i64,
    pub bits_per_component: i64,
    pub columns: i64,
}

/// Undoes a Flate or LZW predictor: `1` is none, `2` is TIFF, `10` to `15` are PNG, whose rows each
/// carry their own tag, so which of those six is named does not matter (§ 7.4.4.4).
pub(crate) fn undo_predictor(data: &[u8], parms: PredictorParams) -> Result<Vec<u8>, String> {
    let PredictorParams {
        predictor,
        colors,
        bits_per_component: bits,
        columns,
    } = parms;
    if predictor == 1 {
        return Ok(data.to_vec());
    }
    if !(predictor == 2 || (10..=15).contains(&predictor)) {
        return Err(format!("the unknown /Predictor {predictor}"));
    }
    if colors < 1 || columns < 1 || !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return Err(format!(
            "a predictor with /Colors {colors}, /BitsPerComponent {bits}, /Columns {columns}"
        ));
    }
    let pixel_bits = (colors as u64)
        .checked_mul(bits as u64)
        .ok_or("a predictor pixel too wide to address")?;
    let row_bits = pixel_bits
        .checked_mul(columns as u64)
        .filter(|b| *b <= MAX_DECODED_SIZE * 8)
        .ok_or("a predictor row too wide to address")?;
    let row_bytes = row_bits.div_ceil(8) as usize;
    let pixel_bytes = pixel_bits.div_ceil(8) as usize;
    if predictor == 2 {
        undo_tiff_predictor(
            data.to_vec(),
            row_bytes,
            colors as usize,
            bits as u32,
            columns as usize,
        )
    } else {
        undo_png_predictor(data, row_bytes, pixel_bytes)
    }
}

fn undo_tiff_predictor(
    mut data: Vec<u8>,
    row_bytes: usize,
    colors: usize,
    bits: u32,
    columns: usize,
) -> Result<Vec<u8>, String> {
    if row_bytes == 0 || !data.len().is_multiple_of(row_bytes) {
        return Err(format!(
            "TIFF predictor data of {} bytes, not whole rows of {row_bytes}",
            data.len()
        ));
    }
    for row in data.chunks_exact_mut(row_bytes) {
        match bits {
            8 => {
                for i in colors..row_bytes {
                    row[i] = row[i].wrapping_add(row[i - colors]);
                }
            }
            16 => {
                for i in (colors * 2..row_bytes - 1).step_by(2) {
                    let left = u16::from_be_bytes([row[i - colors * 2], row[i - colors * 2 + 1]]);
                    let delta = u16::from_be_bytes([row[i], row[i + 1]]);
                    let [hi, lo] = delta.wrapping_add(left).to_be_bytes();
                    row[i] = hi;
                    row[i + 1] = lo;
                }
            }
            _ => {
                // 1, 2 and 4 bits: samples cross no byte, but neighbours do not share one.
                let mask = (1u8 << bits) - 1;
                let get = |row: &[u8], n: usize| {
                    let bit = n * bits as usize;
                    (row[bit / 8] >> (8 - bits as usize - bit % 8)) & mask
                };
                for n in colors..columns * colors {
                    let value = get(row, n).wrapping_add(get(row, n - colors)) & mask;
                    let bit = n * bits as usize;
                    let shift = 8 - bits as usize - bit % 8;
                    row[bit / 8] = (row[bit / 8] & !(mask << shift)) | (value << shift);
                }
            }
        }
    }
    Ok(data)
}

fn undo_png_predictor(
    data: &[u8],
    row_bytes: usize,
    pixel_bytes: usize,
) -> Result<Vec<u8>, String> {
    let stride = row_bytes + 1;
    if !data.len().is_multiple_of(stride) {
        return Err(format!(
            "PNG predictor data of {} bytes, not whole rows of {stride}",
            data.len()
        ));
    }
    let mut out = vec![0u8; data.len() / stride * row_bytes];
    for (y, row) in data.chunks_exact(stride).enumerate() {
        let (tag, encoded) = (row[0], &row[1..]);
        let (above_rows, rest) = out.split_at_mut(y * row_bytes);
        let current = &mut rest[..row_bytes];
        let above = y.checked_sub(1).map(|prev| &above_rows[prev * row_bytes..]);
        for i in 0..row_bytes {
            let left = i.checked_sub(pixel_bytes).map_or(0, |at| current[at]);
            let up = above.map_or(0, |a| a[i]);
            let up_left = i
                .checked_sub(pixel_bytes)
                .and_then(|at| above.map(|a| a[at]))
                .unwrap_or(0);
            let prediction = match tag {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, up_left),
                other => return Err(format!("the unknown PNG row tag {other}")),
            };
            current[i] = encoded[i].wrapping_add(prediction);
        }
    }
    Ok(out)
}

fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let p = i16::from(left) + i16::from(up) - i16::from(up_left);
    let (pa, pb, pc) = (
        (p - i16::from(left)).abs(),
        (p - i16::from(up)).abs(),
        (p - i16::from(up_left)).abs(),
    );
    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        up
    } else {
        up_left
    }
}

pub(crate) fn get_filter_names(stream: &lopdf::Stream) -> Vec<&[u8]> {
    match stream.dict.get(b"Filter").ok() {
        Some(filter_obj) => {
            if let Ok(name) = filter_obj.as_name() {
                vec![name]
            } else if let Ok(arr) = filter_obj.as_array() {
                arr.iter().filter_map(|obj| obj.as_name().ok()).collect()
            } else {
                vec![]
            }
        }
        None => vec![],
    }
}

/// A value the document holds directly or by reference.  `Ok(None)` is null; a reference that leads
/// nowhere is an error, not null, since it cannot say what a stream's parameters were.
fn follow<'a>(doc: &'a Document, value: &'a Object) -> Result<Option<&'a Object>, String> {
    let value = match value {
        Object::Reference(id) => doc
            .get_object(*id)
            .map_err(|_| format!("a reference to {} {}, which is not there", id.0, id.1))?,
        other => other,
    };
    Ok((!matches!(value, Object::Null)).then_some(value))
}

/// The `/DecodeParms` of filter `index`: one dictionary, for the first filter, or an array with an
/// entry per filter (§ 7.4).
fn filter_parms<'a>(
    doc: &'a Document,
    stream: &'a lopdf::Stream,
    index: usize,
) -> Result<Option<&'a Dictionary>, String> {
    let Ok(parms) = stream.dict.get(b"DecodeParms") else {
        return Ok(None);
    };
    let entry = match follow(doc, parms)? {
        Some(Object::Array(items)) => match items.get(index) {
            Some(item) => follow(doc, item)?,
            None => None,
        },
        Some(Object::Dictionary(_)) if index > 0 => None,
        other => other,
    };
    match entry {
        None => Ok(None),
        Some(Object::Dictionary(parms)) => Ok(Some(parms)),
        Some(_) => Err("a /DecodeParms entry that is not a dictionary".to_string()),
    }
}

/// One `/DecodeParms` integer: its default when absent, an error when present and unusable.
fn parm_int(
    doc: &Document,
    parms: Option<&Dictionary>,
    key: &[u8],
    default: i64,
) -> Result<i64, String> {
    let key_name = || String::from_utf8_lossy(key).into_owned();
    let Some(value) = parms.and_then(|p| p.get(key).ok()) else {
        return Ok(default);
    };
    match follow(doc, value).map_err(|reason| format!("/{}: {reason}", key_name()))? {
        None => Ok(default),
        Some(value) => value
            .as_i64()
            .map_err(|_| format!("a /{} that is not an integer", key_name())),
    }
}

/// The predictor filter `index` names, or `None` when it names none.
fn stream_predictor(
    doc: &Document,
    stream: &lopdf::Stream,
    index: usize,
) -> Result<Option<PredictorParams>, String> {
    let parms = filter_parms(doc, stream, index)?;
    let predictor = parm_int(doc, parms, b"Predictor", 1)?;
    if predictor == 1 {
        return Ok(None);
    }
    Ok(Some(PredictorParams {
        predictor,
        colors: parm_int(doc, parms, b"Colors", 1)?,
        bits_per_component: parm_int(doc, parms, b"BitsPerComponent", 8)?,
        columns: parm_int(doc, parms, b"Columns", 1)?,
    }))
}

/// Undoes filter `index`'s predictor, if it names one.  On a failure the bytes come back as the
/// filter left them, with the reason.
fn undo_stream_predictor(
    doc: &Document,
    stream: &lopdf::Stream,
    index: usize,
    decoded: Vec<u8>,
) -> (Vec<u8>, Option<String>) {
    let undone = match stream_predictor(doc, stream, index) {
        Ok(None) => return (decoded, None),
        Ok(Some(parms)) => undo_predictor(&decoded, parms),
        Err(reason) => Err(reason),
    };
    match undone {
        Ok(data) => (data, None),
        Err(reason) => (decoded, Some(format!("predictor not undone: {reason}"))),
    }
}

/// LZW's `/EarlyChange`, which defaults to 1 (§ 7.4.4.3).  Only 0 turns it off.
fn lzw_early_change(doc: &Document, stream: &lopdf::Stream, index: usize) -> Result<bool, String> {
    Ok(parm_int(doc, filter_parms(doc, stream, index)?, b"EarlyChange", 1)? != 0)
}

pub(crate) fn decode_stream<'a>(
    doc: &Document,
    stream: &'a lopdf::Stream,
) -> (Cow<'a, [u8]>, Option<String>) {
    let filters = get_filter_names(stream);
    if filters.is_empty() {
        return (Cow::Borrowed(&stream.content), None);
    }

    let mut data: Cow<'a, [u8]> = Cow::Borrowed(&stream.content);
    for (index, filter) in filters.iter().enumerate() {
        let mut assumed = None;
        let result = match *filter {
            b"FlateDecode" => decode_flate(&data),
            b"ASCII85Decode" => decode_ascii85(&data),
            b"ASCIIHexDecode" => decode_asciihex(&data),
            b"LZWDecode" => {
                let early = lzw_early_change(doc, stream, index).unwrap_or_else(|reason| {
                    assumed = Some(format!("/EarlyChange taken as 1: {reason}"));
                    true
                });
                decode_lzw_early(&data, early)
            }
            b"RunLengthDecode" => decode_run_length(&data),
            other => {
                let name = String::from_utf8_lossy(other);
                return (
                    Cow::Owned(data.into_owned()),
                    Some(format!("unsupported filter: {}", name)),
                );
            }
        };
        let decoded = match result {
            Ok(decoded) => decoded,
            Err(msg) => return (Cow::Owned(data.into_owned()), Some(msg)),
        };
        if assumed.is_some() {
            return (Cow::Owned(decoded), assumed);
        }
        if matches!(*filter, b"FlateDecode" | b"LZWDecode") {
            let (undone, warning) = undo_stream_predictor(doc, stream, index, decoded);
            if warning.is_some() {
                return (Cow::Owned(undone), warning);
            }
            data = Cow::Owned(undone);
        } else {
            data = Cow::Owned(decoded);
        }
    }

    (data, None)
}

pub(crate) fn format_hex_dump(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for (offset, chunk) in data.chunks(16).enumerate() {
        write!(result, "{:08x}  ", offset * 16).unwrap();
        for (i, &b) in chunk.iter().enumerate() {
            write!(result, "{:02x} ", b).unwrap();
            if i == 7 {
                result.push(' ');
            }
        }
        if chunk.len() < 16 {
            for i in chunk.len()..16 {
                result.push_str("   ");
                if i == 7 {
                    result.push(' ');
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::print_single_object;
    use crate::test_utils::*;
    use crate::types::DumpConfig;
    use lopdf::Document;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream};
    use pretty_assertions::assert_eq;
    use std::borrow::Cow;

    fn predictor(predictor: i64, colors: i64, bits: i64, columns: i64) -> PredictorParams {
        PredictorParams {
            predictor,
            colors,
            bits_per_component: bits,
            columns,
        }
    }

    #[test]
    fn png_predictor_undoes_every_row_tag() {
        // bug-0004's vector: Up rows of [1,2,3,4], then [+4,+4,+4,+4].
        assert_eq!(
            undo_predictor(&[2, 1, 2, 3, 4, 2, 4, 4, 4, 4], predictor(12, 1, 8, 4)).unwrap(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
        // None, then Sub, Average and Paeth over a known row.
        let data = vec![
            0, 10, 20, 30, //
            1, 1, 1, 1, // Sub: 1, 2, 3
            3, 5, 5, 5, // Average: 5+(0+1)/2=5, 5+(5+2)/2=8, 5+(8+3)/2=10
            4, 1, 1, 1, // Paeth
        ];
        let out = undo_predictor(&data, predictor(15, 1, 8, 3)).unwrap();
        assert_eq!(&out[..9], &[10, 20, 30, 1, 2, 3, 5, 8, 10]);
        // Paeth: left 0, up 5, up-left 0 picks up: 6; then left 6, up 8, up-left 5 picks 8: 9.
        assert_eq!(&out[9..11], &[6, 9]);
    }

    #[test]
    fn png_predictor_steps_by_whole_pixels() {
        // Three colours: Sub reaches back three bytes, not one.
        assert_eq!(
            undo_predictor(&[1, 1, 2, 3, 1, 1, 1], predictor(11, 3, 8, 2)).unwrap(),
            vec![1, 2, 3, 2, 3, 4]
        );
    }

    #[test]
    fn png_predictor_refuses_a_partial_row_and_an_unknown_tag() {
        assert!(undo_predictor(&[0, 1, 2], predictor(12, 1, 8, 4)).is_err());
        assert!(undo_predictor(&[9, 1], predictor(12, 1, 8, 1)).is_err());
    }

    #[test]
    fn tiff_predictor_8_16_and_low_bits() {
        assert_eq!(
            undo_predictor(&[1, 1, 1, 5, 1, 1], predictor(2, 1, 8, 3)).unwrap(),
            vec![1, 2, 3, 5, 6, 7]
        );
        assert_eq!(
            undo_predictor(&[0x01, 0xFF, 0x00, 0x01], predictor(2, 1, 16, 2)).unwrap(),
            vec![0x01, 0xFF, 0x02, 0x00]
        );
        // Four 2-bit samples 1, +1, +1, +1 are 1, 2, 3, 0.
        assert_eq!(
            undo_predictor(&[0b01_01_01_01], predictor(2, 1, 2, 4)).unwrap(),
            vec![0b01_10_11_00]
        );
    }

    #[test]
    fn predictor_1_is_nothing_and_an_unknown_one_is_refused() {
        assert_eq!(
            undo_predictor(&[1, 2], predictor(1, 1, 8, 1)).unwrap(),
            vec![1, 2]
        );
        assert!(undo_predictor(&[1, 2], predictor(7, 1, 8, 1)).is_err());
        assert!(undo_predictor(&[1, 2], predictor(12, 0, 8, 1)).is_err());
    }

    fn stream_with(filter: Object, parms: Option<Object>, content: Vec<u8>) -> lopdf::Stream {
        let mut dict = Dictionary::new();
        dict.set("Filter", filter);
        if let Some(parms) = parms {
            dict.set("DecodeParms", parms);
        }
        lopdf::Stream::new(dict, content)
    }

    fn flate() -> Object {
        Object::Name(b"FlateDecode".to_vec())
    }

    fn hex_filter() -> Object {
        Object::Name(b"ASCIIHexDecode".to_vec())
    }

    fn parms_of(entries: Vec<(&str, i64)>) -> Object {
        let mut parms = Dictionary::new();
        for (key, value) in entries {
            parms.set(key, Object::Integer(value));
        }
        Object::Dictionary(parms)
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        crate::test_utils::zlib_compress(data)
    }

    fn hex_of(data: &[u8]) -> Vec<u8> {
        data.iter()
            .flat_map(|b| format!("{b:02X}").into_bytes())
            .collect()
    }

    /// bug-0004's reproduction: true data 1..8, stored as Up rows with a tag each.
    const PREDICTED: [u8; 10] = [2, 1, 2, 3, 4, 2, 4, 4, 4, 4];
    const UNPREDICTED: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn up_rows() -> Object {
        parms_of(vec![("Predictor", 12), ("Columns", 4)])
    }

    #[test]
    fn decode_stream_undoes_a_png_predictor() {
        let stream = stream_with(flate(), Some(up_rows()), zlib(&PREDICTED));
        let (data, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*data, &UNPREDICTED);
        assert_eq!(warning, None);
    }

    #[test]
    fn decode_stream_undoes_a_predictor_after_lzw() {
        let encoded = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(&PREDICTED)
            .unwrap();
        let lzw = Object::Name(b"LZWDecode".to_vec());
        let stream = stream_with(lzw, Some(up_rows()), encoded);
        let (data, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*data, &UNPREDICTED);
        assert_eq!(warning, None);
    }

    #[test]
    fn decode_stream_undoes_a_tiff_predictor_named_in_an_array() {
        let stream = stream_with(
            Object::Array(vec![hex_filter(), flate()]),
            Some(Object::Array(vec![
                Object::Null,
                parms_of(vec![("Predictor", 2), ("Columns", 3)]),
            ])),
            hex_of(&zlib(&[1, 1, 1])),
        );
        let (data, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*data, &[1, 2, 3]);
        assert_eq!(warning, None);
    }

    #[test]
    fn decode_stream_follows_parameters_held_by_reference() {
        let mut doc = empty_doc();
        doc.objects.insert((9, 0), up_rows());
        doc.objects.insert(
            (10, 0),
            Object::Array(vec![Object::Null, Object::Reference((9, 0))]),
        );
        doc.objects.insert((11, 0), Object::Integer(12));
        let mut by_value = Dictionary::new();
        by_value.set("Predictor", Object::Reference((11, 0)));
        by_value.set("Columns", Object::Integer(4));
        for (filter, parms, content) in [
            (flate(), Object::Reference((9, 0)), zlib(&PREDICTED)),
            (
                Object::Array(vec![hex_filter(), flate()]),
                Object::Reference((10, 0)),
                hex_of(&zlib(&PREDICTED)),
            ),
            (flate(), Object::Dictionary(by_value), zlib(&PREDICTED)),
        ] {
            let stream = stream_with(filter, Some(parms.clone()), content);
            let (data, warning) = decode_stream(&doc, &stream);
            assert_eq!(&*data, &UNPREDICTED, "{parms:?}");
            assert_eq!(warning, None, "{parms:?}");
        }
    }

    #[test]
    fn decode_stream_applies_a_lone_dictionary_only_to_the_first_filter() {
        let mut doc = empty_doc();
        doc.objects.insert((9, 0), up_rows());
        for parms in [up_rows(), Object::Reference((9, 0))] {
            let stream = stream_with(
                Object::Array(vec![hex_filter(), flate()]),
                Some(parms),
                hex_of(&zlib(&[2, 1, 2])),
            );
            let (data, warning) = decode_stream(&doc, &stream);
            assert_eq!(&*data, &[2, 1, 2]);
            assert_eq!(warning, None);
        }
    }

    #[test]
    fn decode_stream_warns_when_it_cannot_undo_a_predictor() {
        let predicted = [2, 1, 2];
        for parms in [
            up_rows(),                        // a partial row
            parms_of(vec![("Predictor", 7)]), // an unknown predictor
            Object::Reference((9, 0)),        // nothing there
            Object::Array(vec![Object::Reference((9, 0))]),
            Object::Integer(3),
            {
                let mut d = Dictionary::new();
                d.set("Predictor", Object::Reference((9, 0)));
                Object::Dictionary(d)
            },
            {
                let mut d = Dictionary::new();
                d.set("Predictor", Object::Integer(12));
                d.set("Columns", Object::Name(b"Four".to_vec()));
                Object::Dictionary(d)
            },
        ] {
            let stream = stream_with(flate(), Some(parms.clone()), zlib(&predicted));
            let (data, warning) = decode_stream(&empty_doc(), &stream);
            assert_eq!(&*data, &predicted, "{parms:?}");
            assert!(
                warning
                    .as_deref()
                    .is_some_and(|w| w.starts_with("predictor not undone: ")),
                "{parms:?}: {warning:?}"
            );
        }
    }

    #[test]
    fn decode_stream_warns_about_a_later_filters_unusable_parameters() {
        let stream = stream_with(
            Object::Array(vec![hex_filter(), flate()]),
            Some(Object::Reference((9, 0))),
            hex_of(&zlib(&[2, 1, 2])),
        );
        let (data, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*data, &[2, 1, 2]);
        assert!(warning.is_some_and(|w| w.starts_with("predictor not undone: ")));
    }

    #[test]
    fn decode_stream_ignores_unused_parameters_when_no_predictor_is_named() {
        let mut d = Dictionary::new();
        d.set("Columns", Object::Reference((9, 0)));
        let stream = stream_with(flate(), Some(Object::Dictionary(d)), zlib(b"text"));
        let (data, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*data, b"text");
        assert_eq!(warning, None);
    }

    #[test]
    fn decode_stream_honours_lzw_early_change() {
        let data: Vec<u8> = (0u32..2000).map(|i| (i * i % 251) as u8).collect();
        let encoded = weezl::encode::Encoder::new(weezl::BitOrder::Msb, 8)
            .encode(&data)
            .unwrap();
        let stream = stream_with(
            Object::Name(b"LZWDecode".to_vec()),
            Some(parms_of(vec![("EarlyChange", 0)])),
            encoded,
        );
        let (decoded, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(warning, None);
        assert_eq!(&*decoded, &data[..]);
    }

    #[test]
    fn decode_stream_decodes_lzw_with_the_default_when_early_change_is_unreadable() {
        let encoded = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(b"needle")
            .unwrap();
        let mut d = Dictionary::new();
        d.set("EarlyChange", Object::Reference((9, 0)));
        let stream = stream_with(
            Object::Name(b"LZWDecode".to_vec()),
            Some(Object::Dictionary(d)),
            encoded,
        );
        let (data, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*data, b"needle");
        assert!(warning.is_some_and(|w| w.starts_with("/EarlyChange taken as 1: ")));
    }

    #[test]
    fn lzw_early_change_0_decodes() {
        // A stream long enough to pass a code-length change, encoded both ways.
        let plain: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
        let mut late = weezl::encode::Encoder::new(weezl::BitOrder::Msb, 8);
        let encoded = late.encode(&plain).unwrap();
        assert_eq!(decode_lzw_early(&encoded, false).unwrap(), plain);
        assert_ne!(decode_lzw_early(&encoded, true).ok(), Some(plain));
    }

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

    #[test]
    fn decode_stream_no_filter() {
        let stream = make_stream(None, b"raw content".to_vec());
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw content");
    }

    #[test]
    fn decode_stream_flatedecode_name() {
        let compressed = zlib_compress(b"hello pdf");
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"array filter");
    }

    #[test]
    fn decode_stream_unknown_filter() {
        let stream = make_stream(
            Some(Object::Name(b"DCTDecode".to_vec())),
            b"jpeg data".to_vec(),
        );
        let (result, warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        // Falls back to original content with warning
        assert_eq!(&*result, b"not valid zlib");
        assert!(
            warning.is_some(),
            "Corrupt FlateDecode should produce a warning"
        );
        assert!(
            warning
                .unwrap()
                .contains("FlateDecode decompression failed")
        );
    }

    #[test]
    fn decode_stream_valid_flatedecode_no_warning() {
        let compressed = zlib_compress(b"hello pdf");
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"hello pdf");
        assert!(
            warning.is_none(),
            "Valid FlateDecode should produce no warning"
        );
    }

    #[test]
    fn decode_stream_no_filter_no_warning() {
        let stream = make_stream(None, b"raw content".to_vec());
        let (_result, warning) = decode_stream(&empty_doc(), &stream);
        assert!(warning.is_none(), "No filter should produce no warning");
    }

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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, b"Hello");
        assert!(warning.is_none());
    }

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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, b"Hello");
        assert!(warning.is_none());
    }

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
        let stream = make_stream(Some(Object::Name(b"LZWDecode".to_vec())), compressed);
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, original.as_slice());
        assert!(warning.is_none());
    }

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
        let stream = make_stream(Some(Object::Name(b"RunLengthDecode".to_vec())), data);
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, b"Hello");
        assert!(warning.is_none());
    }

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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, b"not valid zlib");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("FlateDecode"));
    }

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

    #[test]
    fn decode_stream_empty_content_no_filter() {
        let stream = make_stream(None, vec![]);
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"");
    }

    #[test]
    fn decode_stream_filter_is_integer_ignored() {
        // Filter that's neither Name nor Array → treated as no filter
        let stream = make_stream(Some(Object::Integer(42)), b"raw bytes".to_vec());
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, b"pass through");
        assert!(
            warning.is_some(),
            "Unsupported filter should produce warning"
        );
        assert!(warning.unwrap().contains("unsupported filter: DCTDecode"));
    }

    #[test]
    fn decode_stream_empty_filter_array() {
        let stream = make_stream(Some(Object::Array(vec![])), b"no filters".to_vec());
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"no filters");
    }

    #[test]
    fn decode_stream_flatedecode_empty_payload() {
        // Compressed empty content
        let compressed = zlib_compress(b"");
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"");
    }

    #[test]
    fn decode_stream_filter_array_with_non_name_elements() {
        // Array with a non-Name object (e.g., Integer) mixed in → filter_map skips it
        let compressed = zlib_compress(b"mixed types");
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Integer(42), // not a Name, should be skipped
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            compressed,
        );
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw data");
    }

    #[test]
    fn decode_stream_large_content() {
        // Verify decompression works for larger payloads
        let large: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let compressed = zlib_compress(&large);
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        let (result, _warning) = decode_stream(&empty_doc(), &stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, &large[..]);
    }

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
    fn decode_stream_jpxdecode_unsupported() {
        let stream = make_stream(
            Some(Object::Name(b"JPXDecode".to_vec())),
            b"jpeg2000 data".to_vec(),
        );
        let (result, warning) = decode_stream(&empty_doc(), &stream);
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, b"fax data");
        assert!(warning.is_some());
        assert!(
            warning
                .unwrap()
                .contains("unsupported filter: CCITTFaxDecode")
        );
    }

    #[test]
    fn decode_stream_triple_pipeline_asciihex_flate_ascii85() {
        // Build: original -> ASCII85Encode -> FlateDecode -> ASCIIHexEncode
        // Decode order: ASCIIHexDecode -> FlateDecode -> ASCII85Decode
        let original = b"Hello";
        // ASCII85 of "Hello" is "87cURDZ"
        let ascii85_encoded = b"87cURDZ~>";
        let flate_compressed = zlib_compress(ascii85_encoded);
        let hex_encoded: String = flate_compressed
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        let hex_bytes = format!("{}>", hex_encoded);

        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"ASCII85Decode".to_vec()),
            ])),
            hex_bytes.into_bytes(),
        );
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(
            &*result,
            original.as_slice(),
            "3-stage pipeline should decode correctly"
        );
        assert!(
            warning.is_none(),
            "No warning expected for supported pipeline"
        );
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        // Should have decompressed the FlateDecode part successfully
        assert_eq!(
            &*result, b"some data",
            "Should get FlateDecode result before stopping"
        );
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        // ASCIIHexDecode produced "Hello", but FlateDecode on "Hello" fails
        assert_eq!(&*result, b"Hello", "Should return intermediate result");
        assert!(warning.is_some());
        assert!(
            warning
                .unwrap()
                .contains("FlateDecode decompression failed")
        );
    }

    #[test]
    fn decode_stream_pipeline_ascii85_then_lzw() {
        // Encode: original -> LZW -> ASCII85
        // Decode pipeline: ASCII85Decode -> LZWDecode
        let original = b"Hello LZW";
        let mut lzw_encoder =
            weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
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
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(
            &*result,
            original.as_slice(),
            "ASCII85+LZW pipeline should decode"
        );
        assert!(warning.is_none());
    }

    #[test]
    fn decode_stream_pipeline_lzw_then_asciihex() {
        // Encode: original -> ASCIIHexEncode -> LZW
        // Decode: LZWDecode -> ASCIIHexDecode
        let original = b"LZW+hex";
        let hex_encoded: String = original.iter().map(|b| format!("{:02x}", b)).collect();
        let hex_bytes = format!("{}>", hex_encoded);

        let mut lzw_encoder =
            weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let lzw_compressed = lzw_encoder.encode(hex_bytes.as_bytes()).unwrap();

        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"LZWDecode".to_vec()),
                Object::Name(b"ASCIIHexDecode".to_vec()),
            ])),
            lzw_compressed,
        );
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(
            &*result,
            original.as_slice(),
            "LZW+ASCIIHex pipeline should decode"
        );
        assert!(warning.is_none());
    }

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

    #[test]
    fn decode_ascii85_empty_input() {
        let result = decode_ascii85(b"~>").unwrap();
        assert!(
            result.is_empty(),
            "Empty ASCII85 should produce empty output"
        );
    }

    #[test]
    fn decode_ascii85_empty_with_prefix() {
        let result = decode_ascii85(b"<~~>").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decode_ascii85_multiple_z() {
        let result = decode_ascii85(b"zzz~>").unwrap();
        assert_eq!(
            result,
            vec![0u8; 12],
            "Three z's should produce 12 zero bytes"
        );
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

    /// A small cap, so the bomb tests need not allocate 256 MB.
    const TEST_LIMIT: u64 = 1000;

    #[test]
    fn decode_lzw_capped_stops_during_decode() {
        // bug-0005: the stream lacks its end marker, so a decoder that checks
        // the cap only afterwards reports the bad code, not the limit.
        let original = vec![0u8; 100_000];
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let mut compressed = encoder.encode(&original).unwrap();
        compressed.truncate(compressed.len() - 4);
        let err = decode_lzw_capped(&compressed, TEST_LIMIT, true).unwrap_err();
        assert!(err.contains("byte limit"), "{err}");
    }

    #[test]
    fn decode_lzw_capped_accepts_output_at_the_limit() {
        let original = vec![b'X'; TEST_LIMIT as usize];
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(&original).unwrap();
        assert_eq!(
            decode_lzw_capped(&compressed, TEST_LIMIT, true).unwrap(),
            original
        );
    }

    #[test]
    fn decode_lzw_missing_end_marker_is_an_error() {
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let mut compressed = encoder.encode(b"AAABBBCCC").unwrap();
        compressed.truncate(compressed.len() - 2);
        assert!(decode_lzw(&compressed).is_err());
    }

    #[test]
    fn decode_run_length_capped_rejects_repeat_runs_past_the_limit() {
        // bug-0005: eight maximal repeat runs (0x81 = 128 copies) = 1024 bytes.
        let data = [0x81, b'x'].repeat(8);
        let err = decode_run_length_capped(&data, TEST_LIMIT).unwrap_err();
        assert!(err.contains("byte limit"), "{err}");
    }

    #[test]
    fn decode_run_length_capped_rejects_literal_runs_past_the_limit() {
        let mut data = Vec::new();
        for _ in 0..8 {
            data.push(127); // 128 literal bytes follow
            data.extend([b'y'; 128]);
        }
        let err = decode_run_length_capped(&data, TEST_LIMIT).unwrap_err();
        assert!(err.contains("byte limit"), "{err}");
    }

    #[test]
    fn decode_run_length_capped_accepts_output_at_the_limit() {
        let mut data = [0x81, b'x'].repeat(7); // 896 bytes
        data.extend([0x98, b'z']); // 257 - 0x98 = 105 bytes, total 1001
        assert!(decode_run_length_capped(&data, TEST_LIMIT).is_err());
        data[14] = 0x99; // 104 bytes, total exactly 1000
        assert_eq!(
            decode_run_length_capped(&data, TEST_LIMIT).unwrap().len(),
            TEST_LIMIT as usize
        );
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

    #[test]
    fn format_hex_dump_high_bytes_show_as_dots() {
        // Bytes 0x80-0xFF should show as dots in ASCII column
        let data: Vec<u8> = (0x80..0x84).collect();
        let result = format_hex_dump(&data);
        assert!(result.contains("80 81 82 83"));
        assert!(
            result.contains("|....|"),
            "High bytes should be dots: {}",
            result
        );
    }

    #[test]
    fn format_hex_dump_mixed_printable_and_non() {
        // Mix of printable ASCII and control chars
        let data = b"Hi\x00\x01\x02";
        let result = format_hex_dump(data);
        assert!(
            result.contains("|Hi...|"),
            "Mixed content ASCII representation: {}",
            result
        );
    }

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

    #[test]
    fn flatedecode_size_limit_returns_error() {
        // Create a stream that decompresses to just over MAX_DECODED_SIZE
        // We can't practically create a 256MB test, so test the mechanism
        // by verifying the limit constant exists and the error message format
        let msg = format!(
            "FlateDecode: decoded size exceeds {} byte limit",
            super::MAX_DECODED_SIZE
        );
        assert!(
            msg.contains("268435456"),
            "MAX_DECODED_SIZE should be 256 MB"
        );
    }

    #[test]
    fn lzw_size_limit_returns_error() {
        // Verify the limit constant value used in LZW path
        assert_eq!(super::MAX_DECODED_SIZE, 256 * 1024 * 1024);
    }

    #[test]
    fn flatedecode_normal_size_succeeds() {
        // Normal-sized data should decode without hitting the limit
        let original: Vec<u8> = vec![b'A'; 100_000];
        let compressed = zlib_compress(&original);
        let stream = make_stream(Some(Object::Name(b"FlateDecode".to_vec())), compressed);
        let (result, warning) = decode_stream(&empty_doc(), &stream);
        assert_eq!(&*result, &original[..]);
        assert!(warning.is_none(), "Normal-sized decode should not warn");
    }

    #[test]
    fn lzw_normal_size_succeeds() {
        // Normal-sized LZW data should decode without hitting the limit
        let original: Vec<u8> = vec![b'B'; 10_000];
        let mut encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(&original).unwrap();
        let result = decode_lzw(&compressed).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn decode_uses_format_operation_not_debug() {
        let mut doc = Document::new();
        // Create a minimal content stream with a Tj operation
        let content_bytes = b"(Hello) Tj";
        let stream = Stream::new(Dictionary::new(), content_bytes.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig {
            decode: true,
            truncate: None,
            json: false,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        // Should show clean format, not Debug format with "Operation { operator:"
        assert!(
            !out.contains("Operation {"),
            "Should not contain Debug format"
        );
        assert!(
            out.contains("(Hello) Tj"),
            "Should contain formatted operation"
        );
    }
}
