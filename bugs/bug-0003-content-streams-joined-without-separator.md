# bug-0003: Multiple page content streams are concatenated with no separator, fusing tokens across boundaries

**Severity:** High
**Classification:** SPEC-CODE mismatch
**Status:** Verified (live: `--operators` on `concat-op.pdf` prints a bogus `ETBT` operator; `--text` prints `AB`)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
When a page’s `/Contents` is an array of several streams, `read_content_streams` concatenates their
decoded bytes with nothing between them.  A segment ending in the token `ET` followed by a segment
beginning with `BT` fuses into the single non-token `ETBT`, corrupting operator parsing and text
extraction.  Every conforming PDF reader joins content-stream segments with whitespace.

## Affected code
- `src/helpers.rs:299` — `bytes.extend_from_slice(&decoded);` in `read_content_streams`, with no
  separator pushed between segments.
- Consumers: `src/text.rs:181` (`--text`), `src/operators.rs:14` (`--operators`), and `--find-text`.

## What it does vs. what it should do
PDF 32000-1 §7.8.2 states that the division between a page’s multiple content streams “may occur only
at the boundaries between lexical tokens”.  A segment ending `…ET` with the next beginning `BT…` is
therefore conforming, and a reader must treat each segment as token-terminated.  This code joins with
no byte, so the last token of segment _N_ fuses with the first token of segment _N+1_.  It should push
a single whitespace byte (`b'\n'`) between segments.

The tool is internally inconsistent: the identical operators in a _single_ stream extract as `A` and
`B` on separate lines (pinned by the existing `extract_text_multiple_bt_blocks` test), while across
two streams they fuse.

## Reproduction
Build a page whose `/Contents` is `[A B]` with:

```
A = BT (A) Tj ET
B = BT (B) Tj ET
```

Then:

```
pdf-dump concat-op.pdf --operators
# currently prints a fused "ETBT" (5 operations instead of 6)
pdf-dump concat-op.pdf --text
# currently prints "AB" instead of "A" and "B" on separate lines
```

Fixture `concat-op.pdf` is in the review scratchpad.  Assert `--operators` shows separate `ET` and
`BT`, and `--text` yields `A` and `B` separately.

## Suggested fix
In `read_content_streams`, push a `b'\n'` between segments (i.e. append a newline after each decoded
segment except optionally the last).

## Why the fix addresses the bug
A whitespace separator restores the token boundary that §7.8.2 guarantees, matching every conforming
reader and making cross-stream and single-stream extraction agree.

## Related
Affects `--text`, `--operators`, and `--find-text` because they all consume `read_content_streams`.
