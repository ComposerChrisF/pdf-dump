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

The per-font Reliable/Degraded/Unreliable verdict is assigned **statically**, from a font’s dictionary alone.  A dynamic safety net — the “>20 % of shown codes unmapped → Degraded” downgrade in `document_verdict` — now backs it up across **all** decode paths (ToUnicode, base-table, `/Differences` overrides, and passthrough all feed the coverage counters as of the universal-coverage change; see Completed).  Two earlier instances of this class — the Standard-14 passthrough over-claim and the ToUnicode-only coverage net — have been fixed, and the base-less `/Differences` over-claim is now self-correcting (a base-less font that actually shows undecodable codes is downgraded by coverage).  One genuine limitation remains, anchored in `src/text.rs` (search `KNOWN LIMITATION`):

- **Variable-width ToUnicode codespace.**  A `Variable`/`Unknown` codespace is forced to one fixed width (2 for CID, else 1), which can mis-split a genuinely variable-width font (common in CJK ToUnicode CMaps mixing 1-byte and 2-byte codes), while it stays Reliable for having a ToUnicode map.  Anchored at the width selection in `build_font_decoder`; the fix is to store the codespace bounds in `cmap.rs` and split per range in `split_codes`.  (Mis-splits now at least feed the coverage net via the change above, so a badly mis-split font tends to downgrade.)  See `feature-plan-usage-aware-reliability.md`, Phase 2.

The residual base-less `/Differences` static over-claim (a font reported Reliable that never actually shows its undecodable codes) is benign — the output is correct in that case — and is left as an optional refinement (feature plan, Phase 3).

## lopdf upstream: preserve encrypt dict

When lopdf loads an encrypted PDF, it removes the `/Encrypt` entry from the trailer and deletes the encrypt dictionary object from `doc.objects`, leaving dangling references in XRef stream objects.  This forces downstream tools to use workarounds to detect encryption after loading.

**Suggestion:** Stop removing the encrypt dictionary and its trailer entry during decryption.  The object should remain in `doc.objects` and `/Encrypt` should stay in `doc.trailer` so API clients can inspect encryption metadata directly.

---

## Completed

Font-aware `--text` now also resolves `/Encoding /Differences` glyph names to Unicode through the embedded Adobe Glyph List (plus the algorithmic `uniXXXX`/`uXXXXXX` forms, `.suffix` stripping, and underscore-joined ligature names), so simple fonts that remap codes without a `/ToUnicode` map decode correctly and classify Reliable.

Standard-14 text fonts (`Helvetica`/`Times-Roman`/`Courier` and their styles) with no `/Encoding` and no `/ToUnicode` are now decoded through the `standard` (StandardEncoding) table — their documented builtin — instead of ASCII-only byte passthrough.  This resolves the former Standard-14 over-claim: `0x27`/`0x60` extract as the curly quotes ’/‘ and the StandardEncoding high range decodes correctly, so the Reliable verdict is now earned rather than presumed.  Embedded fonts with no `/Encoding` keep their honest Degraded passthrough, since their builtin encoding is unknown.

The text-extraction reliability verdict is now **usage-aware**.  Every decode path in `emit_show_string` — ToUnicode, base-table, `/Differences` overrides, and passthrough — feeds the `total`/`unmapped` coverage counters through a shared `push_code` helper, so the “>20 % unmapped → Degraded” downgrade in `document_verdict` applies to all font types rather than ToUnicode only.  This fixed the former ToUnicode-only-coverage limitation and made the base-less `/Differences` over-claim self-correcting: a statically-Reliable font that actually emits a flood of U+FFFD on the bytes the document uses is now correctly downgraded to Degraded.  Stdout text is byte-for-byte unchanged; only the stderr banner / JSON `reliability` verdict moves.

All features from the original Tier 1, Tier 2, and Tier 3 feature plans have been implemented — including `--search` with regex, `--fonts` with encoding diagnostics, `--images`, `--forms`, `--validate` with 10+ structural checks, `--bookmarks`, `--annotations` (with merged link support), `--tags`, `--tree`, `--detail` views (security, embedded, labels, layers), `--inspect`, `--find-text`, multi-filter stream decoding, `--hex`, `--raw`, configurable truncation, and full `--json` support across all modes.

See [CLAUDE.md](../CLAUDE.md) for the current CLI reference.
