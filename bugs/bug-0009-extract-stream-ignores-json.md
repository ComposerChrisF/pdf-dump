# bug-0009: `--extract-stream` silently ignores `--json`

**Severity:** Low
**Classification:** SPEC-CODE mismatch — **SPEC DECISION REQUIRED** on the fix direction
**Status:** Verified (live: `--extract-stream 5 --output x.bin --json` prints the plain success line, exit 0)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`--extract-stream … --json` prints the plain-text `Successfully extracted …` line — `--json` is
silently ignored.  It is also the only mode outside the `print_json_with_recovery` funnel, so the
documented “recovery object in every `--json` mode” cannot apply to it.

## Affected code
- `src/lib.rs:281-315` — the `ExtractStream` arm of `dispatch_standalone` never checks `config.json`.
- Docs: `README.md` (“add `--json` to any command”), `DEBUGGING_WITH_PDF_DUMP.md:38,46` (“`--json`
  (all modes)”).

## What it does vs. what it should do
Every other mode routes through `print_json_with_recovery` and honors `--json`. `--extract-stream`
does not, contradicting the documented “`--json` in all modes” claim and leaving it the one mode that
cannot carry the `recovery` object.

**SPEC DECISION REQUIRED** (do not blindly code-fix): choose one.
- Option A (recommended, matches the portfolio agent-first `--json` checklist): emit a JSON summary
  for extract-stream (e.g. `{ "extracted": true, "object": N, "output": "…", "bytes": M }`) routed
  through `print_json_with_recovery`.
- Option B: scope the docs to “every mode except `--extract-stream`”.

## Reproduction
```
pdf-dump two.pdf --extract-stream 5 --output /tmp/x.bin --json
# currently: "Successfully extracted object 5 to '/tmp/x.bin'." (plain text), exit 0
```

Under Option A, assert stdout is a JSON object with the summary fields (and a `recovery` key when a
malformed stream was recovered).

## Suggested fix
Implement the chosen option.  For Option A, build a `serde_json::Value` summary and pass it through
`print_json_with_recovery` (so the recovery merge applies), guarded by `config.json`.

## Why the fix addresses the bug
Either honoring `--json` for extract-stream or narrowing the documented claim removes the contradiction
between the docs and the behavior.

## Related
`~/.claude/rules/strategy-defaults.md` (agent-first `--json` checklist).  Doc side overlaps with
[[bug-0008-docs-stale-capabilities-and-exit-codes]].
