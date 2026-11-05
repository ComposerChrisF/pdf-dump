# bug-0043: `--annotations` and `--forms` text output does not escape what could rewrite a terminal

**Severity:** Low
**Classification:** CODE bug
**Status:** Verified (invented fixture, on the build after v0.32.0, 4-Nov-2026)
**Affected version:** v0.32.0

## Summary
`DEBUGGING_WITH_PDF_DUMP.md` § Text strings says “Text output escapes what could rewrite a terminal, such as `\u{d}`; JSON has the text as it is.”  `--object`, `--inspect` and `--tree` do.  In `--annotations` text output a comment and its rich text print raw, and in `--forms` a value does, so a carriage return or a bidirectional override in a PDF can overwrite or reorder what a person reads.

## Reproduction
A page with an annotation `<</Subtype/Text/Rect[0 0 9 9]/Contents(a\rb)/RC(<p>c\rd</p>)>>`, and a form with a field `<</T(f)/FT/Tx/V(e\rf)>>`:

```
pdf-dump f.pdf --annotations    # a raw CR in the Contents column and in “rich text:”
pdf-dump f.pdf --forms          # a raw CR in the value column
```

Assert: none of those lines holds a raw CR, and the JSON still has the text as it is.

## Notes
- `text_string::printable` is the escape.  The strings `--annotations` and `--forms` added beside these, `author`, `subject`, `tooltip`, `options` and `captions`, already call it.
- Named from reading the two `print_*` functions and running those three strings.  An appearance’s text, a target, a field name, and XFA data in the same two functions were not run, and each wants checking.  `--text` is another matter: it reports what a page says, and is not covered by that sentence as far as this report looked.
