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

For CLI usage documentation, see the global `pdf-tools.md` rules or `DEBUGGING_WITH_PDF_DUMP.md` in this repo.

## Architecture

The tool is split across ~28 source files in `src/`. The flow is:

1. **CLI parsing** (`types.rs`) — `Args` struct via clap derive. Modes are divided into **document-level** (combinable) and **standalone** (mutually exclusive). `--page` is always a modifier. Help output uses `help_heading` for organized grouping.
2. **Mode resolution** (`types.rs`) — `Args::resolve_mode()` validates exclusivity and returns `ResolvedMode` enum (Default, Combined, Standalone).
3. **Dispatch** (`lib.rs`) — `run()` matches on `ResolvedMode`:
   - `Default` → page info if `--page` present, else overview
   - `Standalone(mode)` → extract-stream, object, inspect, or search
   - `Combined(modes)` → single mode calls directly; multiple modes get section headers (text) or are wrapped in a JSON object (json). Uses `*_json_value()` functions for JSON output.

### Mode → Module mapping

| Mode | Module | Key functions |
|------|--------|--------------|
| Overview (default) | `summary.rs` | `print_overview`, `overview_json_value` |
| `--object` | `object.rs` (~2700 lines) | `print_object`, `object_to_json` |
| `--list` | `object.rs` | `print_list`, `list_json_value` |
| `--page` | `page_info.rs` | `print_page_info`, `page_info_json_value` |
| `--inspect` | `inspect.rs` | `print_inspect`, `inspect_json_value` |
| `--search` | `search.rs` | `print_search`, `search_json_value` |
| `--text` | `text.rs` | `print_text`, `text_json_value` |
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

### Key patterns

- `pub fn run()` in `lib.rs` holds all dispatch (binary crate can't access `pub(crate)` items from lib)
- Shared test helpers in `lib.rs::test_utils`: `output_of`, `empty_doc`, `default_config`, `make_stream`, `zlib_compress`, `json_config`, `build_two_page_doc`, `build_page_doc_with_content`, `make_page_with_annots`
- `validate::collect_reachable_ids` is `pub(crate)` so `object.rs` tests can use it
- `print_*_json()` functions are `#[cfg(test)]` only — dispatch uses `*_json_value()` directly
- Overview encryption detection checks both trailer `/Encrypt` key and XRef stream objects (fallback for post-decryption state where lopdf strips the trailer key)

## Rust Edition

Uses Rust edition **2024** — requires a recent nightly or stable toolchain that supports it.

- `use` items in modules are private by default — test modules need explicit imports
- Pattern matching: `&count` not allowed in implicitly-borrowing patterns — use `**count` instead
