# bug-0021: `--object N` on an indirect-reference object shows the target’s content under N’s header

**Severity:** Medium
**Classification:** CODE bug (with an upstream-behavior dependency the tool chose)
**Status:** Verified (live: `fixture2.pdf --object 8` prints object 4’s stream under object 8’s header; `--list` correctly shows `8 0 Reference`)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
When an object is itself an indirect reference (`8 0 obj 4 0 R endobj`), the object-display modes call
`doc.get_object((8,0))`, which lopdf auto-dereferences, so `--object 8` prints object 4’s content
under object 8’s header.  For a structure-debugging tool this is wrong output: the tool’s own `--list`
correctly reports object 8 as a `Reference`, but `--object`/`--inspect`/`--extract-stream` present the
dereferenced target as though it were object 8.

## Affected code
- Root cause: lopdf `Document::get_object` auto-dereferences reference chains.
- `src/object.rs:305-311` — `print_single_object`.
- `src/object.rs:364-397` — the JSON object builder.
- `src/inspect.rs:524` and `src/inspect.rs:642` — inspect text and JSON.
- `src/lib.rs:281-302` — `--extract-stream`.

## What it does vs. what it should do
`--object 8` prints `Object 8 0 (Stream): << /Length 16 >> stream …` — object 4’s content — and
`--inspect 8` reports “Object 8 is a stream.” The `Object::Reference` arm in `classify_object`
(`inspect.rs:308-311`) is therefore dead code through these paths. `--extract-stream 8` extracts the
target’s stream while naming object 8. `--list` is correct only because it does not dereference.

A structure dumper should display the object’s _stored_ value — a reference — not the dereferenced
target.  Dereferencing should remain available under `--deref`.

## Reproduction
```rust
doc.objects.insert((8, 0), Object::Reference((4, 0))); // object 4 is a stream
// pdf-dump ... --object 8  currently prints object 4's stream under "Object 8 0"
```

Shell: `pdf-dump fixture2.pdf --object 8` (fixture in the review scratchpad, object 8 is `4 0 R`).
Assert `--object 8` reports object 8 as a reference to `4 0 R`, not object 4’s content.

## Suggested fix
For display, look up `doc.objects.get(&id)` directly (no dereference) so the stored value is shown,
or explicitly print the chain (e.g. `8 0 obj = 4 0 R →`).  Keep the auto-dereferenced view behind
`--deref`.  Apply consistently to `--object`, `--inspect`, and `--extract-stream`.

## Why the fix addresses the bug
Showing the object’s own stored value (a reference) rather than its dereferenced target is what a
structure dumper must do, and it makes the object-display modes agree with `--list`.

## Related
[[bug-0020-object-generation-not-addressable]] (both concern faithful object addressing/display).
