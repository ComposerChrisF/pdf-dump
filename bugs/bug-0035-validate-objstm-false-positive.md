# bug-0035: `--validate` flags every `/Type /ObjStm` container as “unreachable from trailer”

**Severity:** Medium
**Classification:** SPEC-CODE mismatch (the documented XRef-stream workaround is incomplete)
**Status:** Verified (live: `plain.pdf --validate` warns “Object 7 0 is unreachable from trailer”; object 7 is `/Type /ObjStm`)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
The unreachable-object check skips only stream objects whose `/Type` is `XRef`.  An object-stream
container (`/Type /ObjStm`) is likewise never referenced by any PDF object reference — the
cross-reference stream points into it by byte offset, not via an `Object::Reference` — so every ObjStm
container is reported “unreachable from trailer”, the exact false positive the XRef skip exists to
prevent.  Any PDF 1.5+ file using object streams (most modern PDFs, including stock `pdf-maker` output)
triggers this noise.

## Affected code
- `src/validate.rs:106-122` — `collect_xref_stream_ids` skips only `/Type /XRef`.
- `src/validate.rs:213-233` — `check_unreachable_objects` uses that skip set.

## What it does vs. what it should do
ObjStm containers are structurally unreferenced by design, exactly like XRef streams (type-2 xref
entries point into them by offset).  The skip set that suppresses the XRef false positive should also
include `/Type /ObjStm`.  This is warning-level, so it does not change the exit code, but it is
spurious on the majority of modern PDFs.

## Reproduction
```
pdf-maker -o plain.pdf --blank-page letter
pdf-dump plain.pdf --validate
# [WARN] Object 7 0 is unreachable from trailer   (object 7 is /Type /ObjStm)
```

Fixture `plain.pdf` is in the review scratchpad.  Rust test: a doc with an unreferenced `/Type /ObjStm`
stream; assert no “unreachable” warning names it.

## Suggested fix
Extend the skip set (in `collect_xref_stream_ids` or the unreachable check) to include stream objects
with `/Type /ObjStm`, alongside `/Type /XRef`.

## Why the fix addresses the bug
ObjStm containers are unreferenced by design, exactly like XRef streams, so they belong in the same
skip set and should not be reported as unreachable.

## Related
[[bug-0034-validate-dangling-root-false-negative]] (same module; validate correctness).
