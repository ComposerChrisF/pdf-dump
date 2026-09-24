# Changelog

All notable changes to `pdf-dump` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0/).

## [Unreleased]

## [0.25.0] - 2026-09-23
### Changed
- **`--text` exits 3 on a Degraded verdict**, not only on Unreliable.  Degraded
  means the tool ran correctly and the input had problems, which is findings by
  the exit-code table; a stderr banner with exit 0 was invisible to any caller
  that branches on the code.  The text is still printed and `--json` still emits.
  Callers audited first: no script or hook branches on `--text`’s exit code.
### Fixed
- Two clippy lints new in Rust 1.98 (`chunks_exact` with a constant size,
  a useless `format!`) that had turned CI red since 2026-09-05.

## [0.24.1] - 2026-09-23
### Fixed
- A form field whose `/Kids` cycles back to itself or an ancestor no longer
  overflows the stack; the default overview, `--json` and `--forms` all survive
  it (bug-0013).
- A page whose `/Contents` is an array no longer fuses the last token of one
  stream with the first of the next (`ET` + `BT` → `ETBT`); `--text`,
  `--operators` and `--find-text` see the boundary (bug-0003).
- `--text` no longer certifies a document reliable when two distinct fonts share
  a resource name, BaseFont and Subtype and only one has a `/ToUnicode`; the worst
  classification survives deduplication (bug-0012).
- The `--text` coverage net counts undecodable bytes, not emitted replacement
  characters, so a multi-byte invalid run is no longer under-counted; output is
  unchanged (bug-0036).
- Text inside a form XObject without its own `Tf` now decodes through the caller’s
  active font, and `q`/`Q` save and restore the text font (bug-0014).
- `--page` reports `MediaBox`, `CropBox` and `Rotate` inherited from an ancestor
  `/Pages` node instead of `-` (bug-0022).

## [0.24.0] - 2026-07-14
### Changed
- Migrate to the canonical portfolio exit-code table and add a “caution” tier.

## [0.23.1] - 2026-06-29
### Changed
- Bump `lopdf` 0.39 → 0.42 (toolchain-wide); migrate the encryption canary off
  the deprecated `load_mem_with_password`.

## [0.23.0] - 2026-06-29
### Added
- `--password` for opening encrypted PDFs.
### Fixed
- Encrypted-PDF overview misparse: a locked file’s collapsed object/page/stream
  counts are no longer presented as authoritative.

## [0.22.0] - 2026-06-29
### Added
- Observable recovery for malformed `/Length` streams (loud stderr banner and a
  `recovery` object in `--json`), plus a `--strict` gate that refuses the repair
  and exits 3.

## [0.21.0] - 2026-06-28
### Fixed
- Lenient stream recovery for PDFs with a wrong `/Length` whose content a strict
  reader would silently drop.

## [0.20.1] - 2026-06-28
### Changed
- Bump `lopdf` 0.36 → 0.39 (matches pdf-maker; fixes AES-256 interop).

## [0.20.0] - 2026-06-28
### Fixed
- `--text` now recurses into Form XObjects, fixing silent under-extraction.

## [0.13.0–0.19.0] - 2026-06-24–2026-06-28
### Added
- A near-complete `--text` extraction overhaul: font-aware decoding with
  reliability detection (0.13.0), then incremental encoding coverage —
  MacRomanEncoding for macOS-exported PDFs (0.14.0), StandardEncoding → Unicode
  (0.15.0), MacExpertEncoding and multi-character ligatures (0.16.0), Adobe
  Glyph List resolution for `/Encoding /Differences` (0.17.0), and variable-width
  ToUnicode codespaces (0.19.0).
### Changed
- The `--text` reliability verdict is usage-aware — all decode paths feed the
  coverage assessment (0.18.0).

## [0.12.7] - 2026-04-29
### Fixed
- Code-review fixes: exit code 3 usage, `--page` open-range handling, and CLI
  discoverability; clippy 1.95.0 lints (0.12.8).

## [0.12.6] - 2026-03-13
### Changed
- Documentation consolidation, README sections, and `cargo fmt` in preparation
  for open-source publication.

Earlier history (the v0.12.0 combinable-mode CLI redesign, the 28-module split,
and the extensive edge-case test and security-hardening work of 0.10.x–0.12.x)
is in the git log.

[Unreleased]: https://github.com/ComposerChrisF/pdf-dump/compare/pdf-dump-v0.24.0...HEAD
