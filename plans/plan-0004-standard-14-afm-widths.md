# Plan: Built-In Widths for the Standard-14 Fonts

## Problem

A Standard-14 font (Helvetica, Times, Courier and their styles, Symbol, ZapfDingbats) may legally omit `/Widths`.  The reader is expected to supply the metrics from the fonts’ Adobe Font Metrics (AFM) files.  pdf-dump has no such tables, so under `--text --layout` a glyph in such a font has no known advance.  `metrics.rs` reports it as unknown rather than inventing a width, the font is Degraded, and the run exits 3 even when the text itself decoded perfectly.  Split out of plan-0002 when that plan’s grid landing closed it (2026-09-23).  `id-redact` accepts the fail-closed interim, so this is a quality improvement, not a blocker.

How often this happens is unmeasured.  Many generators embed widths even for the base fonts, but older and minimal writers (and pdf-maker’s own watermark text, worth checking) may not.

## Proposed Change

Embed the AFM advance widths for the 14 fonts and use them in `metrics::font_metrics` when a Standard-14 font has no `/Widths`.  Glyph widths are keyed by glyph name.  Each code is mapped to its name through the font’s effective encoding: its `/Encoding` base plus `/Differences`, or the font’s builtin encoding (StandardEncoding for the text fonts; Symbol and ZapfDingbats have their own).

- **Reliable only when every shown code resolves** to a glyph with an AFM width.  A code whose glyph name has no entry is unknown for that code, and the font Degrades exactly as it does today.
- **Only the 14 exact base names** (after stripping a subset prefix like `ABCDEF+`, and accepting the usual aliases such as `Arial` → Helvetica only if a real sample shows them).  A non-standard font without `/Widths` stays unknown.
- **No change to plain `--text`**, which does not use widths.

## Implementation Notes

- **Source.**  Adobe’s Core 14 AFM files are freely redistributable, with a license notice that must travel with them.  Embed a compact generated table, not the full AFMs: a glyph-name → width map per font, in the same style as `glyphlist.rs`, which embeds Adobe’s `glyphlist.txt` via `include_str!` into a lazy `OnceLock` map.  Keep the notice in the source file and in the README’s licensing section.
- **Code → glyph name** needs the encoding tables as _names_, not as the Unicode strings `encodings.rs` returns today.  Either add name tables for Standard, WinAnsi, MacRoman and MacExpert, or derive them once from the AGL in reverse.  Reverse derivation is lossy where several names map to one code point, so explicit name tables are the safer choice.
- **Where it plugs in:** the `let Some(widths) = array_of(doc, dict, b"Widths") else { … }` branch of `metrics::font_metrics`, which today returns `Unknown` for these fonts.  Add a `MetricsKind::Afm { names: [Option<&'static str>; 256], widths: &'static HashMap<&'static str, u16> }` variant, or resolve the 256 codes eagerly into a `Simple` table with `Some`/`None` per code.  The eager form reuses the existing per-code `Result` path unchanged.
- **Tests.**
  - A Helvetica page with no `/Widths` lays out identically to the same page with Helvetica’s real `/Widths` supplied, and its verdict is Reliable.
  - A `/Differences` glyph missing from the AFM makes the font Degraded.
  - A subset-prefixed standard name resolves.
  - Where `pdftotext` is installed, the existing poppler oracle test gains a no-`/Widths` Helvetica fixture.
- **Sharing with medpdf:** medpdf has no AFM tables either (checked 2026-09-23).  Its true-redaction plan, which would have needed them, was rejected, so nothing needs sharing now.

## Why Not a Workaround

The alternative is to keep Degrading these fonts forever.  That is safe but noisy: a clean document exits 3, and the caller learns to ignore exit 3, which defeats the gate that makes `--layout` trustworthy.  Guessing widths (a fixed 0.5 em, say) is ruled out by `positive-evidence-of-absence.md`: it would put wrong positions under a Reliable verdict.  The AFM tables are the spec’s own answer (PDF 32000-1 §9.6.2.2) and are a one-time, well-bounded addition.
