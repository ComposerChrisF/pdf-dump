# bug-0033: UTF-16BE / non-UTF-8 PDF text strings are mangled in display and unmatched in search

**Severity:** Medium
**Classification:** CODE bug (output fidelity + text/JSON mismatch) + SPEC-CODE mismatch (search) — **SPEC DECISION REQUIRED** on scope
**Status:** Verified (live: `fixture.pdf --object 6` shows a UTF-16BE Title “Hello”; `--search value=Hello` → “Found 0”)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
PDF text strings are commonly UTF-16BE (with a `FE FF` BOM), the spec encoding for metadata like
`/Title`.  The tool never decodes them: display runs `from_utf8_lossy` and shows mojibake in
authoritative PDF-literal syntax, JSON destroys hex strings, text and JSON disagree, and search never
matches such strings — so document metadata cannot be read or searched.

## Affected code
Display:
- `src/object.rs:139-148` — text mode: literal strings printed lossy.
- `src/object.rs:410-412` — JSON: `Object::String(bytes, _)` ignores the format and uses
  `from_utf8_lossy` for both literal and hex strings.

Search:
- `src/search.rs:97-117` — `KeyEquals`/`ValueContains` compare raw bytes.
- `src/search.rs:131-144` — `regex=` uses `from_utf8(...).is_ok_and(...)`, so a non-UTF-8 value
  silently never matches.

## What it does vs. what it should do
Per PDF 32000-1 §7.9.2.2, a text string beginning with the UTF-16 BOM `FE FF` is UTF-16BE.  Display
should decode it (or preserve the bytes losslessly), and search should match against the decoded form.
Currently: text mode prints `/Title (þÿ H e l l o)`-style mojibake; JSON emits the same for both
literal and hex strings with no `format` field and no lossy marker; and `value=Hello` never matches a
UTF-16BE `/Title` while `regex=` skips it entirely — even though `regex=` _does_ match stream content
lossily, an inconsistent asymmetry.

**SPEC DECISION REQUIRED** (do not blindly code-fix): decide the display/search contract — decode
UTF-16BE to Unicode, and/or provide a lossless byte fallback (octal/hex escape or a parallel
`value_hex`), plus a lossiness flag when neither UTF-8 nor UTF-16BE applies.  Decide whether search
matches the decoded form, the raw bytes, or both.

## Reproduction
Fixture object 6 has a UTF-16BE `/Title` “Hello” (`FE FF 00 48 00 65 …`):

```
pdf-dump fixture.pdf --object 6            # text + json: mojibake, no format/lossy marker
pdf-dump fixture.pdf --search value=Hello  # "Found 0 matching objects."
```

Fixture `fixture.pdf` is in the review scratchpad.  Assert the Title renders as `Hello` (or is
recoverable losslessly) and that `value=Hello` matches it.

## Suggested fix
Add a shared “decode PDF text string” helper that detects the UTF-16BE BOM and decodes per §7.9.2.2,
with a lossless byte fallback and a lossiness flag.  Use it in display (`object.rs` text + JSON) and in
search matching, so metadata renders correctly, text and JSON agree, and `value=`/`regex=` match the
strings users actually search.

## Why the fix addresses the bug
Decoding UTF-16BE (with a lossless fallback) makes metadata render correctly, keeps text and JSON
consistent, and lets the search conditions match the strings users care about.

## Related
`~/.claude/rules/positive-evidence-of-absence.md`: a value the matcher could not decode is Unknown,
reported as “no match”.  Folds in the search-side finding (UTF-16BE values never match) with the
display-side finding.
