# bug-0018: Large `--object` / `--page` ranges are materialized in memory before any validation

**Severity:** High
**Classification:** CODE bug (robustness / resource exhaustion)
**Status:** Verified (measured: `--object 1-5000000` ran ~22 s emitting ~5M stderr lines; `--page 1-100000000` peaked ~409 MB RSS)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
A `--object` or `--page` range is expanded into a fully-materialized `Vec` of every integer in the
range before anything checks it against the document. `--object 1-4294967295` allocates a ~17 GB
vector inside argument parsing (before the PDF is even loaded), and `--page 1-4000000000` allocates a
comparably huge vector before the “page not found” check can fire.  A single argument thus becomes a
memory/CPU denial of service.

## Affected code
- `src/types.rs:449` — `parse_object_spec`: `result.extend(start..=end)`, run inside `resolve_mode`
  (before the document loads).
- `src/types.rs:421-427` — `PageSpec::pages()`: `(*start..=*end).collect()`.
- `src/helpers.rs:16-25` — `build_page_list` maps over the materialized `Vec` for `Single`/`Range`.
- `src/object.rs:333-345` — `print_objects` loops every number, printing a stderr error per miss.

## What it does vs. what it should do
`PageSpec::parse` and `parse_object_spec` accept any `u32` endpoints, and nothing bounds the range
against the document.  For `--page`, `build_page_list` eagerly enumerates the literal range for
`Single`/`Range` before the per-page lookup — contrast `OpenRange`, which lazily filters
`doc.get_pages()` and is safe.  For `--object`, the range is materialized during `resolve_mode`, so the
huge allocation happens before the file is opened.  A range should be intersected with the document’s
actual pages/objects (as `OpenRange` already does), not enumerated literally.

A secondary effect: a range that matches nothing still exits 0 (see the related bug), so
`--object 1-5000000` against a 6-object file prints millions of “not found” lines and exits 0.

## Reproduction
```
pdf-dump tiny.pdf --object 1-5000000        # ~22 s, ~5M stderr lines
pdf-dump tiny.pdf --page 1-100000000 --text # ~409 MB RSS just to print "Page 2 not found."
```

Unit-testable: `helpers::build_page_list(&doc, Some(&PageSpec::Range(1, 4_000_000_000)))` — assert it
returns promptly (intersected with the doc’s pages) rather than allocating billions of entries.

## Suggested fix
For `--page`, intersect the requested range with `doc.get_pages()` (mirror the `OpenRange` path)
instead of enumerating the literal range.  For `--object`, iterate the range lazily and validate each
number against `doc.objects` (or cap absurd spans).  Both should treat “range matches nothing” per the
missing-object contract decided in [[bug-0019-missing-object-modes-exit-zero]].

## Why the fix addresses the bug
Intersecting the range with the document bounds the work to real pages/objects, eliminating the eager
giant allocation while preserving correct output.

## Related
[[bug-0019-missing-object-modes-exit-zero]] (the exit code for “range matches nothing”).
