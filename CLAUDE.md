# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

- **Build:** `cargo build`
- **Run:** `cargo run -- <file.pdf> [options]`
- **Test:** `cargo test`
- **Check (fast lint):** `cargo check`
- **Clippy:** `cargo clippy`

## What This Is

`pdf-dump` is a Rust CLI tool that dumps the internal object structure of a PDF file.  It uses `lopdf` for PDF parsing, `clap` (derive) for CLI arguments, `flate2` for zlib/FlateDecode stream decompression, and `weezl` for LZW decoding.

For CLI usage documentation, see the global `pdf-tools.md` rules or `DEBUGGING_WITH_PDF_DUMP.md` in this repo.

## Architecture

The tool is split across ~30 source files in `src/`.  The flow is:

1. **CLI parsing** (`types.rs`) — `Args` struct via clap derive.  Modes are divided into **document-level** (combinable) and **standalone** (mutually exclusive). `--page` is always a modifier.  Help output uses `help_heading` for organized grouping.
2. **Mode resolution** (`types.rs`) — `Args::resolve_mode()` validates exclusivity and returns `ResolvedMode` enum (Default, Combined, Standalone).
3. **Dispatch** (`lib.rs`) — `run()` matches on `ResolvedMode`:
   - `Default` → page info if `--page` present, else overview
   - `Standalone(mode)` → extract-stream, object, inspect, or search
   - `Combined(modes)` → single mode calls directly; multiple modes get section headers (text) or are wrapped in a JSON object (json).  Uses `*_json_value()` functions for JSON output.

### Mode → Module mapping

| Mode | Module | Key functions |
|------|--------|--------------|
| Overview (default) | `summary.rs` | `print_overview`, `overview_json_value` |
| `--object` | `object.rs` (~2700 lines) | `print_object`, `object_to_json` |
| `--list` | `object.rs` | `print_list`, `list_json_value` |
| `--page` | `page_info.rs` | `print_page_info`, `page_info_json_value` |
| `--inspect` | `inspect.rs` | `print_inspect`, `inspect_json_value` |
| `--search` | `search.rs` | `print_search`, `search_json_value` |
| `--text` | `text.rs` | `print_text`, `text_json_value` (font-aware: decodes via `/ToUnicode` + WinAnsi/MacRoman) |
| `--operators` | `operators.rs` | `print_operators`, `operators_json_value` |
| `--find-text` | `find_text.rs` | `print_find_text`, `find_text_json_value` |
| `--fonts` | `fonts.rs` | `print_fonts`, `fonts_json_value` |
| `--images` | `images.rs` | `print_images`, `images_json_value` |
| `--forms` | `forms.rs` | `print_forms`, `forms_json_value` |
| `--validate` | `validate.rs` | `print_validate`, `validation_json_value` |
| `--bookmarks` | `bookmarks.rs` | `print_bookmarks`, `bookmarks_json_value` |
| `--annotations` | `annotations.rs` | `print_annotations`, `annotations_json_value` |
| `--tree` | `tree.rs` | `print_tree`, `tree_json_value` |
| `--tags` | `structure.rs` | `print_structure`, `structure_json_value` |
| `--detail security` | `security.rs` | `print_security`, `security_json_value` |
| `--detail embedded` | `embedded.rs` | `print_embedded`, `embedded_json_value` |
| `--detail labels` | `page_labels.rs` | `print_page_labels`, `page_labels_json_value` |
| `--detail layers` | `layers.rs` | `print_layers`, `layers_json_value` |
| `--extract-stream` | `lib.rs` (inline) | uses `stream::decode_stream` |

### Foundation modules

| Module | Role |
|--------|------|
| `types.rs` | `Args`, `Config`, `ResolvedMode`, `PageSpec`, `DetailSub` |
| `stream.rs` | `decode_stream` — filter pipeline: FlateDecode, ASCII85, ASCIIHex, LZW, RunLength |
| `helpers.rs` | Shared formatting utilities |
| `refs.rs` | Reference traversal, reverse ref lookup |
| `resources.rs` | Page resource extraction (fonts, XObjects, ExtGState, ColorSpaces) |
| `cmap.rs` | `ToUnicodeCMap::parse` — best-effort ToUnicode CMap parser (bfchar/bfrange, codespace `lo`/`hi` bounds) for `--text`; `byte_width`/`next_code` split show-string bytes into codes, honoring variable-width codespaces (mixed 1-byte/2-byte CJK) |
| `encodings.rs` | `winansi(b)` / `macroman(b)` / `standard(b)` / `macexpert(b)` — WinAnsiEncoding (CP1252), Mac OS Roman, Adobe StandardEncoding, and MacExpertEncoding → Unicode tables for simple fonts lacking ToUnicode.  Return `Option<&'static str>`, so one code can expand to several chars (f-ligatures decompose to ASCII, `rupiah`→`Rp`) |
| `glyphlist.rs` | `glyph_name_to_string(name)` — Adobe Glyph List resolver for `/Encoding /Differences` glyph names.  Embeds Adobe’s `glyphlist.txt` (BSD-licensed) via `include_str!` into a lazy `OnceLock` map, plus algorithmic `uniXXXX` (UTF-16 units, surrogate pairs) / `uXXXXXX` forms, `.suffix` stripping, and underscore-joined ligature components.  Returns owned `String` (AGL entries can be multi-codepoint) |

### Key patterns

- `pub fn run()` in `lib.rs` holds all dispatch (binary crate can’t access `pub(crate)` items from lib)
- Shared test helpers in `lib.rs::test_utils`: `output_of`, `empty_doc`, `default_config`, `make_stream`, `zlib_compress`, `json_config`, `build_two_page_doc`, `build_page_doc_with_content`, `make_page_with_annots`
- `validate::collect_reachable_ids` is `pub(crate)` so `object.rs` tests can use it
- `print_*_json()` functions are `#[cfg(test)]` only — dispatch uses `*_json_value()` directly
- Overview encryption detection checks both trailer `/Encrypt` key and XRef stream objects (fallback for post-decryption state where lopdf strips the trailer key)
- `--text` is font-aware (Tier 1): `text.rs` builds a per-page `FontDecoder` table, tracks the active font via `Tf`, and decodes show-strings through `/ToUnicode` (`cmap.rs`) or a base-encoding table for any of the four named single-byte encodings — WinAnsi/MacRoman/Standard/MacExpert (`encodings.rs`, dispatched via `simple_table_for`; tables return `&'static str`, so f-ligatures decompose to ASCII like `ﬁ`→`fi` for searchable output); undecodable fonts fall back to byte passthrough (no regression).  `/Encoding /Differences` glyph names are resolved to Unicode through the Adobe Glyph List (`glyphlist.rs`): `build_differences_overrides` walks the array into a per-font `HashMap<u8, String>` consulted before the base table, with `glyph_name_to_string` handling the embedded AGL table plus the algorithmic `uniXXXX`/`uXXXXXX` forms, `.suffix` stripping, and underscore ligatures.  It classifies each font Reliable/Degraded/Unreliable (a `/Differences` font is Reliable when every name resolves, Degraded when some do not), prints a loud stderr banner + JSON `reliability` object, and exits 3 when a document is `Unreliable` (CID/Type0 without ToUnicode).  The operator walk is recursive: a `Do` on a form XObject (`/Subtype /Form`) recurses into that form’s content stream via `process_content`, building a decoder table from the form’s own `/Resources` (or inheriting the caller’s, per PDF 32000-1 §7.8.3) and folding its fonts into the same reliability counters — so text drawn inside forms is extracted, not silently dropped.  A visited-set cycle guard (the active recursion stack) plus a `MAX_FORM_DEPTH` cap keep self-referential and deeply-nested forms terminating.  Remaining Tier 2 follow-on: predefined CJK CMaps (`docs/ROADMAP.md`).

## Rust Edition

Uses Rust edition **2024** — requires a recent nightly or stable toolchain that supports it.

- `use` items in modules are private by default — test modules need explicit imports
- Pattern matching: `&count` not allowed in implicitly-borrowing patterns — use `**count` instead
