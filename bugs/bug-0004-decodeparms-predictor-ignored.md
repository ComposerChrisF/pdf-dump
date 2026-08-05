# bug-0004: `/DecodeParms` predictors are ignored; predictor-coded bytes are labeled “decoded”

**Severity:** High
**Classification:** CODE bug (silent wrong output) + SPEC-DOC bug — **SPEC DECISION REQUIRED** on the fix scope
**Status:** Verified (live: `predictor.pdf --object 4 --decode` prints “decoded, 10 bytes” of still-predictor-coded data, no warning)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`decode_stream` applies filters but never consults `/DecodeParms`.  For `FlateDecode`/`LZWDecode` with
a PNG/TIFF predictor (`/DecodeParms << /Predictor 12 /Columns N >>`), the zlib layer succeeds, so the
tool reports no warning and labels the output “decoded” — but the bytes are still predictor-coded
(each row prefixed with a filter byte; values are deltas).  This is the standard encoding for every
Acrobat cross-reference stream and much image data, so it is hit constantly.

## Affected code
- `src/stream.rs:173-219` — `decode_stream` never reads `/DecodeParms`.  A repo-wide grep for
  `DecodeParms`, `Predictor`, `EarlyChange` finds zero hits.
- Docs asserting support without caveat: `README.md:125`, `DEBUGGING_WITH_PDF_DUMP.md:186`.

## What it does vs. what it should do
Because the zlib decode succeeds, `warning` is `None` and `object.rs` labels the result “decoded”.  But
per PDF 32000-1 §7.4.4.4, when `/DecodeParms` specifies `Predictor >= 2`, the filter output must be
run through the (PNG or TIFF) predictor to recover the true data.  Without it, `--extract-stream` writes
predictor-coded bytes and reports success, and `--search stream=<text>` silently misses matches.  The
LZW default `EarlyChange 1` is handled by `with_tiff_size_switch`, but `EarlyChange 0` would decode
wrong and is likewise ignored.

**SPEC DECISION REQUIRED** (do not blindly code-fix): choose the scope.
- Option A (recommended for `FlateDecode` + PNG predictor, since xref streams hit it universally):
  apply the predictor to recover the true bytes.
- Option B (cheapest correct): when `/DecodeParms` has `Predictor >= 2` (or `EarlyChange 0`), return
  the data with a warning (“Predictor N not applied”) via the existing warning path
  (`stream.rs:204-210`), so nothing is mislabeled “decoded”.
- Either way, add a predictor caveat to `README.md` / `DEBUGGING_WITH_PDF_DUMP.md`.

## Reproduction
Build a stream `/Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 >>` whose content is the
zlib of PNG-Up rows `[2,1,2,3,4, 2,4,4,4,4]` (true data `[1,2,3,4,5,6,7,8]`):

```
pdf-dump predictor.pdf --object 4 --decode
# "Stream content (decoded, 10 bytes)" — still filter-byte-prefixed deltas, no warning
# true decoded data is the 8 bytes 1..8
```

Fixture builder `mkstream.py` → `predictor.pdf` is in the review scratchpad.  Under Option A, assert the
decoded bytes equal `[1..8]`; under Option B, assert a “predictor not applied” warning is emitted and
the output is not labeled a clean “decoded”.

## Suggested fix
Implement the chosen option in `decode_stream` (read `/DecodeParms`, apply the PNG/TIFF predictor, or
warn).  Update the docs to state predictor support (or its absence).

## Why the fix addresses the bug
Applying the predictor yields the true decoded bytes; the warning fallback at minimum stops
predictor-coded bytes being presented as authoritative decoded content.

## Related
[[bug-0029-search-raw-bytes-on-decode-failure]] — same “decode success ≠ decoded content” family:
here the decode _succeeds_ yet the bytes are still not the decoded content.
