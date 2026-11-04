# bug-0039: A syntax error in a content stream silently ends text extraction

**Severity:** High
**Classification:** CODE bug (silent wrong output)
**Status:** Verified (found in the review of plan-0006, 3-Nov-2026; Fable agreed, Sol could not determine it from the repo alone)
**Affected version:** v0.29.1, and every version before it

## Summary
lopdf’s `Content::decode` stops at the first token it cannot parse and returns the operations it had as `Ok`.  So the `Err` arm in `text::process_content`, which pushes “Content stream has syntax errors”, does not fire for this, and everything after the bad token is gone with no warning and a `reliable` verdict.

## Reproduction
A page with `/Font /F1` Helvetica and this content:

```
BT /F1 12 Tf 10 700 Td (seen) Tj ET ) >> ] garbage BT /F1 12 Tf 10 600 Td (hidden 7301) Tj ET
```

`pdf-dump f.pdf --text` prints only `seen`, exit 0.  `pdftotext f.pdf -` prints `seen` and `hidden 7301`.  The same content as an annotation’s `/AP /N` stream gives `appearance_text: "seen"`, no warnings, verdict `reliable`, exit 0.

## What it should do
A stream that was not read to its end is a stream not read: say so, and let the verdict show it.  `Reliability` is `Reliable` here only because nothing told it otherwise, so a caller that gates on `--text`’s exit code (Obsidian-Brain’s KCS check) passes a file it read part of.

## Suggested fix
`Content::decode` does not report what it consumed.  Count the operators independently with a tokenizer over the same bytes, and warn when lopdf returned fewer.  plan-0005’s inline-image scanner is such a tokenizer, so it is worth doing once that lands.

## Related
`~/.claude/rules/positive-evidence-of-absence.md`: a read that failed must not look like a read that found nothing more.  The same family as bug-0029.
