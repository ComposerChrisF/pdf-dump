# bug-0001: ASCII85 overflow group silently truncated; malformed final single-character group

**Severity:** Low
**Classification:** CODE bug
**Status:** Verified (code-trace; the verbatim function was run on the demonstrating inputs)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`decode_ascii85` accumulates a 5-character group into a `u64` and emits `value.to_be_bytes()[4..]` —
the low 32 bits — with no check that the group value fits in `u32`.  A group encoding a value greater
than `2^32 − 1` is silently truncated to its low 32 bits instead of being rejected.  A final group of
exactly one leftover character (illegal, since it cannot encode any byte) is also mishandled.

## Affected code
- `src/stream.rs:47-57` — `decode_ascii85`: the accumulation loop and `value.to_be_bytes()[4..]`
  output, with no overflow check and no rejection of a 1-character final group.

## What it does vs. what it should do
Per PDF 32000-1 §7.4.3, a 5-tuple encoding a value greater than `2^32 − 1` is an error, and a final
group of a single character is invalid (it encodes no bytes).  The code takes the low 32 bits of an
overflowing group (silent wrong output) and, for `chunk_len == 1`, emits `chunk_len - 1 == 0` bytes
(silently dropping data).  This is a debugging tool for broken files, so surfacing these malformations
matters.

## Reproduction
```rust
// group value 4,437,053,124 > u32::MAX
assert!(decode_ascii85(b"uuuuu~>").is_err());
// currently returns Ok([0x08, 0x78, 0x0e, 0xc4]) — the low 32 bits
```

Add a unit test in the `src/stream.rs` tests module.

## Suggested fix
After accumulating the group, `if chunk_len == 5 && value > u32::MAX as u64 { return Err(...) }`, and
reject `chunk_len == 1` as a malformed final group (rather than emitting zero bytes).

## Why the fix addresses the bug
The overflow check makes an out-of-range group a loud error instead of silent low-32-bit garbage, and
rejecting the 1-character final group surfaces the malformation instead of dropping data.

## Related
Same “surface malformations in a debugging tool” theme as
[[bug-0026-recovery-truncates-at-embedded-endstream]].
