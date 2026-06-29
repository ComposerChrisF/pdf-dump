# Roadmap

Remaining enhancements and upstream improvements for pdf-dump.

---

## `--text` improvements

Text extraction is now font-aware (Tier 1): show-strings are decoded through each font’s `/ToUnicode` CMap, through tables for all four named single-byte base encodings (WinAnsi, MacRoman, Standard, MacExpert) for simple fonts that lack one, and through the Adobe Glyph List for `/Encoding /Differences` glyph names — with raw byte passthrough as a fallback.  A per-document reliability verdict (Reliable/Degraded/Unreliable) is surfaced via a loud stderr banner, a JSON `reliability` object, and exit code 3 when extraction is unreliable (CID/Type0 fonts without ToUnicode).

Remaining accuracy improvements:

- **Predefined CJK CMaps** — Support the Adobe-Japan1/GB1/CNS1/Korea1 CMap resource files for CID fonts that use them without an embedded ToUnicode.
- **Coordinate-based text ordering** — Use `Tm`/`Td`/`TD` coordinates to sort text blocks top-to-bottom, left-to-right within a page, rather than pure content-stream order.
- **Form XObject recursion** — Follow `Do` into form XObjects so text drawn inside forms is captured (currently only page-level content streams are read).

## lopdf upstream: preserve encrypt dict

When lopdf loads an encrypted PDF, it removes the `/Encrypt` entry from the trailer and deletes the encrypt dictionary object from `doc.objects`, leaving dangling references in XRef stream objects.  This forces downstream tools to use workarounds to detect encryption after loading.

**Suggestion:** Stop removing the encrypt dictionary and its trailer entry during decryption.  The object should remain in `doc.objects` and `/Encrypt` should stay in `doc.trailer` so API clients can inspect encryption metadata directly.

---

## Completed

Font-aware `--text` now also resolves `/Encoding /Differences` glyph names to Unicode through the embedded Adobe Glyph List (plus the algorithmic `uniXXXX`/`uXXXXXX` forms, `.suffix` stripping, and underscore-joined ligature names), so simple fonts that remap codes without a `/ToUnicode` map decode correctly and classify Reliable.

All features from the original Tier 1, Tier 2, and Tier 3 feature plans have been implemented — including `--search` with regex, `--fonts` with encoding diagnostics, `--images`, `--forms`, `--validate` with 10+ structural checks, `--bookmarks`, `--annotations` (with merged link support), `--tags`, `--tree`, `--detail` views (security, embedded, labels, layers), `--inspect`, `--find-text`, multi-filter stream decoding, `--hex`, `--raw`, configurable truncation, and full `--json` support across all modes.

See [CLAUDE.md](../CLAUDE.md) for the current CLI reference.
