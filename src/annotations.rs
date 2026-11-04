use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::io::Write;

use crate::bookmarks::format_dest_value;
use crate::helpers::{
    format_dict_value, get_catalog, name_to_string, obj_to_text, page_annots, resolve_dict,
};
use crate::text::{AppearanceText, Reliability, appearance_text};
use crate::text_string::{read_rich_text, read_text_value};
use crate::types::PageSpec;

pub(crate) struct AnnotationInfo {
    pub page_num: u32,
    /// `None` for an annotation written directly in `/Annots`.
    pub object_id: Option<ObjectId>,
    pub subtype: String,
    pub rect: String,
    pub contents: String,
    pub contents_decoded: bool,
    /// The words of `/RC`, markup removed.
    pub rich_contents: Option<String>,
    pub rich_contents_decoded: bool,
    pub link_type: Option<String>,
    pub target: Option<String>,
    pub appearance: Option<Appearance>,
}

/// What an annotation's normal appearance (`/AP /N`) shows.
pub(crate) struct Appearance {
    /// The `/AS` state selected from a state dictionary, for a checkbox or radio button.
    pub state: Option<String>,
    /// A checkbox's or radio button's state, which draws a mark, not text.  ZapfDingbats' check mark
    /// is not worth a finding, though a state that will not decode, or that writes in another
    /// unreadable font, still is.
    pub is_mark: bool,
    pub read: AppearanceText,
}

impl AnnotationInfo {
    /// Whether this annotation holds text pdf-dump could not read: a finding, exit 3.
    fn has_unread_text(&self) -> bool {
        !self.contents_decoded
            || !self.rich_contents_decoded
            || self.appearance.as_ref().is_some_and(|a| {
                a.read.verdict.is_finding() && !(a.is_mark && a.read.only_symbols_unread)
            })
    }
}

/// The streams of an annotation's normal appearance, as far as they could be found.
struct NormalAppearance {
    streams: Vec<ObjectId>,
    /// The `/AS` selected from a state dictionary.
    state: Option<String>,
    is_state_dictionary: bool,
    /// What stopped part of it being found.  That is not evidence of it not being there.
    problem: Option<String>,
}

impl NormalAppearance {
    fn unreadable(problem: String) -> Self {
        NormalAppearance {
            streams: vec![],
            state: None,
            is_state_dictionary: false,
            problem: Some(problem),
        }
    }
}

/// Finds an annotation's normal appearance: `/N` itself, or from a state dictionary the stream `/AS`
/// selects (every state when `/AS` is absent, since nothing says which is shown).  `None` when the
/// annotation has no appearance, or its selected state has none (a checkbox's usual `/Off`).
fn normal_appearance(doc: &Document, dict: &lopdf::Dictionary) -> Option<NormalAppearance> {
    let ap = match dict.get(b"AP").ok()? {
        Object::Null => return None,
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::Dictionary(d)) => d,
            Ok(Object::Null) | Err(lopdf::Error::ObjectNotFound(_)) => return None,
            _ => {
                return Some(NormalAppearance::unreadable(format!(
                    "/AP at {} {} is not a dictionary",
                    id.0, id.1
                )));
            }
        },
        Object::Dictionary(d) => d,
        _ => {
            return Some(NormalAppearance::unreadable(
                "/AP is not a dictionary".to_string(),
            ));
        }
    };
    let states = match ap.get(b"N").ok()? {
        Object::Null => return None,
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::Stream(_)) => {
                return Some(NormalAppearance {
                    streams: vec![*id],
                    state: None,
                    is_state_dictionary: false,
                    problem: None,
                });
            }
            Ok(Object::Dictionary(d)) => d,
            Ok(Object::Null) | Err(lopdf::Error::ObjectNotFound(_)) => return None,
            _ => {
                return Some(NormalAppearance::unreadable(format!(
                    "/AP /N at {} {} is not readable",
                    id.0, id.1
                )));
            }
        },
        Object::Dictionary(d) => d,
        _ => {
            return Some(NormalAppearance::unreadable(
                "/AP /N is neither a stream nor a dictionary".to_string(),
            ));
        }
    };
    let selected = match dict.get(b"AS") {
        Ok(Object::Name(n)) => Some(n.as_slice()),
        _ => None,
    };
    let mut found = NormalAppearance {
        streams: vec![],
        state: selected.map(|n| String::from_utf8_lossy(n).into_owned()),
        is_state_dictionary: true,
        problem: None,
    };
    match selected {
        Some(name) => match states.get(name).ok()?.as_reference() {
            Ok(id) => found.streams.push(id),
            Err(_) => found.problem = Some("the selected /AP /N state is not a stream".to_string()),
        },
        None => {
            for (_, state) in states.iter() {
                match state.as_reference() {
                    Ok(id) => found.streams.push(id),
                    Err(_) => found.problem = Some("an /AP /N state is not a stream".to_string()),
                }
            }
        }
    }
    Some(found)
}

/// Whether a field is a button: `/FT` is inheritable, so the walk goes up `/Parent`.
fn is_button_field(doc: &Document, dict: &lopdf::Dictionary) -> bool {
    let mut current = dict;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        if let Ok(ft) = current.get(b"FT") {
            return ft.as_name().is_ok_and(|n| n == b"Btn");
        }
        let Some(parent) = current
            .get(b"Parent")
            .ok()
            .and_then(|p| p.as_reference().ok())
        else {
            return false;
        };
        if !seen.insert(parent) {
            return false;
        }
        match doc.get_object(parent) {
            Ok(Object::Dictionary(d)) => current = d,
            _ => return false,
        }
    }
}

fn classify_link(doc: &Document, dict: &lopdf::Dictionary) -> (Cow<'static, str>, String) {
    // Check /Dest first (direct destination)
    if let Ok(dest) = dict.get(b"Dest") {
        return (Cow::Borrowed("GoTo"), format_dest_value(doc, dest));
    }
    // Check /A (action dictionary)
    if let Ok(action_obj) = dict.get(b"A") {
        let action_dict = match action_obj {
            Object::Dictionary(d) => d,
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(Object::Dictionary(d)) => d,
                _ => return (Cow::Borrowed("Unknown"), format!("{} {} R", id.0, id.1)),
            },
            _ => return (Cow::Borrowed("Unknown"), "-".to_string()),
        };
        let action_type = action_dict.get(b"S").ok().and_then(|v| v.as_name().ok());
        match action_type {
            Some(b"GoTo") => {
                let target = action_dict
                    .get(b"D")
                    .ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("GoTo"), target)
            }
            Some(b"GoToR") => {
                let file = action_dict
                    .get(b"F")
                    .ok()
                    .and_then(obj_to_text)
                    .unwrap_or_else(|| "?".to_string());
                let dest = action_dict
                    .get(b"D")
                    .ok()
                    .map(|d| format_dest_value(doc, d))
                    .unwrap_or_default();
                (
                    Cow::Borrowed("GoToR"),
                    format!("{} {}", file, dest).trim().to_string(),
                )
            }
            Some(b"URI") => {
                let uri = action_dict
                    .get(b"URI")
                    .ok()
                    .and_then(obj_to_text)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("URI"), uri)
            }
            Some(b"Named") => {
                let n = action_dict
                    .get(b"N")
                    .ok()
                    .and_then(obj_to_text)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Named"), n)
            }
            Some(b"Launch") => {
                let f = action_dict
                    .get(b"F")
                    .ok()
                    .and_then(obj_to_text)
                    .unwrap_or_else(|| "?".to_string());
                (Cow::Borrowed("Launch"), f)
            }
            Some(other) => (
                Cow::Owned(String::from_utf8_lossy(other).into_owned()),
                "-".to_string(),
            ),
            None => (Cow::Borrowed("Unknown"), "-".to_string()),
        }
    } else {
        (Cow::Borrowed("Unknown"), "-".to_string())
    }
}

pub(crate) fn collect_annotations(
    doc: &Document,
    page_filter: Option<&PageSpec>,
) -> Vec<AnnotationInfo> {
    collect(doc, page_filter, false)
}

fn form_default_resources(doc: &Document) -> Option<&lopdf::Dictionary> {
    let form = resolve_dict(doc, get_catalog(doc)?.get(b"AcroForm").ok()?)?;
    resolve_dict(doc, form.get(b"DR").ok()?)
}

fn collect(
    doc: &Document,
    page_filter: Option<&PageSpec>,
    read_appearances: bool,
) -> Vec<AnnotationInfo> {
    let pages = doc.get_pages();
    let mut annotations = Vec::new();

    for (&page_num, &page_id) in &pages {
        if let Some(spec) = page_filter
            && !spec.contains(page_num)
        {
            continue;
        }

        let page_dict = match doc.get_object(page_id) {
            Ok(Object::Dictionary(d)) => d,
            _ => continue,
        };

        for entry in page_annots(doc, page_dict) {
            let (object_id, annot_dict) = match entry {
                Object::Reference(id) => match doc.get_object(*id) {
                    Ok(Object::Dictionary(d)) => (Some(*id), d),
                    Ok(Object::Stream(s)) => (Some(*id), &s.dict),
                    _ => continue,
                },
                Object::Dictionary(d) => (None, d),
                _ => continue,
            };

            let subtype = annot_dict
                .get(b"Subtype")
                .ok()
                .and_then(name_to_string)
                .unwrap_or_else(|| "-".to_string());

            let rect = annot_dict
                .get(b"Rect")
                .ok()
                .map(format_dict_value)
                .unwrap_or_else(|| "-".to_string());

            let contents = annot_dict
                .get(b"Contents")
                .ok()
                .and_then(|v| read_text_value(doc, v));
            let contents_decoded = contents.as_ref().is_none_or(|t| t.decoded);
            let contents = contents.map(|t| t.text).unwrap_or_default();

            let rich = annot_dict
                .get(b"RC")
                .ok()
                .and_then(|v| read_rich_text(doc, v));
            let rich_contents_decoded = rich.as_ref().is_none_or(|r| r.decoded);
            let rich_contents = rich.map(|r| r.text);

            let (link_type, target) = if subtype == "Link" {
                let (lt, tgt) = classify_link(doc, annot_dict);
                (Some(lt.into_owned()), Some(tgt))
            } else {
                (None, None)
            };

            let appearance = if read_appearances {
                normal_appearance(doc, annot_dict).map(|found| {
                    // § 12.7.2: a widget's default resources are the form's /DR.
                    let fallback = if subtype == "Widget" {
                        form_default_resources(doc)
                            .or_else(|| crate::resources::resolve_page_resources(doc, page_id))
                    } else {
                        crate::resources::resolve_page_resources(doc, page_id)
                    };
                    let mut read = appearance_text(doc, &found.streams, fallback);
                    if let Some(problem) = found.problem {
                        read.warnings.push(problem);
                        read.verdict = Reliability::Unreliable;
                        read.reliability["verdict"] = serde_json::json!(read.verdict.as_str());
                        read.only_symbols_unread = false;
                    }
                    Appearance {
                        state: found.state,
                        is_mark: found.is_state_dictionary
                            && subtype == "Widget"
                            && is_button_field(doc, annot_dict),
                        read,
                    }
                })
            } else {
                None
            };

            annotations.push(AnnotationInfo {
                page_num,
                object_id,
                subtype,
                rect,
                contents,
                contents_decoded,
                rich_contents,
                rich_contents_decoded,
                link_type,
                target,
                appearance,
            });
        }
    }

    annotations
}

fn warn_unread(annotations: &[AnnotationInfo]) -> bool {
    let unread = annotations.iter().filter(|a| a.has_unread_text()).count();
    if unread > 0 {
        eprintln!(
            "Warning: {unread} annotation(s) hold text that could not be read reliably: a comment \
             that would not decode, malformed rich text, or an appearance whose verdict is not \
             reliable."
        );
    }
    unread > 0
}

/// Returns `true` when an annotation holds text that could not be read reliably.
pub(crate) fn print_annotations(
    writer: &mut impl Write,
    doc: &Document,
    page_filter: Option<&PageSpec>,
) -> bool {
    let annotations = collect(doc, page_filter, true);
    wln!(writer, "{} annotations found\n", annotations.len());
    if annotations.is_empty() {
        return false;
    }
    wln!(
        writer,
        "  {:>4}  {:>4}  {:<12} {:<8} {:<30} {:<30} Contents",
        "Page",
        "Obj#",
        "Subtype",
        "Type",
        "Rect",
        "Target"
    );
    for a in &annotations {
        let link_type = a.link_type.as_deref().unwrap_or("-");
        let target = a.target.as_deref().unwrap_or("-");
        let object = a
            .object_id
            .map_or_else(|| "-".to_string(), |id| id.0.to_string());
        wln!(
            writer,
            "  {:>4}  {:>4}  {:<12} {:<8} {:<30} {:<30} {}",
            a.page_num,
            object,
            a.subtype,
            link_type,
            a.rect,
            target,
            a.contents
        );
        if let Some(rich) = &a.rich_contents {
            wln!(writer, "        rich text: {}", rich);
        }
        if let Some(appearance) = &a.appearance {
            if !appearance.read.text.is_empty() {
                wln!(
                    writer,
                    "        appearance ({}): {}",
                    appearance.read.verdict.as_str(),
                    appearance.read.text
                );
            }
            for warning in &appearance.read.warnings {
                wln!(writer, "        appearance warning: {}", warning);
            }
        }
    }
    warn_unread(&annotations)
}

#[cfg(test)]
pub(crate) fn annotations_json_value(doc: &Document, page_filter: Option<&PageSpec>) -> Value {
    annotations_json_value_with_status(doc, page_filter).0
}

/// Returns the JSON and whether an annotation holds text that could not be read reliably.
pub(crate) fn annotations_json_value_with_status(
    doc: &Document,
    page_filter: Option<&PageSpec>,
) -> (Value, bool) {
    let annotations = collect(doc, page_filter, true);
    let items: Vec<Value> = annotations
        .iter()
        .map(|a| {
            json!({
                "page_number": a.page_num,
                "object_number": a.object_id.map(|id| id.0),
                "generation": a.object_id.map(|id| id.1),
                "subtype": a.subtype,
                "rect": a.rect,
                "contents": a.contents,
                "contents_decoded": a.contents_decoded,
                "rich_contents": a.rich_contents,
                "rich_contents_decoded": a.rich_contents_decoded,
                "link_type": a.link_type,
                "target": a.target,
                "appearance_text": a.appearance.as_ref().map(|ap| &ap.read.text),
                "appearance_state": a.appearance.as_ref().and_then(|ap| ap.state.as_ref()),
                "appearance_reliability": a.appearance.as_ref().map(|ap| &ap.read.reliability),
                "appearance_warnings": a.appearance.as_ref().map(|ap| &ap.read.warnings),
            })
        })
        .collect();
    let had_issues = warn_unread(&annotations);
    (
        json!({
            "annotation_count": items.len(),
            "annotations": items,
        }),
        had_issues,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::types::PageSpec;
    use lopdf::Object;
    use lopdf::{Dictionary, Stream, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    fn make_doc_with_annotations() -> Document {
        let mut doc = Document::new();

        // Annotation
        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(50),
            ]),
        );
        annot.set(
            "Contents",
            Object::String(b"Click here".to_vec(), StringFormat::Literal),
        );
        let annot_id = doc.add_object(Object::Dictionary(annot));

        // Content stream
        let content_stream = Stream::new(Dictionary::new(), b"BT ET".to_vec());
        let content_id = doc.add_object(Object::Stream(content_stream));

        // Page
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Contents", Object::Reference(content_id));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        // Pages
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages_dict.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        // Update page /Parent
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
        }

        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        doc
    }

    // ── plan-0006 ──────────────────────────────────────────────────────

    fn utf16(s: &str) -> Vec<u8> {
        let mut out = vec![0xFE, 0xFF];
        out.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        out
    }

    fn literal(bytes: &[u8]) -> Object {
        Object::String(bytes.to_vec(), StringFormat::Literal)
    }

    /// One page whose `/Annots` is `annots`, with `objs` added as they stand.
    fn page_doc(annots: Vec<Object>, objs: Vec<(ObjectId, Object)>) -> Document {
        let mut doc = Document::new();
        for (id, o) in objs {
            doc.objects.insert(id, o);
        }
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((101, 0)));
        page.set("Annots", Object::Array(annots));
        doc.objects.insert((100, 0), Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", Object::Integer(1));
        pages.set("Kids", Object::Array(vec![Object::Reference((100, 0))]));
        doc.objects.insert((101, 0), Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((101, 0)));
        doc.objects.insert((102, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((102, 0)));
        doc
    }

    fn note(entries: Vec<(&str, Object)>) -> Dictionary {
        let mut d = Dictionary::new();
        d.set("Subtype", Object::Name(b"Text".to_vec()));
        for (k, v) in entries {
            d.set(k, v);
        }
        d
    }

    fn font_resources() -> Dictionary {
        let mut helv = Dictionary::new();
        helv.set("Type", Object::Name(b"Font".to_vec()));
        helv.set("Subtype", Object::Name(b"Type1".to_vec()));
        helv.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let mut zadb = Dictionary::new();
        zadb.set("Type", Object::Name(b"Font".to_vec()));
        zadb.set("Subtype", Object::Name(b"Type1".to_vec()));
        zadb.set("BaseFont", Object::Name(b"ZapfDingbats".to_vec()));
        let mut fonts = Dictionary::new();
        fonts.set("Helv", Object::Dictionary(helv));
        fonts.set("ZaDb", Object::Dictionary(zadb));
        let mut res = Dictionary::new();
        res.set("Font", Object::Dictionary(fonts));
        res
    }

    fn appearance_stream(content: &[u8]) -> Object {
        let mut d = Dictionary::new();
        d.set("Subtype", Object::Name(b"Form".to_vec()));
        d.set("Resources", Object::Dictionary(font_resources()));
        Object::Stream(Stream::new(d, content.to_vec()))
    }

    fn ap_n(n: Object) -> Object {
        let mut ap = Dictionary::new();
        ap.set("N", n);
        Object::Dictionary(ap)
    }

    #[test]
    fn utf16_comment_is_decoded() {
        let doc = page_doc(
            vec![Object::Dictionary(note(vec![(
                "Contents",
                literal(&utf16("Storage gate code: 4417")),
            )]))],
            vec![],
        );
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotations"][0]["contents"], "Storage gate code: 4417");
        assert_eq!(v["annotations"][0]["contents_decoded"], true);
    }

    #[test]
    fn undecodable_comment_is_flagged_as_hex_and_is_a_finding() {
        let doc = page_doc(
            vec![Object::Dictionary(note(vec![(
                "Contents",
                literal(&[0xFE, 0xFF, 0x00]),
            )]))],
            vec![],
        );
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        assert_eq!(v["annotations"][0]["contents"], "<FEFF00>");
        assert_eq!(v["annotations"][0]["contents_decoded"], false);
        assert!(had_issues);
    }

    #[test]
    fn rich_comment_is_reported_as_text() {
        let doc = page_doc(
            vec![Object::Dictionary(note(vec![
                ("Contents", literal(b"see below")),
                ("RC", literal(b"<body><p>Lockbox code: 7301</p></body>")),
            ]))],
            vec![],
        );
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotations"][0]["rich_contents"], "Lockbox code: 7301");
    }

    #[test]
    fn a_direct_annotation_is_listed_with_no_object_number() {
        let doc = page_doc(
            vec![
                Object::Dictionary(note(vec![("Contents", literal(b"inline"))])),
                Object::Reference((9, 0)),
            ],
            vec![(
                (9, 0),
                Object::Dictionary(note(vec![("Contents", literal(b"indirect"))])),
            )],
        );
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotation_count"], 2);
        assert_eq!(v["annotations"][0]["contents"], "inline");
        assert_eq!(v["annotations"][0]["object_number"], Value::Null);
        assert_eq!(v["annotations"][1]["object_number"], 9);
        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("2 annotations found") && out.contains("inline"));
    }

    #[test]
    fn annots_held_in_an_indirect_array_are_listed() {
        let mut doc = page_doc(vec![], vec![]);
        doc.objects.insert(
            (9, 0),
            Object::Array(vec![Object::Dictionary(note(vec![(
                "Contents",
                literal(b"via reference"),
            )]))]),
        );
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut((100, 0)) {
            page.set("Annots", Object::Reference((9, 0)));
        }
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotations"][0]["contents"], "via reference");
    }

    #[test]
    fn appearance_text_is_read_with_a_verdict() {
        let doc = page_doc(
            vec![Object::Reference((9, 0))],
            vec![
                (
                    (9, 0),
                    Object::Dictionary(note(vec![("AP", ap_n(Object::Reference((10, 0))))])),
                ),
                (
                    (10, 0),
                    appearance_stream(b"BT /Helv 12 Tf (Only in the appearance) Tj ET"),
                ),
            ],
        );
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        let a = &v["annotations"][0];
        assert_eq!(a["appearance_text"], "Only in the appearance");
        // ZapfDingbats is declared and unused: it must not lower the verdict on Helvetica's text.
        assert_eq!(a["appearance_reliability"]["verdict"], "reliable");
        assert!(!had_issues);
        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("appearance (reliable): Only in the appearance"));
    }

    #[test]
    fn appearance_with_no_resources_of_its_own_uses_the_pages() {
        let mut doc = page_doc(
            vec![Object::Reference((9, 0))],
            vec![
                (
                    (9, 0),
                    Object::Dictionary(note(vec![("AP", ap_n(Object::Reference((10, 0))))])),
                ),
                (
                    (10, 0),
                    Object::Stream(Stream::new(
                        Dictionary::new(),
                        b"BT /Helv 12 Tf (From the page) Tj ET".to_vec(),
                    )),
                ),
            ],
        );
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut((100, 0)) {
            page.set("Resources", Object::Dictionary(font_resources()));
        }
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotations"][0]["appearance_text"], "From the page");
        assert_eq!(
            v["annotations"][0]["appearance_reliability"]["fonts"][0]["base_font"],
            "Helvetica"
        );
    }

    #[test]
    fn appearance_in_an_unreadable_font_is_a_finding_and_its_text_still_prints() {
        let mut cid = Dictionary::new();
        cid.set("Subtype", Object::Name(b"Type0".to_vec()));
        cid.set("BaseFont", Object::Name(b"Mystery".to_vec()));
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Dictionary(cid));
        let mut res = Dictionary::new();
        res.set("Font", Object::Dictionary(fonts));
        let mut form = Dictionary::new();
        form.set("Resources", Object::Dictionary(res));
        let doc = page_doc(
            vec![Object::Reference((9, 0))],
            vec![
                (
                    (9, 0),
                    Object::Dictionary(note(vec![("AP", ap_n(Object::Reference((10, 0))))])),
                ),
                (
                    (10, 0),
                    Object::Stream(Stream::new(form, b"BT /F1 12 Tf (x) Tj ET".to_vec())),
                ),
            ],
        );
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        assert_eq!(
            v["annotations"][0]["appearance_reliability"]["verdict"],
            "unreliable"
        );
        assert!(had_issues);
    }

    #[test]
    fn appearance_that_will_not_decode_is_unreliable() {
        let mut d = Dictionary::new();
        d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let doc = page_doc(
            vec![Object::Reference((9, 0))],
            vec![
                (
                    (9, 0),
                    Object::Dictionary(note(vec![("AP", ap_n(Object::Reference((10, 0))))])),
                ),
                ((10, 0), Object::Stream(Stream::new(d, b"nope".to_vec()))),
            ],
        );
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        let a = &v["annotations"][0];
        assert_eq!(a["appearance_reliability"]["verdict"], "unreliable");
        assert_eq!(a["appearance_warnings"].as_array().unwrap().len(), 1);
        assert!(had_issues);
    }

    fn checkbox(as_state: Option<&str>) -> Dictionary {
        let mut states = Dictionary::new();
        states.set("Yes", Object::Reference((10, 0)));
        states.set("Maybe", Object::Reference((11, 0)));
        let mut d = note(vec![("AP", ap_n(Object::Dictionary(states)))]);
        d.set("Subtype", Object::Name(b"Widget".to_vec()));
        d.set("FT", Object::Name(b"Btn".to_vec()));
        if let Some(s) = as_state {
            d.set("AS", Object::Name(s.as_bytes().to_vec()));
        }
        d
    }

    fn checkbox_doc(as_state: Option<&str>) -> Document {
        page_doc(
            vec![Object::Reference((9, 0))],
            vec![
                ((9, 0), Object::Dictionary(checkbox(as_state))),
                ((10, 0), appearance_stream(b"BT /ZaDb 12 Tf (4) Tj ET")),
                ((11, 0), appearance_stream(b"BT /Helv 12 Tf (maybe) Tj ET")),
            ],
        )
    }

    #[test]
    fn a_checkbox_mark_is_read_but_is_not_a_finding() {
        let doc = checkbox_doc(Some("Yes"));
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        let a = &v["annotations"][0];
        assert_eq!(a["appearance_state"], "Yes");
        assert_eq!(a["appearance_text"], "4");
        // The verdict is reported as it is, even though it makes no finding.
        assert_eq!(a["appearance_reliability"]["verdict"], "degraded");
        assert!(!had_issues);
    }

    #[test]
    fn as_selects_one_state_and_a_state_with_no_stream_has_no_appearance() {
        let selected = annotations_json_value(&checkbox_doc(Some("Maybe")), None);
        assert_eq!(selected["annotations"][0]["appearance_text"], "maybe");
        let off = annotations_json_value(&checkbox_doc(Some("Off")), None);
        assert_eq!(off["annotations"][0]["appearance_text"], Value::Null);
    }

    #[test]
    fn with_no_as_every_state_is_read() {
        let v = annotations_json_value(&checkbox_doc(None), None);
        let text = v["annotations"][0]["appearance_text"].as_str().unwrap();
        assert!(text.contains('4') && text.contains("maybe"), "{text}");
    }

    fn state_doc(annot: Dictionary, appearance: Object) -> Document {
        page_doc(
            vec![Object::Reference((9, 0))],
            vec![((9, 0), Object::Dictionary(annot)), ((10, 0), appearance)],
        )
    }

    fn on_state(mut d: Dictionary) -> Dictionary {
        let mut states = Dictionary::new();
        states.set("On", Object::Reference((10, 0)));
        d.set("AP", ap_n(Object::Dictionary(states)));
        d.set("AS", Object::Name(b"On".to_vec()));
        d
    }

    #[test]
    fn a_state_dictionary_on_anything_but_a_button_is_no_excuse() {
        let doc = state_doc(
            on_state(note(vec![])),
            appearance_stream(b"BT /ZaDb 12 Tf (4) Tj ET"),
        );
        assert!(annotations_json_value_with_status(&doc, None).1);
    }

    #[test]
    fn a_button_state_that_will_not_decode_is_a_finding() {
        let mut widget = on_state(note(vec![]));
        widget.set("Subtype", Object::Name(b"Widget".to_vec()));
        widget.set("FT", Object::Name(b"Btn".to_vec()));
        let mut broken = Dictionary::new();
        broken.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let doc = state_doc(
            widget,
            Object::Stream(Stream::new(broken, b"nope".to_vec())),
        );
        assert!(annotations_json_value_with_status(&doc, None).1);
    }

    #[test]
    fn a_buttons_type_may_come_from_its_parent() {
        let mut widget = on_state(note(vec![]));
        widget.set("Subtype", Object::Name(b"Widget".to_vec()));
        widget.set("Parent", Object::Reference((11, 0)));
        let mut doc = state_doc(widget, appearance_stream(b"BT /ZaDb 12 Tf (4) Tj ET"));
        let mut parent = Dictionary::new();
        parent.set("FT", Object::Name(b"Btn".to_vec()));
        doc.objects.insert((11, 0), Object::Dictionary(parent));
        assert!(!annotations_json_value_with_status(&doc, None).1);
    }

    #[test]
    fn a_state_name_that_is_not_utf8_still_finds_its_stream() {
        let mut states = Dictionary::new();
        states.set(b"Oui\xE9".to_vec(), Object::Reference((10, 0)));
        let mut d = note(vec![("AP", ap_n(Object::Dictionary(states)))]);
        d.set("AS", Object::Name(b"Oui\xE9".to_vec()));
        let doc = state_doc(d, appearance_stream(b"BT /Helv 12 Tf (Lockbox 7301) Tj ET"));
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotations"][0]["appearance_text"], "Lockbox 7301");
    }

    #[test]
    fn a_state_that_is_not_a_stream_reference_is_unreliable_not_absent() {
        let mut states = Dictionary::new();
        states.set("On", Object::Integer(1));
        let mut d = note(vec![("AP", ap_n(Object::Dictionary(states)))]);
        d.set("AS", Object::Name(b"On".to_vec()));
        let doc = page_doc(vec![Object::Dictionary(d)], vec![]);
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        assert_eq!(
            v["annotations"][0]["appearance_reliability"]["verdict"],
            "unreliable"
        );
        assert!(had_issues);
    }

    fn cid_form(content: &[u8]) -> Object {
        let mut cid = Dictionary::new();
        cid.set("Subtype", Object::Name(b"Type0".to_vec()));
        cid.set("BaseFont", Object::Name(b"Mystery".to_vec()));
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Dictionary(cid));
        let mut res = Dictionary::new();
        res.set("Font", Object::Dictionary(fonts));
        let mut form = Dictionary::new();
        form.set("Resources", Object::Dictionary(res));
        Object::Stream(Stream::new(form, content.to_vec()))
    }

    fn appearance_verdict_of(appearance: Object) -> (String, bool) {
        let doc = state_doc(
            note(vec![("AP", ap_n(Object::Reference((10, 0))))]),
            appearance,
        );
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        let verdict = v["annotations"][0]["appearance_reliability"]["verdict"]
            .as_str()
            .unwrap()
            .to_string();
        (verdict, had_issues)
    }

    #[test]
    fn text_shown_with_no_font_selected_is_not_reliable() {
        assert_eq!(
            appearance_verdict_of(cid_form(b"BT <00120034> Tj ET")),
            ("unreliable".to_string(), true)
        );
    }

    #[test]
    fn a_font_name_the_resources_lack_is_not_reliable() {
        assert_eq!(
            appearance_verdict_of(cid_form(b"BT /Nope 12 Tf <00120034> Tj ET")),
            ("unreliable".to_string(), true)
        );
    }

    #[test]
    fn a_font_set_by_a_graphics_state_is_not_reliable() {
        let mut gs = Dictionary::new();
        gs.set(
            "Font",
            Object::Array(vec![Object::Reference((11, 0)), Object::Integer(12)]),
        );
        let mut states = Dictionary::new();
        states.set("G0", Object::Dictionary(gs));
        let mut res = font_resources();
        res.set("ExtGState", Object::Dictionary(states));
        let mut form = Dictionary::new();
        form.set("Resources", Object::Dictionary(res));
        let stream = Object::Stream(Stream::new(
            form,
            b"BT /Helv 12 Tf /G0 gs (shown) Tj ET".to_vec(),
        ));
        assert_eq!(
            appearance_verdict_of(stream),
            ("unreliable".to_string(), true)
        );
    }

    #[test]
    fn a_font_of_one_name_in_a_nested_form_does_not_lower_the_verdict() {
        // The outer stream selects /F1, Helvetica.  A form it draws declares a different, unreadable
        // /F1 and never selects it.
        let mut nested_res = Dictionary::new();
        let mut cid = Dictionary::new();
        cid.set("Subtype", Object::Name(b"Type0".to_vec()));
        cid.set("BaseFont", Object::Name(b"Mystery".to_vec()));
        let mut nested_fonts = Dictionary::new();
        nested_fonts.set("F1", Object::Dictionary(cid));
        nested_res.set("Font", Object::Dictionary(nested_fonts));
        let mut nested = Dictionary::new();
        nested.set("Subtype", Object::Name(b"Form".to_vec()));
        nested.set("Resources", Object::Dictionary(nested_res));

        let mut helv = Dictionary::new();
        helv.set("Subtype", Object::Name(b"Type1".to_vec()));
        helv.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Dictionary(helv));
        let mut xobjects = Dictionary::new();
        xobjects.set("Fm", Object::Reference((11, 0)));
        let mut res = Dictionary::new();
        res.set("Font", Object::Dictionary(fonts));
        res.set("XObject", Object::Dictionary(xobjects));
        let mut outer = Dictionary::new();
        outer.set("Resources", Object::Dictionary(res));

        let mut doc = state_doc(
            note(vec![("AP", ap_n(Object::Reference((10, 0))))]),
            Object::Stream(Stream::new(
                outer,
                b"BT /F1 12 Tf (ok) Tj ET /Fm Do".to_vec(),
            )),
        );
        doc.objects.insert(
            (11, 0),
            Object::Stream(Stream::new(nested, b"q Q".to_vec())),
        );
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        assert_eq!(
            v["annotations"][0]["appearance_reliability"]["verdict"],
            "reliable"
        );
        assert!(!had_issues);
    }

    #[test]
    fn a_widget_appearance_with_no_resources_uses_the_forms_default_resources() {
        let mut doc = state_doc(
            {
                let mut d = note(vec![("AP", ap_n(Object::Reference((10, 0))))]);
                d.set("Subtype", Object::Name(b"Widget".to_vec()));
                d
            },
            Object::Stream(Stream::new(
                Dictionary::new(),
                b"BT /Helv 12 Tf (From DR) Tj ET".to_vec(),
            )),
        );
        let mut form = Dictionary::new();
        form.set("DR", Object::Dictionary(font_resources()));
        if let Ok(Object::Dictionary(catalog)) = doc.get_object_mut((102, 0)) {
            catalog.set("AcroForm", Object::Dictionary(form));
        }
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotations"][0]["appearance_text"], "From DR");
        assert_eq!(
            v["annotations"][0]["appearance_reliability"]["verdict"],
            "reliable"
        );
    }

    #[test]
    fn a_comment_of_the_wrong_type_is_flagged_not_clean() {
        let doc = page_doc(
            vec![Object::Dictionary(note(vec![
                (
                    "Contents",
                    Object::Array(vec![literal(b"Lockbox code 7301")]),
                ),
                ("RC", Object::Array(vec![literal(b"rich 7301")])),
            ]))],
            vec![],
        );
        let (v, had_issues) = annotations_json_value_with_status(&doc, None);
        let a = &v["annotations"][0];
        assert_eq!(a["contents_decoded"], false);
        assert!(
            a["contents"]
                .as_str()
                .unwrap()
                .contains("Lockbox code 7301")
        );
        assert_eq!(a["rich_contents_decoded"], false);
        assert!(a["rich_contents"].as_str().unwrap().contains("rich 7301"));
        assert!(had_issues);
    }

    #[test]
    fn appearance_warnings_print_in_text_mode() {
        let doc = page_doc(
            vec![Object::Dictionary({
                let mut states = Dictionary::new();
                states.set("On", Object::Integer(1));
                let mut d = note(vec![("AP", ap_n(Object::Dictionary(states)))]);
                d.set("AS", Object::Name(b"On".to_vec()));
                d
            })],
            vec![],
        );
        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("appearance warning:"), "{out}");
    }

    #[test]
    fn annotation_without_an_appearance_says_null() {
        let doc = page_doc(
            vec![Object::Dictionary(note(vec![("Contents", literal(b"x"))]))],
            vec![],
        );
        let v = annotations_json_value(&doc, None);
        assert_eq!(v["annotations"][0]["appearance_text"], Value::Null);
        assert_eq!(v["annotations"][0]["appearance_reliability"], Value::Null);
    }

    #[test]
    fn annotations_link_annotation() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("1 annotations found"));
        assert!(out.contains("Link"));
        assert!(out.contains("Click here"));
    }

    #[test]
    fn annotations_page_filter() {
        let doc = make_doc_with_annotations();
        // Page 1 has annotations
        let spec1 = PageSpec::Single(1);
        let out = output_of(|w| print_annotations(w, &doc, Some(&spec1)));
        assert!(out.contains("1 annotations found"));
        // Page 2 doesn't exist, should return 0
        let spec2 = PageSpec::Single(2);
        let out2 = output_of(|w| print_annotations(w, &doc, Some(&spec2)));
        assert!(out2.contains("0 annotations found"));
    }

    #[test]
    fn annotations_no_annotations() {
        let mut doc = Document::new();
        // Page without /Annots
        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages_dict.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
        }

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("0 annotations found"));
    }

    #[test]
    fn annotations_json_output() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| render_json(w, &annotations_json_value(&doc, None)));
        let val: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["annotation_count"], 1);
        assert_eq!(val["annotations"][0]["subtype"], "Link");
        assert_eq!(val["annotations"][0]["contents"], "Click here");
    }

    #[test]
    fn annotations_text_annotation() {
        let mut doc = Document::new();

        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(10),
                Object::Integer(20),
                Object::Integer(30),
                Object::Integer(40),
            ]),
        );
        annot.set(
            "Contents",
            Object::String(b"A note".to_vec(), StringFormat::Literal),
        );
        let annot_id = doc.add_object(Object::Dictionary(annot));

        let mut page_dict = Dictionary::new();
        page_dict.set("Type", Object::Name(b"Page".to_vec()));
        page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        let page_id = doc.add_object(Object::Dictionary(page_dict));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages_dict.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages_dict));

        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
        }

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let out = output_of(|w| print_annotations(w, &doc, None));
        assert!(out.contains("Text"));
        assert!(out.contains("A note"));
    }

    #[test]
    fn annotations_link_uri_type_and_target() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"URI".to_vec()));
        action.set(
            "URI",
            Object::String(b"https://example.com".to_vec(), StringFormat::Literal),
        );

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].link_type.as_deref(), Some("URI"));
        assert_eq!(
            annotations[0].target.as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn annotations_link_goto_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set(
            "Dest",
            Object::Array(vec![
                Object::Reference((10, 0)),
                Object::Name(b"Fit".to_vec()),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].link_type.as_deref(), Some("GoTo"));
    }

    #[test]
    fn annotations_non_link_has_no_link_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert!(annotations[0].link_type.is_none());
        assert!(annotations[0].target.is_none());
    }

    #[test]
    fn annotations_json_includes_link_fields() {
        let doc = make_doc_with_annotations();
        let out = output_of(|w| render_json(w, &annotations_json_value(&doc, None)));
        let val: Value = serde_json::from_str(&out).unwrap();
        // Link annotation should have link_type populated
        let annot = &val["annotations"][0];
        assert_eq!(annot["subtype"], "Link");
        assert!(annot.get("link_type").is_some());
        assert!(annot.get("target").is_some());
    }

    #[test]
    fn annotations_link_gotor_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"GoToR".to_vec()));
        action.set(
            "F",
            Object::String(b"other.pdf".to_vec(), StringFormat::Literal),
        );
        action.set(
            "D",
            Object::Array(vec![Object::Integer(0), Object::Name(b"Fit".to_vec())]),
        );

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("GoToR"));
        assert!(
            annotations[0]
                .target
                .as_ref()
                .unwrap()
                .contains("other.pdf")
        );
    }

    #[test]
    fn annotations_link_named_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"Named".to_vec()));
        action.set("N", Object::Name(b"NextPage".to_vec()));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("Named"));
        assert_eq!(annotations[0].target.as_deref(), Some("NextPage"));
    }

    #[test]
    fn annotations_link_launch_type() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"Launch".to_vec()));
        action.set(
            "F",
            Object::String(b"app.exe".to_vec(), StringFormat::Literal),
        );

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("Launch"));
        assert_eq!(annotations[0].target.as_deref(), Some("app.exe"));
    }

    #[test]
    fn annotations_missing_subtype() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        // No Subtype
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(50),
                Object::Integer(50),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].subtype, "-");
    }

    #[test]
    fn annotations_missing_rect() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        // No Rect
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].rect, "-");
    }

    #[test]
    fn annotations_highlight_type() {
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Highlight".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(10),
                Object::Integer(20),
                Object::Integer(30),
                Object::Integer(40),
            ]),
        );
        annot.set(
            "Contents",
            Object::String(b"Highlighted text".to_vec(), StringFormat::Literal),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].subtype, "Highlight");
        assert_eq!(annotations[0].contents, "Highlighted text");
        // Highlight is not a Link, so no link_type
        assert!(annotations[0].link_type.is_none());
    }

    #[test]
    fn annotations_multiple_on_same_page() {
        let mut doc = Document::new();

        let mut a1 = Dictionary::new();
        a1.set("Subtype", Object::Name(b"Text".to_vec()));
        a1.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(50),
                Object::Integer(50),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(a1));

        let mut a2 = Dictionary::new();
        a2.set("Subtype", Object::Name(b"Link".to_vec()));
        a2.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(60),
                Object::Integer(60),
                Object::Integer(100),
                Object::Integer(100),
            ]),
        );
        doc.objects.insert((21, 0), Object::Dictionary(a2));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0), (21, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 2);
    }

    #[test]
    fn annotations_annots_as_reference() {
        // Arrange: /Annots is an indirect reference to an array
        let mut doc = Document::new();
        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(50),
                Object::Integer(50),
            ]),
        );
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        // Annots array as its own object
        let annots_array = Object::Array(vec![Object::Reference((20, 0))]);
        doc.objects.insert((30, 0), annots_array);

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((2, 0)));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ]),
        );
        page.set("Annots", Object::Reference((30, 0))); // Reference to array
        doc.objects.insert((10, 0), Object::Dictionary(page));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].subtype, "Text");
    }

    #[test]
    fn annotations_page_range_filter() {
        let doc = make_doc_with_annotations();
        let spec = PageSpec::Range(1, 1);
        let annotations = collect_annotations(&doc, Some(&spec));
        assert_eq!(annotations.len(), 1);
    }

    #[test]
    fn annotations_link_unknown_action() {
        let mut doc = Document::new();
        let mut action = Dictionary::new();
        action.set("S", Object::Name(b"CustomAction".to_vec()));

        let mut annot = Dictionary::new();
        annot.set("Subtype", Object::Name(b"Link".to_vec()));
        annot.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(100),
                Object::Integer(20),
            ]),
        );
        annot.set("A", Object::Dictionary(action));
        doc.objects.insert((20, 0), Object::Dictionary(annot));

        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((2, 0), Object::Dictionary(pages_dict));
        make_page_with_annots(&mut doc, (10, 0), (2, 0), &[(20, 0)]);

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference((2, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));

        let annotations = collect_annotations(&doc, None);
        assert_eq!(annotations[0].link_type.as_deref(), Some("CustomAction"));
    }
}
