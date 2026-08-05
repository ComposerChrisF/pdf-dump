# bug-0027: Reverse-reference lookup never scans the trailer, so `--inspect` on the catalog says “Referenced by: (none)”

**Severity:** Medium
**Classification:** CODE bug
**Status:** Verified (live: `fixture2.pdf --inspect 1` on the catalog prints “Referenced by:\n  (none)”)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`collect_reverse_refs` iterates only `doc.objects`, never the trailer, so the catalog — which is
always referenced by the trailer’s `/Root` — is reported as referenced by nothing.  This is a false
absence claim about the one object that is always referenced.

## Affected code
- `src/refs.rs:57-74` — `collect_reverse_refs` iterates only `doc.objects`.
- `src/inspect.rs:607-623` — prints “(none)” when empty; JSON `referenced_by: []`.

## What it does vs. what it should do
The trailer’s `/Root`, `/Info`, and `/Encrypt` are real references. `--inspect <catalog>` on any
normal PDF answers “Referenced by: (none)”.  The machinery to walk the trailer already exists —
`validate.rs::collect_reachable_ids` does it — so `collect_reverse_refs` should scan `doc.trailer` as
a pseudo-container and emit a synthetic source (for example `trailer`, key path `/Root`).

## Reproduction
```rust
doc.trailer.set("Root", Object::Reference((1, 0)));
// collect_reverse_refs(&doc, (1,0)) currently returns empty
```

Shell: `pdf-dump fixture2.pdf --inspect 1` (catalog) currently prints “Referenced by: (none)”.
Assert the reverse-reference result names the trailer (e.g. a source labeled `trailer`, key `/Root`).

## Suggested fix
In `collect_reverse_refs`, scan `doc.trailer` as an additional container, emitting a synthetic source
(e.g. `trailer` with the key path) for each reference it holds.

## Why the fix addresses the bug
Including the trailer makes the reverse-reference answer complete for the catalog and other
trailer-referenced roots, so “(none)” only appears when nothing truly references the object.

## Related
[[bug-0034-validate-dangling-root-false-negative]] — both stem from not walking the trailer; fix
together. `~/.claude/rules/positive-evidence-of-absence.md` (“(none)” must mean “looked and found
none”, not “did not look at the trailer”).
