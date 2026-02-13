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

The entire tool lives in `src/main.rs` (~260 lines). The flow is:

1. **CLI parsing** — `Args` struct via clap derive. Two modes: dump (default) or extract (`--extract-object` + `--output`).
2. **Dump mode** — Prints the trailer, then traverses the object tree starting from the `/Root` reference. `dump_object_and_children` does a depth-first walk using a `BTreeSet<ObjectId>` to avoid revisiting objects. Each object's references are collected during printing and then recursively followed.
3. **Extract mode** — Pulls a single stream object by ID number (generation 0 assumed), decodes it, and writes raw bytes to a file.
4. **`print_object`** — Recursive pretty-printer that handles all `lopdf::Object` variants. Collects `(is_contents, ObjectId)` pairs into `child_refs` for the caller to traverse. When a dictionary key is `/Contents`, the `is_contents` flag propagates so content streams get parsed via `lopdf::content::Content::decode`.
5. **`decode_stream`** — Checks `/Filter` for `FlateDecode` and decompresses with `flate2::ZlibDecoder`. Returns `Cow<[u8]>` (borrowed if no decompression needed).

## Key Flags

- `--decode-streams` — Decompress and display stream contents
- `--truncate-binary-streams` — Limit binary stream output to 100 bytes
- `--extract-object <N> --output <path>` — Extract a stream object to a file

## Rust Edition

Uses Rust edition **2024** — requires a recent nightly or stable toolchain that supports it.
