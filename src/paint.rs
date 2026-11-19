//! What a page draws, read from its operators alone (plan-0012, implemented in v0.37.0).
//!
//! A page whose words are filled paths (outlined text) or a scanned image shows no
//! text, so `--text` prints nothing for it, and no verdict can say why.  This module
//! answers the narrower question a caller can act on: does the page show any text,
//! and does it draw anything else?  `--text` lists a page that draws but shows no
//! text (`pages_without_text` in `--json`, a note on stderr), and `--require-text`
//! makes such a page exit 1.  It reads operators only, never a string’s content, and
//! recurses into form XObjects with the extractors’ cycle guard and depth cap.
//!
//! A page that draws one image, once, that a page with text draws too (a logo on every
//! page) is a repeat (plan-0015, implemented in v0.39.0): it draws nothing a page with
//! text does not, so `--require-text` passes it, and it is still listed, with
//! `repeated_images: true`.  Two images are the same only when everything that decides
//! what they show matches exactly: the data, and the dictionary with every object it
//! refers to (masks, colour spaces).  The page with text need only draw the image, not
//! show it visibly: a PDF built to fool the check can (a ruling of 18-Nov-2026, which
//! kept the check to images).

use std::collections::HashSet;

use lopdf::content::Operation;
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde_json::{Value, json};

use crate::helpers;
use crate::inline_image::decode_content_in;
use crate::text::{MAX_FORM_DEPTH, resolve_xobject_id};

/// Operators that fill or stroke a path, or paint a shading.
const PATH_PAINTING: [&str; 10] = ["S", "s", "f", "F", "f*", "B", "B*", "b", "b*", "sh"];

/// What one page draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PagePaint {
    /// A text-showing operator with a non-empty string.
    pub text: bool,
    /// A path filled or stroked, or a shading painted.
    pub paths: bool,
    /// An image XObject or an inline image.
    pub images: bool,
    /// Some content (the page’s, or a form’s) could not be read to its end.
    pub unread: bool,
    /// Each image drawn, as `identity` writes it, so that equal keys are the same
    /// image.  An inline image not read to its end has none, and makes the page `unread`.
    pub image_keys: HashSet<Vec<u8>>,
    /// How many times an image is drawn, through forms too: copies of one image, or
    /// several images, can be laid out into letters, so a repeat draws one.
    pub image_draws: u32,
    /// Something the page shows cannot be matched exactly against another page: an
    /// operator outside `REPEAT_SAFE` (a clip other than one rectangle or convex
    /// four-sided path, a pattern colour), a `gs` that could change an image, resources
    /// that redefine the device spaces or cannot be read, an image whose identity could
    /// not be written, a `Do` that names nothing readable or a form already being drawn,
    /// an annotation a viewer may draw, or an inline image the reader did not key (its
    /// end was guessed, or only lopdf found it).
    pub unmatchable: bool,
    /// Set by `mark_repeats`: the page draws one image, once, that a page with text draws.
    pub repeated: bool,
}

impl PagePaint {
    /// The page shows no text, yet draws something or could not be read in full:
    /// whatever words it holds are not in the extracted text.  A blank page is not.
    pub(crate) fn lacks_text(&self) -> bool {
        !self.text && (self.paths || self.images || self.unread)
    }

    /// One image, drawn once, is all the page draws, and it is in `shown`.
    fn repeats(&self, shown: &HashSet<Vec<u8>>) -> bool {
        self.could_repeat() && self.image_keys.is_subset(shown)
    }

    /// One image, drawn once, is all the page draws, and it can be matched: the page
    /// repeats any page that draws it.
    fn could_repeat(&self) -> bool {
        !self.text
            && self.images
            && !self.paths
            && !self.unread
            && !self.unmatchable
            && self.image_draws == 1
            && self.image_keys.len() == 1
    }

    /// What the page draws, for a message: “paths and images”, never content.
    pub(crate) fn describe(&self) -> String {
        let drawn: Vec<&str> = [(self.paths, "paths"), (self.images, "images")]
            .into_iter()
            .filter_map(|(yes, what)| yes.then_some(what))
            .collect();
        match (drawn.is_empty(), self.unread) {
            (false, false) => format!("draws {} but shows no text", drawn.join(" and ")),
            (false, true) => format!(
                "draws {} but shows no text, and its content could not be read to its end",
                drawn.join(" and ")
            ),
            _ => "shows no text, and its content could not be read to its end".to_string(),
        }
    }
}

/// Read what page `page_id` draws, through every form it invokes.
pub(crate) fn page_paint(doc: &Document, page_id: ObjectId) -> PagePaint {
    let mut paint = PagePaint::default();
    let resources = crate::resources::resolve_page_resources(doc, page_id);
    // The resources a viewer draws with, which can differ from what the extractors read
    // when the page's own entry is null or a reference to nothing.
    match page_resources(doc, page_id) {
        Ok(own) => {
            paint.unmatchable |= own.map(std::ptr::from_ref) != resources.map(std::ptr::from_ref)
        }
        Err(()) => paint.unmatchable = true,
    }
    paint.unmatchable |= annotations_may_draw(doc, page_id);
    if let Some(data) = helpers::read_content_streams(doc, page_id) {
        paint.unread |= data.unread;
        walk(
            doc,
            &data.bytes,
            resources,
            &mut paint,
            &mut HashSet::new(),
            0,
        );
    }
    paint
}

/// Whether the page carries an annotation a viewer may draw outside the page’s content:
/// one with an appearance, or of any kind but a link or a pop-up, since a viewer builds
/// an appearance for a free-text box or a form field that has none (or `/Annots` cannot
/// be read, which might hide one).
fn annotations_may_draw(doc: &Document, page_id: ObjectId) -> bool {
    let Ok(page) = doc.get_dictionary(page_id) else {
        return true;
    };
    // A null, or a reference to nothing, is no entry (§7.3.10).
    let Some(annots) = helpers::own_entry(doc, page, b"Annots") else {
        return false;
    };
    let Ok((_, Object::Array(annots))) = doc.dereference(annots) else {
        return true;
    };
    annots.iter().any(|annot| match doc.dereference(annot) {
        Ok((_, Object::Dictionary(annot))) => {
            helpers::own_entry(doc, annot, b"AP").is_some()
                || !matches!(annot.get(b"Subtype").and_then(|k| doc.dereference(k)),
                    Ok((_, Object::Name(kind))) if kind == b"Link" || kind == b"Popup")
        }
        Ok((_, Object::Null)) | Err(lopdf::Error::ObjectNotFound(_)) => false,
        _ => true,
    })
}

/// The page's resources as a viewer finds them: its own `/Resources`, or its nearest
/// ancestor's when it has none (a null, or a reference to nothing, is none, §7.3.10).
/// `Err` when the entry found is not a dictionary, or the page tree cannot be walked.
fn page_resources(doc: &Document, page_id: ObjectId) -> Result<Option<&Dictionary>, ()> {
    let mut node = page_id;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(node) {
            return Err(());
        }
        let Ok(dict) = doc.get_dictionary(node) else {
            return Err(());
        };
        if let Some(resources) = helpers::own_entry(doc, dict, b"Resources") {
            return helpers::resolve_dict(doc, resources).map(Some).ok_or(());
        }
        match helpers::own_entry(doc, dict, b"Parent") {
            None => return Ok(None),
            Some(parent) => node = parent.as_reference().map_err(|_| ())?,
        }
    }
}

/// How many objects one image’s identity may take in: past this it has none.
const MAX_IDENTITY_OBJECTS: usize = 1024;

/// Colour spaces a bare name stands for on its own; any other name is a resource a
/// viewer looks up on the page.
const DEVICE_SPACES: [&[u8]; 3] = [b"DeviceGray", b"DeviceRGB", b"DeviceCMYK"];

/// The same, abbreviated, as only an inline image may write them (with `I` for
/// `Indexed`).
const INLINE_DEVICE_SPACES: [&[u8]; 3] = [b"G", b"RGB", b"CMYK"];

/// Whether `name` stands for a device space by itself, in an inline image or not.
fn device_space(name: &[u8], inline: bool) -> bool {
    DEVICE_SPACES.contains(&name) || inline && INLINE_DEVICE_SPACES.contains(&name)
}

/// Whether `space` says everything about itself: a device space, or an array whose
/// base and alternate spaces do, with no resource name anywhere a viewer would look
/// one up.  Such a name could mean another colour space on each page, so an image in
/// a space that is not self-contained has no key; so does a pattern space.  `inline`
/// admits the abbreviations only an inline image may use.
fn self_contained_space(doc: &Document, space: &Object, inline: bool, depth: u32) -> bool {
    if depth > 8 {
        return false;
    }
    match doc.dereference(space) {
        Ok((_, Object::Name(name))) => device_space(name, inline),
        Ok((_, Object::Array(items))) => {
            let nested = |at: usize| {
                items
                    .get(at)
                    .is_some_and(|inner| self_contained_space(doc, inner, inline, depth + 1))
            };
            let Some((_, Object::Name(family))) =
                items.first().and_then(|f| doc.dereference(f).ok())
            else {
                return false;
            };
            match family.as_slice() {
                b"Indexed" => nested(1),
                b"I" => inline && nested(1),
                b"Separation" => nested(2),
                // Attributes can hold more spaces (colorants, a process space).
                b"DeviceN" => items.len() == 4 && nested(2),
                b"CalGray" | b"CalRGB" | b"Lab" => true,
                // A profile's /Alternate is a colour space too.
                b"ICCBased" => match items.get(1).and_then(|p| doc.dereference(p).ok()) {
                    Some((_, Object::Stream(profile))) => match profile.dict.get(b"Alternate") {
                        Ok(alternate) => self_contained_space(doc, alternate, inline, depth + 1),
                        Err(_) => true,
                    },
                    _ => false,
                },
                family => items.len() == 1 && DEVICE_SPACES.contains(&family),
            }
        }
        _ => false,
    }
}

/// Whether `resources` replace the device spaces for what they draw (§8.6.5.6), so
/// that one image looks different under them.
fn redefines_device_spaces(doc: &Document, resources: Option<&Dictionary>) -> bool {
    // A null, or a reference to nothing, is no entry (§7.3.10), at either level.
    resources
        .and_then(|r| helpers::own_entry(doc, r, b"ColorSpace"))
        .is_some_and(|spaces| match helpers::resolve_dict(doc, spaces) {
            Some(spaces) => [&b"DefaultGray"[..], b"DefaultRGB", b"DefaultCMYK"]
                .iter()
                .any(|key| helpers::own_entry(doc, spaces, key).is_some()),
            None => true,
        })
}

/// Append `obj` to `out` with every reference resolved, so that two objects that write
/// the same bytes show the same thing.  A stream’s `/Length` is left out, since its
/// data is written whole.  `None` when a reference does not resolve or the walk
/// passes `budget` objects (a cycle does).
fn identity(doc: &Document, obj: &Object, out: &mut Vec<u8>, budget: &mut usize) -> Option<()> {
    *budget = budget.checked_sub(1)?;
    match obj {
        Object::Null => out.push(b'n'),
        Object::Boolean(b) => out.extend([b'b', u8::from(*b)]),
        Object::Integer(i) => tagged(out, b'i', &i.to_le_bytes()),
        Object::Real(r) => tagged(out, b'r', &r.to_le_bytes()),
        Object::Name(n) => tagged(out, b'N', n),
        Object::String(bytes, _) => tagged(out, b's', bytes),
        Object::Array(items) => {
            tagged(out, b'a', &items.len().to_le_bytes());
            for item in items {
                identity(doc, item, out, budget)?;
            }
        }
        Object::Dictionary(dict) => dict_identity(doc, dict, &[], out, budget)?,
        Object::Stream(stream) => {
            dict_identity(doc, &stream.dict, &[b"Length"], out, budget)?;
            tagged(out, b'S', &stream.content);
        }
        Object::Reference(id) => identity(doc, doc.get_object(*id).ok()?, out, budget)?,
    }
    Some(())
}

/// `bytes`, after `tag` and their length, so that no two sequences run together.
fn tagged(out: &mut Vec<u8>, tag: u8, bytes: &[u8]) {
    out.push(tag);
    out.extend_from_slice(&bytes.len().to_le_bytes());
    out.extend_from_slice(bytes);
}

/// A dictionary’s entries in key order, without those in `skip` (a stream’s `/Length`,
/// since its data is written whole).
fn dict_identity(
    doc: &Document,
    dict: &Dictionary,
    skip: &[&[u8]],
    out: &mut Vec<u8>,
    budget: &mut usize,
) -> Option<()> {
    let mut entries: Vec<_> = dict
        .iter()
        .filter(|(key, _)| !skip.contains(&key.as_slice()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    tagged(out, b'd', &entries.len().to_le_bytes());
    for (key, value) in entries {
        tagged(out, b'k', key);
        identity(doc, value, out, budget)?;
    }
    Some(())
}

/// Entries of an image XObject that cannot change what it shows, and that a copy
/// embedded on each page may vary: left out of its key.
const IMAGE_LABELS: &[&[u8]] = &[b"Length", b"Name", b"StructParent", b"ID", b"Metadata"];

/// An image XObject’s key: the stream, with what it refers to.
fn xobject_key(doc: &Document, id: ObjectId) -> Option<Vec<u8>> {
    let Ok(Object::Stream(image)) = doc.get_object(id) else {
        return None;
    };
    if let Ok(space) = image.dict.get(b"ColorSpace")
        && !self_contained_space(doc, space, false, 0)
    {
        return None;
    }
    // Alternate images are more images, which a viewer may show instead; a soft mask is
    // an image too, and its colour space must say everything about itself as well.
    if helpers::own_entry(doc, &image.dict, b"Alternates").is_some() {
        return None;
    }
    if let Some(mask) = helpers::own_entry(doc, &image.dict, b"SMask")
        && let Ok((_, Object::Stream(mask))) = doc.dereference(mask)
        && let Ok(space) = mask.dict.get(b"ColorSpace")
        && !self_contained_space(doc, space, false, 0)
    {
        return None;
    }
    let mut out = Vec::new();
    let mut budget = MAX_IDENTITY_OBJECTS;
    dict_identity(doc, &image.dict, IMAGE_LABELS, &mut out, &mut budget)?;
    tagged(&mut out, b'S', &image.content);
    Some(out)
}

/// The inline-image keys that have an abbreviation (§8.9.7, Table 91).
const INLINE_ABBREVIATIONS: [(&[u8], &[u8]); 9] = [
    (b"BPC", b"BitsPerComponent"),
    (b"CS", b"ColorSpace"),
    (b"D", b"Decode"),
    (b"DP", b"DecodeParms"),
    (b"F", b"Filter"),
    (b"H", b"Height"),
    (b"IM", b"ImageMask"),
    (b"I", b"Interpolate"),
    (b"W", b"Width"),
];

/// An inline image’s key: its dictionary and data, and the resource behind the colour
/// space it names, which differs between pages with different resources.  The space
/// must be self-contained, once that one name is looked up, and the data exactly as
/// long as the image needs: the reader finds an image's end by searching for `EI`
/// when it cannot work out the length without the page's resources, and a search can
/// stop early.  A filtered image's length cannot be worked out, so it has no key.
fn inline_key(
    doc: &Document,
    dict: &Dictionary,
    data: &[u8],
    resources: Option<&Dictionary>,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    // Both forms of one key could be read either way by different viewers.
    if INLINE_ABBREVIATIONS
        .iter()
        .any(|(short, long)| dict.has(short) && dict.has(long))
    {
        return None;
    }
    if crate::inline_image::exact_length(doc, dict, resources) != Some(data.len()) {
        return None;
    }
    let mut budget = MAX_IDENTITY_OBJECTS;
    dict_identity(doc, dict, &[], &mut out, &mut budget)?;
    tagged(&mut out, b'S', data);
    // A space from the resources is an ordinary object, where the inline abbreviations
    // are names to look up.
    // In the order `exact_length` reads them, so both judge the same space.
    let (space, inline) = match dict.get(b"ColorSpace").or_else(|_| dict.get(b"CS")) {
        Err(_) => return Some(out),
        Ok(Object::Name(name)) if !device_space(name, true) => {
            let space = resources
                .and_then(|r| helpers::resolve_dict(doc, r.get(b"ColorSpace").ok()?))
                .and_then(|spaces| spaces.get(name).ok())?;
            (space, false)
        }
        Ok(space) => (space, true),
    };
    if !self_contained_space(doc, space, inline, 0) {
        return None;
    }
    // A space given in the dictionary is in the key already; a looked-up one is not.
    if !inline {
        identity(doc, space, &mut out, &mut budget)?;
    }
    Some(out)
}

/// Graphics-state entries that cannot change what an image shows, or only hide it:
/// line and stroke settings, rendering intent, overprint, flatness and smoothness,
/// alpha, and the text knockout flag.  Others are allowed only at values that leave an
/// image as it is (`changes_image` checks them): `/BM` Normal, `/SMask` None, the
/// transfer, black-generation, undercolour-removal and halftone entries at `/Default`
/// (or `/Identity` for a transfer), and `/AIS` false, as Distiller and its kin write in
/// nearly every graphics state.  Anything else, such as a transfer function, remaps
/// samples as `/Decode` does.
const PLAIN_STATE_KEYS: [&[u8]; 16] = [
    b"Type", b"LW", b"LC", b"LJ", b"ML", b"D", b"RI", b"OP", b"op", b"OPM", b"FL", b"SM", b"SA",
    b"CA", b"ca", b"TK",
];

/// Whether `gs` with the operand `name` sets anything that could change what an image
/// shows (a soft mask, a blend mode, a transfer function, any entry not known to be
/// plain), or names a state that cannot be read (which might).
fn changes_image(doc: &Document, resources: Option<&Dictionary>, name: Option<&Object>) -> bool {
    let state = name.and_then(|n| n.as_name().ok()).and_then(|name| {
        let states = helpers::resolve_dict(doc, resources?.get(b"ExtGState").ok()?)?;
        helpers::resolve_dict(doc, states.get(name).ok()?)
    });
    let Some(state) = state else {
        return true;
    };
    let named = |value: &Object, allowed: &[&[u8]]| match doc.dereference(value) {
        Ok((_, Object::Name(n))) => allowed.contains(&n.as_slice()),
        _ => false,
    };
    !state.iter().all(|(key, value)| match key.as_slice() {
        b"SMask" => named(value, &[b"None"]),
        // An array lists blend modes for a reader to choose from.
        b"BM" => match doc.dereference(value) {
            Ok((_, Object::Array(modes))) => {
                !modes.is_empty() && modes.iter().all(|m| named(m, &[b"Normal", b"Compatible"]))
            }
            _ => named(value, &[b"Normal", b"Compatible"]),
        },
        b"TR" | b"TR2" => named(value, &[b"Identity", b"Default"]),
        b"BG" | b"BG2" | b"UCR" | b"UCR2" | b"HT" => named(value, &[b"Default"]),
        b"AIS" => matches!(doc.dereference(value), Ok((_, Object::Boolean(false)))),
        key => PLAIN_STATE_KEYS.contains(&key),
    })
}

/// Set `repeated` on each of `pages` (pages that lack text) that draws one image, once,
/// that a page of `doc` with text draws too.  The document is walked only when one could.
pub(crate) fn mark_repeats(doc: &Document, pages: &mut [(u32, PagePaint)]) {
    if !pages.iter().any(|(_, paint)| paint.could_repeat()) {
        return;
    }
    let mut shown = HashSet::new();
    for page_id in doc.get_pages().into_values() {
        let paint = page_paint(doc, page_id);
        if paint.text {
            shown.extend(paint.image_keys);
        }
    }
    for (_, paint) in pages {
        paint.repeated = paint.repeats(&shown);
    }
}

/// Operators that cannot change what a repeated image shows: the graphics state and
/// its colours (checked further by `colour_is_plain`), marked content, and text state
/// and showing (a string that shows anything is text).  On a page that might be a
/// repeat, any other operator but a painting one or a one-shape clip
/// (`Clip`) makes it unmatchable, so an operator not thought of falls on the safe side.
const REPEAT_SAFE: [&str; 49] = [
    "q", "Q", "cm", "Do", "BI", "gs", "BMC", "BDC", "EMC", "MP", "DP", "BX", "EX", "g", "G", "rg",
    "RG", "k", "K", "cs", "CS", "sc", "SC", "scn", "SCN", "w", "J", "j", "M", "d", "ri", "i", "BT",
    "ET", "Tc", "Tw", "Tz", "TL", "Tf", "Tr", "Ts", "Td", "TD", "Tm", "T*", "Tj", "TJ", "'", "\"",
];

/// A path under construction, which may only become a clip of one rectangle (`re`, or
/// a convex four-sided path, which is how some producers write a rectangle): a clip of
/// several shapes, of a concave or crossing one, or of curves, could cut letters out of
/// an image.
#[derive(Clone, Copy, PartialEq)]
enum Clip {
    None,
    Rect,
    /// `m` and the `l`s after it: the corners so far, and how many.
    Sides([(f64, f64); 4], usize),
    Clipped,
}

impl Clip {
    /// Whether a path is under construction, between its first operator and `n`; only
    /// path operators may come then.
    fn is_open(&self) -> bool {
        *self != Clip::None
    }

    /// Take a path operator; false when the path is more than one rectangle or
    /// convex four-sided path, or `op` is not a path operator at all.
    fn take(&mut self, op: &Operation) -> bool {
        let point = || match op.operands.as_slice() {
            [x, y] => Some((x.as_float().ok()?.into(), y.as_float().ok()?.into())),
            _ => None,
        };
        match (op.operator.as_str(), *self) {
            ("re", Clip::None) => *self = Clip::Rect,
            ("m", Clip::None) => match point() {
                Some(p) => *self = Clip::Sides([p; 4], 1),
                None => return false,
            },
            ("l", Clip::Sides(mut corners, n)) if n < 4 => match point() {
                Some(p) => {
                    corners[n] = p;
                    *self = Clip::Sides(corners, n + 1);
                }
                None => return false,
            },
            // A line back to the first corner, which some producers write before `h`.
            ("l", Clip::Sides(corners, 4)) if point() == Some(corners[0]) => {}
            ("h", Clip::Rect | Clip::Sides(_, 4)) => {}
            ("W" | "W*", Clip::Rect) => *self = Clip::Clipped,
            ("W" | "W*", Clip::Sides(corners, 4)) if convex(corners) => {
                *self = Clip::Clipped;
            }
            ("n", _) => *self = Clip::None,
            (op, _) if PATH_PAINTING.contains(&op) => *self = Clip::None,
            _ => return false,
        }
        true
    }
}

/// Whether four corners, in order, bound a convex shape: the path turns the same way,
/// and never straight, at every corner.  The sign of each turn survives any `cm` that
/// can be inverted, so no scaling or stretching makes a concave or crossing shape pass,
/// and no tolerance is needed for a rectangle a producer wrote with rounded corners.
///
/// lopdf holds every number as an `f32`, so the corners here are rounded copies of the
/// ones a viewer draws.  A turn counts only when it is larger than that rounding could
/// make it; one smaller might have the other sign in the file, and is not vouched for.
fn convex(corners: [(f64, f64); 4]) -> bool {
    let largest = corners
        .iter()
        .map(|(x, y)| x.abs().max(y.abs()))
        .fold(0.0, f64::max);
    let unit = 8.0 * f64::from(f32::EPSILON) * largest;
    let turns: Vec<f64> = (0..4)
        .map(|i| {
            let (p, q, r) = (corners[i], corners[(i + 1) % 4], corners[(i + 2) % 4]);
            let (first, second) = ((q.0 - p.0, q.1 - p.1), (r.0 - q.0, r.1 - q.1));
            let turn = first.0 * second.1 - first.1 * second.0;
            let doubt = unit * (first.0.hypot(first.1) + second.0.hypot(second.1));
            if turn.abs() > doubt {
                turn.signum()
            } else {
                0.0
            }
        })
        .collect();
    turns.iter().all(|t| *t > 0.0) || turns.iter().all(|t| *t < 0.0)
}

/// Whether a colour operator sets a plain colour: not a pattern, which paints its own
/// content through a stencil image, and not a colour space that cannot be read.
fn colour_is_plain(doc: &Document, resources: Option<&Dictionary>, op: &Operation) -> bool {
    match op.operator.as_str() {
        "cs" | "CS" => {
            let Some(name) = op.operands.first().and_then(|n| n.as_name().ok()) else {
                return false;
            };
            if device_space(name, false) {
                return true;
            }
            let space = resources
                .and_then(|r| helpers::resolve_dict(doc, r.get(b"ColorSpace").ok()?))
                .and_then(|spaces| spaces.get(name).ok())
                .and_then(|space| doc.dereference(space).ok());
            match space {
                Some((_, Object::Array(items))) => {
                    !matches!(items.first(), Some(Object::Name(family)) if family == b"Pattern")
                }
                Some((_, Object::Name(family))) => family != b"Pattern",
                _ => false,
            }
        }
        "sc" | "SC" | "scn" | "SCN" => !op.operands.iter().any(|o| o.as_name().is_ok()),
        _ => true,
    }
}

fn non_empty_string(obj: &Object) -> bool {
    matches!(obj, Object::String(bytes, _) if !bytes.is_empty())
}

fn walk(
    doc: &Document,
    bytes: &[u8],
    resources: Option<&Dictionary>,
    paint: &mut PagePaint,
    visited: &mut HashSet<ObjectId>,
    depth: u32,
) {
    // With the resources, an inline image's data is read to its worked-out length.
    let content = match decode_content_in(doc, bytes, resources) {
        Ok(content) => content,
        Err(_) => {
            paint.unread = true;
            return;
        }
    };
    paint.unread |= content.stopped_early;
    paint.images |= content.dropped_images > 0;
    for (dict, data) in &content.inline_images {
        paint.images = true;
        paint.image_draws += 1;
        match inline_key(doc, dict, data, resources) {
            Some(key) => {
                paint.image_keys.insert(key);
            }
            None => paint.unmatchable = true,
        }
    }
    paint.unmatchable |= redefines_device_spaces(doc, resources);
    // An inline image lopdf read but the reader did not key: its end was guessed, or
    // only lopdf found it.
    let lopdf_read = content
        .operations
        .iter()
        .filter(|op| op.operator == "BI")
        .count();
    paint.unmatchable |= lopdf_read + content.dropped_images > content.inline_images.len();
    let mut clip = Clip::None;
    for op in &content.operations {
        let name = op.operator.as_str();
        // While a path is open only path operators may come (`cm` there would move the
        // corners still to come).  Otherwise `clip.take` changes nothing for an operator
        // that is not a path operator.
        let plain = REPEAT_SAFE.contains(&name) && colour_is_plain(doc, resources, op);
        let taken = if clip.is_open() {
            clip.take(op)
        } else {
            plain || clip.take(op)
        };
        if !taken {
            paint.unmatchable = true;
        }
        match op.operator.as_str() {
            "Tj" | "'" | "\"" => paint.text |= op.operands.last().is_some_and(non_empty_string),
            "TJ" => {
                paint.text |= matches!(op.operands.first(),
                    Some(Object::Array(items)) if items.iter().any(non_empty_string))
            }
            "BI" => paint.images = true,
            "gs" => paint.unmatchable |= changes_image(doc, resources, op.operands.first()),
            "Do" => {
                // A name that resolves to nothing readable might be anything.
                let id = op
                    .operands
                    .first()
                    .and_then(|n| std::str::from_utf8(n.as_name().ok()?).ok())
                    .and_then(|name| resolve_xobject_id(doc, resources?, name));
                match id {
                    Some(id) => xobject(doc, id, resources, paint, visited, depth),
                    None => paint.unmatchable = true,
                }
            }
            op if PATH_PAINTING.contains(&op) => paint.paths = true,
            _ => {}
        }
    }
    // A path left open when the content ends: what a viewer makes of it is not known.
    paint.unmatchable |= clip.is_open();
}

/// An image XObject is an image; a form is walked, inside its own `/Resources`
/// (or the caller’s, §7.8.3).
fn xobject(
    doc: &Document,
    id: ObjectId,
    parent_resources: Option<&Dictionary>,
    paint: &mut PagePaint,
    visited: &mut HashSet<ObjectId>,
    depth: u32,
) {
    let Ok(Object::Stream(stream)) = doc.get_object(id) else {
        paint.unmatchable = true;
        return;
    };
    match stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|s| s.as_name().ok())
    {
        Some(b"Image") => {
            paint.images = true;
            paint.image_draws += 1;
            match xobject_key(doc, id) {
                Some(key) => {
                    paint.image_keys.insert(key);
                }
                None => paint.unmatchable = true,
            }
        }
        Some(b"Form") => {
            // A form nested past the cap, or one already on the recursion stack,
            // is not read, as in the extractors; past the cap that is unread.
            if depth + 1 > MAX_FORM_DEPTH {
                paint.unread = true;
                return;
            }
            if !visited.insert(id) {
                // A viewer with a depth limit instead draws it again, and again.
                paint.unmatchable = true;
                return;
            }
            let (decoded, warning) = crate::stream::decode_stream(doc, stream);
            paint.unread |= warning.is_some();
            // A null or a reference to nothing is no entry (§7.3.10).
            let resources = match helpers::own_entry(doc, &stream.dict, b"Resources") {
                None => parent_resources,
                Some(own) => {
                    let own = helpers::resolve_dict(doc, own);
                    // Resources the form names but that cannot be read are unknown.
                    paint.unmatchable |= own.is_none();
                    own.or(parent_resources)
                }
            };
            walk(doc, &decoded, resources, paint, visited, depth + 1);
            visited.remove(&id);
        }
        // A PostScript XObject, or one with no /Subtype.
        _ => paint.unmatchable = true,
    }
}

/// The `--json` entry for one page that lacks text.
pub(crate) fn page_json(page_number: u32, paint: &PagePaint) -> Value {
    json!({
        "page_number": page_number,
        "paths": paint.paths,
        "images": paint.images,
        "unread": paint.unread,
        "repeated_images": paint.repeated,
    })
}

/// The stderr note for the pages that lack text.  Silent when there are none.
pub(crate) fn print_note(pages: &[(u32, PagePaint)]) {
    for (page_number, paint) in pages {
        if paint.repeated {
            eprintln!(
                "Note: page {page_number} draws only an image that a page with text draws too \
                 (--require-text passes it)"
            );
            continue;
        }
        eprintln!(
            "Note: page {page_number} {}: words drawn as paths or in an image are not in this \
             text (--require-text makes this exit 1)",
            paint.describe()
        );
    }
}

/// `paint` for a page that showed `shown_codes` codes: a page that showed any has
/// text, and is not walked again.
pub(crate) fn lacking_text(
    doc: &Document,
    page_id: ObjectId,
    shown_codes: u64,
) -> Option<PagePaint> {
    if shown_codes > 0 {
        return None;
    }
    Some(page_paint(doc, page_id)).filter(PagePaint::lacks_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::build_page_doc_with_content;
    use lopdf::Stream;

    /// Page `(2, 0)` drawing `content`, with `/Im0` a page-sized image and `/Fm0` a
    /// form drawing `form` (inside its own empty `/Resources`).
    fn paint_of(content: &[u8], form: &[u8]) -> PagePaint {
        let mut doc = build_page_doc_with_content(content);
        doc.max_id = 4; // the builder inserts objects 1–4 without raising it
        let mut image = Dictionary::new();
        image.set("Type", Object::Name(b"XObject".to_vec()));
        image.set("Subtype", Object::Name(b"Image".to_vec()));
        image.set("Width", 1);
        image.set("Height", 1);
        image.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        image.set("BitsPerComponent", 8);
        let image_id = doc.add_object(Object::Stream(Stream::new(image, vec![0])));
        let mut form_dict = Dictionary::new();
        form_dict.set("Type", Object::Name(b"XObject".to_vec()));
        form_dict.set("Subtype", Object::Name(b"Form".to_vec()));
        form_dict.set("Resources", Dictionary::new());
        let form_id = doc.add_object(Object::Stream(Stream::new(form_dict, form.to_vec())));
        let mut xobjects = Dictionary::new();
        xobjects.set("Im0", Object::Reference(image_id));
        xobjects.set("Fm0", Object::Reference(form_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", xobjects);
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut((2, 0)) {
            page.set("Resources", resources);
        }
        page_paint(&doc, (2, 0))
    }

    /// The letters of a word, drawn as filled outlines and no font anywhere.
    const OUTLINED: &[u8] =
        b"0 0 0 rg 72 700 m 80 700 l 76 712 l h f 84 700 m 90 712 l 96 700 l h f*";

    #[test]
    fn outlined_words_draw_paths_and_show_no_text() {
        let paint = paint_of(OUTLINED, b"");
        assert!(paint.paths && !paint.text && !paint.images && !paint.unread);
        assert!(paint.lacks_text());
        assert_eq!(paint.describe(), "draws paths but shows no text");
    }

    #[test]
    fn a_page_sized_image_alone_lacks_text() {
        let paint = paint_of(b"q 612 0 0 792 0 0 cm /Im0 Do Q", b"");
        assert!(paint.images && !paint.paths && !paint.text);
        assert!(paint.lacks_text());
    }

    #[test]
    fn an_inline_image_alone_lacks_text() {
        let paint = paint_of(
            b"q 100 0 0 100 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI Q",
            b"",
        );
        assert!(paint.images && !paint.text, "{paint:?}");
    }

    #[test]
    fn a_blank_page_does_not_lack_text() {
        for content in [&b""[..], b"q 1 0 0 1 0 0 cm 0 0 m 10 10 l n Q"] {
            let paint = paint_of(content, b"");
            let drawn = (paint.text, paint.paths, paint.images, paint.unread);
            assert_eq!(drawn, (false, false, false, false), "{content:?}");
            assert!(!paint.lacks_text());
        }
    }

    #[test]
    fn text_beside_paths_and_images_is_text() {
        let paint = paint_of(b"0 0 m 100 0 l S /Im0 Do BT /F1 10 Tf (Hi) Tj ET", b"");
        assert!(paint.text && paint.paths && paint.images);
        assert!(!paint.lacks_text());
        for show in [
            &b"BT [(a) 120 (b)] TJ ET"[..],
            b"BT (a) ' ET",
            b"BT 1 2 (a) \" ET",
        ] {
            assert!(paint_of(show, b"").text, "{show:?}");
        }
    }

    #[test]
    fn empty_strings_are_not_text() {
        let paint = paint_of(b"BT () Tj [() -200 ()] TJ ET 0 0 m 1 1 l S", b"");
        assert!(!paint.text && paint.lacks_text());
    }

    #[test]
    fn a_form_is_read_for_text_and_paint() {
        assert!(paint_of(b"/Fm0 Do", b"BT (Hi) Tj ET").text);
        let paint = paint_of(b"/Fm0 Do", OUTLINED);
        assert!(paint.paths && paint.lacks_text());
    }

    #[test]
    fn images_are_keyed_by_their_data() {
        let xobject = paint_of(b"/Im0 Do", b"");
        assert_eq!(xobject.image_keys.len(), 1);
        assert_eq!(
            paint_of(b"/Im0 Do /Im0 Do", b"").image_keys,
            xobject.image_keys
        );
        let inline = |dict: &str, data: &str| {
            paint_of(
                format!("q BI /W 1 /H 1 /BPC 8 {dict} ID {data}\nEI Q").as_bytes(),
                b"",
            )
            .image_keys
        };
        // `/G` is a colour space lopdf does not know: the data is keyed anyway.
        for dict in ["/CS /G", "/CS /DeviceGray", "/IM true"] {
            assert_eq!(inline(dict, "a").len(), 1, "{dict}");
            assert_ne!(inline(dict, "a"), inline(dict, "b"), "{dict}");
        }
        // With neither /CS nor /IM the image is blanked before lopdf reads it, and its
        // length is unknown, as it is for a filtered one or one whose data is too long;
        // one that gives a key twice, in both forms, could be read either way.
        for image in [
            &b"q BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /DeviceGray ID a\nEI Q"[..],
            b"q BI /W 1 /H 1 /BPC 8 /CS /G /CS /G ID a\nEI Q",
            b"q BI /W 1 /Width 1 /H 1 /BPC 8 /CS /G ID a\nEI Q",
            &b"q BI /W 1 /H 1 /BPC 8 ID a\nEI Q"[..],
            b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 61>\nEI Q",
            b"q BI /W 1 /H 1 /BPC 8 /CS /G ID ab\nEI Q",
        ] {
            let paint = paint_of(image, b"");
            assert!(paint.images && paint.unmatchable, "{paint:?}");
            assert!(paint.image_keys.is_empty(), "{paint:?}");
        }
        assert_eq!(inline("/CS /G", "a"), inline("/CS /G", "a"));
        assert_ne!(inline("/CS /G", "a"), xobject.image_keys);
        // Other dimensions over the same bytes are another image.
        let wide = paint_of(b"q BI /W 2 /H 1 /BPC 4 /CS /G ID a\nEI Q", b"");
        assert_ne!(wide.image_keys, inline("/CS /G", "a"));
        // An inline image with no end is unread, so no key can vouch for it.
        let cut = paint_of(b"q BI /W 1 /H 1 /BPC 8 /CS /G ID a", b"");
        assert!(cut.unread && cut.image_keys.is_empty(), "{cut:?}");
    }

    #[test]
    fn a_repeat_draws_only_images_already_shown() {
        let logo = paint_of(b"/Im0 Do", b"");
        let shown = logo.image_keys.clone();
        assert!(logo.repeats(&shown));
        assert!(!logo.repeats(&HashSet::new()), "nothing shown with text");
        for other in [
            &b"/Im0 Do 0 0 m 1 1 l S"[..],
            b"/Im0 Do BT (unterminated",
            b"/Im0 Do q BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI Q",
            b"/Im0 Do q BI /W 1 /H 1 /BPC 8 /CS /G ID a",
        ] {
            let paint = paint_of(other, b"");
            assert!(paint.lacks_text() && !paint.repeats(&shown), "{paint:?}");
        }
        // No key, or an image without one, vouches for nothing.
        let keyless = PagePaint {
            images: true,
            ..PagePaint::default()
        };
        assert!(!keyless.repeats(&shown));
        let unmatchable = PagePaint {
            unmatchable: true,
            ..logo.clone()
        };
        assert!(!unmatchable.repeats(&shown));
        let inline = paint_of(b"q BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI Q", b"");
        assert!(inline.repeats(&inline.image_keys.clone()));
        assert!(
            !paint_of(b"", b"").repeats(&shown),
            "a blank page is no repeat"
        );
    }

    #[test]
    fn an_identity_resolves_references_and_counts_every_entry() {
        let mut doc = build_page_doc_with_content(b"");
        doc.max_id = 4;
        let image = |doc: &mut Document, extra: Vec<(&str, Object)>| {
            let mut dict = Dictionary::new();
            dict.set("Subtype", Object::Name(b"Image".to_vec()));
            dict.set("BitsPerComponent", 8);
            for (key, value) in extra {
                dict.set(key, value);
            }
            let id = doc.add_object(Object::Stream(Stream::new(dict, vec![0; 8])));
            xobject_key(doc, id)
        };
        let eight = doc.add_object(Object::Integer(8));
        let four = doc.add_object(Object::Integer(4));
        let (one, two) = (doc.add_object(1), doc.add_object(2));
        let wide = image(
            &mut doc,
            vec![
                ("Width", Object::Reference(eight)),
                ("Height", Object::Reference(one)),
            ],
        );
        let tall = image(
            &mut doc,
            vec![
                ("Width", Object::Reference(four)),
                ("Height", Object::Reference(two)),
            ],
        );
        assert!(wide.is_some());
        assert_ne!(wide, tall, "an indirect size is read, not lost");
        let plain = image(&mut doc, vec![("Width", 8.into()), ("Height", 1.into())]);
        assert_eq!(wide, plain, "a reference is the object it names");
        let decoded = image(
            &mut doc,
            vec![
                ("Width", 8.into()),
                ("Height", 1.into()),
                ("Decode", vec![1.into(), 0.into()].into()),
            ],
        );
        assert_ne!(plain, decoded);
        let named = image(&mut doc, vec![("ColorSpace", Object::Name(b"P0".to_vec()))]);
        assert_eq!(named, None, "a resource name is no colour-space family");
        let family = image(
            &mut doc,
            vec![("ColorSpace", Object::Name(b"DeviceRGB".to_vec()))],
        );
        assert!(family.is_some());
        let dangling = image(&mut doc, vec![("SMask", Object::Reference((99, 0)))]);
        assert_eq!(dangling, None);
        // A cycle runs out of budget rather than forever.
        let looped = doc.new_object_id();
        doc.objects
            .insert(looped, Object::Array(vec![Object::Reference(looped)]));
        assert_eq!(
            image(&mut doc, vec![("Decode", Object::Reference(looped))]),
            None
        );
    }

    #[test]
    fn soft_masks_appearances_and_unread_palettes_are_unmatchable() {
        let with = |resources: Dictionary, content: &[u8], annots: Option<Object>| {
            let mut doc = build_page_doc_with_content(content);
            doc.objects.insert((9, 0), Object::Name(b"Link".to_vec()));
            if let Ok(Object::Dictionary(page)) = doc.get_object_mut((2, 0)) {
                page.set("Resources", resources);
                if let Some(annots) = annots {
                    page.set("Annots", annots);
                }
            }
            page_paint(&doc, (2, 0))
        };
        let mut states = Dictionary::new();
        states.set(
            "On",
            Dictionary::from_iter(vec![("SMask", Object::Dictionary(Dictionary::new()))]),
        );
        states.set(
            "Off",
            Dictionary::from_iter(vec![("SMask", Object::Name(b"None".to_vec()))]),
        );
        states.set("Plain", Dictionary::new());
        let modes = |names: &[&str]| {
            Object::Array(
                names
                    .iter()
                    .map(|n| Object::Name(n.as_bytes().to_vec()))
                    .collect(),
            )
        };
        states.set(
            "Modes",
            Dictionary::from_iter(vec![("BM", modes(&["Normal", "Compatible"]))]),
        );
        states.set(
            "Multiply",
            Dictionary::from_iter(vec![("BM", modes(&["Normal", "Multiply"]))]),
        );
        states.set("NoModes", Dictionary::from_iter(vec![("BM", modes(&[]))]));
        states.set(
            "Distiller",
            Dictionary::from_iter(vec![
                ("BG2", Object::Name(b"Default".to_vec())),
                ("UCR2", Object::Name(b"Default".to_vec())),
                ("TR2", Object::Name(b"Default".to_vec())),
                ("TR", Object::Name(b"Identity".to_vec())),
                ("HT", Object::Name(b"Default".to_vec())),
                ("AIS", Object::Boolean(false)),
                ("OPM", 1.into()),
            ]),
        );
        states.set(
            "Halftone",
            Dictionary::from_iter(vec![("HT", Object::Dictionary(Dictionary::new()))]),
        );
        states.set(
            "Shape",
            Dictionary::from_iter(vec![("AIS", Object::Boolean(true))]),
        );
        // A function in any of the device-dependent entries, or /Identity where only
        // /Default leaves the device alone.
        for key in ["TR2", "BG", "BG2", "UCR", "UCR2"] {
            states.set(
                format!("Fn{key}"),
                Dictionary::from_iter(vec![(key, Object::Dictionary(Dictionary::new()))]),
            );
        }
        for key in ["BG", "BG2", "UCR", "UCR2", "HT"] {
            states.set(
                format!("Id{key}"),
                Dictionary::from_iter(vec![(key, Object::Name(b"Identity".to_vec()))]),
            );
        }
        states.set(
            "Alpha",
            Dictionary::from_iter(vec![
                ("Type", Object::Name(b"ExtGState".to_vec())),
                ("ca", Object::Real(0.5)),
                ("BM", Object::Name(b"Normal".to_vec())),
            ]),
        );
        states.set(
            "Blend",
            Dictionary::from_iter(vec![("BM", Object::Name(b"Difference".to_vec()))]),
        );
        states.set(
            "Transfer",
            Dictionary::from_iter(vec![("TR", Object::Dictionary(Dictionary::new()))]),
        );
        let resources = Dictionary::from_iter(vec![("ExtGState", Object::Dictionary(states))]);
        for (content, unmatchable) in [
            (&b"/On gs"[..], true),
            (b"/Missing gs", true),
            (b"gs", true),
            (b"/Off gs", false),
            (b"/Plain gs", false),
            (b"/Modes gs", false),
            (b"/Multiply gs", true),
            (b"/NoModes gs", true),
            (b"/Distiller gs", false),
            (b"/Halftone gs", true),
            (b"/Shape gs", true),
            (b"/FnTR2 gs", true),
            (b"/FnBG gs", true),
            (b"/FnBG2 gs", true),
            (b"/FnUCR gs", true),
            (b"/FnUCR2 gs", true),
            (b"/IdBG gs", true),
            (b"/IdBG2 gs", true),
            (b"/IdUCR gs", true),
            (b"/IdUCR2 gs", true),
            (b"/IdHT gs", true),
            (b"/Alpha gs", false),
            (b"/Blend gs", true),
            (b"/Transfer gs", true),
        ] {
            let paint = with(resources.clone(), content, None);
            assert_eq!(paint.unmatchable, unmatchable, "{content:?}");
        }
        let annot = |kind: &str, appearance: bool| {
            let mut dict = Dictionary::new();
            dict.set("Subtype", Object::Name(kind.as_bytes().to_vec()));
            if appearance {
                dict.set("AP", Dictionary::new());
            }
            Object::Array(vec![Object::Dictionary(dict)])
        };
        for (annots, unmatchable) in [
            (annot("Link", false), false),
            // An indirect /Subtype is the name it points to (object 9 holds /Link).
            (
                Object::Array(vec![Object::Dictionary(Dictionary::from_iter(vec![(
                    "Subtype",
                    Object::Reference((9, 0)),
                )]))]),
                false,
            ),
            (annot("Popup", false), false),
            (annot("Link", true), true),
            // A viewer builds an appearance for a free-text box that has none.
            (annot("FreeText", false), true),
            (annot("Widget", false), true),
            (
                Object::Array(vec![Object::Dictionary(Dictionary::new())]),
                true,
            ),
            // A reference to nothing, or a null, is no annotation.
            (
                Object::Array(vec![Object::Reference((99, 0)), Object::Null]),
                false,
            ),
            (Object::Null, false),
            (Object::Integer(1), true),
        ] {
            let paint = with(Dictionary::new(), b"", Some(annots.clone()));
            assert_eq!(paint.unmatchable, unmatchable, "{annots:?}");
        }
        let inline = b"BI /W 1 /H 1 /BPC 8 /CS /P ID a\nEI";
        assert!(with(Dictionary::new(), inline, None).unmatchable);
        let spaces = Dictionary::from_iter(vec![("P", Object::Name(b"DeviceGray".to_vec()))]);
        let named = with(
            Dictionary::from_iter(vec![("ColorSpace", Object::Dictionary(spaces))]),
            inline,
            None,
        );
        assert!(
            !named.unmatchable && named.image_keys.len() == 1,
            "{named:?}"
        );
        // A name inside a palette array is a resource as well.
        let palette = b"BI /W 1 /H 1 /BPC 8 /CS [/I /P 0 <00>] ID a\nEI";
        assert!(with(Dictionary::new(), palette, None).unmatchable);
        let device = b"BI /W 1 /H 1 /BPC 8 /CS [/I /G 0 <00>] ID a\nEI";
        assert!(!with(Dictionary::new(), device, None).unmatchable);
        // The same name for two palettes is two images.
        let in_space = |space: &[u8]| {
            let spaces = Dictionary::from_iter(vec![("P", Object::Name(space.to_vec()))]);
            let resources = Dictionary::from_iter(vec![("ColorSpace", Object::Dictionary(spaces))]);
            with(resources, inline, None).image_keys
        };
        assert_eq!(in_space(b"DeviceGray"), in_space(b"DeviceGray"));
        assert_ne!(in_space(b"DeviceGray"), in_space(b"DeviceRGB"));
        // Two palettes with as many components, under one name, are two images too.
        let in_palette = |rgb: &[u8]| {
            let space = Object::Array(vec![
                Object::Name(b"Indexed".to_vec()),
                Object::Name(b"DeviceRGB".to_vec()),
                0.into(),
                Object::String(rgb.to_vec(), lopdf::StringFormat::Hexadecimal),
            ]);
            let spaces = Dictionary::from_iter(vec![("P", space)]);
            let resources = Dictionary::from_iter(vec![("ColorSpace", Object::Dictionary(spaces))]);
            with(resources, inline, None).image_keys
        };
        assert_eq!(in_palette(&[0, 0, 0]).len(), 1);
        assert_ne!(in_palette(&[0, 0, 0]), in_palette(&[255, 0, 0]));
    }

    #[test]
    fn only_drawing_state_that_cannot_change_an_image_is_matchable() {
        let mut spaces = Dictionary::new();
        spaces.set(
            "Spot",
            Object::Array(vec![Object::Name(b"Separation".to_vec())]),
        );
        spaces.set(
            "Pat",
            Object::Array(vec![Object::Name(b"Pattern".to_vec())]),
        );
        spaces.set("Bare", Object::Name(b"Pattern".to_vec()));
        spaces.set("Alias", Object::Name(b"DeviceGray".to_vec()));
        let mut doc = build_page_doc_with_content(b"");
        doc.max_id = 4;
        let xobject = |doc: &mut Document, subtype: &str, content: &[u8]| {
            let dict =
                Dictionary::from_iter(vec![("Subtype", Object::Name(subtype.as_bytes().to_vec()))]);
            Object::Reference(doc.add_object(Object::Stream(Stream::new(dict, content.to_vec()))))
        };
        let looping = xobject(&mut doc, "Form", b"/Loop Do");
        let postscript = xobject(&mut doc, "PS", b"");
        // A form that redefines DeviceRGB in its own resources.
        let defaults = Dictionary::from_iter(vec![(
            "ColorSpace",
            Object::Dictionary(Dictionary::from_iter(vec![(
                "DefaultRGB",
                Object::Name(b"DeviceRGB".to_vec()),
            )])),
        )]);
        let mut form = Dictionary::from_iter(vec![("Subtype", Object::Name(b"Form".to_vec()))]);
        form.set("Resources", defaults);
        let redefining =
            Object::Reference(doc.add_object(Object::Stream(Stream::new(form, vec![]))));
        let mut form = Dictionary::from_iter(vec![("Subtype", Object::Name(b"Form".to_vec()))]);
        form.set("Resources", Object::Integer(1));
        let unreadable =
            Object::Reference(doc.add_object(Object::Stream(Stream::new(form, vec![]))));
        let xobjects = Dictionary::from_iter(vec![
            ("Loop", looping),
            ("PS0", postscript),
            ("Dflt", redefining),
            ("BadRes", unreadable),
        ]);
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut((2, 0)) {
            page.set(
                "Resources",
                Dictionary::from_iter(vec![
                    ("ColorSpace", Object::Dictionary(spaces)),
                    ("XObject", Object::Dictionary(xobjects)),
                ]),
            );
        }
        let resources = crate::resources::resolve_page_resources(&doc, (2, 0));
        let unmatchable = |content: &[u8]| {
            let mut paint = PagePaint::default();
            walk(&doc, content, resources, &mut paint, &mut HashSet::new(), 0);
            paint.unmatchable
        };
        for content in [
            &b"q 1 0 0 1 0 0 cm Q /Tag BMC EMC /P <</MCID 0>> BDC EMC"[..],
            b"0.5 g 1 0 0 RG 0 0 0 1 k /DeviceRGB cs 1 0 0 sc /Spot CS 0.5 SCN",
            b"/Alias cs 0.5 sc",
            b"BT /F1 12 Tf 72 700 Td () Tj ET",
            b"q 72 750 50 20 re W n Q",
            b"q 72 750 m 122 750 l 122 770 l 72 770 l h W n Q",
            b"72 750 m 122 750 l 122 770 l 72 770 l W* n",
            // A sheared parallelogram, a trapezoid, corners rounded by the producer, and a
            // rectangle turning the other way.
            b"0 0 m 10 0 l 15 5 l 5 5 l h W n",
            b"0 0 m 10.004 0 l 10 5 l 0 5.003 l h W n",
            b"0 0 m 10 0 l 8 5 l 2 5 l h W n",
            b"0 0 10 10 re h W n",
            b"0 0 m 10 0 l 10 5 l 0 5 l 0 0 l h W n",
            b"0 0 m 0 5 l 10 5 l 10 0 l h W n",
            b"BX EX",
            b"q 72 750 50 20 re W* n 0 0 10 10 re n Q",
            b"0 0 10 10 re f",
        ] {
            assert!(
                !unmatchable(content),
                "{:?}",
                String::from_utf8_lossy(content)
            );
        }
        for content in [
            // Letters cut out of the image by a clip.
            &b"72 700 m 80 700 l 76 712 l h W n"[..],
            b"0 0 10 10 re 20 0 10 10 re W n",
            b"0 0 m 1 0 l 1 1 l W n",
            // A concave dart (a V), a crossing bowtie, and `cm` inside the path.
            b"72 730 m 84 700 l 96 730 l 84 712 l h W n",
            b"0 0 m 1 1 l 1 0 l 0 1 l W* n",
            // The same, too small for any tolerance, scaled up or stretched by `cm`.
            &[
                &b"10000 0 0 10000 0 0 cm 0.0072 0.0730 m 0.0084 0.0700 l "[..],
                b"0.0096 0.0730 l 0.0084 0.0712 l h W n",
            ]
            .concat(),
            b"1 0 0 100000 0 0 cm 0 0 m 600 0.0001 l 600 0 l 0 0.0001 l W* n",
            // A path still open when the content ends.
            b"0 0 m 10 0 l",
            b"0 0 10 10 re W",
            // Three corners in a line: no shape at all.
            b"0 0 m 5 0 l 10 0 l 10 5 l W n",
            // A concave shape whose dent is finer than an `f32` can hold at its size.
            b"0 33554432 m 512 33554432 l 4096 33554436 l 3994 33554435.5 l h W n",
            b"0 0 m 1 0 l 2 0 0 2 0 0 cm 1 1 l 0 1 l W n",
            b"0 0 10 10 re q W n Q",
            b"0 0 m (x) 1 l 1 1 l 0 1 l W n",
            b"0 0 m 1 0 l 1 1 l 0 1 l 0 2 l W n",
            b"0 0 m 1 0 l 1 1 l 0 1 l 0 1 l W n",
            b"0 0 m 1 0 l 1 1 l 0 1 l h 5 5 m W n",
            b"0 0 10 10 re 0 0 m W n",
            b"W n",
            // A pattern paints through a stencil image.
            b"/Pattern cs /P0 scn",
            b"/Pat cs",
            b"/Bare CS",
            b"/Missing cs",
            b"cs",
            b"/P0 scn",
            b"/P0 SCN",
            // A Do that names nothing readable, or something but an image or a form.
            b"/Nothing Do",
            b"Do",
            b"/\xff Do",
            // A form that draws itself, which a viewer with a depth limit repeats.
            b"/Loop Do",
            b"/PS0 Do",
            b"/Dflt Do",
            b"/BadRes Do",
            // Anything not on the list.
            b"1 2 3 4 5 6 c",
        ] {
            assert!(
                unmatchable(content),
                "{:?}",
                String::from_utf8_lossy(content)
            );
        }
    }

    #[test]
    fn a_colour_space_is_self_contained_only_without_resource_names() {
        let mut doc = build_page_doc_with_content(b"");
        doc.max_id = 4;
        let name = |n: &str| Object::Name(n.as_bytes().to_vec());
        let array = |items: Vec<Object>| Object::Array(items);
        let to_resource = doc.add_object(name("P0"));
        let to_device = doc.add_object(name("DeviceRGB"));
        let icc = doc.add_object(Object::Stream(Stream::new(Dictionary::new(), vec![])));
        let lookup = Object::String(vec![0; 3], lopdf::StringFormat::Hexadecimal);
        for (space, contained) in [
            (name("DeviceGray"), true),
            (name("G"), true),
            (name("P0"), false),
            (name("Pattern"), false),
            (Object::Reference(to_device), true),
            (Object::Reference(to_resource), false),
            (
                array(vec![
                    name("Indexed"),
                    name("DeviceRGB"),
                    0.into(),
                    lookup.clone(),
                ]),
                true,
            ),
            (
                array(vec![name("I"), name("RGB"), 0.into(), lookup.clone()]),
                true,
            ),
            (
                array(vec![
                    name("Indexed"),
                    name("Base"),
                    0.into(),
                    lookup.clone(),
                ]),
                false,
            ),
            (
                array(vec![
                    name("Indexed"),
                    Object::Reference(to_resource),
                    0.into(),
                    lookup,
                ]),
                false,
            ),
            (
                array(vec![
                    name("Separation"),
                    name("Spot"),
                    name("DeviceCMYK"),
                    Object::Null,
                ]),
                true,
            ),
            (
                array(vec![
                    name("DeviceN"),
                    array(vec![]),
                    name("Base"),
                    Object::Null,
                ]),
                false,
            ),
            (
                array(vec![
                    name("DeviceN"),
                    array(vec![]),
                    name("DeviceCMYK"),
                    Object::Null,
                ]),
                true,
            ),
            // Attributes can name more spaces, so they are not vouched for.
            (
                array(vec![
                    name("DeviceN"),
                    array(vec![]),
                    name("DeviceCMYK"),
                    Object::Null,
                    Object::Dictionary(Dictionary::new()),
                ]),
                false,
            ),
            (array(vec![name("ICCBased"), Object::Reference(icc)]), true),
            (
                array(vec![name("CalRGB"), Object::Dictionary(Dictionary::new())]),
                true,
            ),
            (array(vec![name("Pattern"), name("DeviceRGB")]), false),
            (array(vec![]), false),
            (Object::Integer(1), false),
            (Object::Reference((99, 0)), false),
        ] {
            assert_eq!(
                self_contained_space(&doc, &space, true, 0),
                contained,
                "{space:?}"
            );
        }
        // An image XObject may not use the inline abbreviations; either may write a
        // device space as a one-entry array; a profile's /Alternate must say all too.
        let mut profile = |alternate: &str| {
            let dict = Dictionary::from_iter(vec![("Alternate", name(alternate))]);
            let id = doc.add_object(Object::Stream(Stream::new(dict, vec![])));
            array(vec![name("ICCBased"), Object::Reference(id)])
        };
        for (space, xobject, inline) in [
            (name("G"), false, true),
            (
                array(vec![name("I"), name("DeviceRGB"), 0.into(), Object::Null]),
                false,
                true,
            ),
            (array(vec![name("DeviceGray")]), true, true),
            (
                array(vec![name("DeviceGray"), name("DeviceRGB")]),
                false,
                false,
            ),
            (profile("DeviceRGB"), true, true),
            (profile("P0"), false, false),
            (array(vec![name("ICCBased"), name("P0")]), false, false),
        ] {
            assert_eq!(
                self_contained_space(&doc, &space, false, 0),
                xobject,
                "{space:?}"
            );
            assert_eq!(
                self_contained_space(&doc, &space, true, 0),
                inline,
                "{space:?}"
            );
        }
        let mut deep = name("DeviceGray");
        for _ in 0..10 {
            deep = array(vec![name("Indexed"), deep, 0.into(), Object::Null]);
        }
        assert!(
            !self_contained_space(&doc, &deep, true, 0),
            "too deep to vouch for"
        );
        // A copy embedded per page may differ in its labels, never in what it shows.
        let labelled = |doc: &mut Document, label: Option<(&str, Object)>| {
            let mut dict = Dictionary::new();
            dict.set("Subtype", name("Image"));
            dict.set("ColorSpace", name("DeviceGray"));
            if let Some((key, value)) = label {
                dict.set(key, value);
            }
            let id = doc.add_object(Object::Stream(Stream::new(dict, vec![0])));
            xobject_key(doc, id)
        };
        let bare = labelled(&mut doc, None);
        assert!(bare.is_some());
        for label in ["Name", "StructParent", "ID", "Metadata"] {
            assert_eq!(labelled(&mut doc, Some((label, 7.into()))), bare, "{label}");
        }
        assert_ne!(labelled(&mut doc, Some(("Interpolate", true.into()))), bare);
        // Alternate images, and a soft mask whose colour space names a resource.
        assert_eq!(
            labelled(&mut doc, Some(("Alternates", Object::Array(vec![])))),
            None
        );
        let mut mask = Dictionary::new();
        mask.set("Subtype", name("Image"));
        mask.set("ColorSpace", name("P0"));
        let mask = doc.add_object(Object::Stream(Stream::new(mask, vec![0])));
        assert_eq!(
            labelled(&mut doc, Some(("SMask", Object::Reference(mask)))),
            None
        );
        let mut gray = Dictionary::new();
        gray.set("Subtype", name("Image"));
        gray.set("ColorSpace", name("DeviceGray"));
        let gray = doc.add_object(Object::Stream(Stream::new(gray, vec![0])));
        assert!(labelled(&mut doc, Some(("SMask", Object::Reference(gray)))).is_some());
        let mut image = Dictionary::new();
        image.set("Subtype", name("Image"));
        image.set("ColorSpace", Object::Reference(to_resource));
        let id = doc.add_object(Object::Stream(Stream::new(image, vec![0])));
        assert_eq!(
            xobject_key(&doc, id),
            None,
            "a reference to a resource name"
        );
    }

    #[test]
    fn an_inline_space_resource_must_itself_be_self_contained() {
        let paint = |space: Object, defaults: bool| {
            let mut doc = build_page_doc_with_content(b"BI /W 1 /H 1 /BPC 8 /CS /P ID a\nEI");
            let mut spaces = Dictionary::from_iter(vec![("P", space)]);
            if defaults {
                spaces.set("DefaultGray", Object::Name(b"DeviceGray".to_vec()));
            }
            if let Ok(Object::Dictionary(page)) = doc.get_object_mut((2, 0)) {
                page.set(
                    "Resources",
                    Dictionary::from_iter(vec![("ColorSpace", Object::Dictionary(spaces))]),
                );
            }
            page_paint(&doc, (2, 0))
        };
        let indexed = |base: &str| {
            Object::Array(vec![
                Object::Name(b"Indexed".to_vec()),
                Object::Name(base.as_bytes().to_vec()),
                0.into(),
                Object::String(vec![0; 3], lopdf::StringFormat::Hexadecimal),
            ])
        };
        let plain = paint(indexed("DeviceRGB"), false);
        assert!(
            !plain.unmatchable && plain.image_keys.len() == 1,
            "{plain:?}"
        );
        assert!(
            paint(indexed("Base"), false).unmatchable,
            "a resource name inside it"
        );
        assert!(
            paint(indexed("DeviceRGB"), true).unmatchable,
            "the page redefines DeviceGray"
        );
        // An abbreviation in a resource is a name to look up, not a device space.
        assert!(
            paint(indexed("RGB"), false).unmatchable,
            "an abbreviation in a resource"
        );
    }

    #[test]
    fn resources_that_redefine_or_hide_the_device_spaces_count() {
        let doc = build_page_doc_with_content(b"");
        let of = |spaces: Object| Dictionary::from_iter(vec![("ColorSpace", spaces)]);
        assert!(!redefines_device_spaces(&doc, None));
        assert!(!redefines_device_spaces(&doc, Some(&Dictionary::new())));
        assert!(!redefines_device_spaces(
            &doc,
            Some(&of(Object::Dictionary(Dictionary::new())))
        ));
        assert!(
            redefines_device_spaces(&doc, Some(&of(Object::Integer(1)))),
            "unreadable"
        );
        assert!(
            !redefines_device_spaces(&doc, Some(&of(Object::Null))),
            "a null is no entry"
        );
        let nulls = Dictionary::from_iter(vec![
            ("DefaultGray", Object::Null),
            ("DefaultRGB", Object::Reference((99, 0))),
        ]);
        assert!(!redefines_device_spaces(
            &doc,
            Some(&of(Object::Dictionary(nulls)))
        ));
        for key in ["DefaultGray", "DefaultRGB", "DefaultCMYK"] {
            let spaces = Dictionary::from_iter(vec![(key, Object::Name(b"DeviceGray".to_vec()))]);
            assert!(
                redefines_device_spaces(&doc, Some(&of(Object::Dictionary(spaces)))),
                "{key}"
            );
        }
    }

    #[test]
    fn page_resources_that_cannot_be_read_are_unmatchable() {
        let logo = b"q BI /W 1 /H 1 /BPC 8 /CS /G ID a\nEI Q";
        for (resources, unmatchable) in [
            (None, false),
            (Some(Object::Dictionary(Dictionary::new())), false),
            // A null, or a reference to nothing, is no entry (§7.3.10).
            (Some(Object::Null), false),
            (Some(Object::Reference((99, 0))), false),
            (Some(Object::Integer(1)), true),
            (Some(Object::Name(b"R".to_vec())), true),
        ] {
            let mut doc = build_page_doc_with_content(logo);
            if let Ok(Object::Dictionary(page)) = doc.get_object_mut((2, 0)) {
                page.remove(b"Resources");
                if let Some(resources) = resources.clone() {
                    page.set("Resources", resources);
                }
            }
            let paint = page_paint(&doc, (2, 0));
            assert_eq!(paint.unmatchable, unmatchable, "{resources:?}");
        }
        // A reference to nothing gives way to the parent's resources for a viewer, which
        // is not what the extractors read: unmatchable.
        let mut doc = build_page_doc_with_content(logo);
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut((2, 0)) {
            page.set("Resources", Object::Reference((99, 0)));
        }
        if let Ok(Object::Dictionary(pages)) = doc.get_object_mut((3, 0)) {
            pages.set("Resources", Dictionary::new());
        }
        assert!(page_paint(&doc, (2, 0)).unmatchable);
    }

    #[test]
    fn an_inline_image_whose_end_was_guessed_is_unmatchable() {
        let guessed = paint_of(
            b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /DCT ID ab EI \xFF\xFE\xFD cd\nEI\nQ",
            b"",
        );
        assert!(
            guessed.images && guessed.image_keys.is_empty(),
            "{guessed:?}"
        );
        assert!(guessed.unmatchable, "{guessed:?}");
    }

    #[test]
    fn content_cut_short_is_unread() {
        let paint = paint_of(b"0 0 m 1 1 l S BT (unterminated", b"");
        assert!(paint.unread && paint.lacks_text(), "{paint:?}");
        assert!(paint.describe().contains("could not be read to its end"));
    }
}
