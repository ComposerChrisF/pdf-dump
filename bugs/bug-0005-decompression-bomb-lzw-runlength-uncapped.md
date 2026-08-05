# bug-0005: The decompression-bomb cap is enforced after-the-fact for LZW and not at all for RunLength

**Severity:** Medium
**Classification:** CODE bug (a stated protection not implemented on two of five paths; DoS on adversarial input)
**Status:** Verified (code-trace)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`MAX_DECODED_SIZE` (256 MB) is advertised as preventing decompression bombs, but `decode_lzw` decodes
everything into memory and only then checks the cap, and `decode_run_length` has no cap at all.  Because
LZW reaches roughly thousandfold expansion and RunLength up to 128× per stage — with filter arrays of
attacker-controlled length — a small crafted stream can force multi-gigabyte allocations before the
“prevention” check runs.

## Affected code
- `src/stream.rs:5-6` — the `MAX_DECODED_SIZE` comment claiming bomb prevention.
- `src/stream.rs:115-128` — `decode_lzw`: `decoder.decode(data)` decodes fully, then checks the cap.
- `src/stream.rs:130-156` — `decode_run_length`: no cap.

## What it does vs. what it should do
The `FlateDecode` arm correctly caps _during_ decode via `.take(MAX_DECODED_SIZE)` (`stream.rs:186`).
`decode_lzw` should stream the decode with an output cap (weezl’s chunked API) so it bails once output
exceeds the limit, rather than attempting a multi-GB allocation first. `decode_run_length` should check
`result.len()` incrementally (before each `extend`) so a chain like `[/RunLengthDecode /RunLengthDecode …]`
cannot amplify 128× per stage without bound.  This tool’s routine input is broken/hostile PDFs, so the
cap must hold at every stage.

## Reproduction
Build a stream `/Filter [/RunLengthDecode /RunLengthDecode]` whose payload is crafted so stage 1 emits
maximal repeat-runs; a ~1 MB file forces multi-GB allocation.  Run `pdf-dump f.pdf --object N --decode`
and watch RSS/abort.  Unit test: assert `decode_run_length` returns an error once output would exceed
`MAX_DECODED_SIZE`.

## Suggested fix
Stream the LZW decode with an output cap (weezl’s `into_stream` / chunked API), and check
`result.len()` incrementally in `decode_run_length` before each `extend`, returning an error when the
cap would be exceeded — so the cap the comment promises actually holds at every stage.

## Why the fix addresses the bug
Enforcing the cap during (not after) decode prevents the runaway allocation the comment claims to
prevent, closing the DoS on both the LZW and RunLength paths.

## Related
Same “unbounded work from hostile input” class as
[[bug-0024-page-label-roman-unbounded-loop]].
