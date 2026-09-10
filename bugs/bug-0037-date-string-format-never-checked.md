# bug-0037: A malformed `/CreationDate` is never reported — not as a finding, not even as a note

**Severity:** Medium — a mechanically checkable spec violation that the tool reads, prints, and passes over in silence, including under `--validate`
**Classification:** CODE bug (missing check) — with a **scope decision** on where the check reports; a recommendation is proposed below
**Status:** Verified live against v0.24.0 (2026-09-09), plus a 921-file survey establishing prevalence
**Component:** the overview’s `/Info` rendering, and `--validate`

## Summary

PDF 32000-1 §7.9.4 defines a date string as `D:YYYYMMDDHHmmSSOHH'mm'`.  A value that is not in that form is not a date to a conforming reader — viewers show a blank creation date and preflight tools flag it.  `pdf-dump` never checks.  It prints whatever bytes are there as though they were a date, and `--validate` reports `no issues found`.

## Verified

A real PDF produced by pdf-orchestrator before its `bug-0039` fix carried an ISO-8601 value:

```
CreationDate:2026-09-09T16:31:09
Validation:  no issues found
```

`2026-09-09T16:31:09` is not a PDF date.  The tool rendered it indistinguishably from a valid one and validated the file clean.

## How common is non-spec — the reason this is worth checking

Surveyed **921 PDFs** carrying a `/CreationDate` across `~/Chris`, spanning many producers and roughly two decades:

| Format | Count | Share |
|---|---|---|
| `D:` spec form | 874 | **95 %** |
| ISO 8601 | **0** | **0 %** |
| Other non-spec | 47 | 5 % |

Two conclusions, and they pull in different directions — which is why the scope decision matters:

- **ISO 8601 had zero instances in the wild.**  It was unique to pdf-orchestrator, whose entire output history carried it.  A tool-generated ISO date is an outright defect, not a dialect.
- **But 5 % of real files are non-spec anyway.**  The 47 are almost entirely US-locale `8/19/2010 21:43:49` from 2003–2013-era producers, plus one bare `20041029055004`.  Roughly 1 file in 20 would trip a strict check, and those files are not the user’s to fix.

## Suggested fix — and why the reporting level differs by mode

Add a format check on `/CreationDate` and `/ModDate`.  Report it **by mode**, following the mode-scoping principle in `cli-contract` `plan-0001`:

- **`--validate` is a reporting mode → a finding, exit 3.**  That is what the mode is for, its caller has opted into hearing about problems, and a malformed date is a real, mechanically decidable spec violation.
- **The default overview is not a reporting mode → annotate, stay exit 0.**  Print the value with a marker, e.g. `CreationDate: 8/19/2010 21:43:49  [not a PDF date string]`.  Someone running `pdf-dump file.pdf` to look around should not get a non-zero exit for a 2010 file they did not create and cannot fix — but they should be able to see that the field is malformed rather than being shown it as though it were fine.

That split is what keeps the 5 % from becoming noise while still making the defect visible everywhere it appears.  It also matches this tool’s existing treatment of recovered `/Length` errors: loud where the caller asked, informative where they did not.

**Do not attempt to normalize or reinterpret the value.**  Report what is there and say it is malformed; guessing that `8/19/2010` is US-locale rather than day-first is exactly the kind of inference that turns an inspection tool into an unreliable narrator.

## Why the fix addresses the bug

An inspection tool’s job is to tell the user what is in the file, including when what is in the file is wrong.  Printing a malformed date in the same shape as a valid one asserts a conformance the tool never checked — and in the pdf-orchestrator case it concealed a defect that shipped in every document the tool ever produced.  The check is a regex and a comparison; the cost of not having it was years of invalid dates nobody could see.

## Related

Same shape as `bug-0033` (UTF-16BE text strings displayed lossy): in both, the tool renders a non-conforming value as though it were conforming, and the reader has no way to tell.  A fix for either should consider whether the other’s field wants the same “this value is not what it claims to be” annotation.
