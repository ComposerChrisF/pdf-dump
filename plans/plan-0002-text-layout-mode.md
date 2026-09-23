# Plan: `--text --layout` — Position-Faithful Text Extraction

## Problem

`--text` recovers _what_ a page says but not _where_.  `process_content` (`src/text.rs`) tracks no text matrix, no CTM, and no glyph advances: a line break is inferred from a negative `Td`/`TD` operand or a `T*`, and a space from a `TJ` adjustment below −100.  Reading order is content-stream order.

That is fine for prose and wrong for tables.  The motivating case is a financial statement:

- **Column identity is lost.**  A transaction row fills either the _debit_ or the _credit_ column, never both.  Flattened, the amount carries no trace of which column it came from — the one ambiguity that changes the meaning of the data.
- **Multi-line descriptions detach** from the row whose amount they belong to.
- **Content-stream order is not reading order.**  Statement generators routinely draw all the dates, then all the descriptions, then all the amounts.

The consumer is an AI reading the extraction (after it passes through `pii-redact`, `~/Chris/Proj/Coding/cli-specs/pii-redact-spec.md`), for backward-looking statements that have no CSV/OFX download.  A running-balance column lets the reader recover signs and check completeness — _if_ the columns survive extraction.

## Proposed Change

A `--layout` modifier on `--text` (combinable with `--page` and `--json`):

```
pdf-dump statement.pdf --text --layout            # character-grid text, columns aligned
pdf-dump statement.pdf --text --layout --json     # the same, plus span geometry
```

**Text output** places each glyph on a character grid: lines are clusters of glyphs sharing a baseline (within a tolerance), sorted top to bottom; within a line, glyphs sort left to right and land at column `round(x / cell)`, padded with spaces.  `cell` defaults to the page’s median glyph advance at the dominant font size; `--layout-cell <pt>` overrides it.  This is the shape of poppler’s `pdftotext -layout`.

**JSON output** adds, per page, a `spans` array — each a run of same-font, same-baseline glyphs with its decoded text, its bounding box in default user space (`x0 y0 x1 y1`), font resource name, and effective size.  Spans are the unit a redaction engine needs (medpdf `plan-0007`), and are independently useful for debugging (“why did this word extract out of order?”).

**Reliability** extends the existing verdict rather than adding a second one.  Positions are only as good as the widths behind them: a font whose advances cannot be determined (below) makes layout **Degraded**, which already means exit 3 under the `--text` contract.  Never place glyphs with an invented width and report Reliable.

## Implementation Notes

A real text-state machine, replacing the heuristics only under `--layout` (plain `--text` output must not change — its tests pin it):

- **Graphics state:** `q`/`Q` stack, `cm` concatenation into the CTM.  Form XObjects apply their `/Matrix` on the existing recursion path (`recurse_into_form`).
- **Text state:** `BT`/`ET`; `Tm` and the line matrix; `Td`, `TD` (which also sets leading), `T*`, `'`, `"`; `Tc`, `Tw` (applied only to single-byte code 32), `Tz`, `TL`, `Ts`, `Tf` size.
- **Glyph advance:** simple fonts from `/Widths` + `/FirstChar`; CID fonts from `/W` + `/DW`; Type3 through `/FontMatrix`.  **Standard-14 fonts carry no `/Widths`** and need the AFM width tables — pdf-dump has none today.  Check whether medpdf’s watermark alignment already embeds them before adding a copy; if it does, that is a reason to share, not duplicate.
- **`TJ` adjustments** move the pen by `−n/1000 × size × Tz`; the current `< −100 ⇒ space` heuristic becomes a gap-width comparison against the space advance.
- **Page geometry:** honor `/Rotate` and the CropBox origin, so coordinates in JSON are stated in one documented space.
- **Non-horizontal text** (rotated labels, vertical writing) does not belong on the grid.  Emit it after the page’s grid under a marker line, and count it in JSON, rather than splicing it into rows.
- **Overprint dedup:** some generators fake bold by drawing a string twice with a small offset.  Collapse identical glyphs within a fraction of a point.

**Tests.**  Fixtures are synthetic and built with `pdf-maker --watermark` at exact coordinates: a statement-shaped table with alternating blank debit/credit cells, a description that wraps, and a stream drawing columns out of reading order.  Assert column alignment, not just content.  Poppler’s `pdftotext -layout` is a useful _differential oracle_ in tests where it is installed (medpdf’s visual tests already assume poppler) — not a dependency.  No real statement may be a fixture: the whole point of the downstream pipeline is that its inputs are never read by an AI.

## Why Not a Workaround

`pdftotext -layout` exists and does most of this, but it is outside the portfolio’s reliability contract: it has no Reliable/Degraded verdict and no exit 3 for undecodable fonts, so an extraction that silently mangled the account-number line would look exactly like a clean one.  The redaction pipeline depends on being told when extraction cannot be trusted.  The span geometry is also what medpdf `plan-0007` (true redaction) needs; whether the two share one positioning engine or deliberately keep two, so that verification runs through an independent code path, is an open question recorded there.
