# Roadmap

Remaining enhancements and upstream improvements for pdf-dump.

---

## `--text` improvements

Text extraction is now font-aware (Tier 1, v0.13.0): show-strings are decoded through each font’s `/ToUnicode` CMap and through a WinAnsiEncoding table for simple fonts that lack one, with raw byte passthrough as a fallback.  A per-document reliability verdict (Reliable/Degraded/Unreliable) is surfaced via a loud stderr banner, a JSON `reliability` object, and exit code 3 when extraction is unreliable (CID/Type0 fonts without ToUnicode).

Remaining accuracy improvements:

- **More base-encoding tables** — Add MacRomanEncoding/StandardEncoding → Unicode tables (same shape as the existing WinAnsi table in `encodings.rs`).  Highest-value next step: macOS exports use MacRomanEncoding, whose high-range punctuation (curly quotes, apostrophes) currently passes through to U+FFFD.
- **Adobe Glyph List** — Resolve `/Differences` glyph names to Unicode for simple fonts that lack a ToUnicode map.
- **Predefined CJK CMaps** — Support the Adobe-Japan1/GB1/CNS1/Korea1 CMap resource files for CID fonts that use them without an embedded ToUnicode.
- **Coordinate-based text ordering** — Use `Tm`/`Td`/`TD` coordinates to sort text blocks top-to-bottom, left-to-right within a page, rather than pure content-stream order.
- **Form XObject recursion** — Follow `Do` into form XObjects so text drawn inside forms is captured (currently only page-level content streams are read).

## lopdf upstream: preserve encrypt dict

When lopdf loads an encrypted PDF, it removes the `/Encrypt` entry from the trailer and deletes the encrypt dictionary object from `doc.objects`, leaving dangling references in XRef stream objects.  This forces downstream tools to use workarounds to detect encryption after loading.

**Suggestion:** Stop removing the encrypt dictionary and its trailer entry during decryption.  The object should remain in `doc.objects` and `/Encrypt` should stay in `doc.trailer` so API clients can inspect encryption metadata directly.

## Simplify encryption detection in `summary.rs`

lopdf 0.36.0 has `doc.encryption_state: Option<EncryptionState>` (public field), populated after loading an encrypted PDF.  Currently `summary.rs` detects encryption by checking `doc.trailer.get(b"Encrypt")` and then scanning all XRef stream objects for an `/Encrypt` key as a fallback.

**Fix:** Replace both checks with `doc.encryption_state.is_some()` — this is authoritative and doesn’t depend on trailer or XRef stream state.  Canary tests in `tests/lopdf_canary.rs` will detect if lopdf’s behavior changes.

---

## Completed

All features from the original Tier 1, Tier 2, and Tier 3 feature plans have been implemented — including `--search` with regex, `--fonts` with encoding diagnostics, `--images`, `--forms`, `--validate` with 10+ structural checks, `--bookmarks`, `--annotations` (with merged link support), `--tags`, `--tree`, `--detail` views (security, embedded, labels, layers), `--inspect`, `--find-text`, multi-filter stream decoding, `--hex`, `--raw`, configurable truncation, and full `--json` support across all modes.

See [CLAUDE.md](../CLAUDE.md) for the current CLI reference.
