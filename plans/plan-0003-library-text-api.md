# Plan: A Public Library API for Text Extraction

## Problem

`id-redact` will eventually extract PDF text in-process rather than shelling out to `pdf-dump` (`~/Chris/App/Rust/id-redact/SPEC.md` § Formats, the “PDF, extracted in-process” row).  pdf-dump cannot serve that today:

- **The library’s only public item is `pub fn run()`**, which parses `std::env::args`, prints, and exits.  Everything else — `text::text_json_value`, `layout::layout_page`, the `Reliability` verdict — is `pub(crate)`.
- **The library path exits the process.**  The `w!`/`wln!` macros (`src/lib.rs`) call `std::process::exit` on any write error, and `run()` exits with the table’s codes.  A library that terminates its caller is not a library.
- **The library path prints.**  `text::print_reliability_banner` writes to stderr from inside `text_json_value` and `layout_json_value`, and content-stream warnings go out through `eprintln!`.

Inside id-redact there is no separate step at which a human can stop on pdf-dump’s exit 3, so **the typed verdict is the part this API must get right**.  id-redact proceeds only on `Reliable` and treats `Degraded` or `Unreliable` as Unknown (its own exit 1).  A verdict that is only printed, or only encoded in an exit code, is useless to it.

Priority: low.  Chris, 2026-09-23: “yes, but PDF support is lower priority, so a much later phase” — the PDF path is id-redact’s last phase, after text, CSV, OFX and email.

## Proposed Change

A small public module, say `pdf_dump::extract`, that does no I/O beyond reading the input and never exits:

```rust
pub enum TextMode { Plain, Layout { cell: Option<f64> } }

pub struct ExtractOptions { pub mode: TextMode, pub pages: Option<PageRange>, pub password: Option<String>, pub strict: bool }

pub struct PageText { pub page_number: u32, pub text: String, pub warnings: Vec<String>, /* Layout only: */ pub rotate: Option<i64>, pub crop_box: Option<[f64; 4]> }

pub enum Verdict { Reliable, Degraded, Unreliable }

pub struct Extraction {
    pub pages: Vec<PageText>,
    pub verdict: Verdict,
    pub total_codes: u64,
    pub unmapped_codes: u64,
    pub fonts: Vec<FontVerdict>,        // name, base_font, subtype, classification, reason
    pub recovery: Option<Recovery>,     // the malformed-/Length repairs, as in --json
    pub encrypted_undecrypted: bool,
}

pub fn extract_text(path: &Path, opts: &ExtractOptions) -> Result<Extraction, ExtractError>;
pub fn extract_text_from_bytes(bytes: &[u8], opts: &ExtractOptions) -> Result<Extraction, ExtractError>;
```

- **`Err` is reserved for “could not read the input”** — the conditions that are exit 1 or 2 on the CLI (unreadable file, corrupt PDF, wrong password, a page range beyond the end).  Findings are data in `Ok`, never `Err`, mirroring the exit-code table’s findings-are-not-errors rule.
- **Every signal that makes the CLI exit 3 is a field**, not a side effect: the verdict, `encrypted_undecrypted`, and a `--strict` detection (`recovery` with `repaired: false`).  A caller must be able to reproduce the CLI’s exit code from the struct alone.
- **The CLI becomes a thin consumer** of this API for `--text` and `--text --layout`: it formats the `Extraction`, prints the banner from `Extraction.fonts`, and maps the fields to an exit code.  One extraction path, so the library and the CLI cannot drift apart.

## Implementation Notes

- **Split computation from presentation** in `text.rs` and `layout.rs`.  `extract_text_from_page_with_warnings` and `layout_page` already return data; the page loops in `print_text`, `text_json_value`, `print_layout` and `layout_json_value` are what must stop printing.  Move the banner to the CLI side, fed by the returned font records.
- **Keep `w!`/`wln!` CLI-only.**  They are right for a CLI (a closed pipe is exit 0, not a panic) and wrong for a library.  Nothing reachable from `extract_text` may use them.
- **Move the load pipeline out of `run()`** — password handling, the encrypted-but-undecrypted detection, lenient `/Length` recovery or `--strict` detection — into a function both paths call.  Today it is inline in `run()`, which is why the library has nothing to call.
- **Semver.**  The crate is not published (`Cargo.toml` excludes `bugs/` and `plans/` for a future publish), but a public API is a contract.  Mark the module as unstable in its doc comment until id-redact has used it through one release, and version it with the crate.
- **Tests.**  An integration test in `tests/` that uses only the public API.  It must assert the verdict and counts for a reliable, a degraded and an unreliable fixture, and prove that nothing is written to stdout or stderr (capture both).  A second test extracts the same fixtures through the CLI and the API and asserts equal text and an equal verdict-to-exit-code mapping.  That is the drift guard.
- **Out of scope:** the other modes (`--fonts`, `--validate`, …).  If they are ever wanted as library calls, the same split applies, but nothing needs them now.

## Why Not a Workaround

id-redact could keep shelling out to the installed binary and parse `--json`.  That is what its workflow does today, and it works.  But in-process extraction is the phase this plan serves, and the subprocess route has two costs there.  First, it makes id-redact depend on whichever binary is on `PATH` at run time rather than a pinned version.  Second, its verdict arrives as an exit code plus a JSON field that id-redact would have to cross-check, where a typed `Verdict` cannot be misread.  Copying pdf-dump’s decoder modules into id-redact would be worse: two copies of the CMap, encoding, AGL and reliability code would drift, and the reliability verdict is exactly where drift is dangerous.
