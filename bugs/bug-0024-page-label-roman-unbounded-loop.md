# bug-0024: Page-label roman numerals loop unboundedly on a crafted `/St` value

**Severity:** Medium
**Classification:** CODE bug (unbounded loop / DoS on adversarial input)
**Status:** Verified (code-trace)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`int_to_roman` builds the numeral by repeated subtraction, so its cost is linear in the value.  The
page-label start value `/St` is read straight from the PDF as an `i64` and fed to `int_to_roman` for
the `r`/`R` styles, so a corrupt or malicious `/St` with a huge value makes the loop append `"m"`
trillions of times, building a multi-terabyte string and hanging (or exhausting memory).

## Affected code
- `src/page_labels.rs:17-48` — `int_to_roman`: `while n >= value { result.push_str(numeral); n -= value; }`.
- `src/page_labels.rs:135-149` — `collect_page_labels`, which passes `/St` (read at `:118-127`) into
  `int_to_roman` via `start_val.saturating_add(offset)`.

## What it does vs. what it should do
`/St` is fully attacker-controlled and unbounded.  For `/S /R` with `/St 9000000000000000`, the leading
`1000 → "m"` step appends `"m"` about `9 × 10^12` times.  Roman numerals are only meaningfully defined
into the low thousands, so the function should cap the value it will render (falling back to the
integer string above some bound, as it already does for `n <= 0`).

## Reproduction
A `/PageLabels` dictionary `Nums [0 << /S /R /St 100000000 >>]` over a 1-page document makes
`--detail labels` build a ~100k-character roman numeral (visibly slow); pushing `/St` toward the `i64`
range hangs the tool.

## Suggested fix
Cap the value passed to `int_to_roman` (e.g. above a few thousand, return the integer string), so the
linear-in-value loop cannot run on an attacker-chosen magnitude.

## Why the fix addresses the bug
A bound stops the repeated-subtraction loop from iterating an attacker-controlled number of times,
eliminating the hang/OOM.

## Related
[[bug-0005-decompression-bomb-lzw-runlength-uncapped]] (same “unbounded work from hostile input”
class); [[bug-0023-page-label-alpha-repetition-wrong]] (same module).
