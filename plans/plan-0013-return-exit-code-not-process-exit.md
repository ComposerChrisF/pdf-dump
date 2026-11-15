# Plan: Return an `ExitCode` From `run()` Instead of Calling `process::exit`

## Problem

`src/lib.rs` ends the process with `std::process::exit` at 23 sites: in `run()` and its helpers for each exit-table code, and in `exit_on_write_error`, which the `w!`/`wln!` macros call on any write error.  The lint set adopted on 15-Nov-2026 disallows `process::exit`, for two reasons:

- It skips destructors.  Today nothing is lost (there is no `BufWriter` and no `impl Drop` in `src/`, std flushes stdout on exit, and no exit runs while `write_atomically`’s temp file exists), but that holds only by inspection, and the next buffered writer or guard added would silently break it.
- It scatters the exit table across the code, so the codes `--help` documents are not checked in one place.

The sites are not faults, so the adoption marks each with an `#[expect(clippy::disallowed_methods, reason = "plan-0013: …")]`, because CI denies warnings.  This plan is what removes them.

## Proposed Change

- `run()` returns `std::process::ExitCode` (or a `Result` that `main` maps to one), built from the `cli_contract` codes it already uses.
- Each early `process::exit` becomes a `return` of its code.
- `exit_on_write_error` becomes a write error that propagates to `run()`, which maps it as now (a closed pipe stays a quiet exit, any other write error stays exit 1).
- `main` is the only place a code leaves the process.

No exit code changes, so no caller audit is needed.

## Implementation Notes

- The macros are the larger part: `w!`/`wln!` are used throughout the printers, so they would return `io::Result` and the printers would use `?`.  That can land in one pass or module by module; each step removes the expects it covers.
- Remove each `#[expect(…, reason = "plan-0013: …")]` with the site it covers.  `unfulfilled_lint_expectations` flags any that is left behind.
- The existing integration tests already assert each exit code; they are the regression guard.  Add one for a write error that is not a closed pipe, if none exists.

## Why Not a Workaround

The fix is in the tool’s own control flow; there is nothing to work around.  Keeping the expects for good would leave the destructor-skipping hazard in place with the lint silenced.
