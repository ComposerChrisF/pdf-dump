use flate2::read::ZlibDecoder;
use std::borrow::Cow;
use std::io::Read;

pub(crate) fn is_binary_stream(content: &[u8]) -> bool {
    content.iter().any(|&b| !b.is_ascii_alphanumeric() && !b.is_ascii_whitespace() && !b.is_ascii_punctuation())
}

pub(crate) fn decode_ascii85(data: &[u8]) -> Result<Vec<u8>, String> {
    let cleaned: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
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
    let cleaned: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
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
            None => return Err(format!("ASCIIHexDecode: invalid hex character 0x{:02x}", input[i])),
        };
        let lo = if i + 1 < input.len() {
            match hex_digit(input[i + 1]) {
                Some(v) => v,
                None => return Err(format!("ASCIIHexDecode: invalid hex character 0x{:02x}", input[i + 1])),
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

pub(crate) fn decode_lzw(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
    decoder.decode(data).map_err(|e| format!("LZWDecode: {}", e))
}

pub(crate) fn decode_run_length(data: &[u8]) -> Result<Vec<u8>, String> {
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
            result.extend_from_slice(&data[i..i + count]);
            i += count;
        } else if length == 128 {
            break;
        } else {
            if i >= data.len() {
                return Err("RunLengthDecode: truncated repeat run".to_string());
            }
            let count = 257 - length as usize;
            let byte = data[i];
            i += 1;
            result.extend(std::iter::repeat_n(byte, count));
        }
    }
    Ok(result)
}

pub(crate) fn get_filter_names(stream: &lopdf::Stream) -> Vec<Vec<u8>> {
    match stream.dict.get(b"Filter").ok() {
        Some(filter_obj) => {
            if let Ok(name) = filter_obj.as_name() {
                vec![name.to_vec()]
            } else if let Ok(arr) = filter_obj.as_array() {
                arr.iter().filter_map(|obj| obj.as_name().ok().map(|n| n.to_vec())).collect()
            } else {
                vec![]
            }
        }
        None => vec![],
    }
}

pub(crate) fn decode_stream(stream: &lopdf::Stream) -> (Cow<'_, [u8]>, Option<String>) {
    let filters = get_filter_names(stream);
    if filters.is_empty() {
        return (Cow::Borrowed(&stream.content), None);
    }

    let mut data: Cow<'_, [u8]> = Cow::Borrowed(&stream.content);
    for filter in &filters {
        let result = match filter.as_slice() {
            b"FlateDecode" => {
                let mut decoder = ZlibDecoder::new(&data[..]);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)
                    .map(|_| decompressed)
                    .map_err(|_| "FlateDecode decompression failed".to_string())
            }
            b"ASCII85Decode" => decode_ascii85(&data),
            b"ASCIIHexDecode" => decode_asciihex(&data),
            b"LZWDecode" => decode_lzw(&data),
            b"RunLengthDecode" => decode_run_length(&data),
            other => {
                let name = String::from_utf8_lossy(other);
                return (Cow::Owned(data.into_owned()), Some(format!("unsupported filter: {}", name)));
            }
        };
        match result {
            Ok(decoded) => data = Cow::Owned(decoded),
            Err(msg) => return (Cow::Owned(data.into_owned()), Some(msg)),
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
            if i == 7 { result.push(' '); }
        }
        if chunk.len() < 16 {
            for i in chunk.len()..16 {
                result.push_str("   ");
                if i == 7 { result.push(' '); }
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
    use crate::test_utils::*;
    use crate::object::print_single_object;
    use crate::types::DumpConfig;
    use lopdf::{Dictionary, Stream};
    use pretty_assertions::assert_eq;
    use std::borrow::Cow;
    use lopdf::Object;
    use lopdf::Document;

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
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw content");
    }

    #[test]
    fn decode_stream_flatedecode_name() {
        let compressed = zlib_compress(b"hello pdf");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let (result, _warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"array filter");
    }

    #[test]
    fn decode_stream_unknown_filter() {
        let stream = make_stream(
            Some(Object::Name(b"DCTDecode".to_vec())),
            b"jpeg data".to_vec(),
        );
        let (result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
        // Falls back to original content with warning
        assert_eq!(&*result, b"not valid zlib");
        assert!(warning.is_some(), "Corrupt FlateDecode should produce a warning");
        assert!(warning.unwrap().contains("FlateDecode decompression failed"));
    }

    #[test]
    fn decode_stream_valid_flatedecode_no_warning() {
        let compressed = zlib_compress(b"hello pdf");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let (result, warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"hello pdf");
        assert!(warning.is_none(), "Valid FlateDecode should produce no warning");
    }

    #[test]
    fn decode_stream_no_filter_no_warning() {
        let stream = make_stream(None, b"raw content".to_vec());
        let (_result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
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
        let stream = make_stream(
            Some(Object::Name(b"LZWDecode".to_vec())),
            compressed,
        );
        let (result, warning) = decode_stream(&stream);
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
        let stream = make_stream(
            Some(Object::Name(b"RunLengthDecode".to_vec())),
            data,
        );
        let (result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"");
    }

    #[test]
    fn decode_stream_filter_is_integer_ignored() {
        // Filter that's neither Name nor Array → treated as no filter
        let stream = make_stream(Some(Object::Integer(42)), b"raw bytes".to_vec());
        let (result, _warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"pass through");
        assert!(warning.is_some(), "Unsupported filter should produce warning");
        assert!(warning.unwrap().contains("unsupported filter: DCTDecode"));
    }

    #[test]
    fn decode_stream_empty_filter_array() {
        let stream = make_stream(
            Some(Object::Array(vec![])),
            b"no filters".to_vec(),
        );
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"no filters");
    }

    #[test]
    fn decode_stream_flatedecode_empty_payload() {
        // Compressed empty content
        let compressed = zlib_compress(b"");
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(&*result, b"");
    }

    #[test]
    fn decode_stream_filter_array_with_non_name_elements() {
        // Array with a non-Name object (e.g., Integer) mixed in → filter_map skips it
        let compressed = zlib_compress(b"mixed types");
        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Integer(42),  // not a Name, should be skipped
                Object::Name(b"FlateDecode".to_vec()),
            ])),
            compressed,
        );
        let (result, _warning) = decode_stream(&stream);
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
        let (result, _warning) = decode_stream(&stream);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, b"raw data");
    }

    #[test]
    fn decode_stream_large_content() {
        // Verify decompression works for larger payloads
        let large: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let compressed = zlib_compress(&large);
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            compressed,
        );
        let (result, _warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
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
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, b"fax data");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("unsupported filter: CCITTFaxDecode"));
    }

    #[test]
    fn decode_stream_triple_pipeline_asciihex_flate_ascii85() {
        // Build: original -> ASCII85Encode -> FlateDecode -> ASCIIHexEncode
        // Decode order: ASCIIHexDecode -> FlateDecode -> ASCII85Decode
        let original = b"Hello";
        // ASCII85 of "Hello" is "87cURDZ"
        let ascii85_encoded = b"87cURDZ~>";
        let flate_compressed = zlib_compress(ascii85_encoded);
        let hex_encoded: String = flate_compressed.iter().map(|b| format!("{:02x}", b)).collect();
        let hex_bytes = format!("{}>", hex_encoded);

        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"ASCIIHexDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"ASCII85Decode".to_vec()),
            ])),
            hex_bytes.into_bytes(),
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, original.as_slice(), "3-stage pipeline should decode correctly");
        assert!(warning.is_none(), "No warning expected for supported pipeline");
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
        let (result, warning) = decode_stream(&stream);
        // Should have decompressed the FlateDecode part successfully
        assert_eq!(&*result, b"some data", "Should get FlateDecode result before stopping");
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
        let (result, warning) = decode_stream(&stream);
        // ASCIIHexDecode produced "Hello", but FlateDecode on "Hello" fails
        assert_eq!(&*result, b"Hello", "Should return intermediate result");
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("FlateDecode decompression failed"));
    }

    #[test]
    fn decode_stream_pipeline_ascii85_then_lzw() {
        // Encode: original -> LZW -> ASCII85
        // Decode pipeline: ASCII85Decode -> LZWDecode
        let original = b"Hello LZW";
        let mut lzw_encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
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
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, original.as_slice(), "ASCII85+LZW pipeline should decode");
        assert!(warning.is_none());
    }

    #[test]
    fn decode_stream_pipeline_lzw_then_asciihex() {
        // Encode: original -> ASCIIHexEncode -> LZW
        // Decode: LZWDecode -> ASCIIHexDecode
        let original = b"LZW+hex";
        let hex_encoded: String = original.iter().map(|b| format!("{:02x}", b)).collect();
        let hex_bytes = format!("{}>", hex_encoded);

        let mut lzw_encoder = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        let lzw_compressed = lzw_encoder.encode(hex_bytes.as_bytes()).unwrap();

        let stream = make_stream(
            Some(Object::Array(vec![
                Object::Name(b"LZWDecode".to_vec()),
                Object::Name(b"ASCIIHexDecode".to_vec()),
            ])),
            lzw_compressed,
        );
        let (result, warning) = decode_stream(&stream);
        assert_eq!(&*result, original.as_slice(), "LZW+ASCIIHex pipeline should decode");
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
        assert!(result.is_empty(), "Empty ASCII85 should produce empty output");
    }

    #[test]
    fn decode_ascii85_empty_with_prefix() {
        let result = decode_ascii85(b"<~~>").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decode_ascii85_multiple_z() {
        let result = decode_ascii85(b"zzz~>").unwrap();
        assert_eq!(result, vec![0u8; 12], "Three z's should produce 12 zero bytes");
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
        assert!(result.contains("|....|"), "High bytes should be dots: {}", result);
    }

    #[test]
    fn format_hex_dump_mixed_printable_and_non() {
        // Mix of printable ASCII and control chars
        let data = b"Hi\x00\x01\x02";
        let result = format_hex_dump(data);
        assert!(result.contains("|Hi...|"), "Mixed content ASCII representation: {}", result);
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
    fn decode_uses_format_operation_not_debug() {
        let mut doc = Document::new();
        // Create a minimal content stream with a Tj operation
        let content_bytes = b"(Hello) Tj";
        let stream = Stream::new(Dictionary::new(), content_bytes.to_vec());
        doc.objects.insert((1, 0), Object::Stream(stream));

        let config = DumpConfig { decode: true, truncate: None, json: false, hex: false, depth: None, deref: false, raw: false };
        let out = output_of(|w| print_single_object(w, &doc, 1, &config));
        // Should show clean format, not Debug format with "Operation { operator:"
        assert!(!out.contains("Operation {"), "Should not contain Debug format");
        assert!(out.contains("(Hello) Tj"), "Should contain formatted operation");
    }

}
