# bug-0016: `--inspect --deref` is honored in JSON but ignored in text mode

**Severity:** Low
**Classification:** CODE bug (text/JSON mismatch)
**Status:** Verified (live)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`--inspect N --deref` inline-expands references in JSON output but not in text output, so the two
output forms disagree for identical input.

## Affected code
- `src/inspect.rs:554-562` — `print_info` hardcodes `deref: false` (and `depth: None`,
  `decode: false`) instead of using the real config.
- `src/inspect.rs:655-663` — `inspect_json_value` passes `config.deref`/`config.depth` through.

## What it does vs. what it should do
`--inspect 3 --deref` in text mode shows a bare `2 0 R`, while the same invocation with `--json`
contains `"resolved"` objects. `print_info` should receive and honor the real `config` (at least
`deref` and `depth`), matching the JSON path.

## Reproduction
```
pdf-dump f.pdf --inspect 3 --deref        # text: bare "2 0 R" (deref ignored)
pdf-dump f.pdf --inspect 3 --deref --json # json: contains "resolved" objects
```

Assert both output forms reflect `--deref`.

## Suggested fix
Pass the real `config` (or at least `deref`/`depth`) into `print_info` instead of the hardcoded
defaults, so the text path expands references like the JSON path.

## Why the fix addresses the bug
Threading the actual config makes text mode honor `--deref` exactly as JSON already does, removing the
divergence.
