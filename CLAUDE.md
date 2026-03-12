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

The tool is split across ~28 source files in `src/`. The flow is:

1. **CLI parsing** — `Args` struct via clap derive. Default mode is overview (metadata + validation + stream stats + features). Modes are divided into **document-level modes** (combinable: `--list`, `--validate`, `--fonts`, `--images`, `--forms`, `--bookmarks`, `--annotations`, `--text`, `--operators`, `--tags`, `--tree`, `--find-text`, `--detail`) and **standalone modes** (mutually exclusive: `--object`, `--inspect`, `--search`, `--extract-stream`). `--page` is always a modifier. Help output uses `help_heading` for organized grouping.
2. **Mode resolution** — `Args::resolve_mode()` collects standalone and document-level modes from CLI flags, validates exclusivity (standalone modes can't combine with each other or with document-level modes), and returns a `ResolvedMode` enum (Default, Combined, Standalone).
3. **Dispatch** — `run()` matches on `ResolvedMode`:
   - `Default` → page info if `--page` present, else overview
   - `Standalone(mode)` → extract-stream, object, inspect, or search
   - `Combined(modes)` → single mode calls directly; multiple modes get section headers (text) or are wrapped in a JSON object (json). Uses `*_json_value()` functions for JSON output.
4. **Overview mode** (default, no flags) — Shows PDF version, page/object counts, encryption status, all /Info fields (Producer, Creator, Title, Author, Subject, Keywords, CreationDate, ModDate), catalog properties (PageLayout, PageMode, Lang), validation summary, stream stats (raw bytes, filter histogram, largest streams), object type breakdown, and feature indicators (bookmarks, forms, layers, embedded files, page labels, tagged structure). Decoded byte counts and compression ratio are only shown when `--decode` is passed (to avoid decompressing every stream in large PDFs). Encryption detection checks both the trailer `/Encrypt` key and XRef stream objects (fallback for post-decryption state where lopdf strips the trailer key).
5. **Extract mode** (`--extract-stream`) — Pulls a single stream object by ID number (generation 0 assumed), decodes it, and writes raw bytes to a file.
6. **Object mode** (`--object N` or `--object 1,5,10-15`) — Prints one or more objects without following references. Accepts single numbers, comma-separated lists, ranges, or mixed. Shows type label in header. `--deref` expands references inline.
7. **List mode** (`--list`) — One-line-per-object table showing kind, /Type, and details.
8. **Page mode** (`--page N` or `--page N-M`) — Shows structured page information: MediaBox, CropBox, Rotate, detailed resources (fonts with object IDs, XObjects with image/form counts, ExtGState entries, ColorSpace details), annotation count with subtype breakdown, content stream count/bytes, and text preview (with garbled text detection). JSON output includes full text and `text_extractable` flag. Accepts single pages (e.g. `5`) or inclusive ranges (e.g. `1-3`).
9. **Search mode** (`--search <expr>`) — Find objects matching key/value/stream criteria (Type=Font, key=MediaBox, value=Hello, stream=text). Conditions ANDed. `--list` modifier shows one-line table.
10. **Text mode** (`--text`) — Extract readable text from page content streams (Tj, TJ, ', " operators). `--page N` or `--page N-M` filters to specific pages. Emits warnings on stderr when fonts lack known encodings (CID fonts without ToUnicode, custom fonts without explicit encoding). JSON output includes `"warnings"` array per page.
11. **Operators mode** (`--operators`) — Shows all content stream operators for each page. `--page N` filters to specific pages. Emits warnings for decode failures. JSON output includes `"warnings"` array per page when issues occur.
12. **Forms mode** (`--forms`) — Lists AcroForm fields with qualified names, field types (Tx/Btn/Ch/Sig), values, flags, and page numbers. Walks hierarchical field trees.
13. **Inspect mode** (`--inspect N`) — The definitive "tell me everything about this object" command. Shows role classification, domain-specific details, page associations, full object content dump, forward references with summaries, and reverse references with key paths. Uses `classify_object()` for role detection and `find_page_associations()` for page context.
14. **Fonts mode** (`--fonts`) — Lists all fonts with BaseFont, Subtype, Encoding, embedded status, and encoding diagnostics (ToUnicode, FirstChar/LastChar/Widths, Differences, CIDSystemInfo).
15. **Images mode** (`--images`) — Lists all images with dimensions, color space, BPC, filter, and stream size.
16. **Validate mode** (`--validate`) — Structural validation: broken refs, unreachable objects, required keys, stream lengths, page tree. XRef stream objects (which lopdf leaves in `doc.objects` with stale references after decrypting encrypted PDFs) are excluded from broken-ref and unreachable checks via `collect_xref_stream_ids()`.
17. **Tree mode** (`--tree`) — Shows the object graph as an indented reference tree with IDs, types, and key paths. Marks revisited nodes. Respects `--depth N`.
18. **Bookmarks mode** (`--bookmarks`) — Shows the document outline (bookmark) tree with titles, destinations, and actions.
19. **Annotations mode** (`--annotations`) — Lists all annotations with page number, subtype, rect, contents, and for Link annotations: link type (URI/GoTo/GoToR/Named/Launch) and target. Works with `--page` filter.
20. **Detail modes** (`--detail security|embedded|labels|layers`) — Consolidated detail views for security (encryption/permissions), embedded files, page labels, and layers (OCGs). Multiple `--detail` values can be specified.
21. **Tags mode** (`--tags`) — Shows tagged PDF logical structure tree from `/StructTreeRoot`. Displays element roles, page refs, MCIDs, titles, alt text. Supports `--depth N` to limit tree depth. Cycle detection via `BTreeSet<ObjectId>`.
22. **Find Text mode** (`--find-text "pattern"`) — Case-insensitive text search across pages. Shows matching snippets with context. Works with `--page` filter.
23. **JSON modifier** (`--json`) — Structured JSON output for all modes. Uses `serde_json`. Each PDF object maps to a JSON type schema. With `--deref`, references gain a `"resolved"` field. When combining multiple modes, JSON output wraps each mode's value in a single object keyed by mode name.
24. **`print_object`** — Recursive pretty-printer that handles all `lopdf::Object` variants. Collects `(is_contents, ObjectId)` pairs into `child_refs` for the caller to traverse. When a dictionary key is `/Contents`, the `is_contents` flag propagates so content streams get parsed via `lopdf::content::Content::decode`. With `config.deref`, references show inline summaries.
25. **`decode_stream`** — Filter pipeline processor. Supports FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, and RunLengthDecode. Applies filters sequentially. Returns `(Cow<[u8]>, Option<String>)` — decoded data and optional warning on failure or unsupported filter.
26. **`object_to_json`** — Maps each `lopdf::Object` variant to a `serde_json::Value` with a `type` field + value fields.

## Key Flags

**Document-level modes** (combinable — use multiple at once, output gets section headers):
- `--list` (`-s`) — One-line listing of every object
- `--validate` — Run structural validation checks (broken refs, unreachable objects, required keys, stream lengths, page tree)
- `--fonts` — List all fonts with BaseFont, Subtype, Encoding, and embedded status
- `--images` — List all images with dimensions, color space, BPC, filter, size
- `--forms` — List form fields (AcroForm) with names, types, values, and page numbers
- `--bookmarks` — Show document bookmarks (outline tree)
- `--annotations` — Show annotations with link targets (all pages, or filtered with `--page`)
- `--text` — Extract readable text from content streams (all pages, or `--text --page N`)
- `--operators` — Show content stream operators (all pages, or `--operators --page N`)
- `--tags` — Show tagged PDF structure tree (accessibility tags, supports `--depth`)
- `--tree` — Show the object graph as an indented reference tree with IDs and types
- `--find-text "pattern"` — Case-insensitive text search with context snippets
- `--detail security|embedded|labels|layers` — Detail views (can specify multiple)

**Standalone modes** (mutually exclusive — only one at a time, can't combine with document-level modes):
- `--object N` or `--object 1,5,10-15` (`-o`) — Print one or more objects by number (generation 0), no traversal
- `--inspect N` — Full object explanation: role classification, domain details, page associations, object content, forward/reverse references
- `--search <expr>` — Find objects matching expression (e.g. `Type=Font`, `key=MediaBox`, `value=Hello`, `stream=text`)
- `--extract-stream <N> --output <path>` — Extract a stream object to a file

**Modifier flags** (combine with modes):
- `--json` — Structured JSON output (works with every mode)
- `--page N` or `--page N-M` — Filter to specific pages (with `--text`, `--annotations`, `--operators`, `--find-text`); shows page info when used alone
- `--decode` — Decompress and display stream contents (works with `--object`, `--search`). In overview mode, enables decoded byte counts and compression ratio in stream stats. Supports FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, RunLengthDecode filter pipelines.
- `--truncate <N>` — Limit binary stream output to N bytes
- `--hex` — Display binary streams as hex dump (use with `--decode`)
- `--depth N` — Limit traversal depth (0 = root only). Works with tree, tags, and JSON modes.
- `--dot` — Output tree as GraphViz DOT format (use with `--tree`)
- `--deref` — Inline-expand references to show target summaries (use with `--object`)
- `--raw` — Show raw undecoded stream bytes (use with `--object`, conflicts with `--decode`)

**Special combinations:**
- `--fonts --images` — Combined modes with section headers
- `--fonts --images --json` — Combined modes wrapped in JSON object
- `--search <expr> --list` — Search results as one-line table
- `--search "stream=text"` — Search inside decoded stream content
- `--text --page N` or `--text --page N-M` — Extract text from specific page(s) only
- `--operators --page N` — Show operators for specific page(s) only
- `--annotations --page N` — Show annotations for specific page(s) only
- `--find-text "word" --page N` — Search for text on specific page(s) only
- `--object 1,5,10-15` — Print multiple objects at once
- `--object N --deref` — Print object with references expanded inline
- `--decode --hex` — Hex dump for binary stream content
- `--tree --depth N` — Tree view limited to N levels
- `--tree --json` — Tree as structured JSON
- `--tree --dot` — Tree as GraphViz DOT graph
- `--tree --dot --depth N` — DOT graph limited to N levels
- `--object N --raw` — Show raw undecoded stream bytes
- `--object N --raw --hex` — Raw bytes as hex dump
- `--object N --raw --truncate N` — Truncated raw bytes
- `--tags --depth N` — Structure tree limited to N levels
- `--tags --json` — Structure tree as JSON
- `--inspect N --json` — Object explanation as JSON
- `--detail security --detail embedded` — Multiple detail views

## Rust Edition

Uses Rust edition **2024** — requires a recent nightly or stable toolchain that supports it.
