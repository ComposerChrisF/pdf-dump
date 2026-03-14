# Roadmap

Remaining enhancements and upstream improvements for pdf-dump.

---

## `--text` improvements

The current text extraction outputs raw content-stream order with no encoding awareness. Two improvements would significantly increase accuracy:

- **ToUnicode CMap support** — Use `/ToUnicode` CMaps when available to map character codes to Unicode. This is the single biggest improvement for text accuracy, and most modern PDFs include them.
- **Coordinate-based text ordering** — Use `Tm`/`Td`/`TD` coordinates to sort text blocks top-to-bottom, left-to-right within a page, rather than pure content-stream order.

## lopdf upstream: preserve encrypt dict

When lopdf loads an encrypted PDF, it removes the `/Encrypt` entry from the trailer and deletes the encrypt dictionary object from `doc.objects`, leaving dangling references in XRef stream objects. This forces downstream tools to use workarounds to detect encryption after loading.

**Suggestion:** Stop removing the encrypt dictionary and its trailer entry during decryption. The object should remain in `doc.objects` and `/Encrypt` should stay in `doc.trailer` so API clients can inspect encryption metadata directly.

## Simplify encryption detection in `summary.rs`

lopdf 0.36.0 has `doc.encryption_state: Option<EncryptionState>` (public field), populated after loading an encrypted PDF. Currently `summary.rs` detects encryption by checking `doc.trailer.get(b"Encrypt")` and then scanning all XRef stream objects for an `/Encrypt` key as a fallback.

**Fix:** Replace both checks with `doc.encryption_state.is_some()` — this is authoritative and doesn't depend on trailer or XRef stream state. Canary tests in `tests/lopdf_canary.rs` will detect if lopdf's behavior changes.

---

## Completed

All features from the original Tier 1, Tier 2, and Tier 3 feature plans have been implemented — including `--search` with regex, `--fonts` with encoding diagnostics, `--images`, `--forms`, `--validate` with 10+ structural checks, `--bookmarks`, `--annotations` (with merged link support), `--tags`, `--tree`, `--detail` views (security, embedded, labels, layers), `--inspect`, `--find-text`, multi-filter stream decoding, `--hex`, `--raw`, configurable truncation, and full `--json` support across all modes.

See [CLAUDE.md](../CLAUDE.md) for the current CLI reference.
