# bug-0030: `--search` comma-AND syntax and key case-sensitivity are undocumented; there is no comma escape

**Severity:** Low
**Classification:** SPEC-DOC bug + missing escape
**Status:** Verified (live: `--search "value=Doe, John"` → exit 2, “Invalid condition ‘John’”)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`--search` splits its expression on every comma into AND-ed conditions — a useful, tested feature that
is documented nowhere and has no escape, so a value containing a comma fails with a baffling error.  Two
related behaviors are also undocumented: a dictionary key literally named `key`/`value`/`stream`/`regex`
cannot be searched (the word is reinterpreted as an operator), and `KeyEquals` compares the key
case-sensitively while comparing the value case-insensitively.

## Affected code
- `src/search.rs:25` — splits the expression on every comma (AND-ed conditions).
- `src/search.rs:36-51` — operator words tested case-insensitively _before_ falling through to
  `KeyEquals`, with no escape.
- `src/search.rs:97-104` — the key is matched case-sensitively, the value case-insensitively.
- Help: `src/types.rs:108-113`.

## What it does vs. what it should do
None of this is documented. `--search "value=Doe, John"` splits into `value=Doe` and ` John`, and the
second is not a valid condition, so the tool exits 2 with “Invalid condition ‘John’”.  There is no way
to escape the comma.  Separately, `Value=X` becomes `ValueContains` rather than “does `/Value` equal
X”, and `type=font` finds nothing because the key is `/Type` (case-sensitive) even though the value is
matched case-insensitively.

The comma-AND syntax should be documented with an escape (e.g. `\,`, matching `pdf-maker`), and the
key case-sensitivity vs value case-insensitivity should be stated in `--help` and the debugging guide.

## Reproduction
```
pdf-dump f.pdf --search "value=Doe, John"   # exit 2: "Invalid condition 'John'"
pdf-dump f.pdf --search "type=Font"          # finds nothing (key is /Type, matched case-sensitively)
```

Compare each against the help, which mentions none of this.

## Suggested fix
Document the comma-AND syntax and add an escape (`\,`).  Document the key case-sensitivity vs value
case-insensitivity.  Consider an escape or explicit form for keys that collide with operator words.
This is primarily a documentation and small-escape change.

## Why the fix addresses the bug
Documenting the load-bearing syntax and adding an escape removes the silent reinterpretation and makes
the comma and operator-word cases searchable and predictable.

## Related
[[bug-0028-search-nested-values-not-matched]] and [[bug-0033-text-string-utf16be-mishandled]] (search
behavior/coverage).  Doc side overlaps with [[bug-0006-docs-help-and-readme-flag-text-errors]].
