# bug-0013: Form-field `/Kids` cycle causes a stack-overflow abort from the default command

**Severity:** Critical
**Classification:** CODE bug
**Status:** Verified (live repro: `pdf-dump form-cycle.pdf` aborts with exit 134)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`collect_field_recursive` walks an AcroForm field tree through `/Kids` with no visited-set or depth
guard.  A form field whose `/Kids` array contains itself (or an A→B→A cycle) makes the function
recurse forever until the stack overflows and the process aborts.  Because the default overview
counts form fields, this crash is reachable from the plain `pdf-dump file.pdf` invocation (and from
`--json` and `--forms`), so a single malformed file kills the tool before it prints anything useful.

## Affected code
- `src/forms.rs:93-145` — `collect_field_recursive` recurses into `/Kids` with no cycle guard; a
  kid that has a `/T` sets `has_field_kids = true` and recurses.
- `src/forms.rs:84-88` — `collect_form_fields` entry point.
- `src/summary.rs:385` — the text overview calls `collect_form_fields` for its feature line.
- `src/summary.rs:456` — the JSON overview does the same.

## What it does vs. what it should do
Every other tree walk in the crate carries a `visited` set — `bookmarks.rs` (outline Next/First
loops), `helpers.rs::walk_name_tree` / `walk_number_tree`, `validate.rs::check_page_tree_cycles`.
`collect_field_recursive` does not.  PDF field trees are reference graphs and a corrupt or malicious
file can make `/Kids` point back at an ancestor.  When it does, the recursion never terminates and
Rust aborts with `fatal runtime error: stack overflow, aborting` (exit 134) — an uncatchable crash
and an undocumented exit code (the tool documents only `0/1/2/3`).

The function should thread a `visited: &mut BTreeSet<ObjectId>` through the recursion and skip (or
stop at) an already-seen field id, exactly as the crate’s other tree walks do.

## Reproduction
Build a PDF whose catalog has `/AcroForm << /Fields [4 0 R] >>` where object `4` is a field that
references itself:

```
4 0 obj << /T (loop) /FT /Tx /Kids [4 0 R] >> endobj
```

Then:

```
pdf-dump form-cycle.pdf          # aborts: "fatal runtime error: stack overflow, aborting", exit 134
pdf-dump form-cycle.pdf --json   # same
pdf-dump form-cycle.pdf --forms  # same
```

A two-node cycle (`A /Kids [B]`, `B /Kids [A]`, both with `/T`) behaves identically.  Confirmed live
against the debug binary; the fixture is `form-cycle.pdf` in the review scratchpad.

Rust test sketch: construct that document with the `lopdf` builder (see `lib.rs::test_utils`), then
assert `forms::collect_form_fields(&doc)` returns without aborting (a passing test simply completing
is the assertion; an unguarded version overflows the stack and the test process dies).

## Suggested fix
Add a `visited: &mut BTreeSet<ObjectId>` parameter to `collect_field_recursive`; on entry, `return`
(or `continue` over the kid) if the field’s `ObjectId` is already in the set, otherwise insert it
before recursing.  Model the change on `bookmarks.rs`, which already does this for outline cycles.  A
`MAX_FIELD_DEPTH` cap is a reasonable belt-and-suspenders addition for pathologically deep (but
acyclic) trees.

## Why the fix addresses the bug
A visited-set makes the traversal bounded by the number of distinct field objects, so a `/Kids`
cycle is detected on the second visit and the walk terminates instead of recursing into a stack
overflow — the same guarantee the crate’s other tree walks already provide.

## Related
This is the highest-severity finding in the 2026-07-15 hunt: a crash from the default invocation,
uncatchable by callers, producing an undocumented exit code.  See `~/.claude/rules/cli-exit-codes.md`
(a panic/abort is none of the documented `0/1/2/3`).
