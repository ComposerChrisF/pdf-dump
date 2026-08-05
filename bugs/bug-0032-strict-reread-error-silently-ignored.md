# bug-0032: A re-read I/O error silently skips recovery/`--strict` detection, exiting 0 on a malformed file

**Severity:** Medium
**Classification:** CODE bug (silent wrong exit code / missing mandatory diagnostic)
**Status:** Verified (code-trace)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
The malformed-`/Length` recovery re-reads the file from disk.  If that re-read fails (the file was
replaced or unlinked after load, a permission flip, a network volume drop — a classic TOCTOU window),
the entire recovery/detection block is skipped with no message.  In default mode the output then prints
with stream bodies silently missing and no banner; under `--strict` the detection gate a CI caller
opted into reports nothing and exits 0 on a malformed file.

## Affected code
- `src/lib.rs:125-127` — `if recover::has_candidates(&doc) && let Ok(raw) = std::fs::read(&args.file)`
  — an `Err` from `fs::read` is folded into “nothing to recover/detect” and silently dropped.

## What it does vs. what it should do
This is the Unknown-folded-into-Absent shape from the positive-evidence-of-absence rule: an I/O error
is treated as “no malformed streams”, which gates the tool’s one loud-warning obligation and, under
`--strict`, the exit contract.  On `Err`, the tool should print a loud stderr note naming the file and
the error, and exit non-zero (a tool error, exit 1), because it could not complete a check it is
obliged to run — never silently proceed as if the scan ran clean.

## Reproduction
Hard to race directly; unit-testable by refactoring the block to take an `io::Result<Vec<u8>>` and
asserting that an `Err` produces a loud stderr message and a non-zero exit.  The behavioral contract is
clear from the code: an unreadable re-read must not be treated as a clean scan.

## Suggested fix
Match the `fs::read` result explicitly.  On `Err`, print `could not re-read <file> to verify stream
/Lengths: <err>` to stderr and exit 1 (or surface it and exit non-zero); never fall through to the
clean path.

## Why the fix addresses the bug
Treating the re-read failure as Unknown (loud, non-zero) rather than Absent (clean) restores the
guarantee the recovery and `--strict` contracts make: a malformed file is never silently reported as
fine.

## Related
`~/.claude/rules/positive-evidence-of-absence.md`, `~/.claude/rules/cli-exit-codes.md`.  Same recovery
subsystem as [[bug-0026-recovery-truncates-at-embedded-endstream]].
