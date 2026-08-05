# bug-0025: `--page` is accepted and range-validated but silently ignored by most modes

**Severity:** Low
**Classification:** SPEC-CODE mismatch — **SPEC DECISION REQUIRED**
**Status:** Verified (live: `--fonts --page 1` is byte-identical to `--fonts`, yet `--fonts --page 99` and `--object 1 --page 99` exit 1)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`--page` is accepted and range-validated (and can exit 1) even in modes where it has no effect, so an
out-of-range `--page` fails a run in modes where `--page` does nothing at all, while an in-range
`--page` is silently ignored.  Neither behavior is documented.

## Affected code
- `src/lib.rs:189-195` — the out-of-range `--page` guard runs unconditionally for every mode.
- Docs: `DEBUGGING_WITH_PDF_DUMP.md:45` documents `--page` as filtering only
  `--text`/`--operators`/`--annotations`/`--find-text`.

## What it does vs. what it should do
`--fonts --page 1` produces the same output as `--fonts` (silently ignored), but `--fonts --page 99`
fails with exit 1 because the guard fires regardless of whether the mode consumes `--page`.  Same
accepted-but-ignored class, lower stakes: `--decode`, `--truncate`, `--deref`, `--hex` are accepted
with every mode and ignored where inapplicable; `--dot` is ignored under `--json`
(`--tree --dot --json` emits JSON; `args.dot` is only read in the text branch, `lib.rs:497-503`).

**SPEC DECISION REQUIRED** (do not blindly code-fix): choose the contract.
- Option A: reject or warn when `--page` (or another inert modifier) is supplied to a mode that
  ignores it.
- Option B (recommended): make the out-of-range `--page` guard fire only when the resolved mode
  actually consumes `--page`, and document the silent-ignore-elsewhere behavior.

## Reproduction
```
pdf-dump f.pdf --fonts --page 1    # ignored (same as --fonts)
pdf-dump f.pdf --fonts --page 99   # exit 1, though --fonts never uses --page
```

Assert the chosen contract (e.g. under Option B, `--fonts --page 99` no longer exits 1).

## Suggested fix
Implement the chosen option.  For Option B, gate the `build_page_list` validation in `run()` on whether
the resolved mode is one that consumes `--page`, and add a documentation note about inert modifiers.

## Why the fix addresses the bug
Aligning “does this mode use `--page`?” between the validation and the output removes the
fail-anyway-but-ignore contradiction.

## Related
Doc side overlaps with [[bug-0006-docs-help-and-readme-flag-text-errors]] (`--page` help text is also
stale).
