# Plan: Say Why an Image-Only Page Is Not a Repeat

## Problem

Since v0.39.0, `--require-text` passes a page that only repeats one image a page with text draws (`repeated_images: true`).  A page that falls just short — a logo page whose graphics state holds an entry outside the allowed list, whose link carries an empty appearance, whose inline image is filtered, whose clip is a five-sided path — gets the same message as a scan: “page N draws images but shows no text”, and `repeated_images: false`.  The repeat test has many conditions, and a pipeline owner cannot tell which one stopped the page without reading its content stream by hand, which for a real statement nobody may do.

## Proposed Change

Record the first condition that stopped a candidate page and report it, as content-free text:

- In `--json`, each `pages_without_text` entry that draws only images gains `not_a_repeat: "<reason>"`, for example `"a graphics state sets /TR to a function"`, `"an annotation of type /FreeText"`, `"the image also appears on no page with text"`, `"the image is drawn 2 times"`.
- The stderr note for such a page adds the reason after “shows no text”.

A reason names operators, keys, types, and counts only — never a string’s content, a font, or image data — so it can be passed on from a real statement.

## Implementation Notes

- `PagePaint::unmatchable` becomes the first reason found (an `Option<&'static str>`, or a small enum with a `describe`), set where `unmatchable` is set today; `could_repeat` and `repeats` read it, and `mark_repeats` adds “appears on no page with text” for a page that could repeat but whose image is not in `shown`.
- The existing tests that assert `unmatchable` assert the reason instead; integration tests check the JSON field and the note.

## Why Not a Workaround

The reasons exist only inside the walk; a caller would have to re-implement the whole repeat test to recover them.
