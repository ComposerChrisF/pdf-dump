# pdf-dump

A CLI tool for inspecting and debugging the internal structure of PDF files.

`pdf-dump` shows you what’s actually inside a PDF — objects, streams, fonts, images, form fields, bookmarks, annotations, tagged structure, and more.  Useful for debugging PDF generation, understanding why a PDF looks wrong, or exploring the format.

## Installation

```bash
cargo install pdf-dump
```

Requires a Rust toolchain that supports edition 2024.

## Quick Start

```bash
# Overview: metadata, validation summary, stream stats, feature indicators
pdf-dump file.pdf

# Extract text
pdf-dump file.pdf --text
pdf-dump file.pdf --text --page 3

# Search for text across pages
pdf-dump file.pdf --find-text "invoice"

# Page info: dimensions, resources, fonts, annotations, text preview
pdf-dump file.pdf --page 3

# List fonts or images
pdf-dump file.pdf --fonts
pdf-dump file.pdf --images

# Explain a specific object
pdf-dump file.pdf --inspect 5

# Find all font objects
pdf-dump file.pdf --search Type=Font

# Structural validation
pdf-dump file.pdf --validate

# One-line listing of every object
pdf-dump file.pdf --list
```

## Modes

### Document-level modes (combinable)

These can be used together — output gets section headers automatically:

| Flag | Description |
|------|-------------|
| `--text` | Extract readable text (font-aware: decodes `/ToUnicode` CMaps and WinAnsi/MacRoman encodings; flags unreliable extraction) |
| `--operators` | Show content stream operators |
| `--find-text "pattern"` | Case-insensitive text search with context; exits 3, like `--text`, when the searched text is not reliably decodable |
| `--fonts` | List all fonts with encoding and embedding details |
| `--images` | List all images with dimensions, color space, filters |
| `--forms` | AcroForm fields with names, types and every value (lists, stream values, inherited values, rich text), plus tooltips, options, widget captions and XFA datasets.  A string that will not decode prints as hex and exits 3 |
| `--bookmarks` | Show the document outline tree |
| `--annotations` | Every annotation in `/Annots`, with link targets, comments, rich text, author, subject, whether it is hidden, and the text its appearance shows.  Text that could not be read reliably exits 3 |
| `--tags` | Show tagged PDF structure tree (accessibility) |
| `--tree` | Show the object graph as an indented reference tree |
| `--validate` | Structural checks: broken refs, unreachable objects, required keys |
| `--list` | One-line-per-object table |
| `--detail <view>` | Detail views: `security`, `embedded`, `labels`, `layers` |

```bash
# Combine freely
pdf-dump file.pdf --fonts --images --validate
```

### Text extraction reliability

`--text` is font-aware: it decodes character codes through each font’s `/ToUnicode` CMap (the fix for the classic CID/Type0 mojibake) and through WinAnsiEncoding/MacRomanEncoding tables for simple fonts that lack one, falling back to raw byte passthrough when a font can’t be decoded.  When extraction is not fully trustworthy it prints a loud reliability banner to **stderr** (stdout stays clean for piping) and, in `--json` mode, adds a top-level `reliability` object.  The tool exits **3** whenever the verdict is not `reliable` — `unreliable` (a CID/Type0 font with no ToUnicode map) or `degraded` (a font whose encoding is only partly known, more than 20 % of the codes shown could not be decoded, or a content stream was not read to its end) — so scripts can detect suspect text programmatically.  The text is still printed, and `--json` still emits, on exit 3: the exit code says “read with care”, not “the command failed”.  (Through v0.24.x, `degraded` exited 0.)

A page whose words are drawn as paths (outlined by a print-to-PDF step) or held in a scanned image shows no text, so `--text` prints nothing for it.  The verdict cannot see this, since it judges only the text that was shown.  Instead, `--text` (and `--layout`) names each page that draws paths or images but shows no text, or whose content could not be read to its end.  The names appear in a stderr note and as `pages_without_text` in `--json`, and the exit code is unchanged; a blank page is not named.  **`--require-text`** makes such a page fatal: the caller asserts that every selected page carries text, and a page that does not, or a document that could not be decrypted or has no pages, exits **1** naming it, with nothing written to stdout or `--output`.

### Position-faithful text (`--text --layout`)

`--text --layout` places each glyph by its real position, so tables keep their columns.  That matters for a bank statement, say, where a debit and a credit differ only by which column the amount sits in, and where generators often draw all the dates, then all the descriptions, then all the amounts.  Consecutive glyphs with no real gap between them form a run, which is never split, even by another string drawn over it.  Runs are clustered into lines by baseline, top to bottom, and each run lands at column `round(x / cell)`.  The grid **never inserts a space inside a run**, so a number printed as one string stays one string.  `cell` defaults to the page’s median glyph advance at its dominant font size, and `--layout-cell <pt>` overrides it.

```bash
pdf-dump statement.pdf --text --layout            # character grid, columns aligned
pdf-dump statement.pdf --text --layout --json     # the same, plus per-page rotate, crop_box, cell
```

Positions honor the CTM, form XObject `/Matrix`, `/Rotate` and the CropBox origin, so a landscape page reads as it displays.  Rotated or vertical text does not go on the grid: it follows under a `[non-horizontal text]` line.  There, rotated text is laid out by direction: each direction’s text forms a small grid of its own, read along its baseline, so a rotated address keeps its lines and words even when drawn one glyph at a time.  Vertical writing and mirrored text follow in content order.  Text positioned outside the CropBox follows under `[off-page text]`, shown but never allowed to stretch a line.  Positions are only as good as the glyph widths behind them.  A font whose widths are unknown makes the verdict `degraded` (exit 3) rather than being placed with invented widths.  A Standard-14 font (Helvetica, Times, Courier and their styles, Symbol, ZapfDingbats) may omit `/Widths`.  pdf-dump then uses Adobe’s AFM widths, which are built in, reaching each glyph by name through the font’s encoding.  Only a glyph missing from the font’s AFM is unknown.  Plain `--text` keeps content order and draws no grid, and its verdict does not depend on widths.  It takes its line breaks and word spaces from the same positions: a new line where the baseline moves, and a space where two strings are a word gap apart, so text drawn one glyph at a time keeps its lines and words.  Where a width is unknown, plain `--text` falls back to the operators for these, as before: a `BT`, a `T*`, or a `Td` downwards starts a line, and a `TJ` gap past 100 is a space.

Why not just use `pdftotext -layout`?  It does most of this, but it gives no verdict.  It has no reliable/degraded distinction and no exit 3 for undecodable fonts, so an extraction that silently mangled the account-number line looks exactly like a clean one.  A pipeline that must not pass bad text on (such as id-redact, which redacts identifiers from extracted statements) depends on being told when extraction cannot be trusted.  That verdict is what `--layout` adds.

### Text to a file (`--text --output`)

`--text --output file.txt` (with `--layout` and `--page` as usual) writes the text to a file instead of stdout, byte for byte what stdout would carry, so sensitive text never passes through a stream a build tool captures.  It is written to a temporary file beside the output and renamed into place, so a killed run leaves no partial file.  Exit 1 writes nothing; exit 3 (degraded or unreliable text) still writes it, as stdout gets it.  The directory must exist, and an existing file is replaced.  `--output` with `--json` or another mode is a usage error (exit 2).

### Lenient stream recovery (and `--strict`)

pdf-dump is a tolerant reader by default.  When a content stream declares a wrong `/Length`, a strict parser fails to find `endstream`, drops the body, and the page text silently vanishes; pdf-dump instead re-reads the raw file, recovers the true body by scanning to `endstream`, and carries on.  Every recovery is announced loudly on **stderr** and, in `--json` mode, surfaced as a top-level `recovery` object — `{repaired, strict, count, streams: [{object, generation, file_offset, declared_length, actual_length}]}` — so machine consumers never mistake repaired output for the original document.  The key is absent for well-formed PDFs, and recovery keeps the default exit code **0**.

Pass `--strict` to invert this: pdf-dump detects the malformation, refuses to repair it (so the affected content stays missing, exactly as a spec-conformant reader would see it), still emits the `recovery` object with `repaired: false`, and exits **3** — a hard gate for CI or any caller that must treat a malformed PDF as a failure.

### Encrypted PDFs (and `--password`)

pdf-dump reads PDFs encrypted with the empty password automatically.  For a PDF protected by a non-empty password, supply it with `--password <PASSWORD>` (the user or owner password); pdf-dump then decrypts and reports everything normally.  Without the correct password it cannot read the body, so it never presents the collapsed counts of a locked file as authoritative: the overview reports `encrypted: true`, adds `decrypted: false`, prints a loud stderr banner naming the algorithm, records a validation warning, and exits **3**.  A wrong `--password` exits **1**.

### Standalone modes (one at a time)

| Flag | Description |
|------|-------------|
| `--object N` | Print object(s) by number (`5`, `1,5,12`, `3-7`) |
| `--inspect N` | Full explanation of an object’s role and relationships |
| `--search <expr>` | Find objects matching criteria (`Type=Font`, `key=MediaBox`, `stream=text`) |
| `--extract-stream N --output file` | Extract a decoded stream to a file.  The file’s directory must already exist (exit 1 otherwise, before the document is read); an existing file is replaced |
| `--extract-image N --output f.png` | Write an image as a file a viewer or OCR engine opens: PNG for decoded pixels, the stream as it is for DCTDecode (`.jpg`) and JPXDecode (`.jp2`).  The picture as drawn, in its stored orientation, including CCITT and JBIG2 scans, palettes, CMYK, `/Decode`, predictors and masks, except that a JPEG’s or JPEG 2000’s mask is left unapplied, with a warning, `mask_applied: false` and exit 3, the file still written.  What it cannot convert it refuses, naming what stopped it, and writes nothing |
| `--extract-inline-image PAGE:INDEX --output f.png` | The same for an inline image, as `--images` lists it |

### Modifiers

| Flag | Effect |
|------|--------|
| `--page N` or `--page N-M` | Filter to specific pages; shows page info when used alone |
| `--json` | Structured JSON output (works with every mode) |
| `--decode` | Decompress stream contents |
| `--deref` | Inline-expand references (with `--object`) |
| `--depth N` | Limit traversal depth (with `--tree`, `--tags`, `--json`) |
| `--hex` | Hex dump for binary streams |
| `--raw` | Raw undecoded stream bytes (with `--object`) |
| `--truncate N` | Limit binary output to N bytes |
| `--dot` | GraphViz DOT output (with `--tree`) |

## JSON Output

Every mode supports `--json` for structured output:

```bash
pdf-dump file.pdf --json                    # Overview as JSON
pdf-dump file.pdf --fonts --json            # Font list as JSON
pdf-dump file.pdf --fonts --images --json   # Combined modes wrapped in a JSON object
pdf-dump file.pdf --validate --json         # Validation results as JSON
```

## Supported Stream Filters

FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, RunLengthDecode — applied sequentially for multi-filter pipelines, with `/Predictor` and `/EarlyChange`.  `--extract-image` also decodes CCITTFaxDecode and JBIG2Decode.

pdf-dump reads a PDF’s structure and draws nothing, so exit 0 does not mean every word a viewer shows was read: text in images or drawn as paths needs rendering and OCR.  `DEBUGGING_WITH_PDF_DUMP.md` § What pdf-dump does not read gathers the limits.

## Acknowledgments

Built on [lopdf](https://github.com/J-F-Liu/lopdf), a pure-Rust PDF parsing library.

## Related Projects

- [medpdf](https://github.com/ComposerChrisF/medpdf) — Medium-level PDF API over lopdf (includes medpdf-image for image embedding)
- [pdf-maker](https://github.com/ComposerChrisF/pdf-maker) — CLI tool for merging, watermarking, and manipulating PDF files

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

The Standard-14 glyph widths in `src/core14.txt` are derived from Adobe’s Core 14 AFM files, reduced to each glyph’s code, width and name.  The file keeps Adobe’s copyright notices and the notice that accompanies the AFM files.  That notice permits use, copying and distribution, with or without modification, provided the copyright notices are kept and modifications are noted.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
