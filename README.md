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
| `--find-text "pattern"` | Case-insensitive text search with context |
| `--fonts` | List all fonts with encoding and embedding details |
| `--images` | List all images with dimensions, color space, filters |
| `--forms` | List AcroForm fields with names, types, values |
| `--bookmarks` | Show the document outline tree |
| `--annotations` | Show annotations with link targets |
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

`--text` is font-aware: it decodes character codes through each font’s `/ToUnicode` CMap (the fix for the classic CID/Type0 mojibake) and through WinAnsiEncoding/MacRomanEncoding tables for simple fonts that lack one, falling back to raw byte passthrough when a font can’t be decoded.  When extraction is not fully trustworthy it prints a loud reliability banner to **stderr** (stdout stays clean for piping) and, in `--json` mode, adds a top-level `reliability` object.  The tool exits **3** whenever the verdict is not `reliable` — `unreliable` (a CID/Type0 font with no ToUnicode map) or `degraded` (a font whose encoding is only partly known, or more than 20 % of the codes shown could not be decoded) — so scripts can detect suspect text programmatically.  The text is still printed, and `--json` still emits, on exit 3: the exit code says “read with care”, not “the command failed”.  (Through v0.24.x, `degraded` exited 0.)

### Position-faithful text (`--text --layout`)

`--text --layout` places each glyph by its real position, so tables keep their columns.  That matters for a bank statement, say, where a debit and a credit differ only by which column the amount sits in, and where generators often draw all the dates, then all the descriptions, then all the amounts.  Consecutive glyphs with no real gap between them form a run, which is never split, even by another string drawn over it.  Runs are clustered into lines by baseline, top to bottom, and each run lands at column `round(x / cell)`.  The grid **never inserts a space inside a run**, so a number printed as one string stays one string.  `cell` defaults to the page’s median glyph advance at its dominant font size, and `--layout-cell <pt>` overrides it.

```bash
pdf-dump statement.pdf --text --layout            # character grid, columns aligned
pdf-dump statement.pdf --text --layout --json     # the same, plus per-page rotate, crop_box, cell
```

Positions honor the CTM, form XObject `/Matrix`, `/Rotate` and the CropBox origin, so a landscape page reads as it displays.  Rotated or vertical text does not go on the grid: it follows under a `[non-horizontal text]` line.  Text positioned outside the CropBox follows under `[off-page text]`, shown but never allowed to stretch a line.  Positions are only as good as the glyph widths behind them.  A font whose widths are unknown makes the verdict `degraded` (exit 3) rather than being placed with invented widths.  That includes a Standard-14 font without `/Widths`: pdf-dump does not yet embed the built-in AFM metrics.  Plain `--text` output is unaffected by any of this.

Why not just use `pdftotext -layout`?  It does most of this, but it gives no verdict.  It has no reliable/degraded distinction and no exit 3 for undecodable fonts, so an extraction that silently mangled the account-number line looks exactly like a clean one.  A pipeline that must not pass bad text on (such as id-redact, which redacts identifiers from extracted statements) depends on being told when extraction cannot be trusted.  That verdict is what `--layout` adds.

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
| `--extract-stream N --output file` | Extract a decoded stream to a file |

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

FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, RunLengthDecode — applied sequentially for multi-filter pipelines.

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

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
