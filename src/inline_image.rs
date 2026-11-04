//! Inline images (`BI … ID … EI`), which live inside a content stream and have no object number.
//!
//! lopdf's content parser is no help here: it skips an inline image with any filter or a named
//! colour space, and panics on one with neither `/CS` nor `/IM`.  So this is a scanner of its own
//! over the decoded bytes, which needs only PDF's tokens and never interprets an operator but
//! `BI`, `Do`, `scn` and `SCN`.
//!
//! What is walked: a page's contents, the form XObjects (`Do`), tiling patterns (`scn`) and soft-mask
//! groups (`gs`) it invokes, and its annotations' normal appearances, each once a page under each
//! set of resources it inherits.  Not walked: a Type 3 font's glyph procedures (a bitmap font's
//! glyphs are inline images, and listing them says nothing about pictures), and forms and patterns
//! that nothing on a page invokes, which are not drawn.

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use std::collections::HashSet;

use crate::annotations::{form_default_resources, normal_appearance_streams};
use crate::helpers::{page_annots, page_content_ids, resolve_dict};
use crate::image_export::{ImageSource, decode_stream_fully, parse_color_space};
use crate::text::MAX_FORM_DEPTH;

/// Bounds the decoded content one scan will read.  A tiny file can name one huge stream from every
/// page; past this the scan stops and says so, which is a finding.
const MAX_SCANNED_BYTES: u64 = 1 << 30;

pub(crate) struct InlineImage<'a> {
    pub page: u32,
    /// 1-based on its page, in walking order: the address `--extract-inline-image` takes.
    pub index: u32,
    /// `page`, `form`, `pattern`, or `appearance`.
    pub source: &'static str,
    pub source_object: Option<ObjectId>,
    /// The offset of `BI` in the source's decoded content.  A page's content streams count as one,
    /// joined by newlines.
    pub offset: usize,
    pub dict: Dictionary,
    /// The size of its data.
    pub size: usize,
    /// The data itself, held only for the one image an extraction asked for.
    pub data: Vec<u8>,
    pub resources: Option<&'a Dictionary>,
}

impl InlineImage<'_> {
    pub(crate) fn as_source(&self) -> ImageSource<'_> {
        ImageSource {
            dict: &self.dict,
            data: &self.data,
            resources: self.resources,
            inline: true,
        }
    }
}

#[derive(Default)]
pub(crate) struct InlineScan<'a> {
    pub images: Vec<InlineImage<'a>>,
    /// What could not be walked: a content stream that would not decode, nesting past the limit, an
    /// inline image with no end or one that ends by a guess.  Each is a place an image might be, so
    /// each is a finding.
    pub warnings: Vec<String>,
    scanned_bytes: u64,
    budget_spent: bool,
}

/// Lists every inline image in the document, without keeping their data.
pub(crate) fn scan_document(doc: &Document) -> InlineScan<'_> {
    let mut scan = InlineScan::default();
    let pages = doc.get_pages();
    // lopdf's page walk drops a kid it cannot classify, such as one with no /Type, which a tolerant
    // viewer still draws.  A page not walked is a place an image might be.
    let declared = crate::helpers::get_catalog(doc)
        .and_then(|catalog| resolve_dict(doc, catalog.get(b"Pages").ok()?))
        .and_then(|tree| tree.get(b"Count").ok()?.as_i64().ok());
    if let Some(count) = declared
        && (pages.len() as i64) < count
    {
        scan.warnings.push(format!(
            "the page tree declares {count} page(s) and only {} could be walked",
            pages.len()
        ));
    }
    for (&page_num, &page_id) in &pages {
        scan_page(doc, page_num, page_id, None, &mut scan);
    }
    scan
}

/// Walks one page.  `keep_data_of` names the one image, by index, whose data to hold on to.
pub(crate) fn scan_page<'a>(
    doc: &'a Document,
    page_num: u32,
    page_id: ObjectId,
    keep_data_of: Option<u32>,
    scan: &mut InlineScan<'a>,
) {
    let resources = crate::resources::resolve_page_resources(doc, page_id);
    let mut walk = PageWalk {
        doc,
        page: page_num,
        next_index: 1,
        keep_data_of,
        seen: HashSet::new(),
        scan,
    };
    let Ok(Object::Dictionary(page)) = doc.get_object(page_id) else {
        return;
    };
    if let Some(ids) = page_content_ids(doc, page) {
        walk.scan_page_contents(&ids, resources);
    }
    let widget_resources = form_default_resources(doc).or(resources);
    for annot in page_annots(doc, page) {
        let dict = match annot {
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(Object::Dictionary(d)) => d,
                Ok(Object::Stream(s)) => &s.dict,
                _ => continue,
            },
            Object::Dictionary(d) => d,
            _ => continue,
        };
        let is_widget = dict
            .get(b"Subtype")
            .ok()
            .and_then(|v| v.as_name().ok())
            .is_some_and(|n| n == b"Widget");
        let (streams, problem) = normal_appearance_streams(doc, dict);
        if let Some(problem) = problem {
            walk.warn(problem);
        }
        for id in streams {
            let fallback = if is_widget {
                widget_resources
            } else {
                resources
            };
            walk.scan_object(id, fallback, "appearance", 0);
        }
    }
}

struct PageWalk<'a, 's> {
    doc: &'a Document,
    page: u32,
    next_index: u32,
    keep_data_of: Option<u32>,
    /// Each form or pattern is walked once a page, however many times it is drawn, but once per set
    /// of resources when it has none of its own, since those decide what its names reach.
    seen: HashSet<(ObjectId, usize)>,
    scan: &'s mut InlineScan<'a>,
}

impl<'a> PageWalk<'a, '_> {
    fn warn(&mut self, message: String) {
        self.scan
            .warnings
            .push(format!("page {}: {message}", self.page));
    }

    /// Spends decoded bytes from the scan's budget, saying so once when they run out.
    fn spend(&mut self, bytes: usize) -> bool {
        self.scan.scanned_bytes += bytes as u64;
        if self.scan.scanned_bytes <= MAX_SCANNED_BYTES {
            return true;
        }
        if !self.scan.budget_spent {
            self.scan.budget_spent = true;
            self.scan.warnings.push(format!(
                "stopped after {MAX_SCANNED_BYTES} bytes of decoded content"
            ));
        }
        false
    }

    /// A page's content streams read as one, as the standard says they are (§ 7.8.2).
    fn scan_page_contents(&mut self, ids: &[ObjectId], resources: Option<&'a Dictionary>) {
        let mut bytes = Vec::new();
        for id in ids {
            match self.doc.get_object(*id) {
                Ok(Object::Stream(stream)) => match decode_stream_fully(self.doc, stream) {
                    Ok(decoded) => {
                        if !bytes.is_empty() {
                            bytes.push(b'\n');
                        }
                        bytes.extend_from_slice(&decoded);
                    }
                    Err(why) => self.warn(format!("content stream {}: {why}", id.0)),
                },
                // A reference to nothing reads as null, which draws nothing.
                Err(lopdf::Error::ObjectNotFound(_)) | Ok(Object::Null) => {}
                _ => self.warn(format!("content stream {} is not a stream", id.0)),
            }
        }
        if self.spend(bytes.len()) {
            self.scan_stream(&bytes, resources, "page", None, 0);
        }
    }

    fn scan_object(
        &mut self,
        id: ObjectId,
        parent_resources: Option<&'a Dictionary>,
        source: &'static str,
        depth: u32,
    ) {
        if depth >= MAX_FORM_DEPTH {
            self.warn(format!(
                "nesting past depth {MAX_FORM_DEPTH} at object {}",
                id.0
            ));
            return;
        }
        let stream = match self.doc.get_object(id) {
            Ok(Object::Stream(stream)) => stream,
            Err(lopdf::Error::ObjectNotFound(_)) | Ok(Object::Null) => return,
            _ => {
                self.warn(format!("object {} is not a stream", id.0));
                return;
            }
        };
        let own_resources = stream
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|r| resolve_dict(self.doc, r));
        let resources = own_resources.or(parent_resources);
        let scope = own_resources.map_or_else(
            || parent_resources.map_or(0, |r| std::ptr::from_ref(r) as usize),
            |_| 0,
        );
        if !self.seen.insert((id, scope)) {
            return;
        }
        let bytes = match decode_stream_fully(self.doc, stream) {
            Ok(bytes) => bytes,
            Err(why) => {
                self.warn(format!("object {}: {why}", id.0));
                return;
            }
        };
        if self.spend(bytes.len()) {
            self.scan_stream(&bytes, resources, source, Some(id), depth + 1);
        }
    }

    fn scan_stream(
        &mut self,
        bytes: &[u8],
        resources: Option<&'a Dictionary>,
        source: &'static str,
        source_object: Option<ObjectId>,
        depth: u32,
    ) {
        let mut tokens = Tokens { bytes, pos: 0 };
        let mut last_name: Option<Vec<u8>> = None;
        while let Some(token) = tokens.next_token() {
            match token {
                Token::Name(n) => last_name = Some(n),
                Token::Other => {}
                Token::Operator(op) => {
                    match (op, last_name.as_deref()) {
                        (b"BI", _) => {
                            let offset = tokens.pos - 2;
                            match read_inline_image(self.doc, &mut tokens, resources) {
                                Ok(read) => {
                                    let index = self.next_index;
                                    self.next_index += 1;
                                    if read.ambiguous {
                                        self.warn(format!(
                                            "inline image at offset {offset}: where its data ends is a guess"
                                        ));
                                    }
                                    let keep = self.keep_data_of == Some(index);
                                    self.scan.images.push(InlineImage {
                                        page: self.page,
                                        index,
                                        source,
                                        source_object,
                                        offset,
                                        dict: read.dict,
                                        size: read.data.len(),
                                        data: if keep { read.data.to_vec() } else { Vec::new() },
                                        resources,
                                    });
                                }
                                Err(InlineError::NoEnd) => {
                                    self.warn(format!(
                                        "inline image at offset {offset}: no EI ends its data"
                                    ));
                                    // No acceptable EI lies past here for any later image either, so
                                    // looking again from each would only repeat this, at great cost.
                                    return;
                                }
                                Err(InlineError::Malformed(why)) => {
                                    self.warn(format!("inline image at offset {offset}: {why}"));
                                }
                            }
                        }
                        (b"gs", Some(name)) => {
                            if let Some(id) = soft_mask_group(self.doc, resources, name) {
                                self.scan_object(id, resources, "form", depth);
                            }
                        }
                        (b"Do", Some(name)) => {
                            if let Some(id) = named_resource(self.doc, resources, b"XObject", name)
                                && is_form(self.doc, id)
                            {
                                self.scan_object(id, resources, "form", depth);
                            }
                        }
                        (b"scn" | b"SCN", Some(name)) => {
                            if let Some(id) = named_resource(self.doc, resources, b"Pattern", name)
                                && matches!(self.doc.get_object(id), Ok(Object::Stream(_)))
                            {
                                self.scan_object(id, resources, "pattern", depth);
                            }
                        }
                        _ => {}
                    }
                    last_name = None;
                }
            }
        }
    }
}

/// The form a named graphics state's soft mask draws (`/SMask << /G form >>`).
fn soft_mask_group(
    doc: &Document,
    resources: Option<&Dictionary>,
    name: &[u8],
) -> Option<ObjectId> {
    let states = resolve_dict(doc, resources?.get(b"ExtGState").ok()?)?;
    let state = resolve_dict(doc, states.get(name).ok()?)?;
    let mask = resolve_dict(doc, state.get(b"SMask").ok()?)?;
    mask.get(b"G").ok()?.as_reference().ok()
}

fn named_resource(
    doc: &Document,
    resources: Option<&Dictionary>,
    category: &[u8],
    name: &[u8],
) -> Option<ObjectId> {
    resolve_dict(doc, resources?.get(category).ok()?)?
        .get(name)
        .ok()?
        .as_reference()
        .ok()
}

fn is_form(doc: &Document, id: ObjectId) -> bool {
    let Ok(Object::Stream(stream)) = doc.get_object(id) else {
        return false;
    };
    let subtype = match stream.dict.get(b"Subtype") {
        Ok(Object::Reference(target)) => doc.get_object(*target).ok(),
        Ok(other) => Some(other),
        Err(_) => None,
    };
    matches!(subtype, Some(Object::Name(n)) if n == b"Form")
}

enum Token<'b> {
    Name(Vec<u8>),
    Operator(&'b [u8]),
    /// A number, string, array bracket, or dictionary bracket, whose content does not matter here.
    Other,
}

struct Tokens<'b> {
    bytes: &'b [u8],
    pos: usize,
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

impl<'b> Tokens<'b> {
    fn skip_space(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if is_whitespace(b) {
                self.pos += 1;
            } else if b == b'%' {
                while self
                    .bytes
                    .get(self.pos)
                    .is_some_and(|c| !matches!(c, b'\n' | b'\r'))
                {
                    self.pos += 1;
                }
            } else {
                return;
            }
        }
    }

    fn next_token(&mut self) -> Option<Token<'b>> {
        self.skip_space();
        let b = *self.bytes.get(self.pos)?;
        match b {
            b'/' => Some(Token::Name(self.read_name())),
            b'(' => {
                self.read_literal_string();
                Some(Token::Other)
            }
            b'<' if self.bytes.get(self.pos + 1) != Some(&b'<') => {
                self.read_hex_string();
                Some(Token::Other)
            }
            b'<' | b'>' => {
                self.pos += if self.bytes.get(self.pos + 1) == Some(&b) {
                    2
                } else {
                    1
                };
                Some(Token::Other)
            }
            b'[' | b']' | b'{' | b'}' | b')' => {
                self.pos += 1;
                Some(Token::Other)
            }
            _ => {
                let start = self.pos;
                self.skip_regular();
                let word = &self.bytes[start..self.pos];
                Some(if is_operator(word) {
                    Token::Operator(word)
                } else {
                    Token::Other
                })
            }
        }
    }

    fn skip_regular(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|b| !is_whitespace(*b) && !is_delimiter(*b))
        {
            self.pos += 1;
        }
    }

    /// At a `/`: the name, with its `#xx` escapes resolved.
    fn read_name(&mut self) -> Vec<u8> {
        self.pos += 1;
        let start = self.pos;
        self.skip_regular();
        let raw = &self.bytes[start..self.pos];
        let mut name = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            let hex = (raw[i] == b'#' && i + 2 < raw.len())
                .then(|| {
                    Some(
                        crate::stream::hex_digit(raw[i + 1])? << 4
                            | crate::stream::hex_digit(raw[i + 2])?,
                    )
                })
                .flatten();
            match hex {
                Some(byte) => {
                    name.push(byte);
                    i += 3;
                }
                None => {
                    name.push(raw[i]);
                    i += 1;
                }
            }
        }
        name
    }

    /// At a `(`: the string, with its escapes resolved.
    fn read_literal_string(&mut self) -> Vec<u8> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(&b) = self.bytes.get(self.pos) {
            self.pos += 1;
            match b {
                b'\\' => {
                    let Some(&next) = self.bytes.get(self.pos) else {
                        break;
                    };
                    self.pos += 1;
                    match next {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut value = u32::from(next - b'0');
                            for _ in 0..2 {
                                match self.bytes.get(self.pos) {
                                    Some(d @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(value as u8);
                        }
                        b'\r' => {
                            if self.bytes.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(b);
                }
                _ => out.push(b),
            }
        }
        out
    }

    /// At a `<`: the string its hex digits spell.
    fn read_hex_string(&mut self) -> Vec<u8> {
        self.pos += 1;
        let mut digits = Vec::new();
        while let Some(&b) = self.bytes.get(self.pos) {
            self.pos += 1;
            if b == b'>' {
                break;
            }
            if let Some(d) = crate::stream::hex_digit(b) {
                digits.push(d);
            }
        }
        if digits.len() % 2 == 1 {
            digits.push(0);
        }
        digits.chunks(2).map(|d| d[0] << 4 | d[1]).collect()
    }

    /// One value of an inline image's dictionary.
    fn read_value(&mut self, depth: u32) -> Result<Object, String> {
        if depth > 8 {
            return Err("a dictionary nested too deeply".to_string());
        }
        self.skip_space();
        let Some(&b) = self.bytes.get(self.pos) else {
            return Err("the content ends inside its dictionary".to_string());
        };
        match b {
            b'/' => Ok(Object::Name(self.read_name())),
            b'(' => Ok(Object::String(
                self.read_literal_string(),
                StringFormat::Literal,
            )),
            b'<' if self.bytes.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                let mut dict = Dictionary::new();
                loop {
                    self.skip_space();
                    match self.bytes.get(self.pos) {
                        Some(b'>') if self.bytes.get(self.pos + 1) == Some(&b'>') => {
                            self.pos += 2;
                            return Ok(Object::Dictionary(dict));
                        }
                        Some(b'/') => {
                            let key = self.read_name();
                            dict.set(key, self.read_value(depth + 1)?);
                        }
                        _ => return Err("a malformed nested dictionary".to_string()),
                    }
                }
            }
            b'<' => Ok(Object::String(
                self.read_hex_string(),
                StringFormat::Hexadecimal,
            )),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    match self.bytes.get(self.pos) {
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Object::Array(items));
                        }
                        Some(_) => items.push(self.read_value(depth + 1)?),
                        None => return Err("an unterminated array".to_string()),
                    }
                }
            }
            _ => {
                let start = self.pos;
                self.skip_regular();
                let word = &self.bytes[start..self.pos];
                if word.is_empty() {
                    return Err("a value that could not be read".to_string());
                }
                Ok(parse_word(word)?)
            }
        }
    }
}

fn parse_word(word: &[u8]) -> Result<Object, String> {
    match word {
        b"true" => return Ok(Object::Boolean(true)),
        b"false" => return Ok(Object::Boolean(false)),
        b"null" => return Ok(Object::Null),
        _ => {}
    }
    let text = std::str::from_utf8(word).map_err(|_| "a value that is not text".to_string())?;
    if let Ok(i) = text.parse::<i64>() {
        return Ok(Object::Integer(i));
    }
    text.parse::<f32>()
        .map(Object::Real)
        .map_err(|_| format!("the value {text}"))
}

/// Anything regular that is not a number or a keyword value is an operator.
fn is_operator(word: &[u8]) -> bool {
    let first = word.first().copied().unwrap_or(b'0');
    !(first.is_ascii_digit()
        || matches!(first, b'+' | b'-' | b'.')
        || matches!(word, b"true" | b"false" | b"null"))
}

struct ReadInline<'b> {
    dict: Dictionary,
    data: &'b [u8],
    /// A candidate `EI` was turned down before this one, so the end was a judgement.
    ambiguous: bool,
}

enum InlineError {
    /// No `EI` anywhere ahead, so there is no use looking from any later point either.
    NoEnd,
    Malformed(String),
}

impl From<String> for InlineError {
    fn from(why: String) -> Self {
        InlineError::Malformed(why)
    }
}

/// With `tokens` just past `BI`: reads the dictionary to `ID`, then the data to `EI`, and leaves
/// `tokens` past that.
fn read_inline_image<'b>(
    doc: &Document,
    tokens: &mut Tokens<'b>,
    resources: Option<&Dictionary>,
) -> Result<ReadInline<'b>, InlineError> {
    let dict = read_inline_dict(tokens)?;
    read_inline_data(doc, tokens, dict, resources)
}

/// With `tokens` just past `BI`: reads the dictionary, leaving `tokens` just past `ID`.
fn read_inline_dict(tokens: &mut Tokens) -> Result<Dictionary, InlineError> {
    let mut dict = Dictionary::new();
    loop {
        tokens.skip_space();
        match tokens.bytes.get(tokens.pos) {
            Some(b'/') => {
                let key = tokens.read_name();
                dict.set(key, tokens.read_value(0)?);
            }
            Some(_) if tokens.bytes[tokens.pos..].starts_with(b"ID") => {
                tokens.pos += 2;
                return Ok(dict);
            }
            Some(_) => {
                return Err(InlineError::Malformed(
                    "something other than a key in its dictionary".to_string(),
                ));
            }
            None => {
                return Err(InlineError::Malformed(
                    "the content ends inside its dictionary".to_string(),
                ));
            }
        }
    }
}

/// With `tokens` just past `ID`: reads the data to `EI`, and leaves `tokens` past that.
fn read_inline_data<'b>(
    doc: &Document,
    tokens: &mut Tokens<'b>,
    dict: Dictionary,
    resources: Option<&Dictionary>,
) -> Result<ReadInline<'b>, InlineError> {
    let bytes = tokens.bytes;
    let after_id = tokens.pos;
    // One white-space byte separates ID from the data (§ 8.9.7), and writers on CR LF systems use
    // both.  With a known length the data's end says which: the standard's reading goes first, so a
    // first sample of 0x0A after a lone CR survives.
    let one_byte = match bytes.get(after_id) {
        Some(b) if is_whitespace(*b) => after_id + 1,
        _ => after_id,
    };
    let two_bytes = if bytes[after_id..].starts_with(b"\r\n") {
        after_id + 2
    } else {
        one_byte
    };
    if let Some(len) = exact_length(doc, &dict, resources) {
        for start in [one_byte, two_bytes] {
            if let Some(end) = start.checked_add(len)
                && end <= bytes.len()
                && let Some(after) = ei_after(bytes, end)
            {
                tokens.pos = after;
                return Ok(ReadInline {
                    dict,
                    data: &bytes[start..end],
                    ambiguous: false,
                });
            }
        }
    }
    let start = two_bytes;
    // Flate says for itself where it ends, and then no guess is needed, whatever follows it.
    if first_filter_is_flate(&dict)
        && let Some(len) = flate_length(&bytes[start..])
        && let Some(after) = ei_after(bytes, start + len)
    {
        tokens.pos = after;
        return Ok(ReadInline {
            dict,
            data: &bytes[start..start + len],
            ambiguous: false,
        });
    }
    let found = find_ei(bytes, start).ok_or(InlineError::NoEnd)?;
    tokens.pos = found.after;
    Ok(ReadInline {
        dict,
        data: &bytes[start..found.end],
        ambiguous: found.ambiguous,
    })
}

fn first_filter_is_flate(dict: &Dictionary) -> bool {
    let is_flate = |o: &Object| matches!(o, Object::Name(n) if n == b"Fl" || n == b"FlateDecode");
    match dict.get(b"Filter").or_else(|_| dict.get(b"F")) {
        Ok(Object::Array(items)) => items.first().is_some_and(is_flate),
        Ok(other) => is_flate(other),
        Err(_) => false,
    }
}

/// How many bytes one complete zlib stream takes; `None` when it does not complete.
fn flate_length(data: &[u8]) -> Option<usize> {
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut buffer = [0u8; 1 << 14];
    let mut produced = 0u64;
    loop {
        match std::io::Read::read(&mut decoder, &mut buffer) {
            Ok(0) => return usize::try_from(decoder.total_in()).ok(),
            Ok(n) => {
                produced += n as u64;
                // The same bound the filters keep, against a bomb in a listing.
                if produced > MAX_SCANNED_BYTES {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
}

/// The data's length when it can be worked out, which is only with no filter.  Using it means raw
/// samples that happen to spell ` EI ` do not end the image early.
fn exact_length(
    doc: &Document,
    dict: &Dictionary,
    resources: Option<&Dictionary>,
) -> Option<usize> {
    let get = |long: &[u8], short: &[u8]| dict.get(long).or_else(|_| dict.get(short)).ok();
    if get(b"Filter", b"F").is_some() {
        return None;
    }
    let width = usize::try_from(get(b"Width", b"W")?.as_i64().ok()?).ok()?;
    let height = usize::try_from(get(b"Height", b"H")?.as_i64().ok()?).ok()?;
    let image_mask = matches!(get(b"ImageMask", b"IM"), Some(Object::Boolean(true)));
    let (bits, components) = if image_mask {
        (1, 1)
    } else {
        let bits = usize::try_from(get(b"BitsPerComponent", b"BPC")?.as_i64().ok()?).ok()?;
        let space = parse_color_space(doc, get(b"ColorSpace", b"CS")?, resources, 0).ok()?;
        (bits, space.components())
    };
    width
        .checked_mul(components)?
        .checked_mul(bits)?
        .div_ceil(8)
        .checked_mul(height)
}

/// How far `ei_after` looks across white space for `EI`, which keeps a hostile run of it from costing
/// a full pass for every image.
const MAX_SPACE_BEFORE_EI: usize = 64;

/// The position past `EI` when only a little white space separates it from `at`.
fn ei_after(bytes: &[u8], at: usize) -> Option<usize> {
    let mut i = at;
    while bytes.get(i).is_some_and(|b| is_whitespace(*b)) {
        i += 1;
        if i - at > MAX_SPACE_BEFORE_EI {
            return None;
        }
    }
    (bytes.get(i..i + 2)? == b"EI" && bytes.get(i + 2).is_none_or(|b| is_whitespace(*b)))
        .then_some(i + 2)
}

struct FoundEi {
    end: usize,
    after: usize,
    ambiguous: bool,
}

/// Searches for the `EI` that ends data of no knowable length: white space, `EI`, white space or the
/// end, and what follows must read as content rather than as more of the data.
fn find_ei(bytes: &[u8], start: usize) -> Option<FoundEi> {
    let mut ambiguous = false;
    let mut i = start;
    while i + 2 <= bytes.len() {
        if &bytes[i..i + 2] == b"EI"
            && (i == start || is_whitespace(bytes[i - 1]))
            && bytes.get(i + 2).is_none_or(|b| is_whitespace(*b))
        {
            let after = i + 2;
            if reads_as_content(&bytes[after..]) {
                let end = if i > start { i - 1 } else { i };
                return Some(FoundEi {
                    end,
                    after,
                    ambiguous,
                });
            }
            ambiguous = true;
        }
        i += 1;
    }
    None
}

/// Whether the next few tokens could be content: numbers, names, strings, and short operators.  A
/// binary string is fine, being a string.  Raw data rarely gets past the second token.
fn reads_as_content(bytes: &[u8]) -> bool {
    let mut tokens = Tokens { bytes, pos: 0 };
    for _ in 0..6 {
        let before = tokens.pos;
        let Some(token) = tokens.next_token() else {
            return tokens.pos >= bytes.len() || tokens.pos == before;
        };
        match token {
            Token::Operator(word) => {
                if word.len() > 3
                    || !word
                        .iter()
                        .all(|b| b.is_ascii_alphabetic() || matches!(b, b'*' | b'\'' | b'"'))
                {
                    return false;
                }
            }
            Token::Other => {
                let raw = &bytes[before..tokens.pos];
                let trimmed = raw.iter().position(|b| !is_whitespace(*b)).unwrap_or(0);
                let first = raw.get(trimmed).copied().unwrap_or(b' ');
                let is_string_or_bracket = matches!(first, b'(' | b'<' | b'>' | b'[' | b']');
                let is_number = raw[trimmed..]
                    .iter()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'));
                let is_keyword = matches!(&raw[trimmed..], b"true" | b"false" | b"null");
                if !is_string_or_bracket && !is_number && !is_keyword {
                    return false;
                }
            }
            Token::Name(_) => {}
        }
    }
    true
}

/// A content stream's operations, and whether lopdf got to the end of them.
pub(crate) struct DecodedContent {
    pub operations: Vec<Operation>,
    pub stopped_early: bool,
    /// Inline images with neither `/CS` nor `/IM`, which `operations` leaves out.
    pub dropped_images: usize,
}

/// What a listing of operators says about `DecodedContent::dropped_images`.
pub(crate) const INLINE_IMAGES_DROPPED: &str =
    "An inline image with neither /CS nor /IM is left out of this listing";

/// An operator no content stream has, put after the content to see whether lopdf gets there.
const END_MARKER: &str = "PdfDumpEndOfContent";

/// `Content::decode`, made safe to call on a stranger's bytes.  It has three faults this works around
/// (`tests/lopdf_canary.rs` pins them):
///
/// - It stops at the first token it cannot parse and returns what it had as `Ok`, so everything after
///   a syntax error is gone with no sign.  So a marker goes after the content, and no marker at the
///   end means it stopped early.  The operators are counted here too, because lopdf ends an inline
///   image at an `EI` the scanner turns down, and fewer from lopdf means it lost some on the way.
///   An image whose end was not found stops the read as well, and so does one blanked below whose
///   end was guessed at: what follows it may be its data or may be content.
/// - It panics on an inline image with neither `/CS` nor `/IM`.  So such an image is blanked out
///   first, and counted, so what follows it survives.
/// - It stops at a comment not followed directly by an operator or operand, and at white space other
///   than space, tab, CR and LF.  So those are blanked out too.
pub(crate) fn decode_content(bytes: &[u8]) -> Result<DecodedContent, lopdf::Error> {
    let doc = Document::new();
    let mut tokens = Tokens { bytes, pos: 0 };
    let mut operators = 0usize;
    let mut blanked = Vec::new();
    let mut dropped_images = 0usize;
    let mut end_unsure = false;
    loop {
        let before = tokens.pos;
        tokens.skip_space();
        if bytes[before..tokens.pos]
            .iter()
            .any(|b| !b" \t\r\n".contains(b))
        {
            blanked.push(before..tokens.pos);
        }
        let Some(token) = tokens.next_token() else {
            break;
        };
        let Token::Operator(op) = token else {
            continue;
        };
        if op != b"BI" {
            operators += 1;
            continue;
        }
        let start = tokens.pos - 2;
        // A dictionary the scanner cannot read may still be one lopdf reads, and panics on, and
        // where its image ends is unknown either way.
        let Ok(dict) = read_inline_dict(&mut tokens) else {
            blanked.push(start..bytes.len());
            end_unsure = true;
            break;
        };
        let panics_lopdf = !dict.has(b"CS")
            && !dict.has(b"ColorSpace")
            && !matches!(
                dict.get(b"IM").or_else(|_| dict.get(b"ImageMask")),
                Ok(Object::Boolean(true))
            );
        match read_inline_data(&doc, &mut tokens, dict, None) {
            Ok(read) => {
                // Only a blanked image's guess can cost anything: lopdf reads the rest itself.
                end_unsure |= read.ambiguous && panics_lopdf;
                if panics_lopdf {
                    blanked.push(start..tokens.pos);
                    dropped_images += 1;
                } else {
                    operators += 1;
                }
            }
            Err(_) => {
                end_unsure = true;
                if panics_lopdf {
                    blanked.push(start..bytes.len());
                } else {
                    operators += 1;
                }
                break;
            }
        }
    }
    let mut marked = bytes.to_vec();
    for range in blanked {
        marked[range].fill(b' ');
    }
    marked.push(b'\n');
    marked.extend_from_slice(END_MARKER.as_bytes());
    marked.push(b'\n');
    let mut operations = Content::decode(&marked)?.operations;
    let reached_the_end = operations
        .last()
        .is_some_and(|op| op.operator == END_MARKER);
    if reached_the_end {
        operations.pop();
    }
    Ok(DecodedContent {
        stopped_early: end_unsure || !reached_the_end || operations.len() < operators,
        operations,
        dropped_images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_export::export_image;
    use lopdf::Stream;
    use pretty_assertions::assert_eq;

    /// One page with `content`, and `objs` added as they stand.
    fn page_doc(
        content: &[u8],
        resources: Option<Dictionary>,
        annots: Vec<Object>,
        objs: Vec<(ObjectId, Object)>,
    ) -> Document {
        let mut doc = Document::new();
        for (id, o) in objs {
            doc.objects.insert(id, o);
        }
        doc.objects.insert(
            (50, 0),
            Object::Stream(Stream::new(Dictionary::new(), content.to_vec())),
        );
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((51, 0)));
        page.set("Contents", Object::Reference((50, 0)));
        if let Some(r) = resources {
            page.set("Resources", Object::Dictionary(r));
        }
        if !annots.is_empty() {
            page.set("Annots", Object::Array(annots));
        }
        doc.objects.insert((52, 0), Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(1));
        pages.set("Kids", Object::Array(vec![Object::Reference((52, 0))]));
        doc.objects.insert((51, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((51, 0)));
        doc.objects.insert((53, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((53, 0)));
        doc
    }

    fn scan_of(content: &[u8]) -> (usize, Vec<Vec<u8>>, Vec<String>) {
        let doc = page_doc(content, None, vec![], vec![]);
        let listed = scan_document(&doc);
        let mut data = Vec::new();
        for index in 1..=listed.images.len() as u32 {
            let mut scan = InlineScan::default();
            scan_page(&doc, 1, (52, 0), Some(index), &mut scan);
            data.push(scan.images[index as usize - 1].data.clone());
        }
        (listed.images.len(), data, listed.warnings)
    }

    /// The first image on the first page, with its data.
    fn kept_first(doc: &Document) -> InlineScan<'_> {
        let mut scan = InlineScan::default();
        scan_page(doc, 1, (52, 0), Some(1), &mut scan);
        scan
    }

    #[test]
    fn decode_content_reports_a_stream_lopdf_did_not_finish() {
        let read = decode_content(b"BT (seen) Tj ET ) >> ] garbage BT (hidden) Tj ET").unwrap();
        assert!(read.stopped_early);
        assert_eq!(read.operations.len(), 3);
        let whole =
            decode_content(b"q 1 0 0 1 0 0 cm /F1 12 Tf BT (a) Tj [(b) -5] TJ T* ET Q").unwrap();
        assert!(!whole.stopped_early);
    }

    #[test]
    fn decode_content_keeps_lopdfs_count_on_filtered_and_named_inline_images() {
        for content in [
            &b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 61 >\nEI BT (x) Tj ET"[..],
            b"BI /W 1 /H 1 /BPC 8 /CS /Cs1 ID a\nEI BT (x) Tj ET",
            b"BI /W 1 /H 1 /BPC 8 /CS /RGB /F /Fl ID xxxxxxxx\nEI BT (x) Tj ET",
        ] {
            let read = decode_content(content).unwrap();
            assert!(
                !read.stopped_early,
                "{:?}",
                String::from_utf8_lossy(content)
            );
            assert_eq!(read.dropped_images, 0);
        }
    }

    #[test]
    fn decode_content_counts_a_dropped_image_and_reads_on() {
        let read = decode_content(b"BI /W 1 /H 1 /BPC 8 ID a\nEI BT (x) Tj ET").unwrap();
        assert_eq!(read.dropped_images, 1);
        assert!(!read.stopped_early);
    }

    #[test]
    fn an_image_whose_end_is_missing_or_guessed_stops_the_read() {
        for content in [
            &b"q BI /W 1 /H 1 /BPC 8 ID a never ends BT (secret) Tj ET"[..],
            b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 61> EI \xFF\xFE BT (secret) Tj ET",
            b"q BI /W 1 /H 1 /IM true /F /AHx ID 61> EI \xFF\xFE BT (secret) Tj ET",
            b"BI /W 1 /H 1 /BPC 8 ID ab\nEI \xFF\xFE BT (secret) Tj ET\nEI Q",
        ] {
            let read = decode_content(content).unwrap();
            assert!(read.stopped_early, "{:?}", String::from_utf8_lossy(content));
        }
    }

    #[test]
    fn a_dictionary_the_scanner_cannot_read_does_not_panic() {
        // lopdf reads a reference as a value where the scanner does not, and then panics.
        let read = decode_content(b"q BI /W 1 /H 1 /BPC 8 /X 1 0 R ID a\nEI BT (x) Tj ET").unwrap();
        assert!(read.stopped_early);
        // A nested dictionary cut short must not leave the scanner past the end.
        let _ = decode_content(b"BI /DP << >");
    }

    #[test]
    fn a_stop_is_found_whatever_the_counts_say() {
        for content in [
            &b"BT /F1 12 Tf 72 700 Td (ok Tj ET\nBT (Lockbox code: 7301) Tj ET\n"[..],
            b"BT <41 Tj ET\nBT (secret) Tj ET\n",
            b"q1q1q1q1q1q1q ) BT (secret) Tj ET",
            b"q1q1q1q1q1q ) BT (secret) Tj ET",
            b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 61> EI q q q q q ) BT (secret) Tj ET\nEI Q",
        ] {
            let read = decode_content(content).unwrap();
            assert!(read.stopped_early, "{:?}", String::from_utf8_lossy(content));
        }
    }

    #[test]
    fn the_marker_is_not_among_the_operations() {
        let read = decode_content(b"q Q % a comment with no newline").unwrap();
        assert!(!read.stopped_early);
        let ops: Vec<&str> = read
            .operations
            .iter()
            .map(|o| o.operator.as_str())
            .collect();
        assert_eq!(ops, ["q", "Q"]);
        // An operand left over at the end goes with the marker, as it would with lopdf alone.
        let read = decode_content(b"q 1 2").unwrap();
        assert!(!read.stopped_early);
        assert_eq!(read.operations.len(), 1);
    }

    #[test]
    fn a_comment_or_odd_white_space_is_no_stop() {
        let read = decode_content(b"% DSBlank\n").unwrap();
        assert!(!read.stopped_early);
        assert!(read.operations.is_empty());
        for content in [
            &b"% a comment\nq Q"[..],
            b"% a comment\n\n  q % another\n\n Q",
            b"q\x0CQ\x00",
            b"q Q % a comment with no newline",
        ] {
            let read = decode_content(content).unwrap();
            assert!(
                !read.stopped_early,
                "{:?}",
                String::from_utf8_lossy(content)
            );
            assert_eq!(read.operations.len(), 2, "{content:?}");
        }
    }

    #[test]
    fn a_sound_image_at_the_end_of_the_stream_reads_clean() {
        for content in [
            &b"BT (x) Tj ET BI /W 1 /H 1 /BPC 8 /CS /Cs1 ID a\nEI"[..],
            b"BT (x) Tj ET BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 61>\nEI",
        ] {
            let read = decode_content(content).unwrap();
            assert!(
                !read.stopped_early,
                "{:?}",
                String::from_utf8_lossy(content)
            );
        }
    }

    #[test]
    fn decode_content_counts_every_form_an_operator_takes() {
        // Nothing here stops lopdf, so none of it may read as stopping early.
        for content in [
            &b"BT (it's) ' (a) \" 0 0 Td T* ET f* B* b*"[..],
            b"/Span <</ActualText (x) /Lang (en) /Y true>> BDC EMC",
            b"[/Pattern] cs /P1 scn <0A0B> Tj % a comment\nq Q",
            b"BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI BT (x) Tj ET",
            b"BI /W 1 /H 1 /IM true ID \x80\nEI q Q",
        ] {
            let read = decode_content(content).unwrap();
            assert!(
                !read.stopped_early,
                "{:?}",
                String::from_utf8_lossy(content)
            );
        }
    }

    #[test]
    fn an_image_lopdf_would_panic_on_is_blanked_and_what_follows_survives() {
        let read = decode_content(b"BI /W 1 /H 1 /BPC 8 ID a\nEI BT (x) Tj ET").unwrap();
        assert!(!read.stopped_early);
        let ops: Vec<&str> = read
            .operations
            .iter()
            .map(|o| o.operator.as_str())
            .collect();
        assert_eq!(ops, vec!["BT", "Tj", "ET"]);
        // With no end, it blanks to the end of the stream, keeping what came before.
        let read = decode_content(b"q BI /W 1 /H 1 /BPC 8 ID \xFF\xFE never ends").unwrap();
        let ops: Vec<&str> = read
            .operations
            .iter()
            .map(|o| o.operator.as_str())
            .collect();
        assert_eq!(ops, vec!["q"]);
    }

    #[test]
    fn a_plain_inline_image_is_found_with_its_data_and_place() {
        let content = b"q 1 0 0 1 0 0 cm BI /W 2 /H 1 /BPC 8 /CS /G ID \x07\xC8\nEI Q";
        let doc = page_doc(content, None, vec![], vec![]);
        let scan = scan_document(&doc);
        assert_eq!(scan.images.len(), 1);
        let image = &scan.images[0];
        assert_eq!((image.page, image.index, image.source), (1, 1, "page"));
        assert_eq!(image.offset, 17);
        assert_eq!(&content[image.offset..image.offset + 2], b"BI");
        assert_eq!(image.size, 2);
        // A listing keeps no data; an extraction keeps the one it asked for.
        assert!(image.data.is_empty());
        let mut kept = InlineScan::default();
        scan_page(&doc, 1, (52, 0), Some(1), &mut kept);
        assert_eq!(kept.images[0].data, vec![0x07, 0xC8]);
        assert!(scan.warnings.is_empty());
    }

    #[test]
    fn raw_samples_that_spell_ei_do_not_end_the_image_early() {
        // Known length: the samples hold " EI " and it must not count.
        let (n, data, warnings) = scan_of(b"BI /W 4 /H 1 /BPC 8 /CS /G ID  EI \nEI Q");
        assert_eq!((n, warnings.len()), (1, 0));
        assert_eq!(data[0], b" EI ".to_vec());
    }

    #[test]
    fn filtered_data_ends_at_the_ei_that_reads_as_content_after_it() {
        // Binary after the first " EI " rules it out; only the real one has content behind it.
        let (n, data, _) =
            scan_of(b"BI /W 1 /H 1 /BPC 8 /CS /G /F /DCT ID ab EI \xFF\xFE\xFD cd\nEI\nQ q Q");
        assert_eq!(n, 1);
        assert_eq!(data[0], b"ab EI \xFF\xFE\xFD cd".to_vec());
    }

    #[test]
    fn crlf_after_id_is_one_separator() {
        let (_, data, _) = scan_of(b"BI /W 1 /H 1 /BPC 8 /CS /G ID\r\na\nEI");
        assert_eq!(data[0], b"a".to_vec());
    }

    #[test]
    fn bi_in_a_string_or_a_comment_is_not_an_image() {
        let (n, _, warnings) = scan_of(b"(BI /W 1 ID x EI) Tj <4249> Tj % BI ID EI\nq Q");
        assert_eq!((n, warnings.len()), (0, 0));
    }

    #[test]
    fn several_images_are_numbered_in_order() {
        let content = b"BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI BI /W 1 /H 1 /BPC 8 /CS /G ID b\nEI";
        let doc = page_doc(content, None, vec![], vec![]);
        let scan = scan_document(&doc);
        let got: Vec<(u32, usize)> = scan.images.iter().map(|i| (i.index, i.offset)).collect();
        assert_eq!(got, vec![(1, 0), (2, 35)]);
    }

    #[test]
    fn an_image_with_no_end_is_a_warning_not_silence() {
        let (n, _, warnings) = scan_of(b"BI /W 1 /H 1 /BPC 8 /CS /G /F /Fl ID \xFF\xFE never ends");
        assert_eq!(n, 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no EI"), "{warnings:?}");
    }

    #[test]
    fn a_malformed_dictionary_is_a_warning_and_the_scan_goes_on() {
        let (n, _, warnings) = scan_of(b"BI 12 ID x\nEI q Q BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(n, 1);
    }

    #[test]
    fn dictionary_values_of_every_kind_are_read() {
        let content =
            b"BI /W 1 /H 1 /BPC 8 /CS [/I /RGB 0 <0A141E>] /DP <</Predictor 1>> /I false /X#41 (a\\)b) ID \x00\nEI";
        let doc = page_doc(content, None, vec![], vec![]);
        let scan = kept_first(&doc);
        assert_eq!(scan.images.len(), 1, "{:?}", scan.warnings);
        let image = &scan.images[0];
        assert_eq!(image.dict.get(b"XA").unwrap().as_str().unwrap(), b"a)b");
        // The palette's hex string reads, so the image exports through it.
        let exported = export_image(&doc, &image.as_source()).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(exported.bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buffer).unwrap();
        assert_eq!(buffer, vec![10, 20, 30]);
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        crate::test_utils::zlib_compress(data)
    }

    #[test]
    fn flate_data_ends_where_flate_says_whatever_follows_it() {
        // EI straight after the data, then a binary string: neither should hide the next image.
        let mut content = b"BI /W 1 /H 1 /BPC 8 /CS /G /F /Fl ID ".to_vec();
        let first = zlib(&[1]);
        content.extend(&first);
        content.extend(b"EI Q BT (\xE9\xE8\xE7) Tj ET ");
        content.extend(b"BI /W 1 /H 1 /BPC 8 /CS /G /F /Fl ID ");
        content.extend(zlib(&[2]));
        content.extend(b"\nEI");
        let (n, data, warnings) = scan_of(&content);
        assert_eq!((n, warnings.len()), (2, 0), "{warnings:?}");
        assert_eq!(data[0], first);
    }

    #[test]
    fn a_binary_string_or_comment_after_ei_does_not_lose_the_ei() {
        let (n, _, warnings) =
            scan_of(b"BI /W 1 /H 1 /BPC 8 /CS /G /F /DCT ID ab\nEI Q (\xE9\xE8) Tj BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI");
        assert_eq!((n, warnings.len()), (2, 0), "{warnings:?}");
        let (n, _, warnings) =
            scan_of(b"BI /W 1 /H 1 /BPC 8 /CS /G /F /DCT ID ab\nEI %\xE9\xE8\nBI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI");
        assert_eq!((n, warnings.len()), (2, 0), "{warnings:?}");
    }

    #[test]
    fn an_end_that_had_to_be_guessed_is_a_warning() {
        let (n, _, warnings) =
            scan_of(b"BI /W 1 /H 1 /BPC 8 /CS /G /F /DCT ID ab EI \xFF\xFE\xFD cd\nEI\nQ q Q");
        assert_eq!(n, 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("guess"), "{warnings:?}");
    }

    #[test]
    fn a_huge_width_does_not_overflow_the_data_length() {
        // The string holds " EI " where a wrapped length would land.
        let (_, _, _) =
            scan_of(b"BI /W 2305843009213693951 /H 8 /BPC 8 /CS /G /X ( EI )ID\nabc\nEI");
    }

    #[test]
    fn many_images_with_no_end_stop_at_the_first_and_finish_at_once() {
        let content = b"BI ID x ".repeat(200_000);
        let started = std::time::Instant::now();
        let (n, _, warnings) = scan_of(&content);
        assert_eq!((n, warnings.len()), (0, 1), "{warnings:?}");
        assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    }

    #[test]
    fn a_lone_cr_then_a_first_sample_of_lf_keeps_the_sample() {
        let (_, data, _) = scan_of(b"BI /W 4 /H 1 /BPC 8 /CS /G ID\r\nabc\nEI");
        assert_eq!(data[0], b"\nabc".to_vec());
    }

    #[test]
    fn content_with_an_indirect_filter_or_a_predictor_is_read() {
        let flate_indirect = |doc: &mut Document, id: ObjectId, data: &[u8]| {
            doc.objects
                .insert((60, 0), Object::Name(b"FlateDecode".to_vec()));
            let mut d = Dictionary::new();
            d.set("Filter", Object::Reference((60, 0)));
            doc.objects
                .insert(id, Object::Stream(Stream::new(d, zlib(data))));
        };
        // The page's own content.
        let mut doc = page_doc(b"", None, vec![], vec![]);
        flate_indirect(&mut doc, (50, 0), IMAGE);
        assert_eq!(scan_document(&doc).images.len(), 1);

        // A form's.
        let mut d = Dictionary::new();
        d.set("Subtype", Object::Name(b"Form".to_vec()));
        d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let mut parms = Dictionary::new();
        parms.set("Predictor", Object::Integer(12));
        parms.set("Columns", Object::Integer(IMAGE.len() as i64));
        d.set("DecodeParms", Object::Dictionary(parms));
        let mut rows = vec![0u8];
        rows.extend(IMAGE);
        let doc = page_doc(
            b"/Fm Do",
            Some(with_entry("XObject", "Fm", (9, 0))),
            vec![],
            vec![((9, 0), Object::Stream(Stream::new(d, zlib(&rows))))],
        );
        assert_eq!(scan_document(&doc).images.len(), 1);
    }

    #[test]
    fn contents_held_in_an_indirect_array_are_read() {
        let mut doc = page_doc(b"", None, vec![], vec![]);
        doc.objects.insert(
            (9, 0),
            Object::Stream(Stream::new(Dictionary::new(), IMAGE.to_vec())),
        );
        doc.objects
            .insert((10, 0), Object::Array(vec![Object::Reference((9, 0))]));
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut((52, 0)) {
            page.set("Contents", Object::Reference((10, 0)));
        }
        let scan = scan_document(&doc);
        assert_eq!(
            (scan.images.len(), scan.warnings.len()),
            (1, 0),
            "{:?}",
            scan.warnings
        );
    }

    #[test]
    fn an_indirect_subtype_is_followed() {
        let mut d = Dictionary::new();
        d.set("Subtype", Object::Reference((8, 0)));
        let doc = page_doc(
            b"/Fm Do",
            Some(with_entry("XObject", "Fm", (9, 0))),
            vec![],
            vec![
                ((8, 0), Object::Name(b"Form".to_vec())),
                ((9, 0), Object::Stream(Stream::new(d, IMAGE.to_vec()))),
            ],
        );
        assert_eq!(scan_document(&doc).images.len(), 1);
    }

    #[test]
    fn a_form_with_inherited_resources_is_walked_under_each_callers() {
        // /Z has no resources and runs /Fm.  Under the page, /Fm is empty; under /P, /Fm holds the image.
        let doc = page_doc(
            b"/Z Do /P Do",
            Some({
                let mut x = Dictionary::new();
                x.set("Z", Object::Reference((9, 0)));
                x.set("P", Object::Reference((10, 0)));
                x.set("Fm", Object::Reference((11, 0)));
                let mut r = Dictionary::new();
                r.set("XObject", Object::Dictionary(x));
                r
            }),
            vec![],
            vec![
                ((9, 0), form(b"/Fm Do", None)),
                (
                    (10, 0),
                    form(b"/Z Do", {
                        let mut x = Dictionary::new();
                        x.set("Z", Object::Reference((9, 0)));
                        x.set("Fm", Object::Reference((12, 0)));
                        let mut r = Dictionary::new();
                        r.set("XObject", Object::Dictionary(x));
                        Some(r)
                    }),
                ),
                ((11, 0), form(b"q Q", None)),
                ((12, 0), form(IMAGE, None)),
            ],
        );
        assert_eq!(scan_document(&doc).images.len(), 1);
    }

    #[test]
    fn a_soft_mask_group_is_walked() {
        let mut soft = Dictionary::new();
        soft.set("G", Object::Reference((9, 0)));
        let mut state = Dictionary::new();
        state.set("SMask", Object::Dictionary(soft));
        let mut states = Dictionary::new();
        states.set("G0", Object::Dictionary(state));
        let mut resources = Dictionary::new();
        resources.set("ExtGState", Object::Dictionary(states));
        let doc = page_doc(
            b"/G0 gs",
            Some(resources),
            vec![],
            vec![((9, 0), form(IMAGE, None))],
        );
        assert_eq!(scan_document(&doc).images.len(), 1);
    }

    #[test]
    fn a_page_the_walk_could_not_classify_is_a_warning() {
        let mut doc = page_doc(IMAGE, None, vec![], vec![]);
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut((52, 0)) {
            page.remove(b"Type");
        }
        let scan = scan_document(&doc);
        assert_eq!(scan.warnings.len(), 1);
        assert!(
            scan.warnings[0].contains("page tree"),
            "{:?}",
            scan.warnings
        );
    }

    #[test]
    fn an_unreadable_appearance_is_a_warning() {
        let mut states = Dictionary::new();
        states.set("On", Object::Integer(1));
        let mut ap = Dictionary::new();
        ap.set("N", Object::Dictionary(states));
        let mut annot = Dictionary::new();
        annot.set("AP", Object::Dictionary(ap));
        annot.set("AS", Object::Name(b"On".to_vec()));
        let doc = page_doc(b"q Q", None, vec![Object::Dictionary(annot)], vec![]);
        assert_eq!(scan_document(&doc).warnings.len(), 1);
    }

    #[test]
    fn a_widget_appearance_with_no_resources_resolves_through_the_forms() {
        let mut ap = Dictionary::new();
        ap.set("N", Object::Reference((9, 0)));
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Widget".to_vec()));
        annot.set("AP", Object::Dictionary(ap));
        let mut doc = page_doc(
            b"q Q",
            None,
            vec![Object::Dictionary(annot)],
            vec![
                ((9, 0), form(b"/Fm Do", None)),
                ((10, 0), form(IMAGE, None)),
            ],
        );
        let mut acroform = Dictionary::new();
        acroform.set(
            "DR",
            Object::Dictionary(with_entry("XObject", "Fm", (10, 0))),
        );
        if let Ok(Object::Dictionary(catalog)) = doc.get_object_mut((53, 0)) {
            catalog.set("AcroForm", Object::Dictionary(acroform));
        }
        assert_eq!(scan_document(&doc).images.len(), 1);
    }

    fn form(content: &[u8], resources: Option<Dictionary>) -> Object {
        let mut d = Dictionary::new();
        d.set("Subtype", Object::Name(b"Form".to_vec()));
        if let Some(r) = resources {
            d.set("Resources", Object::Dictionary(r));
        }
        Object::Stream(Stream::new(d, content.to_vec()))
    }

    fn with_entry(category: &str, name: &str, id: ObjectId) -> Dictionary {
        let mut named = Dictionary::new();
        named.set(name, Object::Reference(id));
        let mut r = Dictionary::new();
        r.set(category, Object::Dictionary(named));
        r
    }

    const IMAGE: &[u8] = b"BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI";

    #[test]
    fn an_image_in_a_form_is_found_once_however_often_it_is_drawn() {
        let doc = page_doc(
            b"/Fm Do /Fm Do",
            Some(with_entry("XObject", "Fm", (9, 0))),
            vec![],
            vec![((9, 0), form(IMAGE, None))],
        );
        let scan = scan_document(&doc);
        assert_eq!(scan.images.len(), 1);
        assert_eq!(scan.images[0].source, "form");
        assert_eq!(scan.images[0].source_object, Some((9, 0)));
    }

    #[test]
    fn a_form_that_invokes_itself_terminates() {
        let doc = page_doc(
            b"/Fm Do",
            Some(with_entry("XObject", "Fm", (9, 0))),
            vec![],
            vec![(
                (9, 0),
                form(b"/Fm Do", Some(with_entry("XObject", "Fm", (9, 0)))),
            )],
        );
        assert!(scan_document(&doc).images.is_empty());
    }

    #[test]
    fn a_form_nobody_invokes_is_not_walked() {
        let doc = page_doc(
            b"q Q",
            Some(with_entry("XObject", "Fm", (9, 0))),
            vec![],
            vec![((9, 0), form(IMAGE, None))],
        );
        assert!(scan_document(&doc).images.is_empty());
    }

    #[test]
    fn a_tiling_pattern_is_walked_when_used() {
        let doc = page_doc(
            b"/Pattern cs /P1 scn 0 0 1 1 re f",
            Some(with_entry("Pattern", "P1", (9, 0))),
            vec![],
            vec![(
                (9, 0),
                Object::Stream(Stream::new(Dictionary::new(), IMAGE.to_vec())),
            )],
        );
        let scan = scan_document(&doc);
        assert_eq!(scan.images.len(), 1);
        assert_eq!(scan.images[0].source, "pattern");
    }

    #[test]
    fn an_annotations_appearance_is_walked() {
        let mut ap = Dictionary::new();
        ap.set("N", Object::Reference((9, 0)));
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Stamp".to_vec()));
        annot.set("AP", Object::Dictionary(ap));
        let doc = page_doc(
            b"q Q",
            None,
            vec![Object::Dictionary(annot)],
            vec![((9, 0), form(IMAGE, None))],
        );
        let scan = scan_document(&doc);
        assert_eq!(scan.images.len(), 1);
        assert_eq!(scan.images[0].source, "appearance");
    }

    #[test]
    fn a_form_that_will_not_decode_is_a_warning() {
        let mut d = Dictionary::new();
        d.set("Subtype", Object::Name(b"Form".to_vec()));
        d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let doc = page_doc(
            b"/Fm Do",
            Some(with_entry("XObject", "Fm", (9, 0))),
            vec![],
            vec![((9, 0), Object::Stream(Stream::new(d, b"nope".to_vec())))],
        );
        let scan = scan_document(&doc);
        assert_eq!(scan.warnings.len(), 1);
        assert!(scan.warnings[0].contains("object 9"), "{:?}", scan.warnings);
    }

    #[test]
    fn a_named_colour_space_resolves_for_length_and_export() {
        let mut spaces = Dictionary::new();
        spaces.set("Cs1", Object::Name(b"DeviceRGB".to_vec()));
        let mut resources = Dictionary::new();
        resources.set("ColorSpace", Object::Dictionary(spaces));
        // Three raw samples, the middle one spelling nothing special; the length comes from /Cs1.
        let doc = page_doc(
            b"BI /W 1 /H 1 /BPC 8 /CS /Cs1 ID abc\nEI",
            Some(resources),
            vec![],
            vec![],
        );
        let scan = kept_first(&doc);
        assert_eq!(scan.images[0].data, b"abc".to_vec());
        assert!(export_image(&doc, &scan.images[0].as_source()).is_ok());
    }
}
