# Plan: `--text --layout` — Position-Faithful Text Extraction

## Problem

`--text` recovers _what_ a page says but not _where_.  `process_content` (`src/text.rs`) tracks no text matrix, no CTM, and no glyph advances: a line break is inferred from a negative `Td`/`TD` operand or a `T*`, and a space from a `TJ` adjustment below −100.  Reading order is content-stream order.

That is fine for prose and wrong for tables.  The motivating case is a financial statement:

- **Column identity is lost.**  A transaction row fills either the _debit_ or the _credit_ column, never both.  Flattened, the amount carries no trace of which column it came from — the one ambiguity that changes the meaning of the data.
- **Multi-line descriptions detach** from the row whose amount they belong to.
- **Content-stream order is not reading order.**  Statement generators routinely draw all the dates, then all the descriptions, then all the amounts.

The consumer is an AI reading the extraction (after it passes through `id-redact`, `~/Chris/Proj/Coding/cli-specs/id-redact-spec.md`; the tool was named `pii-redact` when this plan was filed), for backward-looking statements that have no CSV/OFX download.  A running-balance column lets the reader recover signs and check completeness — _if_ the columns survive extraction.

## Proposed Change

A `--layout` modifier on `--text` (combinable with `--page` and `--json`):

```
pdf-dump statement.pdf --text --layout            # character-grid text, columns aligned
pdf-dump statement.pdf --text --layout --json     # the same, plus span geometry
```

**Text output** places each glyph on a character grid: lines are clusters of glyphs sharing a baseline (within a tolerance), sorted top to bottom; within a line, glyphs sort left to right and land at column `round(x / cell)`, padded with spaces.  `cell` defaults to the page’s median glyph advance at the dominant font size; `--layout-cell <pt>` overrides it.  This is the shape of poppler’s `pdftotext -layout`.

**No invented spaces.**  Grid rounding must never put a space between two glyphs that have no real gap between them.  A space inside a run comes only from an actual gap: the pen advance, `TJ` adjustment, or position jump compared against the font’s space advance.  Column padding goes only _between_ runs that are genuinely separate.  `id-redact`’s spec records this as a guarantee pdf-dump owes.  A digit run printed as one string (`12345678`) that came out as `1234 5678` because of column quantization would demote an exact Tier-1 redaction to a Tier-2 hold.  That is still safe, but it is noise, and nobody would notice the drift.  The reverse, padded adjacent columns reading as one run, is expected and acceptable.

**JSON output** adds, per page, a `spans` array — each a run of same-font, same-baseline glyphs with its decoded text, its bounding box, font resource name, and effective size.  Spans are the unit a redaction engine needs (medpdf `plan-0007`), and are independently useful for debugging (“why did this word extract out of order?”).

**Two coordinate spaces, deliberately.**  They serve different consumers:

- **JSON spans are in unrotated default user space**, the page’s own coordinates before `/Rotate` is applied.  That is the space medpdf needs, both to remove glyphs and to draw area rectangles.
- **The text grid is built in visual space**: rotated by `/Rotate` and made relative to the effective CropBox, so a landscape statement reads top to bottom as it displays.
- **Each page’s JSON entry also carries its `/Rotate` and its effective CropBox** (both inherited per `helpers::inherited_page_attr`), so a consumer can map between the two spaces without re-reading the PDF.

**Landing order.**  The grid text ships first, because it is all `id-redact` v1 consumes.  The `spans` array follows in a second landing for medpdf `plan-0007`, and the first landing writes a dated status banner here saying so.

**Reliability** extends the existing verdict rather than adding a second one.  Positions are only as good as the widths behind them, so a font whose advances cannot be determined (see below) makes layout **Degraded**.  Never place glyphs with an invented width and report Reliable.

_Amended 2026-09-23._  As first filed, this paragraph said Degraded “already means exit 3 under the `--text` contract”.  It did not: through v0.24.x only Unreliable exits 3, and Degraded printed a stderr banner and exited 0, so a gate that stops on exit 3 would have passed it.  Chris decided that **Degraded exits 3 for all `--text`**, not only under `--layout`, so each verdict has one exit code.  That change lands in its own release _before_ this plan, with `--help`, the README, and the `pdf-tools` skill updated.  The extracted text is still printed and `--json` still emits on exit 3, because findings are data.

## Implementation Notes

A real text-state machine, replacing the heuristics only under `--layout` (plain `--text` output must not change — its tests pin it):

- **Graphics state:** `q`/`Q` stack, `cm` concatenation into the CTM.  Form XObjects apply their `/Matrix` on the existing recursion path (`recurse_into_form`).
- **Text state:** `BT`/`ET`; `Tm` and the line matrix; `Td`, `TD` (which also sets leading), `T*`, `'`, `"`; `Tc`, `Tw` (applied only to single-byte code 32), `Tz`, `TL`, `Ts`, `Tf` size.
- **Glyph advance:** simple fonts from `/Widths` + `/FirstChar`; CID fonts from `/W` + `/DW`; Type3 through `/FontMatrix`.  **Standard-14 fonts carry no `/Widths`** and need the AFM width tables — pdf-dump has none today, and neither does medpdf (checked 2026-09-23).  Embed Adobe’s 14 AFM width tables in pdf-dump, with Adobe’s license notice, the same way `glyphlist.rs` embeds the AGL.  They need not ship in the same landing: until they do, a Standard-14 font with no `/Widths` is Degraded, which means exit 3.  `id-redact` accepts that interim fail-closed state.  Whether medpdf later shares these tables or keeps its own copy is medpdf `plan-0007`’s open question (share, or keep an independent verification path).
- **`TJ` adjustments** move the pen by `−n/1000 × size × Tz`; the current `< −100 ⇒ space` heuristic becomes a gap-width comparison against the space advance.
- **Page geometry:** honor `/Rotate` and the CropBox origin, so coordinates in JSON are stated in one documented space.
- **Non-horizontal text** (rotated labels, vertical writing) does not belong on the grid.  Emit it after the page’s grid under a marker line, and count it in JSON, rather than splicing it into rows.
- **Overprint dedup:** some generators fake bold by drawing a string twice with a small offset.  Collapse identical glyphs within a fraction of a point.

**Tests.**  One fixture must pin the no-invented-spaces guarantee: a long digit run placed where `round(x / cell)` quantizes badly.  Fixtures are synthetic and built with `pdf-maker --watermark` at exact coordinates: a statement-shaped table with alternating blank debit/credit cells, a description that wraps, and a stream drawing columns out of reading order.  Assert column alignment, not just content.  Poppler’s `pdftotext -layout` is a useful _differential oracle_ in tests where it is installed (medpdf’s visual tests already assume poppler) — not a dependency.  No real statement may be a fixture: the whole point of the downstream pipeline is that its inputs are never read by an AI.

## Prerequisites (landed v0.24.1)

bug-0003 (content streams fused across `/Contents` segments), bug-0012 and bug-0036 (reliability verdict under-reporting), bug-0014 (form XObjects did not inherit the caller’s font; `q`/`Q` not modeled), bug-0022 (`/Rotate`/CropBox not inherited), and bug-0013 (a crash from the default command) were fixed first.  The `q`/`Q` stack from bug-0014 is where this plan’s graphics-state stack grows the CTM, and bug-0022’s `inherited_page_attr` supplies `/Rotate` and the CropBox.

## Why Not a Workaround

`pdftotext -layout` exists and does most of this, but it is outside the portfolio’s reliability contract: it has no Reliable/Degraded verdict and no exit 3 for undecodable fonts, so an extraction that silently mangled the account-number line would look exactly like a clean one.  The redaction pipeline depends on being told when extraction cannot be trusted.  The span geometry is also what medpdf `plan-0007` (true redaction) needs; whether the two share one positioning engine or deliberately keep two, so that verification runs through an independent code path, is an open question recorded there.
