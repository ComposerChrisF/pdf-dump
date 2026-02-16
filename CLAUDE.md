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

The entire tool lives in `src/main.rs` (~5800 lines + ~9800 lines of tests). The flow is:

1. **CLI parsing** — `Args` struct via clap derive. Default mode is overview (metadata + validation + stream stats). Modes: overview (default), dump, extract-stream, inspect object(s), summary, page, metadata, search, text, operators, resources, forms, info, refs-to, fonts, images, validate, tree, stats, bookmarks, annotations, layers, structure. Only one mode flag at a time (with exceptions: `--search --summary`, `--text --page`, `--annotations --page`, `--operators --page`, `--resources --page`). Help output uses `help_heading` for organized grouping.
2. **Overview mode** (default, no flags) — Shows PDF version, page/object counts, encryption status, producer, validation summary (errors/warnings or "no issues"), and stream stats. Designed as the first command for understanding a PDF.
3. **Dump mode** (`--dump`) — Full depth-first traversal. Prints the trailer, then traverses the object tree starting from the `/Root` reference. `dump_object_and_children` does a depth-first walk using a `BTreeSet<ObjectId>` to avoid revisiting objects. Each object's references are collected during printing and then recursively followed. Respects `--depth N` to limit traversal.
4. **Extract mode** (`--extract-stream`) — Pulls a single stream object by ID number (generation 0 assumed), decodes it, and writes raw bytes to a file.
5. **Object mode** (`--object N` or `--object 1,5,10-15`) — Prints one or more objects without following references. Accepts single numbers, comma-separated lists, ranges, or mixed. `--deref` expands references inline.
6. **Summary mode** (`--summary`) — One-line-per-object table showing kind, /Type, and details.
7. **Page mode** (`--page N` or `--page N-M`) — Dumps only the subtree for a specific page or page range by pre-seeding the visited set with /Parent. Accepts single pages (e.g. `5`) or inclusive ranges (e.g. `1-3`). `--deref` expands references inline.
8. **Metadata mode** (`--metadata`) — Shows PDF version, object/page counts, /Info fields, and catalog properties.
9. **Search mode** (`--search <expr>`) — Find objects matching key/value/stream criteria (Type=Font, key=MediaBox, value=Hello, stream=text). Conditions ANDed. `--summary` modifier shows one-line table.
10. **Text mode** (`--text`) — Extract readable text from page content streams (Tj, TJ, ', " operators). `--page N` or `--page N-M` filters to specific pages. Emits warnings on stderr when fonts lack known encodings (CID fonts without ToUnicode, custom fonts without explicit encoding). JSON output includes `"warnings"` array per page.
11. **Operators mode** (`--operators`) — Shows all content stream operators for each page. `--page N` filters to specific pages. Emits warnings for decode failures. JSON output includes `"warnings"` array per page when issues occur.
12. **Resources mode** (`--resources`) — Shows page resource maps: fonts, XObjects, ExtGState, ColorSpaces with details. `--page N` filters. Handles resource inheritance from parent pages.
13. **Forms mode** (`--forms`) — Lists AcroForm fields with qualified names, field types (Tx/Btn/Ch/Sig), values, flags, and page numbers. Walks hierarchical field trees.
14. **Diff mode** (`--diff <file2.pdf>`) — Structural comparison of two PDFs: metadata, page dicts, resources, content streams, fonts. Works with `--page` and `--json`.
15. **Refs-To mode** (`--refs-to N`) — Reverse reference lookup. Finds all objects referencing a given object, with key paths.
16. **Fonts mode** (`--fonts`) — Lists all fonts with BaseFont, Subtype, Encoding, embedded status, and encoding diagnostics (ToUnicode, FirstChar/LastChar/Widths, Differences, CIDSystemInfo).
17. **Images mode** (`--images`) — Lists all images with dimensions, color space, BPC, filter, and stream size.
18. **Validate mode** (`--validate`) — Structural validation: broken refs, unreachable objects, required keys, stream lengths, page tree.
19. **Tree mode** (`--tree`) — Shows the object graph as an indented reference tree with IDs, types, and key paths. Marks revisited nodes. Respects `--depth N`.
20. **Stats mode** (`--stats`) — Document statistics: object type counts, stream byte totals, filter usage histogram, top 10 largest streams.
21. **Bookmarks mode** (`--bookmarks`) — Shows the document outline (bookmark) tree with titles, destinations, and actions.
22. **Annotations mode** (`--annotations`) — Lists all annotations with page number, subtype, rect, and contents. Works with `--page` filter.
23. **Layers mode** (`--layers` / `--ocg`) — Lists Optional Content Groups (layers) with name, default visibility (ON/OFF), and page references. Reads `/OCProperties` from catalog.
24. **Structure mode** (`--structure`) — Shows tagged PDF logical structure tree from `/StructTreeRoot`. Displays element roles, page refs, MCIDs, titles, alt text. Supports `--depth N` to limit tree depth. Cycle detection via `BTreeSet<ObjectId>`.
25. **Info mode** (`--info N`) — The definitive "tell me everything about this object" command. Shows role classification, domain-specific details, page associations, full object content dump, forward references with summaries, and reverse references with key paths. Uses `classify_object()` for role detection and `find_page_associations()` for page context.
26. **JSON modifier** (`--json`) — Structured JSON output for all modes. Uses `serde_json`. Each PDF object maps to a JSON type schema. With `--deref`, references gain a `"resolved"` field.
27. **`print_object`** — Recursive pretty-printer that handles all `lopdf::Object` variants. Collects `(is_contents, ObjectId)` pairs into `child_refs` for the caller to traverse. When a dictionary key is `/Contents`, the `is_contents` flag propagates so content streams get parsed via `lopdf::content::Content::decode`. With `config.deref`, references show inline summaries.
28. **`decode_stream`** — Filter pipeline processor. Supports FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, and RunLengthDecode. Applies filters sequentially. Returns `(Cow<[u8]>, Option<String>)` — decoded data and optional warning on failure or unsupported filter.
29. **`object_to_json`** — Maps each `lopdf::Object` variant to a `serde_json::Value` with a `type` field + value fields.

## Key Flags

**Mode flags** (mutually exclusive):
- `--dump` — Full depth-first dump of all reachable objects from /Root
- `--object N` or `--object 1,5,10-15` (`-o`) — Print one or more objects by number (generation 0), no traversal
- `--summary` (`-s`) — One-line-per-object overview table
- `--page N` or `--page N-M` — Dump the object tree for a specific page or page range (1-based)
- `--metadata` (`-m`) — Print document metadata (version, pages, /Info fields)
- `--search <expr>` — Find objects matching expression (e.g. `Type=Font`, `key=MediaBox`, `value=Hello`, `stream=text`)
- `--text` — Extract readable text from content streams (all pages, or `--text --page N`)
- `--operators` — Show content stream operators (all pages, or `--operators --page N`)
- `--resources` — Show page resource maps (all pages, or `--resources --page N`)
- `--forms` — List form fields (AcroForm) with names, types, values, and page numbers
- `--refs-to N` — Find all objects that reference object N, with key paths
- `--fonts` — List all fonts with BaseFont, Subtype, Encoding, and embedded status
- `--images` — List all images with dimensions, color space, BPC, filter, size
- `--validate` — Run structural validation checks (broken refs, unreachable objects, required keys, stream lengths, page tree)
- `--tree` — Show the object graph as an indented reference tree with IDs and types
- `--stats` — Show document statistics (object types, stream sizes, filter usage)
- `--bookmarks` — Show document bookmarks (outline tree)
- `--annotations` — Show annotations (all pages, or filtered with `--page`)
- `--layers` / `--ocg` — Show optional content groups (layers) with names, visibility, pages
- `--structure` — Show tagged PDF logical structure tree (supports `--depth`)
- `--info N` — Full object explanation: role classification, domain details, page associations, object content, forward/reverse references
- `--extract-stream <N> --output <path>` — Extract a stream object to a file

**Modifier flags** (combine with modes):
- `--json` — Structured JSON output (works with every mode)
- `--diff <file2.pdf>` — Compare two PDFs structurally (works with default, `--page`, `--json`)
- `--decode-streams` — Decompress and display stream contents (works with dump, --object, --page, --search). Supports FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, RunLengthDecode filter pipelines.
- `--truncate <N>` — Limit binary stream output to N bytes
- `--hex` — Display binary streams as hex dump (use with `--decode-streams`)
- `--depth N` — Limit traversal depth (0 = root only). Works with dump, page, tree, and JSON modes.
- `--dot` — Output tree as GraphViz DOT format (use with `--tree`)
- `--deref` — Inline-expand references to show target summaries (use with `--object` or `--page`)
- `--raw` — Show raw undecoded stream bytes (use with `--object`, conflicts with `--decode-streams`)

**Special combinations:**
- `--search <expr> --summary` — Search results as one-line table
- `--search "stream=text"` — Search inside decoded stream content
- `--text --page N` or `--text --page N-M` — Extract text from specific page(s) only
- `--operators --page N` — Show operators for specific page(s) only
- `--resources --page N` — Show resources for specific page(s) only
- `--annotations --page N` — Show annotations for specific page(s) only
- `--object 1,5,10-15` — Print multiple objects at once
- `--object N --deref` — Print object with references expanded inline
- `--page N --deref` — Page subtree with references expanded
- `--diff <file2.pdf> --page N` — Compare only page N
- `--diff <file2.pdf> --json` — JSON diff output
- `--decode-streams --hex` — Hex dump for binary stream content
- `--tree --depth N` — Tree view limited to N levels
- `--tree --json` — Tree as structured JSON
- `--tree --dot` — Tree as GraphViz DOT graph
- `--tree --dot --depth N` — DOT graph limited to N levels
- `--object N --raw` — Show raw undecoded stream bytes
- `--object N --raw --hex` — Raw bytes as hex dump
- `--object N --raw --truncate N` — Truncated raw bytes
- `--structure --depth N` — Structure tree limited to N levels
- `--structure --json` — Structure tree as JSON
- `--info N` — Full object explanation (role + content + refs)
- `--info N --json` — Object explanation as JSON

## Rust Edition

Uses Rust edition **2024** — requires a recent nightly or stable toolchain that supports it.
