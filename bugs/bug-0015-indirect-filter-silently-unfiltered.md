# bug-0015: An indirect `/Filter` is silently treated as “no filter”; `--extract-stream` writes still-compressed bytes and reports success

**Severity:** High
**Classification:** CODE bug (silent wrong output)
**Status:** Verified (live: `pdf-dump indirect-filter.pdf --extract-stream 4 --output if.bin` writes raw zlib `78 9c …`, prints “Successfully extracted”, exit 0, no warning)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`get_filter_names` handles `/Filter` only when it is a Name or an array of Names.  A spec-legal
_indirect_ `/Filter 5 0 R` (a reference to a Name) falls through to an empty filter list, which is
indistinguishable from “unfiltered”.  The stream is then treated as raw: `--extract-stream` writes the
still-compressed bytes and reports success, and `--object --decode` labels the raw bytes “decoded” —
all with no warning.

## Affected code
- `src/stream.rs:158-171` — `get_filter_names`: `as_name()` or `as_array()` of names, else `vec![]`;
  an `Object::Reference` hits the `else`.
- `src/lib.rs:281-302` — the `--extract-stream` path, which writes the (undecoded) bytes and prints
  success.

## What it does vs. what it should do
An indirect object reference is valid PDF; the filter simply cannot be resolved without the
`Document`.  Inside an array (`[/FlateDecode 5 0 R]`) the `filter_map(|o| o.as_name().ok())` silently
drops the reference and applies a _partial_ chain with `warning: None`, which is worse.  Contrast the
pinned contract for a genuinely corrupt filter (`tests/integration.rs:1816`), which at least warns on
stderr.  An unresolved filter should either be resolved (correct) or surfaced as a warning — never
treated as “no filter”.

## Reproduction
Build a stream whose `/Filter` is an indirect reference to `/FlateDecode`:

```
4 0 obj << /Length N /Filter 5 0 R >> stream <zlib of "SecretPayload"> endstream endobj
5 0 obj /FlateDecode endobj
```

Then:

```
pdf-dump indirect-filter.pdf --extract-stream 4 --output if.bin
# "Successfully extracted", exit 0; if.bin begins with 78 9c (raw zlib — NOT decoded)
```

Fixture builder `mkstream.py` → `indirect-filter.pdf` is in the review scratchpad.  Assert that
either the extracted bytes are the decompressed `SecretPayload`, or a warning is emitted that the
indirect filter was not resolved.

## Suggested fix
`get_filter_names` cannot dereference without `&Document`.  Preferred: thread `&Document` through so
an indirect `/Filter` (and indirect elements inside a `/Filter` array) are resolved to their Names.
Interim: detect `Object::Reference` and emit an “indirect /Filter not resolved” warning via the
existing warning path in `decode_stream`, rather than returning an empty filter list.

## Why the fix addresses the bug
Resolving (or at least warning on) an indirect filter stops still-encoded bytes from being presented
as decoded content and stops a partial filter chain from being applied silently.

## Related
`~/.claude/rules/positive-evidence-of-absence.md`: an unresolvable filter is Unknown, not “no
filter”.  Same “decode success ≠ decoded content” family as
[[bug-0004-decodeparms-predictor-ignored]].
