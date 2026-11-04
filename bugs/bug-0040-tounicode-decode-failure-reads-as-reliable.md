# bug-0040: A `/ToUnicode` that will not decode reads as no `/ToUnicode`, verdict reliable, exit 0

**Severity:** High
**Classification:** CODE bug (silent wrong output)
**Status:** Verified (live, on v0.31.2, below)
**Affected version:** v0.31.2

## Summary
`text::build_font_decoder` discards the warning from `decode_stream` and parses whatever came back.  A `/ToUnicode` stream that will not decode parses to an empty CMap, so the font falls through to its base-encoding table.  The text printed may be wrong, since a CMap that could not be read is what said how to read it, yet the verdict is `reliable` and the exit is 0.

## Affected code
`src/text.rs`, in `build_font_decoder`: `let (bytes, _warn) = crate::stream::decode_stream(doc, stream);`, then `if !cmap.is_empty()` falls through.

## What it does vs. what it should do
A font whose `/ToUnicode` could not be read is one whose encoding is unknown, not one that has no map.  It should be classified at least `degraded`, so `--text` and `--find-text` exit 3 and say why, and an annotation’s appearance reads below `reliable`.

## Reproduction
A page showing `(A)` in a Type1 Helvetica with `/Encoding /WinAnsiEncoding` and `/ToUnicode 6 0 R`, object 6 being `/Filter /FlateDecode` over the bytes `this is not zlib`:

```
pdf-dump f.pdf --text --json
# text "A", reliability verdict "reliable", exit 0, nothing on stderr
```

Assert: the verdict is not `reliable`, and the exit is 3.

## Same fault elsewhere, lesser
Two other sites drop the warning, with smaller effects.  `summary.rs` adds bytes it could not decode to the decoded-byte total, and `validate.rs` reports “invalid syntax” for a stream it could not decode.  `search.rs` is bug-0029.

## Related
`~/.claude/rules/positive-evidence-of-absence.md`: “a tool that could not _read_ something assumed it was _empty_”.  Found by the rust-reviewer during bug-0004’s review, 4-Nov-2026.
