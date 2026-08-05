# bug-0017: `is_garbled_text` flags correctly-extracted non-ASCII text as non-extractable

**Severity:** Low
**Classification:** SPEC DECISION REQUIRED (test-enshrined design choice)
**Status:** Verified (code-trace; pinned by the test `garbled_text_unicode_chars`)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
The `--page` text-preview heuristic treats every character outside the ASCII printable band
(`0x20..=0x7E`) as “non-printable”, and if more than half the non-whitespace characters fall outside
it, reports the text as non-extractable.  Text of French, Japanese, etc. that was extracted perfectly
via `/ToUnicode` trips the threshold, so the preview is suppressed and `text_extractable` is set false
— while the JSON `text` field still holds the correct text, so the object self-contradicts.

## Affected code
- `src/page_info.rs:33-54` — `is_garbled_text` (the ASCII-band heuristic).
- `src/page_info.rs:147-158` — its use in the preview.
- `src/page_info.rs:389-391` — the JSON `text_extractable: false`.

## What it does vs. what it should do
A page of `café résumé` or CJK text extracts correctly but has more than 50% of its characters outside
`0x20..=0x7E`, so the heuristic labels it “(text not extractable — fonts lack Unicode mappings)” and
sets `text_extractable: false`, even though the `text` field is populated with the correct string.  The
test `garbled_text_unicode_chars` (`page_info.rs:862-867`) enshrines this, making it a design decision
rather than an accident.

**SPEC DECISION REQUIRED** (do not blindly code-fix): the ASCII-only heuristic is wrong for non-Latin
text, but a test pins the current behavior, so confirm the intended semantics first.
- Option A (recommended): drop the char-band heuristic and rely on the Unicode-aware reliability /
  coverage machinery already in `text.rs` (the `U+FFFD` ratio and per-font classification).
- Option B: count only `U+FFFD` and control characters as “non-printable” instead of all non-ASCII.

## Reproduction
A one-page document whose text extracts as `café résumé`:

```
pdf-dump f.pdf --page 1 --json
# text_extractable: false, preview "(text not extractable …)" — contradicting the populated "text"
```

Assert `text_extractable` is true for correctly-extracted non-ASCII text.

## Suggested fix
Implement the chosen option.  For Option A, replace the `is_garbled_text` call with the document/page
reliability verdict already computed by the text subsystem, and update the pinning test to reflect the
Unicode-aware semantics.

## Why the fix addresses the bug
Reusing the Unicode-aware verdict (rather than a parallel ASCII heuristic) stops correctly-extracted
non-ASCII text from being labeled non-extractable and removes the self-contradiction with the `text`
field.

## Related
The reliability machinery it should defer to is shared with
[[bug-0012-font-dedup-drops-unreliable-verdict]] and
[[bug-0011-find-text-unreliable-silent-success]].
