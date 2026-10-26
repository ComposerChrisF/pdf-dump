# Plan: Forms, Annotations and Attachments, Reported in Full

_Filed 25-Oct-2026 from the review of Obsidian-Brain’s webapp v0.44.0, whose KCS check (`webapp/deploy/kcs-mail/mailgate.py`) reads a PDF’s form values, comments and attachments through `--forms`, `--annotations` and `--detail embedded` (Chris’s ruling on ask `ai-front-door-fetch-gate-and/0024`, A)._

## Problem

The check must never call a PDF read when part of it was not.  These modes leave five things out or garble them.  Each is verified on fixture PDFs (Obsidian-Brain’s `test_doccheck.py` builds them):

1. **Text strings are decoded lossily.**  `/V` and `/Contents` go through `String::from_utf8_lossy` (`forms.rs`, `annotations.rs`).  A UTF-16BE string (BOM `FE FF`), which ISO 32000-1 § 7.9.2.2 prescribes for any text beyond PDFDocEncoding, arrives as `U+FFFD U+FFFD` followed by NUL-interleaved characters.  A PDFDocEncoding string with bytes above 0x7F is mangled too.
2. **Some values are placeholders.**  An array `/V` (a multi-select list) prints as `[array]`, and a stream `/V` as `(empty)`.  A `/V` on a parent field that has kids, which is inheritable, is not reported.  `/RV` and a comment’s `/RC` (rich text) are not reported at all.
3. **XFA is not reported.**  A form whose values live in `/AcroForm /XFA` datasets shows no fields.
4. **Appearance streams are not read.**  `--text` follows `Do` into form XObjects, but not an annotation’s `/AP`.  So text a widget or a comment shows only in its appearance is invisible, and nothing says it is there.
5. **Some attachments and annotations are not counted.**  `--detail embedded` counts only an indirect filespec in the `EmbeddedFiles` name tree.  It misses a direct filespec there, and an `/EF` filespec reached from a `Launch` or `GoToE` action.  `--annotations` lists only annotations referenced indirectly from `/Annots`.

Until these land, the check fails closed on what it can see.  It refuses a mangled value and `[array]`, and searches (`--search`) for `Type=EmbeddedFile`, `key=RC`, `key=RV`, a `/V` on a parent text or choice field with `/Kids`, and the XFA XML.  It cannot see the appearance-stream text (4) or an inline annotation, which it documents as a known gap.

## Proposed Change

- **Decode text strings as the standard says:** the UTF-16BE BOM, the UTF-8 BOM (PDF 2.0), and otherwise PDFDocEncoding.  A string that cannot be decoded is flagged in `--json` (for example `"value_decoded": false`), never passed through lossily.
- **Report every field value:** arrays as JSON arrays, stream values decoded, values inherited from parents (with `"inherited": true`), and `/RV` and `/RC` as text, with rich text’s markup stripped.
- **Report XFA:** at least `"xfa": true`, and better, the datasets’ text.
- **Read appearance text:** for each annotation, the text its normal appearance shows (`/AP /N`, through the same extractor `--text` uses), with its reliability verdict.
- **Count every attachment:** every filespec with `/EF`, wherever it is reached, and every annotation in `/Annots`, direct or indirect.

## Implementation Notes

- The decoding in (1) is shared by every mode that prints a text string (`--bookmarks` titles, `--detail` names), so it belongs in one helper.
- Each item gets a fixture and asserts the decoded value, not only that the key is present.
- When this lands, `mailgate.py` drops its fail-closed searches one by one, each as its pdf-dump counterpart is verified.  The known gap in (4) closes.

## Why Not a Workaround

Decoding PDF strings and walking annotation appearances is PDF knowledge, which is pdf-dump’s domain.  The check’s searches are a fail-closed stopgap: they hold files rather than read them, so every PDF with a rich-text comment or an XFA form waits for a reviewer until this lands.
