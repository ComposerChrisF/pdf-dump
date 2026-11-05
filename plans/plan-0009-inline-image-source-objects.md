# Plan: Name the Streams a Page’s Inline Image Came From

## Problem

An `--images --json` entry in `inline_images` names where the image came from with `source` and `source_object`.  A form, pattern or appearance has one stream, so `source_object` names it.  A page’s content can be several streams, read as one, so `source_object` is null there, and `offset` counts into the joined content.

A caller therefore cannot tell which streams it has already walked.  Obsidian-Brain’s gate holds a PDF when a `--search` hit falls in a stream the listing did not name, and asked for this in closing its request `Obsidian-Brain/r0002` (4-Nov-2026).  It reads `/Contents` from `--search "Type=Page,key=Contents"` meanwhile.  That counts every page’s streams as walked once any page has a listed inline image, and it does not follow an indirect array.  Not urgent: that workaround is in its patch, and its landing waits on Chris.

## Proposed Change

Each entry in `inline_images` gains `source_objects`, a list of object numbers, set for every source:

- `source: "page"`: the page’s content streams, in the order they are joined.
- `form`, `pattern`, `appearance`: the one stream, the same number as `source_object`.

`source_object` stays as it is, since callers read it.  The list names every stream of the page, not the one holding the image, because `offset` is the only position there is and it counts into the join.

## Implementation Notes

- `src/inline_image.rs` (`InlineImage`, `source_object`, the scan’s page and form paths), `src/images.rs` (the JSON), and `DEBUGGING_WITH_PDF_DUMP.md` § Mode-specific schemas.
- `helpers::page_content_ids` already lists a page’s streams and follows an indirect array.  It drops an array element that is not a reference, as a direct stream cannot occur there.
- Object numbers alone match `source_object`, which drops the generation too.  A stream at a non-zero generation is rare but real (bug-0020), and a caller passing the number back gets the live one.
- Tell Obsidian-Brain when it lands, so that it can drop the workaround.

## Why Not a Workaround

The workaround exists, and is what the problem describes: it over-counts, and it misses an indirect array.  Which streams pdf-dump read for an entry is a fact only pdf-dump has for certain, and a second reader of `/Contents` can disagree with it.
