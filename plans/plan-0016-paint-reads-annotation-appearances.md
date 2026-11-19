# Plan: Read Annotation Appearances When Asking Whether a Page Shows Text

## Problem

`paint.rs` reads a page’s content streams and the forms they invoke, and nothing else.  An annotation’s appearance stream (a stamp, a free-text box, a form field) is drawn by a viewer on top of the page, and it can hold outlined or scanned words just as page content can.  `--text` does not extract text from appearances, and `PagePaint` does not look at them, so a page whose only marks are an annotation’s appearance counts as blank: it is not listed in `pages_without_text`, and `--require-text` passes it.

Since v0.39.0 an annotation with an appearance stops a page from counting as a repeat, so that exemption cannot pass one.  The blank-page case predates it.

## Proposed Change

Walk each annotation’s normal appearance (`/AP /N`, or the stream `/AS` selects in it) into the page’s `PagePaint`, with the same form walk, cycle guard, and depth cap.  A page whose appearances draw paths or images and show no text is then listed and fails `--require-text`.  An annotation the viewer does not draw (`/F` with the Hidden or NoView bit) is left out.  An annotation with no `/AP` is drawn too, from its own entries: a viewer builds one for a free-text box from `/Contents` and `/DA`, and for form fields when the AcroForm sets `/NeedAppearances`.  Walking `/AP` cannot see those, so such an annotation is counted as drawing something unknown, as the repeat test already does.  The converse belongs here too: the repeat test now treats any annotation with `/AP` as drawing, so a link whose appearance is an empty stream, which some generators put on every link, stops a logo page from passing.  Once appearances are walked, one found to draw nothing need not.  One entry a viewer draws without any appearance is a link’s border (`/Border`, whose default width is 1, or `/BS`): a rectangle, which cannot carry words, so the repeat test allows a link without `/AP` whatever its border; walking appearances should keep that.

## Implementation Notes

- `annotations.rs` already selects the normal appearance stream (`/AP /N`, by `/AS` when it is a state dictionary) for `--annotations`; reuse that selection rather than writing a second one.
- Whether appearance text counts as text for `--require-text` is a contract question: `--text` does not print it, so a page whose only words are in a free-text box would show text to `PagePaint` but none in the output.  The likely answer is that it does not count, so such a page fails, unless `--text` starts extracting appearance text (this repo’s plan-0008 covers forms and annotations).
- Tests: synthetic lopdf PDFs only.  A page with only a stamp drawing paths is listed; a hidden one is not.

## Why Not a Workaround

The question “does this page show anything the text lacks” is pdf-dump’s own, and a caller cannot answer it without walking the same objects.
