# Changelog

All notable changes to `pdf-dump` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0/).

## [0.37.0] - 2026-11-09
### Added
- **`--text` and `--text --layout` name each page that draws but shows no text.**  A new module,
  `paint.rs`, reads from a page's operators alone, through form XObjects, whether the page shows
  text, fills or strokes paths, or draws images, and whether its content could not be read to
  its end.  A page that draws paths or images but shows no text (outlined words, a scan) is named
  in a stderr note and, under `--json`, in `pages_without_text: [{page_number, paths, images,
  unread}]`, which is always present.  The verdict and the exit code are unchanged: one caller
  withholds a PDF on exit 3 but sends it to OCR on exit 0.
- **`--require-text` (requires `--text`).**  The caller asserts that every selected page carries
  text.  A page that draws but shows no text exits 1 and is named, and so does a locked or
  pageless document.  Nothing is written: no stdout, and no `--output` file is created or
  replaced.  A blank page passes.  The exit-1 entry in `--help` names it, and the `--help`
  preamble about OCR mentions both.  The request came from id-redact, a statement pipeline.

## [0.36.0] - 2026-11-09
### Added
- **Rotated text is laid out by direction under `--text --layout`.**  Glyphs whose baseline is
  not horizontal used to print under `[non-horizontal text]` in content order, one line per
  show-string, so a rotated block drawn one glyph per `BT`/`Tm`/`Tj`/`ET` printed one glyph per
  line, and a multi-line rotated block lost its lines.  Each baseline direction (in whole
  degrees) now becomes its own frame and is laid out as a small grid by the same chunk and line
  rules as the page: words and lines are kept and columns are aligned.  Vertical writing and
  mirrored (left-handed) text stay in content order.  Plain `--text` is unchanged.  The request
  came from id-redact, a statement pipeline.

## [0.35.0] - 2026-11-08
### Added
- **A Standard-14 font that omits `/Widths` takes Adobe’s Core 14 AFM widths under
  `--text --layout`** (Courier, Helvetica and Times in four styles each, Symbol, ZapfDingbats).
  A glyph is reached by name through the font’s encoding: a named WinAnsi, MacRoman or Standard
  base, or the font’s builtin encoding, with `/Differences` applied on top.  A subset prefix is
  stripped from the `/BaseFont`; no aliases are recognised.  A glyph missing from the AFM, or an
  encoding with no glyph-name table (MacExpertEncoding, for one), stays unknown and degrades the
  verdict (exit 3) as before.
- Such fonts were always degraded and placed with a 0.5 em guess, so text drawn glyph by glyph
  in them gained a space after every wide glyph (capitals, `m`, `w`).  They now lay out with true
  widths.  Plain `--text` is unchanged.  The request came from id-redact, on a real case.
- `src/core14.txt` keeps Adobe’s copyright notices and the AFM notice; the README License
  section says so.

## [0.34.0] - 2026-11-06
### Added
- **`--text [--layout] [--page …] --output <file>` writes the text to a file,** the same bytes
  stdout would carry, and none of it to stdout.  It is written to a temporary file beside the
  output, synced, then renamed into place, so a failed run leaves no partial file and an
  existing file is replaced whole.  A run that exits 1 writes nothing; one that exits 3
  (degraded or unreliable text) still writes the file.  The output directory must exist, and is
  checked before the document is read.  `--output` with `--json`, or with another mode beside
  `--text`, is a usage error (exit 2).

## [0.33.0] - 2026-11-04
### Added
- **`--annotations` reports `author` (`/T`), `subject` (`/Subj`), `flags` and `hidden`,** the
  last true for the Hidden or NoView bit, and `has_down_appearance` and
  `has_rollover_appearance`.  A widget’s `/T` is its field’s name, so it is not an author.
- **`--forms` reports `tooltip` (`/TU`), `options` (`/Opt`, as `{export, label, decoded}`) and
  `captions` (`/MK /CA` of each widget, as `{text, decoded}`),** with `*_decoded` and, for the
  first two, `*_inherited` beside them.
- One that will not decode exits 3, as does an annotation’s `/F` that is not an integer.  A file
  that used to exit 0 can therefore exit 3.

## [0.32.0] - 2026-11-04
### Changed
- **`--extract-image` exits 3 when it leaves a mask unapplied.**  A JPEG or JPEG 2000 with a
  mask is still written as it is, with the warning and `mask_applied: false`, but it exited 0,
  so a script ignoring the JSON could read a scan’s background and miss words kept in the mask.
  Masked JPEGs are common, so a caller should read `mask_applied` before treating 3 as a problem.

## [0.31.8] - 2026-11-04
### Fixed
- **An indirect `/Filter` is followed.**  It read as no filter, so `--extract-stream` wrote
  still-compressed bytes with exit 0, `--object --decode` labelled them decoded, `--forms`
  reported an encoded value as `value_decoded: true`, and an XFA packet could read as holding
  no data.  A `/Filter` that cannot be used, such as an array entry that is not a name, is
  now a warning with the bytes as stored, where part of the chain was applied silently.
  `--images` and `--inspect` name it, where they showed `-`, and `--extract-image` refuses
  a `/Filter` reference it cannot follow.
  (bug-0015)

## [0.31.7] - 2026-11-04
### Fixed
- **`--object`, `--inspect`, `--tree` and `--search` read text strings as the standard
  says.**  A UTF-16BE `/Title` printed as mojibake and never matched a search, and raw
  UTF-8, which is invalid, read as correct, which hid a real defect in another tool.
  `--object` now prints `(Hello) [UTF-16BE]`, marks a string that is not plain ASCII with
  its encoding or `[not decodable]`, and escapes what could rewrite a terminal.  Its JSON
  gains `decoded`, `encoding`, `format` and, unless the bytes are plain ASCII, `hex`, so
  nothing is lost; `value` is the decoded text.  `--search` matches what a string says and
  its raw bytes, so a search that matched before still does, and `regex=` now reads a
  string, Name or key that is not UTF-8, where it skipped it.  (bug-0033)

## [0.31.6] - 2026-11-04
### Fixed
- **An object at a generation other than 0 can be reached, and the live one is what a number
  means.**  `--list` and `--search` showed such an object, while `--object`, `--inspect`,
  `--extract-stream` and `--extract-image` said it was not there.  And on a number freed
  and reused, they showed the stale copy at generation 0 with exit 0, while the page used
  the live one.  A number now means its only generation, or, with several, the one the
  cross-reference table says is live, and the output names it.  If nothing says which, it
  exits 1 naming them, in a range too.  In `--json`, an `error` entry has no `generation`
  key.  (bug-0020)

## [0.31.5] - 2026-11-04
### Added
- `DEBUGGING_WITH_PDF_DUMP.md` § What pdf-dump does not read gathers the limits, and adds
  vector lettering, text a viewer would not show, a JPEG’s `/ColorTransform`, and stored
  orientation.  `--help` says that exit 0 does not mean every visible word was read.

## [0.31.4] - 2026-11-04
### Fixed
Found by Fable, Sol and Opus, reviewing what v0.30.0 to v0.31.3 promise, and by
Obsidian-Brain’s mail gate:
- **Text extraction now knows when lopdf did not reach the end of a content stream.**
  An unterminated string, an inline image with no `EI` or with an end that was guessed
  at, and syntax errors the operator count missed all lost what followed with verdict
  `reliable` and exit 0, so `--find-text` said “No matches” of text in the file.  They
  now exit 3 with a `degraded` verdict.  Shapes lopdf could not read now read: an inline
  image whose `EI` ends the stream, and a comment followed by a blank line or white space,
  each of which lost the page’s text.
- **An inline image whose dictionary holds a reference no longer crashes** `--text`,
  `--operators`, `--validate` or `--object` with a panic in lopdf.  What follows it counts
  as not read.
- **`--validate` reads a page’s content streams as one,** as the spec does.  It warned of
  invalid syntax on a page whose streams divide inside a dictionary.
- **Malformed markup in XFA no longer reads as decoded.**  A tag that stops the search
  for `datasets`, or a `<!` that is no declaration, sets `xfa_decoded: false`, exit 3,
  where the data was lost with exit 0.
- **`--validate` and `--page` follow an indirect `/Contents` array,** as `--text` has
  since v0.31.0.  `--validate` missed such a page’s syntax errors, and `--page` reported
  0 bytes.
- **An inline image with no `/CS` no longer vanishes without a word:** `--operators`
  and `--object` warn, and `--validate` reports it.
- **`--extract-image` refuses a colour key that would leave a picture blank,** as it does
  a mask, and a `/DecodeParms` of the wrong kind, which it read as none.
- `--extract-stream`, `--object --decode` and `--search` read a `/DecodeParms` that refers
  to nothing as null, as the rest of the tool and § 7.3.10 do, rather than warning.

## [0.31.3] - 2026-11-04
### Fixed
- **`--object --decode`, `--extract-stream` and `--search stream=` undo predictors.**
  A stream with a PNG or TIFF `/Predictor` in its `/DecodeParms` came out still
  predictor-coded, labelled “decoded”, with no warning.  This is how every Acrobat
  cross-reference stream and much image data are stored.  `/EarlyChange 0` is honoured
  too, and parameters held by reference are followed.  A predictor that is unusable, or
  does not fit the data, is a warning, with the bytes as the filter left them.  (bug-0004)

## [0.31.2] - 2026-11-04
### Fixed
- **A syntax error in a content stream no longer ends text extraction silently.**
  lopdf stops at the first token it cannot parse and reports success, so everything
  after it was lost with verdict `reliable` and exit 0.  `--text`, `--text --layout`
  and `--find-text` now warn, and exit 3 with a `degraded` verdict, and so do a content
  stream or form that will not decode.  `--operators` and `--object` warn, `--validate`
  reports it, and an annotation’s appearance reads `unreliable`.  (bug-0039)
- **An inline image with neither `/CS` nor `/IM` no longer crashes** `--text`,
  `--operators`, `--validate` or `--object` with a panic in lopdf.  The text after it
  now survives.

## [0.31.1] - 2026-11-04
### Fixed
- CI’s clippy job, on a lint newer than the toolchain v0.30.0 was built with, and CI’s
  minimum-Rust job, which now names 1.92.

## [0.31.0] - 2026-11-04
### Added
- **`--extract-image N --output f`** writes an image XObject as a file a viewer or OCR
  engine opens, and **`--extract-inline-image PAGE:INDEX --output f`** does the same
  for an inline image (plan-0005).  PNG for decoded pixels; the stream as it is for
  DCTDecode (`.jpg`) and JPXDecode (`.jp2`).  The written image is the picture as
  drawn: predictors, `/EarlyChange`, `/Decode` and palettes are applied, CMYK and
  ICCBased are converted naively, 16-bit samples reduce to 8, CCITTFaxDecode and
  JBIG2Decode decode, and masks composite onto white.  `--json` reports the path, the
  format, the pixel size and the source colour space and filter.
- **What it cannot convert it refuses:** exit 1, naming what stopped it, and nothing
  written.  An output extension no image can be written under is exit 2; one for
  another format than this image’s is exit 1, because only the document can tell.
- **`--images` lists inline images** with their page, index, source and offset, and
  every image’s `export_format`.  It exits 3 when a content stream could not be walked.
### Changed
- **Minimum Rust is 1.92,** which `hayro-jbig2` and `hayro-ccitt` require.

## [0.30.0] - 2026-11-03
### Added
- **`--forms` reports every value** (plan-0006).  A list value is a JSON array in
  `values`; a stream value is decoded; a value inherited from a parent field is
  reported with `inherited: true`, and a parent holding its own value is an entry
  too; `/RV` is reported as text in `rich_value`, markup removed; a signature’s
  text is reported.  `value` stays a string in every case.
- **`--forms` reports XFA:** `xfa`, `xfa_packets`, `xfa_datasets_text`, `xfa_decoded`.
- **`--annotations` reads each appearance:** `appearance_text`, `appearance_state`,
  `appearance_reliability`, `appearance_warnings`, through the extractor `--text`
  uses.  `/RC` is reported as text in `rich_contents`.  A widget with no `/Resources`
  of its own uses the form’s `/DR`.
- **`--annotations` lists direct annotations** as well as indirect ones
  (`object_number` is then `null`), and `/Annots` held in an indirect array.
- **`--detail embedded` counts every attachment:** every filespec with `/EF`
  wherever it is reached (a direct filespec, a `Launch` or `GoToE` action, a
  `FileAttachment` annotation), every stream in `/EF` or `/RF`, and any
  `/Type /EmbeddedFile` stream no `/EF` names.
  `reached_from` says which; `filespec_object` is `null` for a direct filespec.
### Changed
- **Text strings are decoded as the standard says** in `--forms`, `--annotations`,
  `--detail`, `--bookmarks`, `--tags` and the overview’s metadata: the UTF-16BE mark,
  the UTF-8 mark, otherwise PDFDocEncoding, with language escapes removed.  A string
  that cannot be decoded prints as hex, never passed through lossily, and `--forms`,
  `--annotations` and `--detail embedded` also flag it in `--json`
  (`value_decoded: false` and its kin).  Raw UTF-8 with no mark, which is invalid, now
  reads as mojibake rather than looking correct.  `--object`, `--inspect` and
  `--search` are unchanged (bug-0033).
- **Behavior change (exit codes): `--forms` and `--annotations` can exit 3.**  A value
  or comment that will not decode, malformed rich text, unreadable XFA data, or an
  appearance verdict other than `reliable` is a finding, and `--json` still emits.  A
  checkbox’s or radio button’s state is excused when all it fails on is Symbol or
  ZapfDingbats, since it draws a mark, not text.
- `--forms` prints a value in full rather than cut to 20 characters.

## [0.29.1] - 2026-11-03
### Changed
- **`--extract-stream --output` checks its output path before the document is
  read.**  A missing directory, a directory in the file’s place, or a dangling
  link exits 1 naming the path, where a missing directory used to fail only after
  the document was parsed.  No directory is ever created.  An existing file is
  still replaced, as before.  (plan-0007)
- **Adopts `cli-contract` 0.6** for the exit codes, the closed-pipe check and the
  output-file checks.  A closed pipe still exits 0 with nothing on stderr, now
  pinned by a test, and a test holds `--help`’s exit table to the contract’s.

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
