# bug-0028: `--search value=` / `regex=` examine only top-level dictionary values, missing nested arrays/dicts

**Severity:** Medium
**Classification:** SPEC-CODE mismatch — **SPEC DECISION REQUIRED**
**Status:** Verified (code-trace; behavior follows directly)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`--help` says `value=<text>` matches “Any Name/String value”, but `value=` and `regex=` only examine a
dictionary’s direct top-level values.  A name inside an array (`/Filter [/FlateDecode]`), a value in a
nested dictionary, or an indirect bare String object is never matched.

## Affected code
- `src/search.rs:106-117` and `src/search.rs:131-144` — `value=`/`regex=` use `dict.iter().any(...)`
  with no recursion into nested containers.
- `src/search.rs:72-76` — non-dict/stream objects can never match anything.

## What it does vs. what it should do
`/Filter [/FlateDecode]` does not match `value=FlateDecode`; a name inside `/Encoding /Differences [...]`
or a string in a nested dictionary is invisible; a bare indirect String object can never match.  The
help’s “any value” claim implies these should match.

**SPEC DECISION REQUIRED** (do not blindly code-fix): choose the contract.
- Option A (recommended): recurse into nested arrays/dicts, mirroring how
  `refs.rs::collect_refs_recursive` walks containers.
- Option B: narrow the docs to “top-level values only”.

## Reproduction
```rust
// dict with Filter [/FlateDecode]
// value=FlateDecode currently does NOT match
```

Under Option A, assert `value=FlateDecode` matches a dictionary whose `/Filter` is
`[/FlateDecode]`.

## Suggested fix
Implement the chosen option.  For Option A, add recursion into nested arrays and dictionaries in the
`value=`/`regex=` evaluation, bounded like the other container walks.

## Why the fix addresses the bug
Recursing into containers makes the behavior match the “any value” claim in the help; narrowing the
docs makes the claim match the behavior.  Either removes the mismatch.

## Related
Overlaps with [[bug-0033-text-string-utf16be-mishandled]] (search-side value matching) and
[[bug-0030-search-syntax-comma-and-case-undocumented]] (search documentation).
