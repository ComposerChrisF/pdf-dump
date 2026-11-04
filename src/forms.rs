use lopdf::{Dictionary, Document, Object, ObjectId};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::helpers::{get_catalog, page_annots, resolve_array, resolve_dict};
use crate::stream::decode_stream;
use crate::text_string::{
    decode_text_string, find_elements, read_rich_text, read_text_stream, read_text_value,
    strip_markup,
};

pub(crate) struct FormFieldInfo {
    /// `None` for a field written directly in `/Fields` or `/Kids`.
    pub object_id: Option<ObjectId>,
    pub qualified_name: String,
    pub name_decoded: bool,
    pub field_type: String,
    /// The value as one string: `"text"` quoted, `/Name`, a number, `[…]` for a list, `<hex>` for
    /// a string that would not decode, or `(empty)`.
    pub value: String,
    /// The items of a list value (a multi-select), each decoded.
    pub values: Option<Vec<String>>,
    pub value_decoded: bool,
    /// The value came from an ancestor (`/V` is inheritable).
    pub inherited: bool,
    /// The words of `/RV`, markup removed.
    pub rich_value: Option<String>,
    pub rich_value_inherited: bool,
    pub rich_value_decoded: bool,
    /// A field whose kids are fields too, reported for the value it holds itself.
    pub has_field_kids: bool,
    pub page_number: Option<u32>,
    pub flags: u32,
}

/// `/XFA` from the AcroForm: an XFA form's values live in its datasets, not in `/Fields`.
pub(crate) struct XfaInfo {
    pub packets: Vec<String>,
    /// The text of the datasets, markup removed; `None` when the form has none.
    pub datasets_text: Option<String>,
    /// False when a packet would not decode, was not UTF-8, or held malformed markup.
    pub decoded: bool,
}

pub(crate) fn collect_form_fields(doc: &Document) -> (Option<ObjectId>, bool, Vec<FormFieldInfo>) {
    // Find AcroForm in catalog
    let catalog_id = match doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|o| o.as_reference().ok())
    {
        Some(id) => id,
        None => return (None, false, vec![]),
    };
    let catalog = match doc.get_object(catalog_id) {
        Ok(Object::Dictionary(d)) => d,
        _ => return (None, false, vec![]),
    };
    let acroform_ref = match catalog.get(b"AcroForm") {
        Ok(Object::Reference(id)) => *id,
        Ok(Object::Dictionary(d)) => {
            // Inline AcroForm - use catalog_id as placeholder
            return collect_form_fields_from_dict(doc, d, catalog_id);
        }
        _ => return (None, false, vec![]),
    };
    let acroform_dict = match doc.get_object(acroform_ref) {
        Ok(Object::Dictionary(d)) => d,
        _ => return (Some(acroform_ref), false, vec![]),
    };
    collect_form_fields_from_dict(doc, acroform_dict, acroform_ref)
}

pub(crate) fn collect_form_fields_from_dict(
    doc: &Document,
    acroform_dict: &Dictionary,
    acroform_id: ObjectId,
) -> (Option<ObjectId>, bool, Vec<FormFieldInfo>) {
    let need_appearances = acroform_dict
        .get(b"NeedAppearances")
        .ok()
        .and_then(|v| match v {
            Object::Boolean(b) => Some(*b),
            _ => None,
        })
        .unwrap_or(false);

    let fields_array = match acroform_dict
        .get(b"Fields")
        .ok()
        .and_then(|f| resolve_array(doc, f))
    {
        Some(arr) => arr,
        None => return (Some(acroform_id), need_appearances, vec![]),
    };

    // Build page annotation map: widget object_id -> page number
    let pages = doc.get_pages();
    let mut widget_to_page: BTreeMap<ObjectId, u32> = BTreeMap::new();
    for (&page_num, &page_id) in &pages {
        if let Ok(Object::Dictionary(page_dict)) = doc.get_object(page_id) {
            for annot in page_annots(doc, page_dict) {
                if let Ok(id) = annot.as_reference() {
                    widget_to_page.insert(id, page_num);
                }
            }
        }
    }

    let mut walk = FieldWalk {
        doc,
        widget_to_page,
        fields: Vec::new(),
        visited: BTreeSet::new(),
    };
    for field in fields_array {
        walk.visit(field, "", true, &Inherited::default());
    }

    (Some(acroform_id), need_appearances, walk.fields)
}

/// What a field takes from its ancestors when it does not say for itself.
#[derive(Clone, Default)]
struct Inherited<'a> {
    field_type: Option<String>,
    flags: Option<i64>,
    value: Option<&'a Object>,
    rich_value: Option<&'a Object>,
}

struct FieldWalk<'a> {
    doc: &'a Document,
    widget_to_page: BTreeMap<ObjectId, u32>,
    fields: Vec<FormFieldInfo>,
    /// Every field id already entered, so a `/Kids` cycle (a field listing itself or an ancestor)
    /// is entered once and then skipped instead of recursing until the stack overflows.
    visited: BTreeSet<ObjectId>,
}

fn field_dict<'a>(
    doc: &'a Document,
    field: &'a Object,
) -> Option<(Option<ObjectId>, &'a Dictionary)> {
    match field {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Dictionary(d) => Some((Some(*id), d)),
            Object::Stream(s) => Some((Some(*id), &s.dict)),
            _ => None,
        },
        Object::Dictionary(d) => Some((None, d)),
        _ => None,
    }
}

impl<'a> FieldWalk<'a> {
    fn visit(
        &mut self,
        field: &'a Object,
        parent_name: &str,
        parent_name_decoded: bool,
        from_parent: &Inherited<'a>,
    ) {
        let doc = self.doc;
        let Some((field_id, dict)) = field_dict(doc, field) else {
            return;
        };
        if let Some(id) = field_id
            && !self.visited.insert(id)
        {
            return;
        }

        let partial = dict.get(b"T").ok().and_then(|v| read_text_value(doc, v));
        let name_decoded = parent_name_decoded && partial.as_ref().is_none_or(|t| t.decoded);
        let partial_name = partial.map(|t| t.text).unwrap_or_default();
        let qualified_name = if parent_name.is_empty() {
            partial_name
        } else if partial_name.is_empty() {
            parent_name.to_string()
        } else {
            format!("{}.{}", parent_name, partial_name)
        };

        let inherited = Inherited {
            field_type: dict
                .get(b"FT")
                .ok()
                .and_then(|v| v.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .or_else(|| from_parent.field_type.clone()),
            flags: dict
                .get(b"Ff")
                .ok()
                .and_then(|v| v.as_i64().ok())
                .or(from_parent.flags),
            value: own_entry(doc, dict, b"V").or(from_parent.value),
            rich_value: own_entry(doc, dict, b"RV").or(from_parent.rich_value),
        };

        let kids = dict
            .get(b"Kids")
            .ok()
            .and_then(|k| resolve_array(doc, k))
            .unwrap_or(&[]);
        // A kid that names itself, holds a value, or has kids of its own is a field; the others are
        // this field's widgets.
        let has_field_kids = kids.iter().any(|kid| {
            field_dict(doc, kid)
                .is_some_and(|(_, d)| d.has(b"T") || d.has(b"V") || d.has(b"RV") || d.has(b"Kids"))
        });

        let own_value = own_entry(doc, dict, b"V").is_some();
        let own_rich_value = own_entry(doc, dict, b"RV").is_some();
        if !has_field_kids || own_value || own_rich_value {
            let is_signature = inherited.field_type.as_deref() == Some("Sig");
            let mut render = ValueRender {
                doc,
                is_signature,
                budget: MAX_VALUE_NODES,
            };
            let rendered = inherited
                .value
                .map_or_else(RenderedValue::empty, |v| render.value(v, 0));
            let rich = inherited.rich_value.and_then(|v| read_rich_text(doc, v));
            self.fields.push(FormFieldInfo {
                object_id: field_id,
                qualified_name: qualified_name.clone(),
                name_decoded,
                field_type: inherited
                    .field_type
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
                value: rendered.display,
                values: rendered.items,
                value_decoded: rendered.decoded,
                inherited: inherited.value.is_some() && !own_value,
                rich_value_decoded: rich.as_ref().is_none_or(|r| r.decoded),
                rich_value: rich.map(|r| r.text),
                rich_value_inherited: inherited.rich_value.is_some() && !own_rich_value,
                has_field_kids,
                page_number: field_id.and_then(|id| self.widget_to_page.get(&id).copied()),
                flags: inherited.flags.unwrap_or(0) as u32,
            });
        }

        if has_field_kids {
            for kid in kids {
                self.visit(kid, &qualified_name, name_decoded, &inherited);
            }
        }
    }
}

/// An entry the field holds itself.  Null, or a reference to nothing, is the same as no entry
/// (§ 7.3.9), so inheritance applies.
fn own_entry<'a>(doc: &Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let value = dict.get(key).ok()?;
    match value {
        Object::Null => None,
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::Null) | Err(lopdf::Error::ObjectNotFound(_)) => None,
            _ => Some(value),
        },
        _ => Some(value),
    }
}

struct RenderedValue {
    display: String,
    items: Option<Vec<String>>,
    decoded: bool,
}

impl RenderedValue {
    fn empty() -> Self {
        RenderedValue {
            display: "(empty)".to_string(),
            items: None,
            decoded: true,
        }
    }
    fn text(text: crate::text_string::TextString) -> Self {
        RenderedValue {
            display: if text.decoded {
                format!("\"{}\"", text.text)
            } else {
                text.text
            },
            items: None,
            decoded: text.decoded,
        }
    }
    fn scalar(display: String) -> Self {
        RenderedValue {
            display,
            items: None,
            decoded: true,
        }
    }
}

/// Bounds how much of one value is walked.  Depth alone would not: an array of a dozen references
/// to itself is shallow to look at and takes a dozen to the sixteenth steps to read.
const MAX_VALUE_NODES: u32 = 10_000;
const MAX_VALUE_DEPTH: u32 = 16;

/// In a signature, `/ByteRange`, `/Contents` and their kin are binary evidence, not text anyone reads.
const SIGNATURE_BINARY_KEYS: [&[u8]; 4] = [b"ByteRange", b"Cert", b"Contents", b"Reference"];

struct ValueRender<'a> {
    doc: &'a Document,
    is_signature: bool,
    budget: u32,
}

impl ValueRender<'_> {
    fn value(&mut self, value: &Object, depth: u32) -> RenderedValue {
        if depth > MAX_VALUE_DEPTH || self.budget == 0 {
            return RenderedValue {
                display: "(nested too deeply to read)".to_string(),
                items: None,
                decoded: false,
            };
        }
        self.budget -= 1;
        let doc = self.doc;
        match value {
            Object::String(bytes, _) => RenderedValue::text(decode_text_string(bytes)),
            Object::Stream(stream) => RenderedValue::text(read_text_stream(stream)),
            Object::Name(n) => RenderedValue::scalar(format!("/{}", String::from_utf8_lossy(n))),
            Object::Integer(i) => RenderedValue::scalar(i.to_string()),
            Object::Real(r) => RenderedValue::scalar(r.to_string()),
            Object::Boolean(b) => RenderedValue::scalar(b.to_string()),
            Object::Null => RenderedValue::empty(),
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(target) => self.value(target, depth + 1),
                // A reference to nothing reads as null (ISO 32000-1 § 7.3.10).
                Err(lopdf::Error::ObjectNotFound(_)) => RenderedValue::empty(),
                // A cycle: lopdf gave up, which says nothing about the value being empty.
                Err(_) => RenderedValue {
                    display: "(unresolvable reference)".to_string(),
                    items: None,
                    decoded: false,
                },
            },
            Object::Array(items) => {
                let rendered: Vec<RenderedValue> = items
                    .iter()
                    .map(|item| self.value(item, depth + 1))
                    .collect();
                let decoded = rendered.iter().all(|r| r.decoded);
                let parts: Vec<String> = rendered.into_iter().map(|r| r.display).collect();
                RenderedValue {
                    display: format!("[{}]", parts.join(", ")),
                    items: Some(parts.iter().map(|p| unquote(p).to_string()).collect()),
                    decoded,
                }
            }
            Object::Dictionary(d) => {
                let mut decoded = true;
                let mut parts = Vec::new();
                for (key, item) in d.iter() {
                    if self.is_signature && SIGNATURE_BINARY_KEYS.contains(&key.as_slice()) {
                        continue;
                    }
                    let r = self.value(item, depth + 1);
                    decoded &= r.decoded;
                    parts.push(format!("/{} {}", String::from_utf8_lossy(key), r.display));
                }
                RenderedValue {
                    display: format!("<<{}>>", parts.join(" ")),
                    items: None,
                    decoded,
                }
            }
        }
    }
}

fn unquote(display: &str) -> &str {
    display
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(display)
}

pub(crate) fn collect_xfa(doc: &Document) -> Option<XfaInfo> {
    let acroform = resolve_dict(doc, get_catalog(doc)?.get(b"AcroForm").ok()?)?;
    let mut xfa = acroform.get(b"XFA").ok()?;
    if let Object::Reference(id) = xfa {
        match doc.get_object(*id) {
            Ok(target) => xfa = target,
            Err(_) => {
                return Some(XfaInfo {
                    packets: vec![],
                    datasets_text: None,
                    decoded: false,
                });
            }
        }
    }

    let mut read_failed = false;
    let mut packets = Vec::new();
    // Each datasets packet whole, and each datasets element of any other packet.
    let mut datasets: Vec<String> = Vec::new();
    match xfa {
        // One stream holding the whole XDP document.
        Object::Stream(_) => match read_xfa_packet(doc, xfa) {
            Some(xml) => datasets.extend(
                find_elements(&xml, "datasets")
                    .into_iter()
                    .map(str::to_string),
            ),
            None => read_failed = true,
        },
        // Packets as pairs: a name, then a stream.
        Object::Array(items) => {
            for pair in items.chunks(2) {
                let name = pair
                    .first()
                    .and_then(|n| match n {
                        Object::String(bytes, _) => Some(decode_text_string(bytes).text),
                        _ => None,
                    })
                    .unwrap_or_default();
                match pair.get(1).and_then(|stream| read_xfa_packet(doc, stream)) {
                    Some(xml) if name == "datasets" => datasets.push(xml),
                    Some(xml) => datasets.extend(
                        find_elements(&xml, "datasets")
                            .into_iter()
                            .map(str::to_string),
                    ),
                    None => read_failed = true,
                }
                packets.push(name);
            }
        }
        _ => read_failed = true,
    }

    let mut well_formed = true;
    let texts: Vec<String> = datasets
        .iter()
        .map(|xml| {
            let (text, ok) = strip_markup(xml);
            well_formed &= ok;
            text
        })
        .collect();
    Some(XfaInfo {
        packets,
        datasets_text: (!texts.is_empty()).then(|| texts.join("\n")),
        decoded: well_formed && !read_failed,
    })
}

/// One XFA packet as text; `None` when it is not a stream, will not decode, or is not UTF-8.
fn read_xfa_packet(doc: &Document, obj: &Object) -> Option<String> {
    let stream = match obj {
        Object::Stream(s) => s,
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Stream(s) => s,
            _ => return None,
        },
        _ => return None,
    };
    let (bytes, warning) = decode_stream(stream);
    if warning.is_some() {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    Some(text.trim_start_matches('\u{FEFF}').to_string())
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn warn_unread(fields: &[FormFieldInfo], xfa: Option<&XfaInfo>) -> bool {
    let unread = fields
        .iter()
        .filter(|f| !f.value_decoded || !f.rich_value_decoded)
        .count();
    if unread > 0 {
        eprintln!(
            "Warning: {unread} form field value(s) could not be decoded; their bytes are shown as hex."
        );
    }
    let xfa_unread = xfa.is_some_and(|x| !x.decoded);
    if xfa_unread {
        eprintln!("Warning: the form's XFA data could not be fully read.");
    }
    unread > 0 || xfa_unread
}

/// Returns `true` when a value could not be decoded (a finding, exit 3).
pub(crate) fn print_forms(writer: &mut impl Write, doc: &Document) -> bool {
    let (acroform_id, need_appearances, fields) = collect_form_fields(doc);
    let xfa = collect_xfa(doc);

    match acroform_id {
        None => {
            wln!(writer, "No AcroForm found in document.");
            return false;
        }
        Some(id) => {
            wln!(
                writer,
                "AcroForm found (obj {}), NeedAppearances: {}",
                id.0,
                need_appearances
            );
        }
    }

    if let Some(x) = &xfa {
        wln!(writer, "XFA form: yes (packets: {})", x.packets.join(", "));
        if let Some(text) = &x.datasets_text {
            wln!(writer, "XFA datasets text:\n{}", text);
        }
    }

    wln!(writer, "{} form fields\n", fields.len());
    if !fields.is_empty() {
        wln!(
            writer,
            "  {:>4}  {:<24} {:<6}  {:<20} Page",
            "Obj#",
            "FieldName",
            "Type",
            "Value"
        );
    }
    for f in &fields {
        let page_str = f
            .page_number
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        let object = f
            .object_id
            .map_or_else(|| "-".to_string(), |id| id.0.to_string());
        let inherited = if f.inherited { " (inherited)" } else { "" };
        wln!(
            writer,
            "  {:>4}  {:<24} {:<6}  {:<20} {}",
            object,
            truncate_str(&f.qualified_name, 24),
            &f.field_type,
            format!("{}{}", f.value, inherited),
            page_str,
        );
        if let Some(rich) = &f.rich_value {
            wln!(writer, "        rich text: {}", rich);
        }
    }
    warn_unread(&fields, xfa.as_ref())
}

#[cfg(test)]
pub(crate) fn forms_json_value(doc: &Document) -> Value {
    forms_json_value_with_status(doc).0
}

/// Returns the JSON and whether a value could not be decoded (a finding, exit 3).
pub(crate) fn forms_json_value_with_status(doc: &Document) -> (Value, bool) {
    let (acroform_id, need_appearances, fields) = collect_form_fields(doc);
    let xfa = collect_xfa(doc);

    let items: Vec<Value> = fields
        .iter()
        .map(|f| {
            json!({
                "object_number": f.object_id.map(|id| id.0),
                "generation": f.object_id.map(|id| id.1),
                "field_name": f.qualified_name,
                "field_name_decoded": f.name_decoded,
                "field_type": f.field_type,
                "value": f.value,
                "values": f.values,
                "value_decoded": f.value_decoded,
                "inherited": f.inherited,
                "rich_value": f.rich_value,
                "rich_value_inherited": f.rich_value_inherited,
                "rich_value_decoded": f.rich_value_decoded,
                "has_field_kids": f.has_field_kids,
                "flags": f.flags,
                "page_number": f.page_number,
            })
        })
        .collect();

    let had_issues = warn_unread(&fields, xfa.as_ref());
    (
        json!({
            "acroform_object": acroform_id.map(|id| id.0),
            "need_appearances": need_appearances,
            "xfa": xfa.is_some(),
            "xfa_packets": xfa.as_ref().map_or(&[][..], |x| x.packets.as_slice()),
            "xfa_datasets_text": xfa.as_ref().and_then(|x| x.datasets_text.as_ref()),
            "xfa_decoded": xfa.as_ref().is_none_or(|x| x.decoded),
            "field_count": items.len(),
            "fields": items,
        }),
        had_issues,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use lopdf::Object;
    use lopdf::{Dictionary, StringFormat};
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    /// A catalog whose `/AcroForm /Fields` lists `fields`, with `objs` added verbatim.
    fn form_doc_with_fields(objs: Vec<(ObjectId, Dictionary)>, fields: Vec<ObjectId>) -> Document {
        let mut doc = Document::new();
        for (id, d) in objs {
            doc.objects.insert(id, Object::Dictionary(d));
        }
        let mut acroform = Dictionary::new();
        acroform.set(
            "Fields",
            Object::Array(fields.into_iter().map(Object::Reference).collect()),
        );
        let af_id = doc.add_object(Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference(af_id));
        let cat_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(cat_id));
        doc
    }

    fn named_field(name: &str, kids: Vec<ObjectId>) -> Dictionary {
        let mut d = Dictionary::new();
        d.set(
            "T",
            Object::String(name.as_bytes().to_vec(), StringFormat::Literal),
        );
        d.set("FT", Object::Name(b"Tx".to_vec()));
        d.set(
            "Kids",
            Object::Array(kids.into_iter().map(Object::Reference).collect()),
        );
        d
    }

    #[test]
    fn field_kids_self_cycle_terminates() {
        // bug-0013: `4 0 obj << /T (loop) /Kids [4 0 R] >>` overflowed the stack.
        let doc = form_doc_with_fields(
            vec![((4, 0), named_field("loop", vec![(4, 0)]))],
            vec![(4, 0)],
        );
        let (_, _, fields) = collect_form_fields(&doc);
        assert!(
            fields.len() <= 1,
            "cycle must not multiply fields: {}",
            fields.len()
        );
    }

    #[test]
    fn field_kids_two_node_cycle_terminates() {
        let doc = form_doc_with_fields(
            vec![
                ((4, 0), named_field("a", vec![(5, 0)])),
                ((5, 0), named_field("b", vec![(4, 0)])),
            ],
            vec![(4, 0)],
        );
        let (_, _, fields) = collect_form_fields(&doc);
        assert!(
            fields.len() <= 2,
            "cycle must not multiply fields: {}",
            fields.len()
        );
    }

    #[test]
    fn field_kids_cycle_overview_survives() {
        // The crash was reachable from the default command, which counts fields.
        let doc = form_doc_with_fields(
            vec![((4, 0), named_field("loop", vec![(4, 0)]))],
            vec![(4, 0)],
        );
        let out = output_of(|w| print_forms(w, &doc));
        assert!(!out.is_empty());
    }

    fn build_form_doc() -> Document {
        let mut doc = Document::new();
        // Form field 1 - text field
        let mut field1 = Dictionary::new();
        field1.set(
            "T",
            Object::String(b"FirstName".to_vec(), StringFormat::Literal),
        );
        field1.set("FT", Object::Name(b"Tx".to_vec()));
        field1.set("V", Object::String(b"John".to_vec(), StringFormat::Literal));
        doc.objects.insert((20, 0), Object::Dictionary(field1));
        // Form field 2 - button
        let mut field2 = Dictionary::new();
        field2.set(
            "T",
            Object::String(b"Subscribe".to_vec(), StringFormat::Literal),
        );
        field2.set("FT", Object::Name(b"Btn".to_vec()));
        field2.set("V", Object::Name(b"Yes".to_vec()));
        doc.objects.insert((22, 0), Object::Dictionary(field2));
        // AcroForm
        let mut acroform = Dictionary::new();
        acroform.set("NeedAppearances", Object::Boolean(true));
        acroform.set(
            "Fields",
            Object::Array(vec![Object::Reference((20, 0)), Object::Reference((22, 0))]),
        );
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        // Catalog
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        // Need Pages for page mapping
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(1));
        pages_dict.set("Kids", Object::Array(vec![Object::Reference((30, 0))]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        // Page with annotations pointing to fields
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference((5, 0)));
        page.set(
            "Annots",
            Object::Array(vec![Object::Reference((20, 0)), Object::Reference((22, 0))]),
        );
        doc.objects.insert((30, 0), Object::Dictionary(page));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        doc
    }

    #[test]
    fn forms_shows_fields() {
        let doc = build_form_doc();
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("AcroForm found"));
        assert!(out.contains("NeedAppearances: true"));
        assert!(out.contains("2 form fields"));
        assert!(out.contains("FirstName"));
        assert!(out.contains("Tx"));
        assert!(out.contains("\"John\""));
        assert!(out.contains("Subscribe"));
        assert!(out.contains("Btn"));
        assert!(out.contains("/Yes"));
    }

    #[test]
    fn forms_json_structure() {
        let doc = build_form_doc();
        let out = output_of(|w| render_json(w, &forms_json_value(&doc)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["acroform_object"].is_number());
        assert_eq!(parsed["need_appearances"], true);
        assert_eq!(parsed["field_count"], 2);
        assert!(parsed["fields"].is_array());
        let fields = parsed["fields"].as_array().unwrap();
        assert_eq!(fields[0]["field_name"], "FirstName");
        assert_eq!(fields[0]["field_type"], "Tx");
    }

    #[test]
    fn forms_page_mapping() {
        let doc = build_form_doc();
        let out = output_of(|w| render_json(w, &forms_json_value(&doc)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let fields = parsed["fields"].as_array().unwrap();
        // Fields should be mapped to page 1 via Annots
        assert_eq!(fields[0]["page_number"], 1);
    }

    #[test]
    fn forms_no_acroform() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("No AcroForm found"));
    }

    #[test]
    fn forms_empty_fields() {
        let mut doc = Document::new();
        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        assert!(out.contains("0 form fields"));
    }

    #[test]
    fn forms_hierarchical_fields() {
        let mut doc = Document::new();
        // Child field
        let mut child = Dictionary::new();
        child.set(
            "T",
            Object::String(b"FirstName".to_vec(), StringFormat::Literal),
        );
        child.set("FT", Object::Name(b"Tx".to_vec()));
        child.set(
            "V",
            Object::String(b"Alice".to_vec(), StringFormat::Literal),
        );
        doc.objects.insert((21, 0), Object::Dictionary(child));
        // Parent field with Kids
        let mut parent = Dictionary::new();
        parent.set(
            "T",
            Object::String(b"Person".to_vec(), StringFormat::Literal),
        );
        parent.set("Kids", Object::Array(vec![Object::Reference((21, 0))]));
        doc.objects.insert((20, 0), Object::Dictionary(parent));
        // AcroForm
        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));
        let out = output_of(|w| print_forms(w, &doc));
        // Should show qualified name Person.FirstName
        assert!(out.contains("Person.FirstName"));
    }

    #[test]
    fn forms_json_no_acroform() {
        let mut doc = Document::new();
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let out = output_of(|w| render_json(w, &forms_json_value(&doc)));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["acroform_object"].is_null());
        assert_eq!(parsed["field_count"], 0);
    }

    #[test]
    fn forms_inline_acroform() {
        // Arrange: AcroForm is inlined in catalog (not a reference)
        let mut doc = Document::new();
        let mut field = Dictionary::new();
        field.set("T", Object::String(b"Name".to_vec(), StringFormat::Literal));
        field.set("FT", Object::Name(b"Tx".to_vec()));
        doc.objects.insert((20, 0), Object::Dictionary(field));

        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Dictionary(acroform));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (acroform_id, _, fields) = collect_form_fields(&doc);
        assert!(acroform_id.is_some());
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].qualified_name, "Name");
    }

    #[test]
    fn forms_field_missing_type() {
        // Field without /FT should show "-"
        let mut doc = Document::new();
        let mut field = Dictionary::new();
        field.set(
            "T",
            Object::String(b"NoType".to_vec(), StringFormat::Literal),
        );
        // No FT key
        doc.objects.insert((20, 0), Object::Dictionary(field));

        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (_, _, fields) = collect_form_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field_type, "-");
    }

    #[test]
    fn forms_field_missing_value() {
        // Field without /V should show "(empty)"
        let mut doc = Document::new();
        let mut field = Dictionary::new();
        field.set(
            "T",
            Object::String(b"Empty".to_vec(), StringFormat::Literal),
        );
        field.set("FT", Object::Name(b"Tx".to_vec()));
        // No V key
        doc.objects.insert((20, 0), Object::Dictionary(field));

        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (_, _, fields) = collect_form_fields(&doc);
        assert_eq!(fields[0].value, "(empty)");
    }

    #[test]
    fn forms_field_missing_name() {
        // Field without /T — partial name defaults to empty string
        let mut doc = Document::new();
        let mut field = Dictionary::new();
        field.set("FT", Object::Name(b"Tx".to_vec()));
        // No T key
        doc.objects.insert((20, 0), Object::Dictionary(field));

        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (_, _, fields) = collect_form_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].qualified_name, "");
    }

    #[test]
    fn forms_value_types() {
        // Test different /V value types
        let mut doc = Document::new();

        // Integer value
        let mut f1 = Dictionary::new();
        f1.set(
            "T",
            Object::String(b"IntField".to_vec(), StringFormat::Literal),
        );
        f1.set("FT", Object::Name(b"Tx".to_vec()));
        f1.set("V", Object::Integer(42));
        doc.objects.insert((20, 0), Object::Dictionary(f1));

        // Boolean value
        let mut f2 = Dictionary::new();
        f2.set(
            "T",
            Object::String(b"BoolField".to_vec(), StringFormat::Literal),
        );
        f2.set("FT", Object::Name(b"Btn".to_vec()));
        f2.set("V", Object::Boolean(true));
        doc.objects.insert((21, 0), Object::Dictionary(f2));

        // Array value
        let mut f3 = Dictionary::new();
        f3.set(
            "T",
            Object::String(b"ArrayField".to_vec(), StringFormat::Literal),
        );
        f3.set("FT", Object::Name(b"Ch".to_vec()));
        f3.set("V", Object::Array(vec![Object::Integer(1)]));
        doc.objects.insert((22, 0), Object::Dictionary(f3));

        let mut acroform = Dictionary::new();
        acroform.set(
            "Fields",
            Object::Array(vec![
                Object::Reference((20, 0)),
                Object::Reference((21, 0)),
                Object::Reference((22, 0)),
            ]),
        );
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (_, _, fields) = collect_form_fields(&doc);
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].value, "42");
        assert_eq!(fields[1].value, "true");
        assert!(fields[2].value.starts_with('['), "{}", fields[2].value);
        assert!(fields[2].values.is_some());
    }

    // ── plan-0006: every value, decoded ───────────────────────────────

    fn utf16(s: &str) -> Vec<u8> {
        let mut out = vec![0xFE, 0xFF];
        out.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        out
    }

    fn literal(bytes: &[u8]) -> Object {
        Object::String(bytes.to_vec(), StringFormat::Literal)
    }

    fn text_field(name: &str, value: Option<Object>) -> Dictionary {
        let mut d = Dictionary::new();
        d.set("T", literal(name.as_bytes()));
        d.set("FT", Object::Name(b"Tx".to_vec()));
        if let Some(v) = value {
            d.set("V", v);
        }
        d
    }

    fn only_field(doc: &Document) -> Value {
        let v = forms_json_value(doc);
        assert_eq!(v["field_count"], 1, "{v}");
        v["fields"][0].clone()
    }

    #[test]
    fn utf16_value_is_decoded_and_keeps_the_quoted_form() {
        let doc = form_doc_with_fields(
            vec![(
                (4, 0),
                text_field("ssn", Some(literal(&utf16("La‘i 123-45-6789")))),
            )],
            vec![(4, 0)],
        );
        let f = only_field(&doc);
        assert_eq!(f["value"], "\"La‘i 123-45-6789\"");
        assert_eq!(f["value_decoded"], true);
    }

    #[test]
    fn pdfdoc_value_and_utf16_name_are_decoded() {
        let mut d = text_field("x", Some(literal(&[0x93, 0xE9, 0xA0])));
        d.set("T", literal(&utf16("Kōkua")));
        let doc = form_doc_with_fields(vec![((4, 0), d)], vec![(4, 0)]);
        let f = only_field(&doc);
        assert_eq!(f["value"], "\"ﬁé€\"");
        assert_eq!(f["field_name"], "Kōkua");
    }

    #[test]
    fn undecodable_value_is_flagged_as_hex_and_is_a_finding() {
        let doc = form_doc_with_fields(
            vec![((4, 0), text_field("x", Some(literal(&[0xFE, 0xFF, 0x00]))))],
            vec![(4, 0)],
        );
        let (v, had_issues) = forms_json_value_with_status(&doc);
        assert_eq!(v["fields"][0]["value"], "<FEFF00>");
        assert_eq!(v["fields"][0]["value_decoded"], false);
        assert!(had_issues);
        assert!(output_of(|w| assert!(print_forms(w, &doc))).contains("<FEFF00>"));
    }

    #[test]
    fn a_decodable_form_is_not_a_finding() {
        let doc = form_doc_with_fields(
            vec![((4, 0), text_field("x", Some(literal(b"plain"))))],
            vec![(4, 0)],
        );
        assert!(!forms_json_value_with_status(&doc).1);
    }

    #[test]
    fn list_value_is_a_json_array_of_decoded_items_and_a_string_in_value() {
        let doc = form_doc_with_fields(
            vec![(
                (4, 0),
                text_field(
                    "pick",
                    Some(Object::Array(vec![literal(b"one"), literal(&utf16("twō"))])),
                ),
            )],
            vec![(4, 0)],
        );
        let f = only_field(&doc);
        assert_eq!(f["values"], serde_json::json!(["one", "twō"]));
        // A string still, so a consumer that reads `value` as text reads the items, not nothing.
        assert_eq!(f["value"], "[\"one\", \"twō\"]");
    }

    #[test]
    fn stream_value_is_decoded_text() {
        let mut doc = form_doc_with_fields(
            vec![((4, 0), text_field("note", Some(Object::Reference((9, 0)))))],
            vec![(4, 0)],
        );
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            zlib_compress(&utf16("long text")),
        );
        doc.objects.insert((9, 0), Object::Stream(stream));
        let f = only_field(&doc);
        assert_eq!(f["value"], "\"long text\"");
        assert_eq!(f["value_decoded"], true);
    }

    #[test]
    fn stream_value_that_will_not_decode_is_flagged() {
        let mut doc = form_doc_with_fields(
            vec![((4, 0), text_field("note", Some(Object::Reference((9, 0)))))],
            vec![(4, 0)],
        );
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"nope".to_vec(),
        );
        doc.objects.insert((9, 0), Object::Stream(stream));
        assert_eq!(only_field(&doc)["value_decoded"], false);
    }

    #[test]
    fn reference_cycle_as_a_value_terminates_and_is_flagged() {
        let mut doc = form_doc_with_fields(
            vec![((4, 0), text_field("x", Some(Object::Reference((9, 0)))))],
            vec![(4, 0)],
        );
        doc.objects.insert((9, 0), Object::Reference((9, 0)));
        assert_eq!(only_field(&doc)["value_decoded"], false);
    }

    #[test]
    fn a_kid_inherits_a_parents_value_and_says_so() {
        let mut parent = text_field("ssn", Some(literal(b"123-45-6789")));
        parent.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        // A kid naming itself, with no value or type of its own.
        let mut kid = Dictionary::new();
        kid.set("T", literal(b"copy"));
        let doc = form_doc_with_fields(vec![((4, 0), parent), ((5, 0), kid)], vec![(4, 0)]);
        let v = forms_json_value(&doc);
        let fields = v["fields"].as_array().unwrap();
        // The parent holds the value itself, and the kid shows it too.
        assert_eq!(fields.len(), 2, "{v}");
        assert_eq!(fields[0]["field_name"], "ssn");
        assert_eq!(fields[0]["value"], "\"123-45-6789\"");
        assert_eq!(fields[0]["inherited"], false);
        assert_eq!(fields[0]["has_field_kids"], true);
        assert_eq!(fields[1]["field_name"], "ssn.copy");
        assert_eq!(fields[1]["value"], "\"123-45-6789\"");
        assert_eq!(fields[1]["inherited"], true);
        assert_eq!(fields[1]["field_type"], "Tx", "/FT inherits as well");
    }

    #[test]
    fn a_kids_own_value_is_not_marked_inherited_and_the_parents_is_still_seen() {
        let mut parent = text_field("p", Some(literal(b"from parent")));
        parent.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        let doc = form_doc_with_fields(
            vec![
                ((4, 0), parent),
                ((5, 0), text_field("k", Some(literal(b"own")))),
            ],
            vec![(4, 0)],
        );
        let v = forms_json_value(&doc);
        let values: Vec<&str> = v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["value"].as_str().unwrap())
            .collect();
        assert_eq!(values, vec!["\"from parent\"", "\"own\""]);
        assert_eq!(v["fields"][1]["inherited"], false);
    }

    #[test]
    fn a_parent_of_widgets_is_one_field_as_before() {
        let mut parent = text_field("p", Some(literal(b"v")));
        parent.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        let mut widget = Dictionary::new();
        widget.set("Subtype", Object::Name(b"Widget".to_vec()));
        let doc = form_doc_with_fields(vec![((4, 0), parent), ((5, 0), widget)], vec![(4, 0)]);
        assert_eq!(only_field(&doc)["value"], "\"v\"");
    }

    #[test]
    fn rich_value_is_reported_as_text() {
        let mut d = text_field("r", Some(literal(b"plain")));
        d.set(
            "RV",
            literal(b"<body><p>Lockbox <b>code</b>: 7301</p></body>"),
        );
        let doc = form_doc_with_fields(vec![((4, 0), d)], vec![(4, 0)]);
        let f = only_field(&doc);
        assert_eq!(f["rich_value"], "Lockbox code: 7301");
        assert_eq!(f["rich_value_decoded"], true);
        assert!(output_of(|w| print_forms(w, &doc)).contains("rich text: Lockbox code: 7301"));
    }

    #[test]
    fn malformed_rich_value_is_a_finding_and_keeps_its_text() {
        let mut d = text_field("r", None);
        d.set("RV", literal(b"<p>seen</p><p oops"));
        let doc = form_doc_with_fields(vec![((4, 0), d)], vec![(4, 0)]);
        let (v, had_issues) = forms_json_value_with_status(&doc);
        assert!(had_issues);
        assert_eq!(v["fields"][0]["rich_value_decoded"], false);
        assert!(
            v["fields"][0]["rich_value"]
                .as_str()
                .unwrap()
                .contains("seen")
        );
    }

    #[test]
    fn signature_value_reports_its_text_and_not_its_binary() {
        let mut sig = Dictionary::new();
        sig.set("Name", literal(&utf16("Jānet")));
        // Undefined in PDFDocEncoding: if this were read as text it would raise a finding.
        sig.set("Contents", literal(&[0x00, 0x01, 0x7F]));
        let mut field = text_field("s", Some(Object::Dictionary(sig)));
        field.set("FT", Object::Name(b"Sig".to_vec()));
        let doc = form_doc_with_fields(vec![((4, 0), field)], vec![(4, 0)]);
        let f = only_field(&doc);
        assert_eq!(f["value"], "<</Name \"Jānet\">>");
        assert_eq!(f["value_decoded"], true);
    }

    #[test]
    fn a_field_written_directly_in_fields_is_reported() {
        let mut doc = form_doc_with_fields(vec![], vec![]);
        let cat = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let af = doc
            .get_object(cat)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(af) {
            d.set(
                "Fields",
                Object::Array(vec![Object::Dictionary(text_field(
                    "direct",
                    Some(literal(b"seen")),
                ))]),
            );
        }
        let f = only_field(&doc);
        assert_eq!(f["value"], "\"seen\"");
        assert_eq!(f["object_number"], Value::Null);
    }

    fn xfa_doc(xfa: Object, extra: Vec<(ObjectId, Object)>) -> Document {
        let mut doc = form_doc_with_fields(vec![], vec![]);
        let cat = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let af = doc
            .get_object(cat)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(af) {
            d.set("XFA", xfa);
        }
        for (id, o) in extra {
            doc.objects.insert(id, o);
        }
        doc
    }

    const XDP: &[u8] = br#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template>Please wait</template><xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><code>Lockbox code: 7301</code><name>Jane &amp; Co</name></xfa:data></xfa:datasets></xdp:xdp>"#;

    #[test]
    fn xfa_in_one_stream_reports_the_datasets_text_and_not_the_template() {
        let stream = make_stream(None, XDP.to_vec());
        let doc = xfa_doc(
            Object::Reference((9, 0)),
            vec![((9, 0), Object::Stream(stream))],
        );
        let v = forms_json_value(&doc);
        assert_eq!(v["xfa"], true);
        assert_eq!(v["xfa_datasets_text"], "Lockbox code: 7301\nJane & Co");
        assert_eq!(v["xfa_decoded"], true);
    }

    #[test]
    fn xfa_as_packets_names_them_and_reads_the_datasets_packet() {
        let template = make_stream(None, b"<template>not this</template>".to_vec());
        let data = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            zlib_compress(b"<xfa:datasets><v>the value</v></xfa:datasets>"),
        );
        let doc = xfa_doc(
            Object::Array(vec![
                literal(b"template"),
                Object::Reference((9, 0)),
                literal(b"datasets"),
                Object::Reference((10, 0)),
            ]),
            vec![
                ((9, 0), Object::Stream(template)),
                ((10, 0), Object::Stream(data)),
            ],
        );
        let v = forms_json_value(&doc);
        assert_eq!(
            v["xfa_packets"],
            serde_json::json!(["template", "datasets"])
        );
        assert_eq!(v["xfa_datasets_text"], "the value");
    }

    #[test]
    fn xfa_that_will_not_decode_is_a_finding() {
        let stream = make_stream(
            Some(Object::Name(b"FlateDecode".to_vec())),
            b"nope".to_vec(),
        );
        let doc = xfa_doc(
            Object::Reference((9, 0)),
            vec![((9, 0), Object::Stream(stream))],
        );
        let (v, had_issues) = forms_json_value_with_status(&doc);
        assert_eq!(v["xfa"], true);
        assert_eq!(v["xfa_decoded"], false);
        assert!(had_issues);
    }

    #[test]
    fn no_xfa_says_so() {
        let doc = form_doc_with_fields(vec![], vec![]);
        let v = forms_json_value(&doc);
        assert_eq!(v["xfa"], false);
        assert_eq!(v["xfa_decoded"], true);
    }

    #[test]
    fn every_datasets_packet_is_read_and_a_name_with_no_stream_is_a_finding() {
        let first = make_stream(None, b"<v>FIRST 7301</v>".to_vec());
        let second = make_stream(None, b"<v>second</v>".to_vec());
        let doc = xfa_doc(
            Object::Array(vec![
                literal(b"datasets"),
                Object::Reference((9, 0)),
                literal(b"datasets"),
                Object::Reference((10, 0)),
            ]),
            vec![
                ((9, 0), Object::Stream(first)),
                ((10, 0), Object::Stream(second)),
            ],
        );
        assert_eq!(
            forms_json_value(&doc)["xfa_datasets_text"],
            "FIRST 7301\nsecond"
        );

        let lone = xfa_doc(Object::Array(vec![literal(b"datasets")]), vec![]);
        let (v, had_issues) = forms_json_value_with_status(&lone);
        assert_eq!(v["xfa_decoded"], false);
        assert!(had_issues);
    }

    #[test]
    fn xfa_datasets_in_a_comment_do_not_replace_the_real_ones() {
        let stream = make_stream(
            None,
            b"<xdp:xdp><!-- <xfa:datasets>example</xfa:datasets> --><xfa:datasets><v>real</v></xfa:datasets></xdp:xdp>"
                .to_vec(),
        );
        let doc = xfa_doc(
            Object::Reference((9, 0)),
            vec![((9, 0), Object::Stream(stream))],
        );
        assert_eq!(forms_json_value(&doc)["xfa_datasets_text"], "real");
    }

    #[test]
    fn a_value_below_a_nameless_intermediate_field_is_found() {
        let top = named_field("top", vec![(5, 0)]);
        let mut middle = Dictionary::new();
        middle.set("Kids", Object::Array(vec![Object::Reference((6, 0))]));
        let doc = form_doc_with_fields(
            vec![
                ((4, 0), top),
                ((5, 0), middle),
                (
                    (6, 0),
                    text_field("leaf", Some(literal(b"Lockbox code 7301"))),
                ),
            ],
            vec![(4, 0)],
        );
        let v = forms_json_value(&doc);
        let leaf = v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["value"] == "\"Lockbox code 7301\"");
        assert_eq!(leaf.expect("the leaf's value")["field_name"], "top.leaf");
    }

    #[test]
    fn binary_keys_are_skipped_only_in_a_signature() {
        let mut d = Dictionary::new();
        d.set("Contents", literal(b"Lockbox code 7301"));
        let doc = form_doc_with_fields(
            vec![((4, 0), text_field("f", Some(Object::Dictionary(d))))],
            vec![(4, 0)],
        );
        assert!(
            only_field(&doc)["value"]
                .as_str()
                .unwrap()
                .contains("Lockbox code 7301")
        );
    }

    #[test]
    fn a_self_referencing_array_of_many_references_finishes_and_is_flagged() {
        let mut doc = form_doc_with_fields(
            vec![((4, 0), text_field("f", Some(Object::Reference((9, 0)))))],
            vec![(4, 0)],
        );
        doc.objects
            .insert((9, 0), Object::Array(vec![Object::Reference((9, 0)); 12]));
        assert_eq!(only_field(&doc)["value_decoded"], false);
    }

    #[test]
    fn a_null_value_on_a_kid_inherits_the_parents() {
        let mut parent = text_field("top", Some(literal(b"shared")));
        parent.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        let mut kid = text_field("k", Some(Object::Null));
        kid.remove(b"FT");
        let doc = form_doc_with_fields(vec![((4, 0), parent), ((5, 0), kid)], vec![(4, 0)]);
        let v = forms_json_value(&doc);
        assert_eq!(v["fields"][1]["value"], "\"shared\"");
        assert_eq!(v["fields"][1]["inherited"], true);
    }

    #[test]
    fn an_ancestors_undecodable_name_flags_the_descendants() {
        let mut parent = named_field("x", vec![(5, 0)]);
        parent.set("T", literal(&[0xFE, 0xFF, 0x00]));
        let doc = form_doc_with_fields(
            vec![
                ((4, 0), parent),
                ((5, 0), text_field("k", Some(literal(b"v")))),
            ],
            vec![(4, 0)],
        );
        let f = only_field(&doc);
        assert_eq!(f["field_name"], "<FEFF00>.k");
        assert_eq!(f["field_name_decoded"], false);
    }

    #[test]
    fn own_value_and_inherited_rich_value_are_told_apart() {
        let mut parent = text_field("top", None);
        parent.set("RV", literal(b"<p>rich</p>"));
        parent.set("Kids", Object::Array(vec![Object::Reference((5, 0))]));
        let doc = form_doc_with_fields(
            vec![
                ((4, 0), parent),
                ((5, 0), text_field("k", Some(literal(b"own")))),
            ],
            vec![(4, 0)],
        );
        let v = forms_json_value(&doc);
        let kid = &v["fields"][1];
        assert_eq!(kid["inherited"], false);
        assert_eq!(kid["rich_value_inherited"], true);
        assert_eq!(kid["rich_value"], "rich");
    }

    #[test]
    fn forms_need_appearances_false() {
        let mut doc = Document::new();
        let mut acroform = Dictionary::new();
        acroform.set("NeedAppearances", Object::Boolean(false));
        acroform.set("Fields", Object::Array(vec![]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (_, need_appearances, _) = collect_form_fields(&doc);
        assert!(!need_appearances);
    }

    #[test]
    fn forms_deeply_nested_hierarchy() {
        // Arrange: grandparent -> parent -> child field
        let mut doc = Document::new();

        let mut child = Dictionary::new();
        child.set("T", Object::String(b"City".to_vec(), StringFormat::Literal));
        child.set("FT", Object::Name(b"Tx".to_vec()));
        child.set("V", Object::String(b"NYC".to_vec(), StringFormat::Literal));
        doc.objects.insert((30, 0), Object::Dictionary(child));

        let mut parent = Dictionary::new();
        parent.set(
            "T",
            Object::String(b"Address".to_vec(), StringFormat::Literal),
        );
        parent.set("Kids", Object::Array(vec![Object::Reference((30, 0))]));
        doc.objects.insert((20, 0), Object::Dictionary(parent));

        let mut grandparent = Dictionary::new();
        grandparent.set(
            "T",
            Object::String(b"Person".to_vec(), StringFormat::Literal),
        );
        grandparent.set("Kids", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((10, 0), Object::Dictionary(grandparent));

        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((10, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (_, _, fields) = collect_form_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].qualified_name, "Person.Address.City");
    }

    #[test]
    fn forms_flags_preserved() {
        let mut doc = Document::new();
        let mut field = Dictionary::new();
        field.set(
            "T",
            Object::String(b"ReadOnly".to_vec(), StringFormat::Literal),
        );
        field.set("FT", Object::Name(b"Tx".to_vec()));
        field.set("Ff", Object::Integer(1)); // ReadOnly flag
        doc.objects.insert((20, 0), Object::Dictionary(field));

        let mut acroform = Dictionary::new();
        acroform.set("Fields", Object::Array(vec![Object::Reference((20, 0))]));
        doc.objects.insert((15, 0), Object::Dictionary(acroform));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("AcroForm", Object::Reference((15, 0)));
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(vec![]));
        doc.objects.insert((5, 0), Object::Dictionary(pages_dict));
        catalog.set("Pages", Object::Reference((5, 0)));
        doc.objects.insert((4, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((4, 0)));

        let (_, _, fields) = collect_form_fields(&doc);
        assert_eq!(fields[0].flags, 1);
    }

    #[test]
    fn forms_json_structure_complete() {
        let doc = build_form_doc();
        let val = forms_json_value(&doc);
        assert!(val["acroform_object"].is_number());
        assert_eq!(val["need_appearances"], true);
        assert_eq!(val["field_count"], 2);
        let fields = val["fields"].as_array().unwrap();
        let f = &fields[0];
        assert!(f.get("object_number").is_some());
        assert!(f.get("generation").is_some());
        assert!(f.get("field_name").is_some());
        assert!(f.get("field_type").is_some());
        assert!(f.get("value").is_some());
        assert!(f.get("flags").is_some());
        assert!(f.get("page_number").is_some());
    }
}
