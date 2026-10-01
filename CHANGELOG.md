# Changelog

All notable changes to `pdf-dump` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0/).

## [Unreleased]

## [0.29.0] - 2026-10-01
### Changed
- **Behavior change (exit codes): `--find-text` now exits 3, with the stderr
  reliability banner, when the searched text is degraded or unreliable,
  whether or not anything matched.**  “No matches” over undecodable text is not
  evidence that a word is absent; a search of a CID font without a ToUnicode map
  used to print “No matches”, exit 0, and show no banner, while `--text` on the
  same file exited 3.  `--find-text` now shares `--text`’s reliability verdict
  (`document_verdict(...).is_finding()`) and its banner.  Callers that treated
  exit 0 from `--find-text` as “searched reliably” must now handle exit 3.
  (bug-0011)
- **`--find-text --json` gains a `reliability` object**, the same shape as
  `--text --json`.
- `--help`, README, and `DEBUGGING_WITH_PDF_DUMP.md` document the shared
  signaling.

## [0.28.0] - 2026-10-01
### Changed
- **Behavior change (exit codes): an `--object` range now means “the objects
  present in the span”.**  Gaps inside a range no longer exit 1 and no longer
  print per-gap errors; only a range that holds no objects at all exits 1, with
  a single `No objects in range A-B.` line (and, under `--json`, one
  `{"range", "error"}` item).  A single explicit number that is missing still
  exits 1.  Callers that relied on a gap in a range failing must name the
  numbers explicitly.  (bug-0018)
- **Behavior change (JSON shape): any range, `5-5` included, now uses the
  `{"objects": [...]}` list shape.**  Only a single explicit number uses the
  single-object shape.
- `--object` ranges resolve against the document’s objects instead of being
  expanded during argument parsing, so `--object 1-4294967295` costs only the
  objects present rather than a ~17 GB vector.  `--page` ranges are walked only
  up to the first absent page, so their cost is bounded by the page count; the
  `Page N not found. Document has M pages.` error and exit 1 are unchanged.
- `--help` documents `--object` range semantics, and the exit-code table
  distinguishes an explicit missing number from an empty range.

## [0.27.0] - 2026-10-01
### Changed
- **Behavior change (exit codes): callers that check the exit status of
  `--object` or `--inspect` may need updating.**  A named object the document
  lacks now exits 1 under `--object` and `--inspect` (it exited 0 before),
  matching `--extract-stream` and `--page`.  In a list such as
  `--object 1,9999`, any miss exits 1, and the objects that were found still
  print.  Object 0 (the free-list head, never a real object) is a usage error,
  exit 2, for `--object` (including ranges starting at 0), `--inspect`, and
  `--extract-stream`.  (bug-0019)
- `--inspect`’s “object not found” error moves from stdout to stderr.
- The single-object `--object --json` error now carries `object_number` and
  `generation`, like the list and `--inspect` forms.
- The `--help` exit-code table names the new exit-1 and exit-2 cases.

## [0.26.1] - 2026-10-01
### Fixed
- bug-0010: `--find-text` folds case per character and maps folded offsets back
  to the source text, so `İ` and `ẞ` no longer panic and snippets no longer drift
  past the match.
- bug-0024: page-label roman numerals render 1..=3999 and fall back to decimal
  above, so a hostile `/St` cannot drive the subtraction loop.
- bug-0005: LZW decodes in 4 KiB chunks and RunLength checks before each run,
  both failing as soon as output would pass `MAX_DECODED_SIZE`.

## [0.26.0] - 2026-09-23
### Added
- `--text --layout`: position-faithful text extraction on a character grid,
  so table columns survive (plan-0002, first landing).  A real text-state
  machine (CTM, `cm`, `q`/`Q`, text matrix, `Tc`/`Tw`/`Tz`/`TL`/`Ts`, `TJ`
  adjustments, form `/Matrix`) positions every glyph from its font’s widths
  (`/Widths`, CID `/W`/`/DW`, Type3 `/FontMatrix`), in visual space (`/Rotate`
  and CropBox honored).  Glyphs with no real gap form a run that the grid never
  splits, even when another string (a padding space, leader dots) is drawn
  over it.  Rotated and vertical text follows the grid under a
  `[non-horizontal text]` line, text outside the CropBox under
  `[off-page text]`, and overprinted fake bold collapses.  A glyph whose width
  is unknown makes its font degraded (exit 3); no width is ever invented.
  `--layout-cell <pt>` (at least 1) sets the grid, and `--json` adds per-page
  `rotate`, `crop_box`, `cell`, `non_horizontal_glyphs` and `off_page_glyphs`.
  Plain `--text` output is unchanged.

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
