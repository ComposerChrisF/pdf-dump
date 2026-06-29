# Roadmap

Remaining enhancements and upstream improvements for pdf-dump.

---

## `--text` improvements

Text extraction is now font-aware (Tier 1): show-strings are decoded through each font’s `/ToUnicode` CMap, through tables for all four named single-byte base encodings (WinAnsi, MacRoman, Standard, MacExpert) for simple fonts that lack one, and through the Adobe Glyph List for `/Encoding /Differences` glyph names — with raw byte passthrough as a fallback.  A per-document reliability verdict (Reliable/Degraded/Unreliable) is surfaced via a loud stderr banner, a JSON `reliability` object, and exit code 3 when extraction is unreliable (CID/Type0 fonts without ToUnicode).

Remaining accuracy improvements:

- **Predefined CJK CMaps** — Support the Adobe-Japan1/GB1/CNS1/Korea1 CMap resource files for CID fonts that use them without an embedded ToUnicode.
- **Coordinate-based text ordering** — Use `Tm`/`Td`/`TD` coordinates to sort text blocks top-to-bottom, left-to-right within a page, rather than pure content-stream order.
- **Form XObject recursion** — Follow `Do` into form XObjects so text drawn inside forms is captured (currently only page-level content streams are read).

### Known limitation: usage-aware reliability for base-less `/Differences` fonts

The `/Differences` reliability verdict is **static** — it weighs only whether the glyph _names_ resolved through the Adobe Glyph List, not which character codes the content actually shows.  When a `/Differences` font has no recognized base `/Encoding` (so non-overridden codes fall to single-byte passthrough, accurate only for ASCII), it is still reported **Reliable** on the strength of its resolved names alone.  A non-ASCII, non-overridden byte in such a font would then extract as U+FFFD under a “reliable” banner.

This is a deliberate trade-off: the common real case is a recognized base encoding _plus_ `/Differences`, where the verdict is genuinely correct, and the base-less case is rare.  Revisiting would mean a _usage-aware_ verdict that downgrades a base-less font to Degraded once the content shows a code outside its override map.  The decision is anchored in `build_font_decoder` in `src/text.rs` (search `KNOWN LIMITATION`).

## lopdf upstream: preserve encrypt dict

When lopdf loads an encrypted PDF, it removes the `/Encrypt` entry from the trailer and deletes the encrypt dictionary object from `doc.objects`, leaving dangling references in XRef stream objects.  This forces downstream tools to use workarounds to detect encryption after loading.

**Suggestion:** Stop removing the encrypt dictionary and its trailer entry during decryption.  The object should remain in `doc.objects` and `/Encrypt` should stay in `doc.trailer` so API clients can inspect encryption metadata directly.

---

## Completed

Font-aware `--text` now also resolves `/Encoding /Differences` glyph names to Unicode through the embedded Adobe Glyph List (plus the algorithmic `uniXXXX`/`uXXXXXX` forms, `.suffix` stripping, and underscore-joined ligature names), so simple fonts that remap codes without a `/ToUnicode` map decode correctly and classify Reliable.

All features from the original Tier 1, Tier 2, and Tier 3 feature plans have been implemented — including `--search` with regex, `--fonts` with encoding diagnostics, `--images`, `--forms`, `--validate` with 10+ structural checks, `--bookmarks`, `--annotations` (with merged link support), `--tags`, `--tree`, `--detail` views (security, embedded, labels, layers), `--inspect`, `--find-text`, multi-filter stream decoding, `--hex`, `--raw`, configurable truncation, and full `--json` support across all modes.

See [CLAUDE.md](../CLAUDE.md) for the current CLI reference.
