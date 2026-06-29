# Roadmap

Remaining enhancements and upstream improvements for pdf-dump.

---

## `--text` improvements

Text extraction is now font-aware (Tier 1): show-strings are decoded through each font’s `/ToUnicode` CMap, through tables for all four named single-byte base encodings (WinAnsi, MacRoman, Standard, MacExpert) for simple fonts that lack one, and through the Adobe Glyph List for `/Encoding /Differences` glyph names — with raw byte passthrough as a fallback.  A per-document reliability verdict (Reliable/Degraded/Unreliable) is surfaced via a loud stderr banner, a JSON `reliability` object, and exit code 3 when extraction is unreliable (CID/Type0 fonts without ToUnicode).

Remaining accuracy improvements:

- **Predefined CJK CMaps** — Support the Adobe-Japan1/GB1/CNS1/Korea1 CMap resource files for CID fonts that use them without an embedded ToUnicode.
- **Coordinate-based text ordering** — Use `Tm`/`Td`/`TD` coordinates to sort text blocks top-to-bottom, left-to-right within a page, rather than pure content-stream order.
- **Form XObject recursion** — Follow `Do` into form XObjects so text drawn inside forms is captured (currently only page-level content streams are read).

### Known limitations: the reliability verdict is static

The per-font Reliable/Degraded/Unreliable verdict is assigned **statically**, from a font’s dictionary alone — it does not look at which character codes the content actually shows, nor at how many replacement characters the decode emitted.  That keeps the verdict cheap and predictable, but it lets a font be reported **Reliable** while a subset of its codes silently extracts as U+FFFD or the wrong glyph.  Three instances of this class remain known and deliberate (a fourth, Standard-14 passthrough, was fixed — see below); each is anchored in `src/text.rs` (search `KNOWN LIMITATION`).  Revisiting any of them points toward the same fix — a _usage-aware_ verdict that downgrades once the content actually shows a code the decoder cannot handle.

- **Base-less `/Differences` fonts.**  A `/Differences` font whose glyph names all resolve through the Adobe Glyph List is reported Reliable even when it has no recognized base `/Encoding`, so its _non_-overridden codes fall to single-byte passthrough (accurate only for ASCII).  Anchored in `build_font_decoder`.  Common real case (recognized base + `/Differences`) is genuinely correct; the base-less case is rare.
- **Coverage net only watches ToUnicode.**  The dynamic “>20 % of codes unmapped → Degraded” downgrade reads counters incremented only on the ToUnicode path, so the U+FFFD a base-table miss or passthrough emits never trips it.  Table/passthrough fonts get only the coarser static verdict.  Anchored in `document_verdict`.
- **Variable-width ToUnicode codespace.**  A `Variable`/`Unknown` codespace is forced to one fixed width (2 for CID, else 1), which can mis-split a genuinely variable-width font, while it stays Reliable for having a ToUnicode map.  Anchored at the width selection in `build_font_decoder`; a fix would honor per-range widths in `split_codes`.

## lopdf upstream: preserve encrypt dict

When lopdf loads an encrypted PDF, it removes the `/Encrypt` entry from the trailer and deletes the encrypt dictionary object from `doc.objects`, leaving dangling references in XRef stream objects.  This forces downstream tools to use workarounds to detect encryption after loading.

**Suggestion:** Stop removing the encrypt dictionary and its trailer entry during decryption.  The object should remain in `doc.objects` and `/Encrypt` should stay in `doc.trailer` so API clients can inspect encryption metadata directly.

---

## Completed

Font-aware `--text` now also resolves `/Encoding /Differences` glyph names to Unicode through the embedded Adobe Glyph List (plus the algorithmic `uniXXXX`/`uXXXXXX` forms, `.suffix` stripping, and underscore-joined ligature names), so simple fonts that remap codes without a `/ToUnicode` map decode correctly and classify Reliable.

Standard-14 text fonts (`Helvetica`/`Times-Roman`/`Courier` and their styles) with no `/Encoding` and no `/ToUnicode` are now decoded through the `standard` (StandardEncoding) table — their documented builtin — instead of ASCII-only byte passthrough.  This resolves the former Standard-14 over-claim: `0x27`/`0x60` extract as the curly quotes ’/‘ and the StandardEncoding high range decodes correctly, so the Reliable verdict is now earned rather than presumed.  Embedded fonts with no `/Encoding` keep their honest Degraded passthrough, since their builtin encoding is unknown.

All features from the original Tier 1, Tier 2, and Tier 3 feature plans have been implemented — including `--search` with regex, `--fonts` with encoding diagnostics, `--images`, `--forms`, `--validate` with 10+ structural checks, `--bookmarks`, `--annotations` (with merged link support), `--tags`, `--tree`, `--detail` views (security, embedded, labels, layers), `--inspect`, `--find-text`, multi-filter stream decoding, `--hex`, `--raw`, configurable truncation, and full `--json` support across all modes.

See [CLAUDE.md](../CLAUDE.md) for the current CLI reference.
