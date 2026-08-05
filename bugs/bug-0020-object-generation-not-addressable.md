# bug-0020: Objects at generation ≠ 0 are unaddressable; `--object N` reports “not found” for an object `--list` shows

**Severity:** Medium
**Classification:** CODE bug — **SPEC DECISION REQUIRED** (interface)
**Status:** Verified (live: `fixture2.pdf --list` shows `7 1 Dictionary`, but `--object 7` → “Object 7 not found.”)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
The object-addressed modes hardcode generation 0 when looking up objects.  A PDF whose cross-reference
table carries an object at generation 1 (legal after an incremental update reuses a freed number) is
shown by `--list` and `--search` with `Gen 1`, but `--object 7` / `--inspect 7` / `--extract-stream 7`
answer “not found” — a false statement (and, per the related bug, exit 0) — with no way to supply a
generation.

## Affected code
- `src/object.rs:304` — `let obj_id = (obj_num, 0);` (also `:367`, `:379`).
- `src/inspect.rs:490`, `:523`, `:641` — “Generation 0 assumed — tool convention”.
- `src/lib.rs:285` — `--extract-stream` uses `(obj_num, 0)`.

## What it does vs. what it should do
lopdf keys `doc.objects` by the exact `(number, generation)` pair.  Hardcoding generation 0 means an
object stored at `(7,1)` is invisible to the object-addressed modes even though `--list`/`--search`
prove it exists — the tool contradicts itself.  Since this tool targets exactly the unusual/malformed
files where non-zero generations appear, it should be able to reach them.

**SPEC DECISION REQUIRED** (do not blindly code-fix): choose the interface.
- Option A (recommended): when `(num, 0)` is absent and a unique `(num, g)` exists, fall back to it,
  and name the found generation in any error.
- Option B: add explicit generation syntax (e.g. `--object 7:1`).
- At minimum, the “not found” error should name the generation that _does_ exist and exit 1.

## Reproduction
```rust
doc.objects.insert((7, 1), Object::Dictionary(/* … */));
// pdf-dump ... --object 7  currently: "Object 7 not found."; --list shows "7  1  Dictionary"
```

Fixture `fixture2.pdf` (has `7 1 obj`) is in the review scratchpad.  Assert `--object 7` finds the
generation-1 object (Option A) or that the error names the existing generation.

## Suggested fix
Implement the chosen interface.  For Option A, when `doc.objects.get(&(num, 0))` misses, scan
`doc.objects` for entries with the same number; if exactly one exists, use it (and mention the
generation in output); if several exist, list them.  Apply to `--object`, `--inspect`, and
`--extract-stream` consistently.

## Why the fix addresses the bug
Looking up the object’s real generation makes the object-addressed modes agree with `--list` and
`--search`, so an object the tool can clearly see is also reachable.

## Related
[[bug-0019-missing-object-modes-exit-zero]] (the false “not found” also exits 0),
[[bug-0021-object-indirect-reference-masquerade]].
