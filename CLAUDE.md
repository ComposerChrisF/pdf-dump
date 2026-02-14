# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

- **Build:** `cargo build`
- **Run:** `cargo run -- <file.pdf> [options]`
- **Test:** `cargo test`
- **Check (fast lint):** `cargo check`
- **Clippy:** `cargo clippy`

## What This Is

`pdf-dump` is a Rust CLI tool that dumps the internal object structure of a PDF file. It uses `lopdf` for PDF parsing, `clap` (derive) for CLI arguments, and `flate2` for zlib/FlateDecode stream decompression.

## Architecture

The entire tool lives in `src/main.rs` (~1000 lines + ~1600 lines of tests). The flow is:

1. **CLI parsing** — `Args` struct via clap derive. Modes: dump (default), extract, inspect object, summary, page, metadata, search, text. Only one mode flag at a time (with exceptions: `--search --summary`, `--text --page`).
2. **Dump mode** — Prints the trailer, then traverses the object tree starting from the `/Root` reference. `dump_object_and_children` does a depth-first walk using a `BTreeSet<ObjectId>` to avoid revisiting objects. Each object's references are collected during printing and then recursively followed.
3. **Extract mode** — Pulls a single stream object by ID number (generation 0 assumed), decodes it, and writes raw bytes to a file.
4. **Object mode** (`--object N`) — Prints a single object without following references.
5. **Summary mode** (`--summary`) — One-line-per-object table showing kind, /Type, and details.
6. **Page mode** (`--page N`) — Dumps only the subtree for a specific page by pre-seeding the visited set with /Parent.
7. **Metadata mode** (`--metadata`) — Shows PDF version, object/page counts, /Info fields, and catalog properties.
8. **Search mode** (`--search <expr>`) — Find objects matching key/value criteria (Type=Font, key=MediaBox, value=Hello). Conditions ANDed. `--summary` modifier shows one-line table.
9. **Text mode** (`--text`) — Extract readable text from page content streams (Tj, TJ, ', " operators). `--page N` filters to a single page.
10. **Diff mode** (`--diff <file2.pdf>`) — Structural comparison of two PDFs: metadata, page dicts, resources, content streams, fonts. Works with `--page` and `--json`.
11. **JSON modifier** (`--json`) — Structured JSON output for all modes. Uses `serde_json`. Each PDF object maps to a JSON type schema.
12. **`print_object`** — Recursive pretty-printer that handles all `lopdf::Object` variants. Collects `(is_contents, ObjectId)` pairs into `child_refs` for the caller to traverse. When a dictionary key is `/Contents`, the `is_contents` flag propagates so content streams get parsed via `lopdf::content::Content::decode`.
13. **`decode_stream`** — Checks `/Filter` for `FlateDecode` and decompresses with `flate2::ZlibDecoder`. Returns `Cow<[u8]>` (borrowed if no decompression needed).
14. **`object_to_json`** — Maps each `lopdf::Object` variant to a `serde_json::Value` with a `type` field + value fields.

## Key Flags

**Mode flags** (mutually exclusive):
- `--object N` (`-o N`) — Print a single object by number (generation 0), no traversal
- `--summary` (`-s`) — One-line-per-object overview table
- `--page N` — Dump the object tree for a specific page (1-based)
- `--metadata` (`-m`) — Print document metadata (version, pages, /Info fields)
- `--search <expr>` — Find objects matching expression (e.g. `Type=Font`, `key=MediaBox`, `value=Hello`)
- `--text` — Extract readable text from content streams (all pages, or `--text --page N`)
- `--extract-object <N> --output <path>` — Extract a stream object to a file

**Modifier flags** (combine with modes):
- `--json` — Structured JSON output (works with every mode)
- `--diff <file2.pdf>` — Compare two PDFs structurally (works with default, `--page`, `--json`)
- `--decode-streams` — Decompress and display stream contents (works with dump, --object, --page, --search)
- `--truncate-binary-streams` — Limit binary stream output to 100 bytes

**Special combinations:**
- `--search <expr> --summary` — Search results as one-line table
- `--text --page N` — Extract text from specific page only
- `--diff <file2.pdf> --page N` — Compare only page N
- `--diff <file2.pdf> --json` — JSON diff output

## Rust Edition

Uses Rust edition **2024** — requires a recent nightly or stable toolchain that supports it.
