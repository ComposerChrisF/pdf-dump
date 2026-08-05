# bug-0034: `--validate` reports “0 errors” on a document whose `/Root` dangles

**Severity:** High
**Classification:** CODE bug (validation false negative)
**Status:** Verified (live repro: `pdf-dump dangling-root.pdf --validate` prints “0 errors”, exit 0)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`--validate` never checks references held by the trailer, and its required-keys check treats a
`/Root` that points at a non-existent object as “present”.  A PDF whose trailer `/Root` dangles —
the single most important reference in the file — passes validation with zero errors and exit 0.

## Affected code
- `src/validate.rs:124-146` — `check_broken_references` iterates only `doc.objects`; it never
  examines the trailer’s `/Root`, `/Info`, `/Encrypt`.
- `src/validate.rs:235-260` — `check_required_keys`: with a dangling `/Root`, both the “Catalog
  missing /Pages” arm and the “Trailer missing /Root” arm are skipped, so nothing is reported.

## What it does vs. what it should do
The machinery to walk the trailer already exists — `collect_reachable_ids` (`validate.rs:172-211`)
does it — but `check_broken_references` does not use it, so a broken trailer reference is invisible.
Separately, in `check_required_keys`, `root_ref = Some((99,0))` while object 99 is absent: the
`if let Ok(Dictionary) = get_object(root_ref)` guard is false (so the “Catalog missing /Pages” error
is skipped), and the `else` “Trailer missing /Root” arm is also skipped because a reference
syntactically exists.  The document therefore has no resolvable catalog yet reports zero errors.

Validation should flag a broken trailer reference and a `/Root` that does not resolve to a catalog
dictionary as ERROR-level issues, which per the exit-code contract makes the run exit 3.

## Reproduction
```rust
let mut doc = /* minimal doc */;
doc.trailer.set("Root", Object::Reference((99, 0))); // object 99 does not exist
// validate::print_validation(&mut buf, &doc) currently: only "unreachable" WARNs, "0 errors"
```

Shell: `pdf-dump dangling-root.pdf --validate` currently prints unreachable WARNs, `Summary: 0
errors`, exit 0.  Fixture `dangling-root.pdf` is in the review scratchpad.  Assert an ERROR-level
issue is produced (e.g. “trailer references non-existent object 99” or “Root does not resolve to a
catalog”) and the run exits 3.

## Suggested fix
Add the trailer’s references (`/Root`, `/Info`, `/Encrypt`) to `check_broken_references`.  In
`check_required_keys`, when `/Root` is a reference that does not resolve to a catalog dictionary,
emit an ERROR rather than silently skipping both arms.

## Why the fix addresses the bug
Checking the trailer’s references closes the gap that lets the catalog dangle undetected, and the
ERROR classification routes the run to exit 3 as the contract requires.

## Related
[[bug-0027-reverse-refs-skip-trailer]] (both stem from not walking the trailer — fix together);
[[bug-0035-validate-objstm-false-positive]]; `~/.claude/rules/cli-exit-codes.md` (validation errors
exit 3).
