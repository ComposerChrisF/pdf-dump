//! What a page draws, read from its operators alone (plan-0012, implemented in v0.37.0).
//!
//! A page whose words are filled paths (outlined text) or a scanned image shows no
//! text, so `--text` prints nothing for it, and no verdict can say why.  This module
//! answers the narrower question a caller can act on: does the page show any text,
//! and does it draw anything else?  `--text` lists a page that draws but shows no
//! text (`pages_without_text` in `--json`, a note on stderr), and `--require-text`
//! makes such a page exit 1.  It reads operators only, never a string’s content, and
//! recurses into form XObjects with the extractors’ cycle guard and depth cap.

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId};
use serde_json::{Value, json};

use crate::helpers;
use crate::inline_image::decode_content;
use crate::text::{MAX_FORM_DEPTH, resolve_xobject_id};

/// Operators that fill or stroke a path, or paint a shading.
const PATH_PAINTING: [&str; 10] = ["S", "s", "f", "F", "f*", "B", "B*", "b", "b*", "sh"];

/// What one page draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PagePaint {
    /// A text-showing operator with a non-empty string.
    pub text: bool,
    /// A path filled or stroked, or a shading painted.
    pub paths: bool,
    /// An image XObject or an inline image.
    pub images: bool,
    /// Some content (the page’s, or a form’s) could not be read to its end.
    pub unread: bool,
}

impl PagePaint {
    /// The page shows no text, yet draws something or could not be read in full:
    /// whatever words it holds are not in the extracted text.  A blank page is not.
    pub(crate) fn lacks_text(&self) -> bool {
        !self.text && (self.paths || self.images || self.unread)
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
    let content = match decode_content(bytes) {
        Ok(content) => content,
        Err(_) => {
            paint.unread = true;
            return;
        }
    };
    paint.unread |= content.stopped_early;
    paint.images |= content.dropped_images > 0;
    for op in &content.operations {
        match op.operator.as_str() {
            "Tj" | "'" | "\"" => paint.text |= op.operands.last().is_some_and(non_empty_string),
            "TJ" => {
                paint.text |= matches!(op.operands.first(),
                    Some(Object::Array(items)) if items.iter().any(non_empty_string))
            }
            "BI" => paint.images = true,
            "Do" => {
                let Some(name) = op.operands.first().and_then(|n| n.as_name().ok()) else {
                    continue;
                };
                let name = String::from_utf8_lossy(name);
                if let Some(id) = resources.and_then(|r| resolve_xobject_id(doc, r, &name)) {
                    xobject(doc, id, resources, paint, visited, depth);
                }
            }
            op if PATH_PAINTING.contains(&op) => paint.paths = true,
            _ => {}
        }
    }
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
        return;
    };
    match stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|s| s.as_name().ok())
    {
        Some(b"Image") => paint.images = true,
        Some(b"Form") => {
            // A form nested past the cap, or one already on the recursion stack,
            // is not read, as in the extractors; past the cap that is unread.
            if depth + 1 > MAX_FORM_DEPTH {
                paint.unread = true;
                return;
            }
            if !visited.insert(id) {
                return;
            }
            let (decoded, warning) = crate::stream::decode_stream(doc, stream);
            paint.unread |= warning.is_some();
            let resources = stream
                .dict
                .get(b"Resources")
                .ok()
                .and_then(|r| helpers::resolve_dict(doc, r))
                .or(parent_resources);
            walk(doc, &decoded, resources, paint, visited, depth + 1);
            visited.remove(&id);
        }
        _ => {}
    }
}

/// The `--json` entry for one page that lacks text.
pub(crate) fn page_json(page_number: u32, paint: &PagePaint) -> Value {
    json!({
        "page_number": page_number,
        "paths": paint.paths,
        "images": paint.images,
        "unread": paint.unread,
    })
}

/// The stderr note for the pages that lack text.  Silent when there are none.
pub(crate) fn print_note(pages: &[(u32, PagePaint)]) {
    for (page_number, paint) in pages {
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
            assert_eq!(paint, PagePaint::default(), "{content:?}");
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
    fn content_cut_short_is_unread() {
        let paint = paint_of(b"0 0 m 1 1 l S BT (unterminated", b"");
        assert!(paint.unread && paint.lacks_text(), "{paint:?}");
        assert!(paint.describe().contains("could not be read to its end"));
    }
}
