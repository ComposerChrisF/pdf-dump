# bug-0023: Page-label alpha style is wrong past 26 (`AB` instead of `BB`)

**Severity:** Low
**Classification:** SPEC-CODE mismatch
**Status:** Verified (code-trace)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`int_to_alpha` implements bijective base-26 (spreadsheet-column style: `…Z, AA, AB, AC…`), but the PDF
page-label `A`/`a` styles use repeated letters (`…Z, AA, BB, CC, …, ZZ, AAA`).  The two agree only up
to 27, so any alpha-numbered range with more than 26 pages is mislabeled.

## Affected code
- `src/page_labels.rs:50-66` — `int_to_alpha`, used by `format_page_label` (`:68-78`).

## What it does vs. what it should do
PDF 32000-1 Table 159 defines the `A`/`a` styles as: `A` to `Z` for the first 26, then `AA` to `ZZ`
for the next 26, then `AAA`, and so on — i.e. the letter is repeated.  The code instead produces
bijective base-26, so 28 → `AB` (should be `BB`), 52 → `AZ` (should be `ZZ`), 53 → `BA` (should be
`AAA`). 27 → `AA` matches by coincidence, which is exactly what the existing `int_to_alpha_basic` test
pins, masking the divergence past 27.

## Reproduction
```rust
assert_eq!(int_to_alpha(28, true), "BB"); // currently returns "AB"
```

Or a `/PageLabels` document with `/S /A` and 53 pages: the label of page 53 should be `AAA`; the tool
prints `BA`.

## Suggested fix
Replace bijective base-26 with the repeated-letter scheme: `letter = (n - 1) % 26`,
`count = (n - 1) / 26 + 1`, then repeat the letter `count` times.

## Why the fix addresses the bug
The repeated-letter formula matches Table 159 exactly, unlike spreadsheet-style base-26.

## Related
[[bug-0024-page-label-roman-unbounded-loop]] (same module, `page_labels.rs`).  The existing
`int_to_alpha_basic` test only covers up to 27 and must be extended.
