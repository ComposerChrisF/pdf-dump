# Plan: A Page That Only Repeats Images From a Text Page Passes `--require-text`

## Problem

`--require-text` exists to catch a page whose words are outlined as paths or held in a scanned image.  It fails any page that draws something but shows no text, so it also fails a page that draws nothing but a logo the other pages carry too.  A real case: a four-page account statement whose pages 1–3 carry text and whose page 4 draws only the issuer’s small logo, the same image every page draws.  `--text --layout --require-text` refuses the whole file with “page 4 draws images but shows no text”, so every statement of that kind fails a pipeline that gates on the flag.

Such a page cannot hold words the extracted text lacks: everything it draws is drawn, byte for byte, on a page the flag passes.

## Proposed Change

A page that shows no text is a **repeat** when it draws images and nothing else (no path, no shading, nothing unread), and every image it draws is also drawn on a page of the document that shows text, selected by `--page` or not.

- `--require-text` passes a repeat.  Every other page that draws but shows no text fails as before.
- A repeat stays audible.  It is still listed in `pages_without_text`, with a new field `repeated_images: true` (false on every other entry), and the stderr note says so: `page 4 draws only images that a page with text draws too, so it adds no words (--require-text passes it)`.
- A document in which no page shows text has no repeats, so a one-page document holding only a logo still fails.

“The same image” is the same data: an image XObject or inline image whose stream bytes, `/Width` and `/Height` match.  This covers one XObject shared by every page and a copy embedded per page alike.  An inline image lopdf cannot hand over (one with neither `/CS` nor `/IM`, which the content reader blanks) has no data to compare, so a page drawing one is never a repeat.

No area threshold: a full-page scan repeated from a page with text adds no words either, and the repeat test alone is exact.

## Implementation Notes

- `paint.rs`: `PagePaint` records a key for each image it draws (a hash of the bytes and dimensions) and whether any image had none; `mark_repeats(doc, pages)` walks the document’s pages once, only when some page lacks text, collects the keys drawn on pages that show text, and sets `repeated` on each page that qualifies.  `print_note` and `page_json` read it.
- Callers: the four `pages_without_text` sites in `text.rs` and `layout.rs`, and the `--require-text` gate in `lib.rs`.
- Docs: the flag’s `--help`, the exit-code paragraph above it, README, `DEBUGGING_WITH_PDF_DUMP.md` (the schema and its paragraph), CLAUDE.md’s `paint.rs` row.
- Tests: synthetic lopdf PDFs only.  A text page and a logo-only page sharing one XObject pass; the same with a per-page copy of the image passes; a logo-only page whose image differs fails; a logo page that also draws a path fails; a document of logo-only pages fails; `--page` naming only the logo page passes.

## Why Not a Workaround

The gate is the tool’s own contract.  A caller cannot tell a repeated logo from a scan without reading the image data itself, which is exactly what pdf-dump already walks.
