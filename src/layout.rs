//! `--text --layout`: position-faithful text extraction (plan-0002, implemented in v0.26.0).
//!
//! Plain `--text` recovers _what_ a page says.  This module recovers _where_,
//! so a table survives extraction: each glyph is positioned by a real text-state
//! machine (CTM, text matrix, font size, `Tc`/`Tw`/`Tz`/`Ts`, glyph advances from
//! `metrics.rs`) and placed on a character grid, in the shape of poppler’s
//! `pdftotext -layout`.
//!
//! **Coordinate spaces.**  Glyph positions are computed in default user space,
//! then mapped to _visual_ space for the grid: rotated by the page’s `/Rotate` and
//! made relative to its effective CropBox, so a landscape page reads top to bottom
//! as it displays.  The page’s `/Rotate` and effective CropBox are reported in
//! `--json`, so a consumer can map between the two.
//!
//! **The grid.**  Glyphs first form _chunks_ in content order: consecutive inked
//! glyphs on one baseline with no real gap between them (a gap wider than 0.4 ×
//! the font’s space advance, or a jump backwards, ends a chunk).  Whitespace
//! glyphs carry no ink and end a chunk rather than joining one.  A chunk is never
//! split: another string drawn over it (a padding space, leader dots) cannot
//! break it apart.  Chunks are clustered into lines by baseline, sorted left to
//! right, and abutting chunks rejoin into _runs_; each run lands at column
//! `round(x / cell)`, never overlapping the previous run and always at least one
//! space after it.  **Grid rounding never inserts a space inside a run** (a
//! guarantee `id-redact` depends on: a digit run printed as one string stays one
//! string); padding goes only between runs.  `cell` is the page’s median glyph
//! advance at its dominant font size, or `--layout-cell`.
//!
//! Glyphs that are not left-to-right horizontal in visual space (rotated labels,
//! vertical writing) are not spliced into rows; they follow the grid under a
//! `[non-horizontal text]` line, in content order.  Glyphs positioned outside the
//! CropBox (or at a non-finite position) follow under `[off-page text]`: shown,
//! never dropped, and never allowed to stretch a line.
//!
//! **Reliability** extends the `--text` verdict.  Every shown code counts toward
//! the coverage ratio; one difference from plain `--text` is that a font decoded
//! by passthrough counts each non-ASCII byte as its own unmapped code, where plain
//! `--text` passes valid UTF-8 runs through.  A glyph placed without a known
//! advance (widths unknown for its font or its code, or text shown with no usable
//! font) makes its font Degraded, which exits 3.  Positions are never invented
//! under a Reliable verdict.

use lopdf::content::Content;
use lopdf::{Document, Object, ObjectId};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;

use crate::helpers;
use crate::metrics::{FontMetrics, font_metrics};
use crate::text::{
    self, FONT_WARNING_MARKER, FontDecoder, FontReliabilityRecord, MAX_FORM_DEPTH, Reliability,
};
use crate::types::PageSpec;

/// Advance assumed for a glyph whose width is unknown, in em.  Used only to keep
/// placement moving; the font is reported Degraded whenever it is used.
const FALLBACK_ADVANCE: f64 = 0.5;
/// Space advance assumed when the font does not state one, in em.
const FALLBACK_SPACE: f64 = 0.25;
/// A gap wider than this fraction of the space advance separates two runs.
const WORD_GAP_FRACTION: f64 = 0.4;
/// Chunks whose baselines differ by at most this fraction of the smaller font size
/// share a line.
const BASELINE_TOLERANCE: f64 = 0.35;
/// Consecutive glyphs whose baselines differ by more than this fraction of the
/// font size do not share a chunk (a superscript starts a new one).
const CHUNK_BASELINE_TOLERANCE: f64 = 0.2;
/// A glyph starting more than this fraction of its font size before the previous
/// glyph’s end is a jump backwards, not kerning.
const MAX_KERN_OVERLAP: f64 = 0.35;
/// A second chunk with the same text, offset by less than this fraction of the
/// first glyph’s advance on both axes, is the same string drawn twice (fake bold).
const OVERPRINT_FRACTION: f64 = 0.3;
/// The smallest grid cell, in points, automatic or `--layout-cell`.
pub(crate) const MIN_CELL: f64 = 1.0;
/// No run starts past this column, whatever the page size claims.
const MAX_COLUMN: usize = 20_000;
/// A glyph whose baseline direction deviates from visual left-to-right by more
/// than this ratio (|dy| / dx) is not horizontal.
const HORIZONTAL_SLOPE: f64 = 0.05;
/// Marker line introducing a page’s non-horizontal text.
pub(crate) const NON_HORIZONTAL_MARKER: &str = "[non-horizontal text]";
/// Marker line introducing a page’s text positioned outside its CropBox.
pub(crate) const OFF_PAGE_MARKER: &str = "[off-page text]";

/// An affine matrix `[a b c d e f]` in PDF’s row-vector convention: a point maps
/// as `[x y 1] × M`, so `M1 × M2` applies `M1` first.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Matrix {
    const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn translate(tx: f64, ty: f64) -> Matrix {
        Matrix {
            e: tx,
            f: ty,
            ..Matrix::IDENTITY
        }
    }

    fn from_operands(ops: &[Object]) -> Option<Matrix> {
        let n: Vec<f64> = ops.iter().take(6).map(number).collect::<Option<_>>()?;
        (n.len() == 6).then(|| Matrix {
            a: n[0],
            b: n[1],
            c: n[2],
            d: n[3],
            e: n[4],
            f: n[5],
        })
    }

    /// `self × other`: apply `self`, then `other`.
    fn then(self, o: Matrix) -> Matrix {
        Matrix {
            a: self.a * o.a + self.b * o.c,
            b: self.a * o.b + self.b * o.d,
            c: self.c * o.a + self.d * o.c,
            d: self.c * o.b + self.d * o.d,
            e: self.e * o.a + self.f * o.c + o.e,
            f: self.e * o.b + self.f * o.d + o.f,
        }
    }

    fn point(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    fn vector(self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y, self.b * x + self.d * y)
    }
}

fn number(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

/// Maps default user space to visual space: rotated clockwise by `/Rotate`
/// and relative to the effective CropBox, y up.
#[derive(Clone, Copy, Debug)]
struct PageFrame {
    rotate: i64,
    crop: [f64; 4],
}

impl PageFrame {
    fn point(&self, x: f64, y: f64) -> (f64, f64) {
        let [x0, y0, x1, y1] = self.crop;
        match self.rotate {
            90 => (y - y0, x1 - x),
            180 => (x1 - x, y1 - y),
            270 => (y1 - y, x - x0),
            _ => (x - x0, y - y0),
        }
    }

    /// Width and height of the CropBox as displayed.
    fn visual_size(&self) -> (f64, f64) {
        let [x0, y0, x1, y1] = self.crop;
        match self.rotate {
            90 | 270 => (y1 - y0, x1 - x0),
            _ => (x1 - x0, y1 - y0),
        }
    }

    fn vector(&self, dx: f64, dy: f64) -> (f64, f64) {
        match self.rotate {
            90 => (dy, -dx),
            180 => (-dx, -dy),
            270 => (-dy, dx),
            _ => (dx, dy),
        }
    }
}

/// Parse a rectangle array into `[llx lly urx ury]`, normalized.
fn rect(obj: &Object) -> Option<[f64; 4]> {
    let Object::Array(a) = obj else { return None };
    let n: Vec<f64> = a.iter().map(number).collect::<Option<_>>()?;
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(n).ok()?;
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// The page’s normalized `/Rotate` (0/90/180/270) and effective CropBox
/// (CropBox ∩ MediaBox, both inheritable), with any warnings about either.
fn page_frame(doc: &Document, page_id: ObjectId, warnings: &mut Vec<String>) -> PageFrame {
    let media = helpers::inherited_page_attr(doc, page_id, b"MediaBox").and_then(rect);
    let media = media.unwrap_or_else(|| {
        warnings.push("Page has no usable MediaBox; assuming US Letter [0 0 612 792]".to_string());
        [0.0, 0.0, 612.0, 792.0]
    });
    let crop = match helpers::inherited_page_attr(doc, page_id, b"CropBox").and_then(rect) {
        Some(c) => {
            let i = [
                c[0].max(media[0]),
                c[1].max(media[1]),
                c[2].min(media[2]),
                c[3].min(media[3]),
            ];
            if i[0] < i[2] && i[1] < i[3] { i } else { media }
        }
        None => media,
    };
    let rotate = match helpers::inherited_page_attr(doc, page_id, b"Rotate") {
        None => 0,
        Some(v) => match number(v) {
            // A Real like 90.0 is accepted; the value, not its type, must be a
            // multiple of 90.
            Some(r) if r.fract() == 0.0 && (r as i64).rem_euclid(90) == 0 => {
                (r as i64).rem_euclid(360)
            }
            _ => {
                warnings.push(format!(
                    "Page /Rotate {} is not a multiple of 90; laid out unrotated",
                    helpers::format_dict_value(v)
                ));
                0
            }
        },
    };
    PageFrame { rotate, crop }
}

/// One font as the layout engine sees it: how to decode it, how wide its
/// glyphs are, and its reliability record.
struct LayoutFont {
    decoder: FontDecoder,
    metrics: FontMetrics,
    record: FontReliabilityRecord,
}

fn build_layout_fonts(
    doc: &Document,
    resources: &lopdf::Dictionary,
) -> HashMap<String, LayoutFont> {
    text::font_dicts(doc, resources)
        .into_iter()
        .map(|(name, dict)| {
            let (decoder, record) = text::build_font_decoder(doc, dict, &name);
            let metrics = font_metrics(doc, dict);
            (
                name,
                LayoutFont {
                    decoder,
                    metrics,
                    record,
                },
            )
        })
        .collect()
}

/// The graphics-state parameters `--layout` models: the CTM and the text state
/// (all of which `q`/`Q` save and restore, and a form XObject inherits).
#[derive(Clone, Copy)]
struct GState<'a> {
    ctm: Matrix,
    font: Option<&'a LayoutFont>,
    /// The last `Tf` named a font missing from the resources.
    font_missing: bool,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    /// `Tz` / 100.
    h_scale: f64,
    leading: f64,
    rise: f64,
}

impl GState<'_> {
    fn initial() -> Self {
        GState {
            ctm: Matrix::IDENTITY,
            font: None,
            font_missing: false,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        }
    }
}

/// Where a glyph goes in the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    Grid,
    NonHorizontal,
    OffPage,
}

/// A glyph in visual space.
#[derive(Clone, Debug)]
struct Glyph {
    /// Content order across the page, including forms.
    seq: usize,
    /// Which show-string (one `Tj`, one `TJ` string element, …) drew this glyph.
    string: usize,
    /// Left edge of the glyph’s own advance (not counting `Tc`/`Tw`).
    x: f64,
    /// Baseline.
    y: f64,
    /// Right edge of the glyph’s own advance.
    end_x: f64,
    size: f64,
    /// A gap wider than this ends the chunk after this glyph.
    word_gap: f64,
    text: String,
    placement: Placement,
}

/// Accumulators for one page.
struct PageState {
    frame: PageFrame,
    glyphs: Vec<Glyph>,
    /// Show-strings seen so far on the page.
    strings: usize,
    warnings: Vec<String>,
    total_codes: u64,
    unmapped_codes: u64,
    fonts: Vec<FontReliabilityRecord>,
    /// Fonts already reported Degraded for unknown widths on this page.
    width_flagged: HashSet<String>,
}

impl PageState {
    fn flag_unknown_widths(&mut self, font: Option<&LayoutFont>, reason: &str) {
        let key = font.map_or("", |f| f.record.name.as_str());
        if self.width_flagged.contains(key) {
            return;
        }
        self.width_flagged.insert(key.to_string());
        let mut record = match font {
            Some(f) => f.record.clone(),
            None => FontReliabilityRecord {
                name: "(no font)".to_string(),
                base_font: "-".to_string(),
                subtype: String::new(),
                classification: Reliability::Reliable,
                has_to_unicode: false,
                reason: String::new(),
            },
        };
        record.classification = record.classification.max(Reliability::Degraded);
        record.reason = if record.reason.is_empty() {
            reason.to_string()
        } else {
            format!("{}; {}", record.reason, reason)
        };
        self.fonts.push(record);
    }
}

/// Everything `--layout` extracts from one page.
pub(crate) struct LayoutPage {
    pub text: String,
    pub warnings: Vec<String>,
    pub total_codes: u64,
    pub unmapped_codes: u64,
    pub fonts: Vec<FontReliabilityRecord>,
    pub rotate: i64,
    pub crop_box: [f64; 4],
    pub cell: f64,
    pub non_horizontal_glyphs: usize,
    pub off_page_glyphs: usize,
}

pub(crate) fn layout_page(
    doc: &Document,
    page_id: ObjectId,
    cell_override: Option<f64>,
) -> LayoutPage {
    let mut warnings = Vec::new();
    if let Ok(Object::Dictionary(page_dict)) = doc.get_object(page_id) {
        warnings.extend(text::check_page_font_encodings(doc, page_dict));
    }
    let frame = page_frame(doc, page_id, &mut warnings);
    let mut state = PageState {
        frame,
        glyphs: Vec::new(),
        strings: 0,
        warnings,
        total_codes: 0,
        unmapped_codes: 0,
        fonts: Vec::new(),
        width_flagged: HashSet::new(),
    };
    let resources = crate::resources::resolve_page_resources(doc, page_id);
    match helpers::read_content_streams(doc, page_id) {
        Some(data) => {
            state.warnings.extend(data.warnings);
            let mut visited = HashSet::new();
            process_content(
                doc,
                &data.bytes,
                resources,
                GState::initial(),
                &mut state,
                &mut visited,
                0,
            );
        }
        None => {
            if let Some(res) = resources {
                state
                    .fonts
                    .extend(build_layout_fonts(doc, res).into_values().map(|f| f.record));
            }
        }
    }
    let grid = render_grid(&state.glyphs, cell_override);
    LayoutPage {
        text: grid.text,
        warnings: state.warnings,
        total_codes: state.total_codes,
        unmapped_codes: state.unmapped_codes,
        fonts: state.fonts,
        rotate: frame.rotate,
        crop_box: frame.crop,
        cell: grid.cell,
        non_horizontal_glyphs: grid.non_horizontal,
        off_page_glyphs: grid.off_page,
    }
}

/// Walk one content stream, positioning every glyph it shows.  Recurses into
/// form XObjects with their `/Matrix` applied; `gs` is the graphics state in
/// force at entry (the caller’s, for a form).
fn process_content(
    doc: &Document,
    bytes: &[u8],
    resources: Option<&lopdf::Dictionary>,
    entry: GState<'_>,
    state: &mut PageState,
    visited: &mut HashSet<ObjectId>,
    depth: u32,
) {
    // A fresh binding, so `gs` may also hold this content's own (shorter-lived) fonts.
    let mut gs: GState<'_> = entry;
    let fonts = resources
        .map(|r| build_layout_fonts(doc, r))
        .unwrap_or_default();
    state.fonts.extend(fonts.values().map(|f| f.record.clone()));
    if bytes.is_empty() {
        return;
    }
    let operations = match Content::decode(bytes) {
        Ok(content) => content.operations,
        Err(_) => {
            state
                .warnings
                .push("Content stream has syntax errors".to_string());
            return;
        }
    };

    let no_font = FontMetrics::unknown("text shown with no font selected (no Tf)");
    let missing_font = FontMetrics::unknown("Tf names a font missing from the resources");
    let mut saved: Vec<GState> = Vec::new();
    let mut tm = Matrix::IDENTITY;
    let mut tlm = Matrix::IDENTITY;
    for op in &operations {
        let ops = &op.operands;
        let num = |i: usize| ops.get(i).and_then(number);
        match op.operator.as_str() {
            "q" => saved.push(gs),
            "Q" => {
                if let Some(g) = saved.pop() {
                    gs = g;
                }
            }
            "cm" => {
                if let Some(m) = Matrix::from_operands(ops) {
                    gs.ctm = m.then(gs.ctm);
                }
            }
            "BT" => {
                tm = Matrix::IDENTITY;
                tlm = Matrix::IDENTITY;
            }
            "Tf" => {
                if let Some(Object::Name(n)) = ops.first() {
                    gs.font = fonts.get(String::from_utf8_lossy(n).as_ref());
                    gs.font_missing = gs.font.is_none();
                }
                if let Some(size) = num(1) {
                    gs.size = size;
                }
            }
            "Tc" => gs.char_spacing = num(0).unwrap_or(gs.char_spacing),
            "Tw" => gs.word_spacing = num(0).unwrap_or(gs.word_spacing),
            "Tz" => gs.h_scale = num(0).map_or(gs.h_scale, |z| z / 100.0),
            "TL" => gs.leading = num(0).unwrap_or(gs.leading),
            "Ts" => gs.rise = num(0).unwrap_or(gs.rise),
            "Td" | "TD" => {
                if let (Some(tx), Some(ty)) = (num(0), num(1)) {
                    if op.operator == "TD" {
                        gs.leading = -ty;
                    }
                    tlm = Matrix::translate(tx, ty).then(tlm);
                    tm = tlm;
                }
            }
            "Tm" => {
                if let Some(m) = Matrix::from_operands(ops) {
                    tlm = m;
                    tm = m;
                }
            }
            "T*" => {
                tlm = Matrix::translate(0.0, -gs.leading).then(tlm);
                tm = tlm;
            }
            "Tj" => {
                if let Some(Object::String(s, _)) = ops.first() {
                    show(
                        s,
                        &gs,
                        &mut tm,
                        state,
                        if gs.font_missing {
                            &missing_font
                        } else {
                            &no_font
                        },
                    );
                }
            }
            "'" => {
                tlm = Matrix::translate(0.0, -gs.leading).then(tlm);
                tm = tlm;
                if let Some(Object::String(s, _)) = ops.first() {
                    show(
                        s,
                        &gs,
                        &mut tm,
                        state,
                        if gs.font_missing {
                            &missing_font
                        } else {
                            &no_font
                        },
                    );
                }
            }
            "\"" => {
                gs.word_spacing = num(0).unwrap_or(gs.word_spacing);
                gs.char_spacing = num(1).unwrap_or(gs.char_spacing);
                tlm = Matrix::translate(0.0, -gs.leading).then(tlm);
                tm = tlm;
                if let Some(Object::String(s, _)) = ops.get(2) {
                    show(
                        s,
                        &gs,
                        &mut tm,
                        state,
                        if gs.font_missing {
                            &missing_font
                        } else {
                            &no_font
                        },
                    );
                }
            }
            "TJ" => {
                if let Some(Object::Array(items)) = ops.first() {
                    for item in items {
                        match item {
                            Object::String(s, _) => show(
                                s,
                                &gs,
                                &mut tm,
                                state,
                                if gs.font_missing {
                                    &missing_font
                                } else {
                                    &no_font
                                },
                            ),
                            other => {
                                if let Some(n) = number(other) {
                                    let adj = -n / 1000.0 * gs.size;
                                    let vertical = gs.font.is_some_and(|f| f.metrics.vertical);
                                    tm = if vertical {
                                        Matrix::translate(0.0, adj)
                                    } else {
                                        Matrix::translate(adj * gs.h_scale, 0.0)
                                    }
                                    .then(tm);
                                }
                            }
                        }
                    }
                }
            }
            "Do" => {
                if let Some(Object::Name(n)) = ops.first() {
                    let name = String::from_utf8_lossy(n);
                    process_form(doc, resources, &name, gs, state, visited, depth);
                }
            }
            _ => {}
        }
    }
}

/// Position and record every glyph of one show-string, advancing `tm`.
/// `fallback` supplies the (unknown) metrics when no usable font is selected.
fn show(
    bytes: &[u8],
    gs: &GState<'_>,
    tm: &mut Matrix,
    state: &mut PageState,
    fallback: &FontMetrics,
) {
    let (decoder, metrics) = match gs.font {
        Some(f) => (Some(&f.decoder), &f.metrics),
        None => (None, fallback),
    };
    let space = match metrics.code_bytes {
        1 => metrics.advance(32).ok().filter(|w| *w > 0.0),
        _ => None,
    }
    .unwrap_or(FALLBACK_SPACE);
    let frame = state.frame;
    let (page_w, page_h) = frame.visual_size();
    let string = state.strings;
    state.strings += 1;
    // The text-space-to-glyph scaling of the text rendering matrix (§9.4.4),
    // `[Tfs·Th 0 0 Tfs 0 Trise]`; its signs matter (a negative Tf size or Tz
    // mirrors the text), so direction and extent come from the full matrix.
    let scaling = Matrix {
        a: gs.size * gs.h_scale,
        b: 0.0,
        c: 0.0,
        d: gs.size,
        e: 0.0,
        f: gs.rise,
    };

    text::for_each_code(
        bytes,
        decoder,
        metrics.code_bytes,
        |code, nbytes, decoded| {
            state.total_codes += 1;
            if decoded == "\u{FFFD}" {
                state.unmapped_codes += 1;
            }
            let w0 = match metrics.advance(code) {
                Ok(w) => w,
                Err(reason) => {
                    state.flag_unknown_widths(gs.font, &reason);
                    FALLBACK_ADVANCE
                }
            };
            let trm = scaling.then(*tm).then(gs.ctm);
            let (ox, oy) = {
                let p = trm.point(0.0, 0.0);
                frame.point(p.0, p.1)
            };
            let (ex, _) = {
                let p = trm.point(w0, 0.0);
                frame.point(p.0, p.1)
            };
            let (dx, dy) = {
                let v = trm.vector(1.0, 0.0);
                frame.vector(v.0, v.1)
            };
            let size = {
                let v = trm.vector(0.0, 1.0);
                v.0.hypot(v.1)
            };
            let margin = size.max(1.0);
            let on_page = [ox, oy, ex, size].iter().all(|v| v.is_finite())
                && (-margin..=page_w + margin).contains(&ox)
                && (-margin..=page_h + margin).contains(&oy);
            let placement = if !on_page {
                Placement::OffPage
            } else if !metrics.vertical && dx > 0.0 && dy.abs() <= HORIZONTAL_SLOPE * dx {
                Placement::Grid
            } else {
                Placement::NonHorizontal
            };
            state.glyphs.push(Glyph {
                seq: state.glyphs.len(),
                string,
                x: ox.min(ex),
                y: oy,
                end_x: ox.max(ex),
                size,
                word_gap: WORD_GAP_FRACTION * space * dx.hypot(dy),
                text: decoded.to_string(),
                placement,
            });
            let word = if nbytes == 1 && code == 32 {
                gs.word_spacing
            } else {
                0.0
            };
            *tm = if metrics.vertical {
                Matrix::translate(0.0, -gs.size + gs.char_spacing + word)
            } else {
                Matrix::translate((w0 * gs.size + gs.char_spacing + word) * gs.h_scale, 0.0)
            }
            .then(*tm);
        },
    );
}

/// Resolve `/<name> Do` to a form XObject and lay out its content inside the
/// caller’s graphics state with the form’s `/Matrix` applied.  Mirrors
/// `text::process_form_xobject`: skips missing resources, cycles (the active
/// recursion stack), and nesting beyond `MAX_FORM_DEPTH`.
fn process_form(
    doc: &Document,
    parent_resources: Option<&lopdf::Dictionary>,
    name: &str,
    gs: GState<'_>,
    state: &mut PageState,
    visited: &mut HashSet<ObjectId>,
    depth: u32,
) {
    let Some(xobj_id) = parent_resources.and_then(|r| text::resolve_xobject_id(doc, r, name))
    else {
        return;
    };
    if depth + 1 > MAX_FORM_DEPTH {
        state.warnings.push(format!(
            "Form XObject nesting exceeds depth {MAX_FORM_DEPTH}; some text may be omitted"
        ));
        return;
    }
    if !visited.insert(xobj_id) {
        return;
    }
    if let Ok(Object::Stream(stream)) = doc.get_object(xobj_id)
        && stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|v| v.as_name().ok())
            == Some(b"Form".as_slice())
    {
        let (decoded, warn) = crate::stream::decode_stream(stream);
        if let Some(w) = warn {
            state
                .warnings
                .push(format!("Form XObject {} {}: {}", xobj_id.0, xobj_id.1, w));
        }
        let form_resources = stream
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|r| helpers::resolve_dict(doc, r))
            .or(parent_resources);
        let mut inner = gs;
        if let Some(m) = stream
            .dict
            .get(b"Matrix")
            .ok()
            .and_then(|m| helpers::resolve_array(doc, m))
            .and_then(Matrix::from_operands)
        {
            inner.ctm = m.then(gs.ctm);
        }
        process_content(
            doc,
            &decoded,
            form_resources,
            inner,
            state,
            visited,
            depth + 1,
        );
    }
    visited.remove(&xobj_id);
}

struct Grid {
    text: String,
    cell: f64,
    non_horizontal: usize,
    off_page: usize,
}

/// Consecutive inked glyphs on one baseline with no real gap between them.
/// Never split by the grid.
#[derive(Debug)]
struct Chunk {
    seq: usize,
    x: f64,
    y: f64,
    /// Right edge of the chunk’s last _inked_ glyph: what the grid sees.
    end_x: f64,
    /// Right edge of its last glyph of any kind: where the next glyph continues.
    pen_end: f64,
    size: f64,
    /// Advance of the chunk’s first glyph (the overprint scale).
    first_advance: f64,
    word_gap: f64,
    text: String,
}

fn has_ink(g: &Glyph) -> bool {
    !g.text.trim().is_empty()
}

/// Group the grid glyphs, in content order, into chunks.  A whitespace glyph
/// joins a chunk only between inked glyphs (so prose keeps its own spacing) and
/// never starts one: it draws nothing, so a stray padding space drawn over other
/// text cannot become a column of its own.
fn build_chunks(glyphs: &[Glyph]) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut open = false;
    for g in glyphs.iter().filter(|g| g.placement == Placement::Grid) {
        if open && let Some(c) = chunks.last_mut() {
            let gap = g.x - c.pen_end;
            let same_baseline = (g.y - c.y).abs() <= CHUNK_BASELINE_TOLERANCE * g.size.min(c.size);
            if same_baseline && gap >= -MAX_KERN_OVERLAP * g.size && gap <= c.word_gap {
                c.text.push_str(&g.text);
                c.pen_end = c.pen_end.max(g.end_x);
                if has_ink(g) {
                    c.end_x = c.end_x.max(g.end_x);
                }
                c.size = c.size.max(g.size);
                c.word_gap = g.word_gap;
                continue;
            }
        }
        if !has_ink(g) {
            continue;
        }
        chunks.push(Chunk {
            seq: g.seq,
            x: g.x,
            y: g.y,
            end_x: g.end_x,
            pen_end: g.end_x,
            size: g.size,
            first_advance: g.end_x - g.x,
            word_gap: g.word_gap,
            text: g.text.clone(),
        });
        open = true;
    }
    for c in &mut chunks {
        c.text.truncate(c.text.trim_end().len());
    }
    chunks
}

/// The median glyph advance at the page’s dominant font size, floored at `MIN_CELL`.
fn grid_cell(glyphs: &[Glyph]) -> f64 {
    let visible = || {
        glyphs
            .iter()
            .filter(|g| g.placement == Placement::Grid && has_ink(g) && g.end_x > g.x)
    };
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for g in visible() {
        *counts.entry((g.size * 2.0).round() as i64).or_default() += 1;
    }
    // Most frequent size; ties go to the larger, deterministically.
    let Some((&dominant, _)) = counts.iter().max_by_key(|(size, n)| (**n, **size)) else {
        return 6.0;
    };
    let mut advances: Vec<f64> = visible()
        .filter(|g| (g.size * 2.0).round() as i64 == dominant)
        .map(|g| g.end_x - g.x)
        .collect();
    advances.sort_by(f64::total_cmp);
    advances[advances.len() / 2].max(MIN_CELL)
}

/// Text of glyphs that cannot go on the grid, one line per show-string, in
/// content order.
fn side_section(glyphs: &[Glyph], placement: Placement) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut prev_string = None;
    for g in glyphs.iter().filter(|g| g.placement == placement) {
        match (prev_string, runs.last_mut()) {
            (Some(p), Some(run)) if g.string == p => run.push_str(&g.text),
            _ => runs.push(g.text.clone()),
        }
        prev_string = Some(g.string);
    }
    runs.into_iter()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

fn render_grid(glyphs: &[Glyph], cell_override: Option<f64>) -> Grid {
    let cell = cell_override
        .unwrap_or_else(|| grid_cell(glyphs))
        .max(MIN_CELL);
    let mut chunks = build_chunks(glyphs);

    // Top to bottom, then left to right.
    chunks.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x.total_cmp(&b.x)));
    let mut lines: Vec<Vec<Chunk>> = Vec::new();
    let (mut line_y, mut line_size) = (f64::NAN, 0.0);
    for c in chunks {
        let tolerance = BASELINE_TOLERANCE * c.size.min(line_size);
        match lines.last_mut() {
            Some(line) if (line_y - c.y).abs() <= tolerance => line.push(c),
            _ => {
                line_y = c.y;
                line_size = c.size;
                lines.push(vec![c]);
            }
        }
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for mut line in lines {
        line.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.seq.cmp(&b.seq)));
        out.push(render_line(&dedup_overprint(line), cell));
    }
    // Drop empty lines at the page's top and bottom, and shift out the left
    // margin every line shares.  Neither changes the relative alignment of any
    // two columns.
    let first = out.iter().position(|l| !l.is_empty()).unwrap_or(out.len());
    let last = out
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(first, |i| i + 1);
    let out = &out[first..last];
    let margin = out
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    let mut text = out
        .iter()
        .map(|l| l.get(margin..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    let mut counts = [0usize; 2];
    for (i, (placement, marker)) in [
        (Placement::NonHorizontal, NON_HORIZONTAL_MARKER),
        (Placement::OffPage, OFF_PAGE_MARKER),
    ]
    .into_iter()
    .enumerate()
    {
        counts[i] = glyphs.iter().filter(|g| g.placement == placement).count();
        if counts[i] == 0 {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(marker);
        for run in side_section(glyphs, placement) {
            text.push('\n');
            text.push_str(&run);
        }
    }
    Grid {
        text,
        cell,
        non_horizontal: counts[0],
        off_page: counts[1],
    }
}

/// Drop a chunk that repeats an earlier chunk’s text at (nearly) the same spot:
/// a string drawn twice with a small offset to fake bold.  The tolerance scales
/// with the glyph’s advance, and a chunk is never compared with itself, so a
/// repeated character inside one string (`1100`) is never a duplicate.  `line`
/// is sorted by x.
fn dedup_overprint(line: Vec<Chunk>) -> Vec<Chunk> {
    let mut kept: Vec<Chunk> = Vec::with_capacity(line.len());
    for c in line {
        let dup = kept
            .iter()
            .rev()
            .take_while(|k| c.x - k.x <= OVERPRINT_FRACTION * k.first_advance)
            .any(|k| {
                let tol = OVERPRINT_FRACTION * k.first_advance;
                tol > 0.0
                    && k.text == c.text
                    && (c.x - k.x).abs() <= tol
                    && (c.y - k.y).abs() <= tol
            });
        if !dup {
            kept.push(c);
        }
    }
    kept
}

/// Lay one line of x-sorted chunks onto the grid.  Abutting chunks rejoin into
/// a run whose text is never split; each run starts at `round(x / cell)`, but
/// always at least one column after the previous run, and never past
/// `MAX_COLUMN`.
fn render_line(chunks: &[Chunk], cell: f64) -> String {
    let mut runs: Vec<(f64, String)> = Vec::new();
    let mut run_end = f64::NEG_INFINITY;
    let mut gap_limit = 0.0;
    for c in chunks {
        let gap = c.x - run_end;
        match runs.last_mut() {
            Some((_, text)) if gap >= -MAX_KERN_OVERLAP * c.size && gap <= gap_limit => {
                text.push_str(&c.text)
            }
            _ => runs.push((c.x, c.text.clone())),
        }
        run_end = run_end.max(c.end_x);
        gap_limit = c.word_gap;
    }

    let mut line = String::new();
    let mut len = 0usize;
    for (x, text) in runs {
        let col = ((x / cell).round().max(0.0) as usize).min(MAX_COLUMN);
        let col = if len == 0 { col } else { col.max(len + 1) };
        line.extend(std::iter::repeat_n(' ', col.saturating_sub(len)));
        line.push_str(&text);
        len = col + text.chars().count();
    }
    line.truncate(line.trim_end().len());
    line
}

// ── Output ──────────────────────────────────────────────────────────────

/// Print every selected page as a character grid.  Returns whether the run
/// has findings (verdict other than Reliable → exit 3).
pub(crate) fn print_layout(
    writer: &mut impl Write,
    doc: &Document,
    page_filter: Option<&PageSpec>,
    cell_override: Option<f64>,
) -> bool {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => {
            eprintln!("Error: {msg}");
            return false;
        }
    };
    let mut content_warnings = BTreeSet::new();
    let mut all_fonts = Vec::new();
    let (mut total, mut unmapped) = (0u64, 0u64);
    for (pn, page_id) in &page_list {
        wln!(writer, "--- Page {} ---", pn);
        let page = layout_page(doc, *page_id, cell_override);
        content_warnings.extend(
            page.warnings
                .into_iter()
                .filter(|w| !w.contains(FONT_WARNING_MARKER)),
        );
        total += page.total_codes;
        unmapped += page.unmapped_codes;
        all_fonts.extend(page.fonts);
        wln!(writer, "{}", page.text);
    }
    for warn in &content_warnings {
        eprintln!("Warning: {warn}");
    }
    let fonts = text::dedup_font_records(all_fonts);
    text::print_reliability_banner(&fonts, total, unmapped);
    text::document_verdict(&fonts, total, unmapped).is_finding()
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// `--text --layout --json`: the `--text` schema plus per-page geometry.
pub(crate) fn layout_json_value(
    doc: &Document,
    page_filter: Option<&PageSpec>,
    cell_override: Option<f64>,
) -> (Value, bool) {
    let page_list = match helpers::build_page_list(doc, page_filter) {
        Ok(list) => list,
        Err(msg) => return (json!({"error": msg}), false),
    };
    let mut pages = Vec::new();
    let mut all_fonts = Vec::new();
    let (mut total, mut unmapped) = (0u64, 0u64);
    for (pn, page_id) in &page_list {
        let page = layout_page(doc, *page_id, cell_override);
        total += page.total_codes;
        unmapped += page.unmapped_codes;
        let mut entry = serde_json::Map::new();
        entry.insert("page_number".into(), json!(pn));
        entry.insert("text".into(), json!(page.text));
        entry.insert("rotate".into(), json!(page.rotate));
        entry.insert(
            "crop_box".into(),
            json!(page.crop_box.iter().map(|v| round2(*v)).collect::<Vec<_>>()),
        );
        entry.insert("cell".into(), json!(round2(page.cell)));
        entry.insert(
            "non_horizontal_glyphs".into(),
            json!(page.non_horizontal_glyphs),
        );
        entry.insert("off_page_glyphs".into(), json!(page.off_page_glyphs));
        if !page.warnings.is_empty() {
            entry.insert("warnings".into(), json!(page.warnings));
        }
        pages.push(Value::Object(entry));
        all_fonts.extend(page.fonts);
    }
    let fonts = text::dedup_font_records(all_fonts);
    text::print_reliability_banner(&fonts, total, unmapped);
    let reliability = text::reliability_json_value(&fonts, total, unmapped);
    let had_issues = text::document_verdict(&fonts, total, unmapped).is_finding();
    (
        json!({"layout": true, "pages": pages, "reliability": reliability}),
        had_issues,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Dictionary, Stream};

    /// A simple Type1 font with explicit widths for codes 32..=126.
    fn font_with_widths(base: &str, width_of: impl Fn(u8) -> i64) -> Dictionary {
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(base.as_bytes().to_vec()));
        font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        font.set("FirstChar", Object::Integer(32));
        font.set("LastChar", Object::Integer(126));
        font.set(
            "Widths",
            Object::Array((32u8..=126).map(|c| Object::Integer(width_of(c))).collect()),
        );
        font
    }

    /// Every glyph 600 units: 6 pt at 10 pt, so columns are exact.
    fn mono() -> Dictionary {
        font_with_widths("ABCDEF+Mono", |_| 600)
    }

    /// Helvetica-like: digits 556, space 278, everything else 600.
    fn prop() -> Dictionary {
        font_with_widths("ABCDEF+Prop", |c| match c {
            b'0'..=b'9' => 556,
            b' ' => 278,
            _ => 600,
        })
    }

    struct Fixture {
        doc: Document,
        page: ObjectId,
    }

    /// One page with `/F1` = `font`, optional `/Fm0` form XObject, and extra
    /// page-dictionary keys (Rotate, CropBox, …).
    fn fixture(
        font: Dictionary,
        content: &[u8],
        form: Option<(Dictionary, &[u8])>,
        page_extra: &[(&str, Object)],
    ) -> Fixture {
        let mut doc = Document::new();
        let font_id = doc.add_object(Object::Dictionary(font));
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Reference(font_id));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(fonts));
        if let Some((mut dict, body)) = form {
            dict.set("Type", Object::Name(b"XObject".to_vec()));
            dict.set("Subtype", Object::Name(b"Form".to_vec()));
            let form_id = doc.add_object(Object::Stream(Stream::new(dict, body.to_vec())));
            let mut xobjects = Dictionary::new();
            xobjects.set("Fm0", Object::Reference(form_id));
            resources.set("XObject", Object::Dictionary(xobjects));
        }
        let c_id = doc.add_object(Object::Stream(Stream::new(
            Dictionary::new(),
            content.to_vec(),
        )));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Contents", Object::Reference(c_id));
        page.set("Resources", Object::Dictionary(resources));
        page.set(
            "MediaBox",
            Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
        );
        for (k, v) in page_extra {
            page.set(*k, v.clone());
        }
        let page_id = doc.add_object(Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages.set("Count", Object::Integer(1));
        let pages_id = doc.add_object(Object::Dictionary(pages));
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page_id) {
            d.set("Parent", Object::Reference(pages_id));
        }
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let cat_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference(cat_id));
        Fixture { doc, page: page_id }
    }

    /// `(text) Tj` at absolute `(x, y)` in font `/F1` at 10 pt.
    fn at(x: f64, y: f64, text: &str) -> String {
        format!("BT /F1 10 Tf 1 0 0 1 {x} {y} Tm ({text}) Tj ET\n")
    }

    fn layout(f: &Fixture) -> LayoutPage {
        layout_page(&f.doc, f.page, None)
    }

    fn lines(page: &LayoutPage) -> Vec<&str> {
        page.text.lines().collect()
    }

    fn col(line: &str, needle: &str) -> usize {
        line.find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {line:?}"))
    }

    // ── The motivating case: a statement table ──────────────────────────

    /// Dates, then descriptions, then amounts — drawn column by column, the way
    /// statement generators do — with debit and credit in separate columns and
    /// one description wrapping onto a second line.
    fn statement() -> Fixture {
        let mut content = String::new();
        // Amounts first (debit x=300, credit x=384, balance x=468).
        content += &at(300.0, 700.0, "4.50");
        content += &at(468.0, 700.0, "95.50");
        content += &at(384.0, 688.0, "1000.00");
        content += &at(468.0, 688.0, "1095.50");
        // Then descriptions (x=102), one wrapping to y=676.
        content += &at(102.0, 700.0, "Coffee");
        content += &at(102.0, 688.0, "Payroll deposit");
        content += &at(102.0, 676.0, "ACME Corp");
        // Then dates (x=48).
        content += &at(48.0, 700.0, "01/02");
        content += &at(48.0, 688.0, "01/03");
        // Headers last.
        content += &at(48.0, 712.0, "Date");
        content += &at(102.0, 712.0, "Description");
        content += &at(300.0, 712.0, "Debit");
        content += &at(384.0, 712.0, "Credit");
        content += &at(468.0, 712.0, "Balance");
        fixture(mono(), content.as_bytes(), None, &[])
    }

    #[test]
    fn statement_rows_come_out_in_reading_order_with_columns_aligned() {
        let page = layout(&statement());
        let l = lines(&page);
        assert_eq!(l.len(), 4, "{:#?}", l);
        // Cell 6 pt, margin x=48 → column (x - 48) / 6.
        assert_eq!(col(l[0], "Date"), 0);
        assert_eq!(col(l[0], "Debit"), 42);
        assert_eq!(col(l[0], "Credit"), 56);
        assert_eq!(col(l[0], "Balance"), 70);
        assert!(l[1].starts_with("01/02"), "{:?}", l[1]);
        assert_eq!(col(l[1], "Coffee"), 9);
        assert_eq!(col(l[1], "4.50"), 42, "debit stays under Debit");
        assert_eq!(col(l[1], "95.50"), 70);
        assert_eq!(col(l[2], "1000.00"), 56, "credit stays under Credit");
        assert_eq!(col(l[2], "1095.50"), 70);
        // The wrapped description is its own line, with no amount on it.
        assert_eq!(l[3].trim(), "ACME Corp");
        assert_eq!(col(l[3], "ACME Corp"), 9);
        assert_eq!(page.cell, 6.0);
    }

    #[test]
    fn statement_is_reliable_with_widths_known() {
        let f = statement();
        let (json, had_issues) = layout_json_value(&f.doc, None, None);
        assert_eq!(json["reliability"]["verdict"], "reliable", "{json}");
        assert!(!had_issues);
        assert_eq!(json["layout"], true);
        let p = &json["pages"][0];
        assert_eq!(p["rotate"], 0);
        assert_eq!(p["crop_box"], json!([0.0, 0.0, 612.0, 792.0]));
        assert_eq!(p["cell"], 6.0);
        assert_eq!(p["non_horizontal_glyphs"], 0);
    }

    // ── No invented spaces (id-redact’s guarantee) ──────────────────────

    #[test]
    fn digit_run_at_a_badly_rounding_position_stays_contiguous() {
        // Digits advance 5.56 pt; at x = 103 every glyph boundary falls at a
        // different fraction of the cell, the case column quantization breaks.
        let content = at(103.0, 700.0, "12345678") + &at(10.0, 700.0, "Acct");
        for cell in [None, Some(7.0), Some(4.3), Some(12.5)] {
            let f = fixture(prop(), content.as_bytes(), None, &[]);
            let page = layout_page(&f.doc, f.page, cell);
            assert!(
                page.text.contains("12345678"),
                "cell {cell:?}: digits split: {:?}",
                page.text
            );
        }
    }

    #[test]
    fn digits_positioned_one_by_one_stay_contiguous() {
        // Some generators place every glyph with its own Tm.
        let mut content = at(10.0, 700.0, "Acct");
        for (i, d) in "98765432".chars().enumerate() {
            content += &at(101.3 + i as f64 * 5.56, 700.0, &d.to_string());
        }
        let f = fixture(prop(), content.as_bytes(), None, &[]);
        assert!(
            layout(&f).text.contains("98765432"),
            "{:?}",
            layout(&f).text
        );
    }

    #[test]
    fn a_real_gap_always_yields_at_least_one_space() {
        // Mono: "Hello" ends at 52 + 30 = 82; a 3 pt gap exceeds 0.4 × 6 pt.  Here
        // rounding alone would butt the runs together: "Hello" spans columns
        // 9–13 and round(85 / 6) = 14.
        let content = at(52.0, 700.0, "Hello") + &at(85.0, 700.0, "World");
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        assert_eq!(layout(&f).text, "Hello World");
    }

    #[test]
    fn a_kerning_sized_gap_is_not_a_space() {
        let content = at(50.0, 700.0, "Hello") + &at(81.0, 700.0, "World");
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        assert_eq!(layout(&f).text, "HelloWorld");
    }

    #[test]
    fn tj_adjustments_space_words_but_not_kerning() {
        let content = b"BT /F1 10 Tf 50 700 Td [(Hello) -500 (World)] TJ ET\n\
                        BT /F1 10 Tf 50 680 Td [(Ke) 60 (rn)] TJ ET";
        let f = fixture(mono(), content, None, &[]);
        assert_eq!(lines(&layout(&f)), ["Hello World", "Kern"]);
    }

    #[test]
    fn word_spacing_applies_only_to_code_32() {
        // Tw widens the space glyph; with Tw on a non-space it would push "b" away.
        let content = b"BT /F1 10 Tf 20 Tw 50 700 Td (ab c) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        let page = layout(&f);
        assert!(page.text.starts_with("ab"), "{:?}", page.text);
        // "c" lands 6 + 20 pt past the space: well right of plain "ab c".
        assert!(col(&page.text, "c") > 4, "{:?}", page.text);
    }

    // ── Geometry ────────────────────────────────────────────────────────

    #[test]
    fn cm_inside_q_shifts_text_and_q_restores_it() {
        let content = b"q 1 0 0 1 60 0 cm BT /F1 10 Tf 100 700 Td (B) Tj ET Q \
                        BT /F1 10 Tf 100 700 Td (A) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        assert_eq!(layout(&f).text, format!("A{}B", " ".repeat(9)));
    }

    #[test]
    fn form_matrix_positions_form_text() {
        let mut form = Dictionary::new();
        form.set(
            "Matrix",
            Object::Array(vec![
                1.into(),
                0.into(),
                0.into(),
                1.into(),
                120.into(),
                0.into(),
            ]),
        );
        let content = format!("{}/Fm0 Do", at(100.0, 700.0, "Page"));
        let f = fixture(
            mono(),
            content.as_bytes(),
            Some((form, b"BT /F1 10 Tf 100 700 Td (Form) Tj ET")),
            &[],
        );
        assert_eq!(layout(&f).text, format!("Page{}Form", " ".repeat(16)));
    }

    #[test]
    fn horizontal_scaling_shrinks_advances() {
        // Tz 50: glyphs advance 3 pt, so "ab" then a word 12 pt later.
        let content = b"BT /F1 10 Tf 50 Tz 50 700 Td (ab) Tj ET" as &[u8];
        let f = fixture(mono(), content, None, &[]);
        let page = layout_page(&f.doc, f.page, Some(3.0));
        assert_eq!(page.text, "ab");
    }

    #[test]
    fn baseline_jitter_stays_on_one_line() {
        let content = at(50.0, 700.0, "Left") + &at(200.0, 700.8, "Right");
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        assert_eq!(lines(&layout(&f)).len(), 1);
    }

    #[test]
    fn overprinted_fake_bold_collapses() {
        let content = at(100.0, 700.0, "Bold") + &at(100.3, 700.0, "Bold");
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        assert_eq!(layout(&f).text, "Bold");
    }

    #[test]
    fn repeated_letters_are_not_mistaken_for_overprint() {
        let f = fixture(mono(), at(100.0, 700.0, "1100").as_bytes(), None, &[]);
        assert_eq!(layout(&f).text, "1100");
    }

    #[test]
    fn rotated_page_reads_in_display_orientation() {
        // /Rotate 90: text drawn up the page (Tm x-axis = +y) displays
        // left-to-right; smaller user x displays higher.
        let content = b"BT /F1 10 Tf 0 1 -1 0 80 100 Tm (Upper) Tj ET \
                        BT /F1 10 Tf 0 1 -1 0 100 100 Tm (Lower) Tj ET";
        let f = fixture(mono(), content, None, &[("Rotate", 90.into())]);
        let page = layout(&f);
        assert_eq!(lines(&page), ["Upper", "Lower"]);
        assert_eq!(page.rotate, 90);
        assert_eq!(page.non_horizontal_glyphs, 0);
    }

    #[test]
    fn rotated_label_goes_after_the_grid() {
        let content = format!(
            "{}BT /F1 10 Tf 0 1 -1 0 300 300 Tm (Label) Tj ET",
            at(50.0, 700.0, "Body")
        );
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        let page = layout(&f);
        assert_eq!(lines(&page), ["Body", NON_HORIZONTAL_MARKER, "Label"]);
        assert_eq!(page.non_horizontal_glyphs, 5);
    }

    #[test]
    fn crop_box_is_intersected_with_media_box_and_inherited() {
        let crop = Object::Array(vec![100.into(), (-50).into(), 700.into(), 700.into()]);
        let f = fixture(
            mono(),
            at(150.0, 600.0, "x").as_bytes(),
            None,
            &[("CropBox", crop)],
        );
        assert_eq!(layout(&f).crop_box, [100.0, 0.0, 612.0, 700.0]);
    }

    // ── Review regressions (synthetic; the originals were personal PDFs) ─

    #[test]
    fn padding_space_drawn_inside_an_amount_does_not_split_it() {
        // An Excel export: the amount is one TJ with tiny kerns, and a padding
        // `( )` string is drawn afterwards at an x inside the amount's "1".
        let content = b"BT /F1 10 Tf 1 0 0 1 280 700 Tm ($) Tj ET\n\
            BT /F1 10 Tf 1 0 0 1 290.93 700 Tm [(1) -3 (,2) -3 (4) -3 (7) -3 (.03)] TJ ET\n\
            BT /F1 10 Tf 1 0 0 1 291.29 700 Tm ( ) Tj ET";
        let f = fixture(prop(), content, None, &[]);
        let page = layout(&f);
        assert!(page.text.contains("1,247.03"), "{:?}", page.text);
    }

    #[test]
    fn leader_dots_drawn_over_a_price_do_not_interleave_with_it() {
        let content = format!(
            "{}{}",
            at(290.0, 700.0, "3.50"),
            at(200.0, 700.0, "..............................")
        );
        let f = fixture(prop(), content.as_bytes(), None, &[]);
        let page = layout(&f);
        // Two separate runs: the price is never spliced into the dots.
        assert!(page.text.contains(". 3.50"), "{:?}", page.text);
        assert!(
            page.text.contains(".............................."),
            "{:?}",
            page.text
        );
    }

    #[test]
    fn a_large_stamp_does_not_merge_the_rows_beneath_it() {
        let content = format!(
            "{}{}{}BT /F1 60 Tf 1 0 0 1 350 520 Tm (PAID) Tj ET",
            at(50.0, 512.0, "01/01 Coffee 4.50"),
            at(50.0, 500.0, "01/02 Opening 1.00"),
            ""
        );
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        let page = layout(&f);
        let l = lines(&page);
        assert!(l.iter().any(|x| x.contains("01/01 Coffee 4.50")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("01/02 Opening 1.00")), "{l:#?}");
    }

    #[test]
    fn tiny_horizontal_scaling_keeps_repeated_digits() {
        let content = b"BT /F1 10 Tf 10 Tz 50 700 Td (Acct 1100229) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        let page = layout(&f);
        assert!(page.text.contains("1100229"), "{:?}", page.text);
        assert!(page.text.contains("Acct"), "{:?}", page.text);
    }

    #[test]
    fn tiny_font_size_keeps_repeated_digits() {
        let content = b"BT /F1 1 Tf 50 700 Td (1100229) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        assert!(layout(&f).text.contains("1100229"), "{:?}", layout(&f).text);
    }

    #[test]
    fn zero_width_glyphs_are_not_mistaken_for_overprint() {
        let font = font_with_widths("ABCDEF+Zero", |c| if c == b'Z' { 0 } else { 600 });
        let f = fixture(font, at(50.0, 700.0, "Z Z").as_bytes(), None, &[]);
        assert_eq!(layout(&f).text, "Z Z");
    }

    #[test]
    fn repeated_digits_drawn_right_to_left_at_a_tiny_size_survive() {
        // Two "8"s 0.6 pt apart (adjacent at 1 pt), the right one drawn first: two
        // chunks with the same text, closer than any absolute tolerance.
        let content = b"BT /F1 1 Tf 1 0 0 1 50.6 700 Tm (8) Tj ET \
                        BT /F1 1 Tf 1 0 0 1 50 700 Tm (8) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        assert_eq!(layout_page(&f.doc, f.page, Some(MIN_CELL)).text, "88");
    }

    #[test]
    fn prose_keeps_its_own_single_spaces() {
        // The spaces are drawn, so they stay exactly as drawn: with a 2 pt cell,
        // placing each word on the grid independently would put 3 spaces here.
        let f = fixture(
            prop(),
            at(103.0, 700.0, "Payroll deposit via ACME Corp").as_bytes(),
            None,
            &[],
        );
        assert_eq!(
            layout_page(&f.doc, f.page, Some(2.0)).text,
            "Payroll deposit via ACME Corp"
        );
    }

    #[test]
    fn a_trailing_space_is_not_lost_when_the_next_word_comes_later() {
        // "Payroll " ends in a drawn space; "deposit" is drawn after an unrelated
        // string elsewhere, so it starts a new chunk.  The space must survive.
        let content = format!(
            "{}{}{}",
            at(50.0, 700.0, "Payroll "),
            at(50.0, 600.0, "Other"),
            at(50.0 + 8.0 * 6.0, 700.0, "deposit")
        );
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        assert_eq!(lines(&layout(&f))[0], "Payroll deposit");
    }

    #[test]
    fn a_lone_space_string_between_rows_adds_no_blank_line() {
        let content =
            at(50.0, 700.0, "Row one") + &at(80.0, 694.0, " ") + &at(50.0, 688.0, "Row two");
        let f = fixture(mono(), content.as_bytes(), None, &[]);
        assert_eq!(lines(&layout(&f)), ["Row one", "Row two"]);
    }

    #[test]
    fn a_padding_space_drawn_before_the_amount_adds_no_column() {
        // A padding `( )` drawn first, at an x inside the amount, must change
        // nothing: the page reads exactly as it does without it.
        let amount = b"BT /F1 10 Tf 1 0 0 1 280 700 Tm ($) Tj ET\n\
            BT /F1 10 Tf 1 0 0 1 290.93 700 Tm (1,247.03) Tj ET";
        let padded = [
            b"BT /F1 10 Tf 1 0 0 1 291.29 700 Tm ( ) Tj ET\n".as_slice(),
            amount,
        ]
        .concat();
        let plain = fixture(prop(), amount, None, &[]);
        let with_pad = fixture(prop(), &padded, None, &[]);
        for cell in [None, Some(2.0)] {
            assert_eq!(
                layout_page(&with_pad.doc, with_pad.page, cell).text,
                layout_page(&plain.doc, plain.page, cell).text,
                "cell {cell:?}"
            );
        }
    }

    #[test]
    fn mirrored_text_matrix_with_negative_size_reads_left_to_right() {
        // Tf -10 with Tm [-1 0 0 -1]: the rendering matrix is [10 0 0 10], upright.
        let content = b"BT /F1 -10 Tf -1 0 0 -1 50 700 Tm (12345678) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        let page = layout(&f);
        assert_eq!(page.non_horizontal_glyphs, 0, "{:?}", page.text);
        assert_eq!(page.text, "12345678");
    }

    #[test]
    fn negative_horizontal_scaling_does_not_split_every_glyph() {
        // Tz -100 with Tm [-1 0 0 1]: again upright, left to right.
        let content = b"BT /F1 10 Tf -100 Tz -1 0 0 1 50 700 Tm (12345678) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        assert_eq!(layout(&f).text, "12345678");
    }

    #[test]
    fn hostile_coordinates_go_off_page_without_ballooning() {
        let content = b"BT /F1 10 Tf 1 0 0 1 9000000000000000000 700 Tm (A) Tj ET \
                        BT /F1 10 Tf 1 0 0 1 600000000 700 Tm (B) Tj ET \
                        BT /F1 10 Tf 1 0 0 1 50 700 Tm (C) Tj ET";
        let f = fixture(mono(), content, None, &[]);
        let page = layout(&f);
        assert_eq!(page.off_page_glyphs, 2);
        assert_eq!(lines(&page), ["C", OFF_PAGE_MARKER, "A", "B"]);
    }

    #[test]
    fn a_huge_media_box_cannot_stretch_a_line_past_the_column_cap() {
        let huge = Object::Array(vec![0.into(), 0.into(), Object::Real(1e12), 792.into()]);
        let content = at(0.0, 700.0, "L") + &at(1e11, 700.0, "R");
        let f = fixture(mono(), content.as_bytes(), None, &[("MediaBox", huge)]);
        let page = layout_page(&f.doc, f.page, Some(MIN_CELL));
        assert!(page.text.len() <= MAX_COLUMN + 2, "{}", page.text.len());
        assert!(page.text.ends_with('R'));
    }

    #[test]
    fn real_valued_rotate_is_honored() {
        let content = b"BT /F1 10 Tf 0 1 -1 0 80 100 Tm (Upper) Tj ET \
                        BT /F1 10 Tf 0 1 -1 0 100 100 Tm (Lower) Tj ET";
        let f = fixture(mono(), content, None, &[("Rotate", Object::Real(90.0))]);
        let page = layout(&f);
        assert_eq!(page.rotate, 90);
        assert_eq!(lines(&page), ["Upper", "Lower"]);
    }

    #[test]
    fn widths_without_first_char_make_layout_degraded() {
        let mut font = mono();
        font.remove(b"FirstChar");
        let f = fixture(font, at(50.0, 700.0, "Hi there").as_bytes(), None, &[]);
        let (json, had_issues) = layout_json_value(&f.doc, None, None);
        assert_eq!(json["reliability"]["verdict"], "degraded", "{json}");
        assert!(had_issues);
    }

    #[test]
    fn tf_naming_a_missing_font_says_so() {
        let f = fixture(mono(), b"BT /F9 10 Tf 50 700 Td (Hi) Tj ET", None, &[]);
        let (json, _) = layout_json_value(&f.doc, None, None);
        let reasons = json["reliability"]["fonts"].to_string();
        assert!(reasons.contains("missing from the resources"), "{reasons}");
    }

    // ── Reliability ─────────────────────────────────────────────────────

    #[test]
    fn standard_14_without_widths_is_degraded_under_layout_only() {
        let mut helv = Dictionary::new();
        helv.set("Type", Object::Name(b"Font".to_vec()));
        helv.set("Subtype", Object::Name(b"Type1".to_vec()));
        helv.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        let f = fixture(helv, at(50.0, 700.0, "Hi").as_bytes(), None, &[]);
        let (json, had_issues) = layout_json_value(&f.doc, None, None);
        assert_eq!(json["reliability"]["verdict"], "degraded", "{json}");
        assert!(had_issues, "degraded layout must exit 3");
        let reason = json["reliability"]["fonts"][0]["reason"].as_str().unwrap();
        assert!(reason.contains("AFM"), "{reason}");
        // The text is still extracted, and plain --text is unaffected.
        assert_eq!(json["pages"][0]["text"], "Hi");
        let (plain, plain_issues) = crate::text::text_json_value(&f.doc, None);
        assert_eq!(plain["reliability"]["verdict"], "reliable");
        assert!(!plain_issues);
    }

    #[test]
    fn text_with_no_font_selected_is_degraded() {
        let f = fixture(mono(), b"BT 50 700 Td (Hi) Tj ET", None, &[]);
        let (json, had_issues) = layout_json_value(&f.doc, None, None);
        assert_eq!(json["reliability"]["verdict"], "degraded", "{json}");
        assert!(had_issues);
    }

    #[test]
    fn cid_font_uses_w_widths() {
        // Identity-H, ToUnicode A/B, widths 1000 each: 10 pt per glyph at 10 pt.
        let mut doc_font = Dictionary::new();
        doc_font.set("Type", Object::Name(b"Font".to_vec()));
        doc_font.set("Subtype", Object::Name(b"Type0".to_vec()));
        doc_font.set("BaseFont", Object::Name(b"ABCDEF+Cid".to_vec()));
        doc_font.set("Encoding", Object::Name(b"Identity-H".to_vec()));
        let mut f = fixture(doc_font, b"", None, &[]);
        let cmap = b"begincodespacerange <0000> <FFFF> endcodespacerange \
                     beginbfchar <0041> <0041> <0042> <0042> endbfchar";
        let tu = f.doc.add_object(Object::Stream(Stream::new(
            Dictionary::new(),
            cmap.to_vec(),
        )));
        let mut desc = Dictionary::new();
        desc.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
        desc.set("DW", Object::Integer(1000));
        let desc_id = f.doc.add_object(Object::Dictionary(desc));
        let content = b"BT /F1 10 Tf 50 700 Td <00410042> Tj ET \
                        BT /F1 10 Tf 71 700 Td <0041> Tj ET";
        let c_id = f.doc.add_object(Object::Stream(Stream::new(
            Dictionary::new(),
            content.to_vec(),
        )));
        let font_id = {
            let Ok(Object::Dictionary(page)) = f.doc.get_object(f.page) else {
                panic!()
            };
            let res = page.get(b"Resources").unwrap().as_dict().unwrap();
            let fonts = res.get(b"Font").unwrap().as_dict().unwrap();
            fonts.get(b"F1").unwrap().as_reference().unwrap()
        };
        if let Ok(Object::Dictionary(d)) = f.doc.get_object_mut(font_id) {
            d.set("ToUnicode", Object::Reference(tu));
            d.set(
                "DescendantFonts",
                Object::Array(vec![Object::Reference(desc_id)]),
            );
        }
        if let Ok(Object::Dictionary(d)) = f.doc.get_object_mut(f.page) {
            d.set("Contents", Object::Reference(c_id));
        }
        let page = layout_page(&f.doc, f.page, Some(10.0));
        // "AB" ends at 70; the second "A" at 71 is 1 pt on: same run.
        assert_eq!(page.text, "ABA");
        assert!(
            page.fonts
                .iter()
                .all(|r| r.classification == Reliability::Reliable)
        );
    }
}
