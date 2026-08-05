# bug-0002: A malformed pair in a CMap section skips the section-end keyword and destroys later valid sections

**Severity:** Medium
**Classification:** CODE bug (robustness on malformed input)
**Status:** Verified (code-trace)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
The `/ToUnicode` CMap parser advances by two tokens when it hits an unexpected token inside a
`codespacerange` or `bfchar` section.  Because that two-token jump can step over the section’s
terminating keyword, one stray token in an early section makes the parser run past the section end
and consume the tokens of later, valid sections — so the whole CMap parses empty and the font falls
to passthrough/Unreliable.  This violates the module’s documented lenient-degradation contract (fewer
mappings, not destroyed sections).

## Affected code
- `src/cmap.rs:174-182` — `parse_codespace`: after `Token::Hex(lo)`, if `tokens.get(i+1)` is not
  `Hex`, it does `i += 2`, which can skip `endcodespacerange`.
- `src/cmap.rs:196-199` — `parse_bfchar`: the same `i += 2` pattern.

## What it does vs. what it should do
Given a stray unpaired bound, the parser should re-examine the next token (advance by 1) so a
terminating keyword is never skipped.  Instead, advancing by 2 jumps over the keyword and the parser
keeps scanning into the following section, swallowing its content as if it were part of the malformed
section.  The damage is non-local: one bad token early erases every later valid section.

## Reproduction
CMap token stream (one stray unpaired bound in the codespace section):

```
begincodespacerange <00> endcodespacerange
beginbfchar <41> <0041> endbfchar
```

The codespace parser skips `endcodespacerange` and consumes the bfchar’s `<41> <0041>` as a codespace
pair, so the bfchar mapping (code `0x41` → `A`) is lost.  Build a `/ToUnicode` stream with that content,
attach it to a simple font, and `--text` should still map `0x41` to `A` but does not.

Unit test: feed that token sequence to the section parser and assert the bfchar mapping survives.

## Suggested fix
On a non-`Hex` partner in `parse_codespace` and `parse_bfchar`, advance the index by 1 (re-examine the
token) rather than 2, so a terminating keyword is always seen and the parser stops the current section
instead of overrunning it.

## Why the fix addresses the bug
Advancing by one keeps the section-end keyword visible, confining the damage to the single malformed
pair and preserving every later valid section.

## Related
The module’s documented contract is lenient degradation; this restores it.  See `docs/ROADMAP.md`
(`--text` reliability machinery).
