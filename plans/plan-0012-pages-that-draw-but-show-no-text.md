# Plan: Say When a Page Draws but Shows No Text, and Let a Caller Require Text

## Problem

A page whose words are drawn as filled paths (a print-to-PDF step that outlined the text), or whose words are a scanned image, shows no text at all.  `--text` then prints nothing for that page and exits 0.  `--help` already warns that exit 0 does not mean every word a viewer shows was read, but nothing tells a caller _which_ pages are like this.  A pipeline that redacts or indexes the text cannot tell “this statement has no text” from “this statement was never read”, and writes an empty result with exit 0.  Only the caller knows whether the document ought to carry text, so only the caller can make this fatal.

## Proposed Change

- **What a page draws**, read from its operators alone, recursing through form XObjects:
  - whether it shows any text (a `Tj`, `TJ`, `'` or `"` with a non-empty string);
  - whether it fills or strokes a path (`S s f F f* B B* b b*`, or `sh`);
  - whether it draws an image (an image XObject, or an inline image).

  Content that could not be read to its end is noted too.

- **Reported as data.**  A page that shows no text but draws paths or images, or whose content could not be read to its end, is listed:
  - in `--text --json` and `--text --layout --json`, as `pages_without_text`: `[{page_number, paths, images, unread}]`, always present and empty when there is none;
  - on stderr, as a note naming the page and what it draws, never its content.

  A blank page draws nothing and is not listed.  **The verdict and exit code do not change.**  The reliability verdict says how far the text that was shown can be trusted, and callers depend on that meaning.  One gate withholds a PDF on exit 3 but OCRs its images on exit 0, so making a scanned page exit 3 by default would stop it reading every scan.

- **`--require-text`** (with `--text`, `--layout` or `--output`): the caller asserts that every selected page carries text.  A page listed as above makes the run exit **1**, naming each such page and what it draws, and **nothing is written**:
  - no text goes to stdout;
  - no `--output` file is created or replaced;
  - no JSON is printed.

  This is the exit table’s caller-claim/world mismatch, like `--page` beyond the end.  A blank page passes.

## Implementation Notes

- A new module, `paint.rs`: `page_paint(doc, page_id) -> PagePaint { text, paths, images, unread }` and `PagePaint::lacks_text()`.  It walks `decode_content` operations with the same cycle guard and `MAX_FORM_DEPTH` as the extractors.
- The `--text` and `--layout` page loops call it only for a page that showed no codes, so text-bearing pages cost nothing extra.  `run()` calls it for every selected page under `--require-text`, before anything is dispatched or written.
- Tests (synthetic): words drawn as filled paths with no font resources, beside a page with real text; a page holding only a page-sized image; a blank page; text inside a form; an inline image.  Each is checked for the JSON list, the exit code, and the absent `--output` file.

## Why Not a Workaround

A caller could approximate it from `--operators --json` and `--images`, but only by re-implementing form recursion, inline-image detection and the painting-operator set.  Each such caller would draw the line differently, and a gate that decides whether a document was read must draw it the same way every time.
