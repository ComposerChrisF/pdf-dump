# bug-0019: `--object` and `--inspect` exit 0 on a missing object, disagreeing with `--extract-stream` / `--page`

**Severity:** High
**Classification:** SPEC-CODE mismatch / CODE bug — **SPEC DECISION REQUIRED**
**Status:** Verified (live: `--object 999` → stderr message, exit 0; `--inspect 999` → STDOUT message, exit 0; `--extract-stream 999` → exit 1; `--page 99` → exit 1)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
A syntactically-valid object number the document does not contain is the same caller-claim/world
mismatch class as an out-of-range `--page`, for which the tool documents and returns exit 1.  But
`--object 999` and `--inspect 999` print an error and exit 0 (silent success for scripts), while
`--extract-stream 999` and `--page 99` exit 1.  Two coupled defects ride along: `--inspect` writes its
error to STDOUT, and `--object 0` (never a valid object number) parses and falls into the same silent
exit-0.

## Affected code
- `src/lib.rs:204-206` — `dispatch_standalone` returns `()`; the caller hardcodes `had_issues = false`.
- `src/object.rs:327-330` — missing object: `eprintln!` (stderr), no exit signal; `:373`, `:387-393`
  — JSON error objects, no signal.
- `src/inspect.rs:526-529` — the error is written to STDOUT via `wln!(writer, …)`; `:644-650` JSON.
- Compare `src/lib.rs:310-313` (`--extract-stream` exits 1) and the `--page` exit-1 rationale at
  `src/types.rs:121-124`.

## What it does vs. what it should do
The `--help` exit-code table defines exit 1 as a caller-claim/world mismatch (used for out-of-range
`--page`, “naming the real count”).  An object number beyond the document is exactly that class, yet
two of the three object-addressed modes exit 0.  A script cannot distinguish “found and printed” from
“not there” because both exit 0.  Along with it: `--inspect`’s error belongs on stderr (README:77
promises “stdout stays clean for piping”), and `--object 0` should arguably be a usage error (exit 2),
since `PageSpec` already rejects page 0 that way.

**SPEC DECISION REQUIRED** (do not blindly code-fix): confirm the intended contract before changing
behavior, because tests deliberately pin the current behavior.
- Recommended: a missing object in `--object`/`--inspect` exits 1 (matching `--extract-stream` and
  `--page` and the portfolio exit-code rule); `--inspect`’s error moves to stderr.
- Decide whether `--object 0` becomes a usage error (exit 2).
- Decide the multi-object case: `--object 1,999` currently prints object 1 then an error for 999 and
  exits 0 — should any miss in a list drive exit 1?

## Reproduction
```
pdf-dump f.pdf --object 9999        # currently: stderr message, exit 0   → want exit 1
pdf-dump f.pdf --inspect 9999       # currently: STDOUT message, exit 0   → want stderr + exit 1
pdf-dump f.pdf --object 9999 --json # currently: {"error": …}, exit 0     → want exit 1
```

Note `tests/integration.rs:478` `object_flag_nonexistent_object_fails` is named “fails” but asserts
only the stderr text, never the status — the expected pin is missing.  Revisit `parse_object_spec_zero`
and `multi_object_missing_reports_error_in_json` deliberately when changing the contract.

## Suggested fix
After the decision: route `dispatch_standalone`’s object/inspect misses to `had_issues`/exit 1 (give
`dispatch_standalone` a `bool` return like `dispatch_default`, or set a flag), move `--inspect`’s
error to `eprintln!`, and add `object_number`/`generation` to the single-object JSON error to match
the multi-object and inspect forms.  Update the named integration test to assert the status.

## Why the fix addresses the bug
One exit-code answer for “you named something not in the document”, across `--object`, `--inspect`,
`--extract-stream`, and `--page`, lets scripts branch reliably — the entire purpose of the exit-code
rule.

## Related
[[bug-0018-large-range-materialized-before-validation]] (range-matches-nothing exit code),
[[bug-0020-object-generation-not-addressable]] (the false “not found” also exits 0),
`~/.claude/rules/cli-exit-codes.md`.
