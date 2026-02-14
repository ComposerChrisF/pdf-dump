# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

- **Build:** `cargo build`
- **Run:** `cargo run -- <file.pdf> [options]`
- **Test:** `cargo test`
- **Check (fast lint):** `cargo check`
- **Clippy:** `cargo clippy`

## What This Is

`pdf-dump` is a Rust CLI tool that dumps the internal object structure of a PDF file. It uses `lopdf` for PDF parsing, `clap` (derive) for CLI arguments, `flate2` for zlib/FlateDecode stream decompression, and `weezl` for LZW decoding.

## Architecture

The entire tool lives in `src/main.rs` (~2500 lines + ~5500 lines of tests). The flow is:

1. **CLI parsing** — `Args` struct via clap derive. Modes: dump (default), extract, inspect object, summary, page, metadata, search, text, refs-to, fonts, images, validate, tree. Only one mode flag at a time (with exceptions: `--search --summary`, `--text --page`).
2. **Dump mode** — Prints the trailer, then traverses the object tree starting from the `/Root` reference. `dump_object_and_children` does a depth-first walk using a `BTreeSet<ObjectId>` to avoid revisiting objects. Each object's references are collected during printing and then recursively followed. Respects `--depth N` to limit traversal.
3. **Extract mode** — Pulls a single stream object by ID number (generation 0 assumed), decodes it, and writes raw bytes to a file.
4. **Object mode** (`--object N`) — Prints a single object without following references.
5. **Summary mode** (`--summary`) — One-line-per-object table showing kind, /Type, and details.
6. **Page mode** (`--page N`) — Dumps only the subtree for a specific page by pre-seeding the visited set with /Parent.
7. **Metadata mode** (`--metadata`) — Shows PDF version, object/page counts, /Info fields, and catalog properties.
8. **Search mode** (`--search <expr>`) — Find objects matching key/value criteria (Type=Font, key=MediaBox, value=Hello). Conditions ANDed. `--summary` modifier shows one-line table.
9. **Text mode** (`--text`) — Extract readable text from page content streams (Tj, TJ, ', " operators). `--page N` filters to a single page.
10. **Diff mode** (`--diff <file2.pdf>`) — Structural comparison of two PDFs: metadata, page dicts, resources, content streams, fonts. Works with `--page` and `--json`.
11. **Refs-To mode** (`--refs-to N`) — Reverse reference lookup. Finds all objects referencing a given object, with key paths.
12. **Fonts mode** (`--fonts`) — Lists all fonts with BaseFont, Subtype, Encoding, and embedded status.
13. **Images mode** (`--images`) — Lists all images with dimensions, color space, BPC, filter, and stream size.
14. **Validate mode** (`--validate`) — Structural validation: broken refs, unreachable objects, required keys, stream lengths, page tree.
15. **Tree mode** (`--tree`) — Shows the object graph as an indented reference tree with IDs, types, and key paths. Marks revisited nodes. Respects `--depth N`.
16. **JSON modifier** (`--json`) — Structured JSON output for all modes. Uses `serde_json`. Each PDF object maps to a JSON type schema.
17. **`print_object`** — Recursive pretty-printer that handles all `lopdf::Object` variants. Collects `(is_contents, ObjectId)` pairs into `child_refs` for the caller to traverse. When a dictionary key is `/Contents`, the `is_contents` flag propagates so content streams get parsed via `lopdf::content::Content::decode`.
18. **`decode_stream`** — Filter pipeline processor. Supports FlateDecode, ASCII85Decode, ASCIIHexDecode, and LZWDecode. Applies filters sequentially. Returns `(Cow<[u8]>, Option<String>)` — decoded data and optional warning on failure or unsupported filter.
19. **`object_to_json`** — Maps each `lopdf::Object` variant to a `serde_json::Value` with a `type` field + value fields.

## Key Flags

**Mode flags** (mutually exclusive):
- `--object N` (`-o N`) — Print a single object by number (generation 0), no traversal
- `--summary` (`-s`) — One-line-per-object overview table
- `--page N` — Dump the object tree for a specific page (1-based)
- `--metadata` (`-m`) — Print document metadata (version, pages, /Info fields)
- `--search <expr>` — Find objects matching expression (e.g. `Type=Font`, `key=MediaBox`, `value=Hello`)
- `--text` — Extract readable text from content streams (all pages, or `--text --page N`)
- `--refs-to N` — Find all objects that reference object N, with key paths
- `--fonts` — List all fonts with BaseFont, Subtype, Encoding, and embedded status
- `--images` — List all images with dimensions, color space, BPC, filter, size
- `--validate` — Run structural validation checks (broken refs, unreachable objects, required keys, stream lengths, page tree)
- `--tree` — Show the object graph as an indented reference tree with IDs and types
- `--extract-object <N> --output <path>` — Extract a stream object to a file

**Modifier flags** (combine with modes):
- `--json` — Structured JSON output (works with every mode)
- `--diff <file2.pdf>` — Compare two PDFs structurally (works with default, `--page`, `--json`)
- `--decode-streams` — Decompress and display stream contents (works with dump, --object, --page, --search). Supports FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode filter pipelines.
- `--truncate-binary-streams` — Limit binary stream output to 100 bytes (shorthand for `--truncate 100`)
- `--truncate <N>` — Limit binary stream output to N bytes (conflicts with `--truncate-binary-streams`)
- `--hex` — Display binary streams as hex dump (use with `--decode-streams`)
- `--depth N` — Limit traversal depth (0 = root only). Works with dump, page, tree, and JSON modes.

**Special combinations:**
- `--search <expr> --summary` — Search results as one-line table
- `--text --page N` — Extract text from specific page only
- `--diff <file2.pdf> --page N` — Compare only page N
- `--diff <file2.pdf> --json` — JSON diff output
- `--decode-streams --hex` — Hex dump for binary stream content
- `--tree --depth N` — Tree view limited to N levels
- `--tree --json` — Tree as structured JSON

## Rust Edition

Uses Rust edition **2024** — requires a recent nightly or stable toolchain that supports it.
