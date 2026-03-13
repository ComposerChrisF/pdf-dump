# pdf-dump

A CLI tool for inspecting and debugging the internal structure of PDF files.

`pdf-dump` shows you what's actually inside a PDF — objects, streams, fonts, images, form fields, bookmarks, annotations, tagged structure, and more. Useful for debugging PDF generation, understanding why a PDF looks wrong, or exploring the format.

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
| `--text` | Extract readable text from content streams |
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

### Standalone modes (one at a time)

| Flag | Description |
|------|-------------|
| `--object N` | Print object(s) by number (`5`, `1,5,12`, `3-7`) |
| `--inspect N` | Full explanation of an object's role and relationships |
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

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
