# Roadmap

Remaining enhancements and upstream improvements for pdf-dump.

---

## `--text` improvements

Text extraction is now font-aware (Tier 1): show-strings are decoded through each font’s `/ToUnicode` CMap, through tables for all four named single-byte base encodings (WinAnsi, MacRoman, Standard, MacExpert) for simple fonts that lack one, and through the Adobe Glyph List for `/Encoding /Differences` glyph names — with raw byte passthrough as a fallback.  A per-document reliability verdict (Reliable/Degraded/Unreliable) is surfaced via a loud stderr banner, a JSON `reliability` object, and exit code 3 when extraction is unreliable (CID/Type0 fonts without ToUnicode).

Remaining accuracy improvements:

- **Predefined CJK CMaps** — Support the Adobe-Japan1/GB1/CNS1/Korea1 CMap resource files for CID fonts that use them without an embedded ToUnicode.
- **Coordinate-based text ordering** — Use `Tm`/`Td`/`TD` coordinates to sort text blocks top-to-bottom, left-to-right within a page, rather than pure content-stream order.

### Known limitations: the reliability verdict is static

The per-font Reliable/Degraded/Unreliable verdict is assigned **statically**, from a font’s dictionary alone.  A dynamic safety net — the “>20 % of shown codes unmapped → Degraded” downgrade in `document_verdict` — backs it up across **all** decode paths (ToUnicode, base-table, `/Differences` overrides, and passthrough all feed the coverage counters; see Completed).  Every genuine instance of this class has now been addressed: the Standard-14 passthrough over-claim, the ToUnicode-only coverage net, the base-less `/Differences` over-claim (now self-correcting via coverage), and the variable-width ToUnicode codespace (now split per codespace range; see Completed).

The one residual is benign and optional: a base-less `/Differences` font reported Reliable that never actually shows its undecodable codes — the output is correct in that case, so a dedicated static fix is left as an optional refinement (`feature-plan-usage-aware-reliability.md`, Phase 3).

## Dependencies: lopdf

**Done (2026-06-29):** the whole `~/Chris/App/Rust/Pdf/*` toolchain is on a single lopdf **0.42.0** (pdf-dump, pdf-maker, pdf-orchestrator, the medpdf workspace; font-dump has no lopdf).  A shared version keeps parse/encrypt behavior consistent; `tests/lopdf_canary.rs` is the tripwire for the encryption / xref / save_modern behaviors the toolchain depends on.

**Residual:** lopdf **0.43.0 is broken** — its default `time` feature references a nonexistent `FormatItem::StringLiteral`, so the crate will not compile.  Revisit 0.43.x once upstream fixes it; its only addition over 0.42 is an unused “encryption status in PdfMetadata” feature, so there is no urgency.  When bumping, retest the save_modern + encryption sentinel (see Completed): lopdf still mis-encrypts ObjStm-packed strings as of 0.42, which is why pdf-orchestrator uses traditional `save()` for encrypted output.

## lopdf upstream: preserve encrypt dict

When lopdf loads an encrypted PDF, it removes the `/Encrypt` entry from the trailer and deletes the encrypt dictionary object from `doc.objects`, leaving dangling references in XRef stream objects.  This forces downstream tools to use workarounds to detect encryption after loading.

**Suggestion:** Stop removing the encrypt dictionary and its trailer entry during decryption.  The object should remain in `doc.objects` and `/Encrypt` should stay in `doc.trailer` so API clients can inspect encryption metadata directly.

---

## Completed

The whole PDF toolchain was bumped to a single lopdf **0.42.0** in one coordinated pass (pdf-dump, pdf-maker, pdf-orchestrator, the medpdf workspace).  0.43.0 was skipped because it does not compile (see Dependencies above).

Observable recovery and a strict gate (v0.22).  Stream-`/Length` recoveries are now surfaced machine-readably: `run()` builds a `recovery` object that `print_json_with_recovery` merges into every `--json` mode (absent for clean PDFs).  `--strict` inverts the default — detect the malformed `/Length`, refuse to repair it, and exit 3 — a hard gate for CI.

Encrypted-PDF reporting and `--password` (v0.23).  The overview’s `encrypted` is now authoritative (`encryption_state.is_some() || doc.is_encrypted()`), so a password-protected PDF opened without its password is reported `encrypted: true` / `decrypted: false` with a loud banner, a validation warning, and exit 3 — instead of the old silent `encrypted: false`, 1 object, 0 pages.  `--password <PW>` decrypts via `Document::load_with_password` so such files read fully.

A hardened lopdf save_modern + encryption sentinel lives in `tests/lopdf_canary.rs`.  lopdf’s `save_modern()` packs strings into an ObjStm it leaves unencrypted under `/Encrypt`, corrupting them (still present through 0.42; pdf-orchestrator works around it with traditional `save()` for encrypted output).  The sentinel checks decoded content (not exit status), asserts an ObjStm was actually emitted (precondition), and requires an exact `/Info` round-trip, so it cannot falsely report “fixed”; it fails loudly when lopdf finally fixes the bug.

`pdf-dump` now reads leniently past a malformed `/Length`.  When a writer emits a content stream whose `/Length` does not match the bytes between `stream` and `endstream`, lopdf cannot find `endstream` where the length says and drops the object to a bare dictionary — its body silently lost (page text vanishes).  `recover.rs` runs once after `Document::load`: it relocates each such object by its cross-reference offset, extracts the true body by scanning to `endstream`, and promotes it back to `Object::Stream`, so every mode (text, `--list`, `--object`, extract-stream, …) sees the repaired document.  Each recovery prints a loud stderr banner naming the object, its declared vs. actual length, and the file offset.  The motivating case was pdf-maker’s overlay output (see its `feature-plan-overlay-content-stream-length.md`).

`--text` now follows `Do` into form XObjects, so text drawn inside a form is extracted instead of silently dropped — a page whose body lives entirely in a form previously extracted empty under a Reliable verdict.  `process_content` walks each form’s content stream recursively, builds a decoder table from the form’s own `/Resources` (inheriting the caller’s when the form has none, per PDF 32000-1 §7.8.3), and feeds the form’s fonts into the same reliability counters as the page.  A visited-set cycle guard (the active recursion stack) plus a `MAX_FORM_DEPTH` cap make self-referential and pathologically nested forms terminate; an over-deep chain emits a warning and stops.

Font-aware `--text` now also resolves `/Encoding /Differences` glyph names to Unicode through the embedded Adobe Glyph List (plus the algorithmic `uniXXXX`/`uXXXXXX` forms, `.suffix` stripping, and underscore-joined ligature names), so simple fonts that remap codes without a `/ToUnicode` map decode correctly and classify Reliable.

Standard-14 text fonts (`Helvetica`/`Times-Roman`/`Courier` and their styles) with no `/Encoding` and no `/ToUnicode` are now decoded through the `standard` (StandardEncoding) table — their documented builtin — instead of ASCII-only byte passthrough.  This resolves the former Standard-14 over-claim: `0x27`/`0x60` extract as the curly quotes ’/‘ and the StandardEncoding high range decodes correctly, so the Reliable verdict is now earned rather than presumed.  Embedded fonts with no `/Encoding` keep their honest Degraded passthrough, since their builtin encoding is unknown.

The text-extraction reliability verdict is now **usage-aware**.  Every decode path in `emit_show_string` — ToUnicode, base-table, `/Differences` overrides, and passthrough — feeds the `total`/`unmapped` coverage counters through a shared `push_code` helper, so the “>20 % unmapped → Degraded” downgrade in `document_verdict` applies to all font types rather than ToUnicode only.  This fixed the former ToUnicode-only-coverage limitation and made the base-less `/Differences` over-claim self-correcting: a statically-Reliable font that actually emits a flood of U+FFFD on the bytes the document uses is now correctly downgraded to Degraded.  Stdout text is byte-for-byte unchanged; only the stderr banner / JSON `reliability` verdict moves.

Variable-width ToUnicode codespaces are now split per codespace range.  `cmap.rs` stores each `begincodespacerange` entry’s `lo`/`hi` bounds and exposes `next_code`, which extracts the next character code by honoring those ranges (the shortest matching range wins, a best-effort form of PDF 32000-1 §9.7.6.2).  `build_font_decoder` carries the `CodeWidth` into `FontDecoder::ToUnicode`, so a `Fixed` codespace keeps the constant-width fast path (Identity-H is unchanged) while a `Variable` one decodes a mixed 1-byte/2-byte CJK CMap correctly instead of forcing a single width — and any residual mis-decode still feeds the coverage net.

All features from the original Tier 1, Tier 2, and Tier 3 feature plans have been implemented — including `--search` with regex, `--fonts` with encoding diagnostics, `--images`, `--forms`, `--validate` with 10+ structural checks, `--bookmarks`, `--annotations` (with merged link support), `--tags`, `--tree`, `--detail` views (security, embedded, labels, layers), `--inspect`, `--find-text`, multi-filter stream decoding, `--hex`, `--raw`, configurable truncation, and full `--json` support across all modes.

See [CLAUDE.md](../CLAUDE.md) for the current CLI reference.
