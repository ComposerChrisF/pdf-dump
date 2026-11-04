# bug-0041: A sound inline image delimited by tabs makes lopdf fail the whole stream, so the page’s text is lost

**Severity:** Low
**Classification:** CODE bug (upstream fault, not worked around)
**Status:** Verified (fixture run through v0.31.3 and the follow-up build beside poppler, 4-Nov-2026)
**Affected version:** v0.31.3

## Summary
lopdf needs a space, LF or CR after an inline image’s `EI`, while PDF allows any white space.  So a filtered or named-colour-space image delimited by tabs turns `Content::decode` into `Err`, and `--text` prints nothing for that page.  It is loud (exit 3, “Content stream has syntax errors”), so nothing reads as clean, but the file is sound and poppler reads it.

## Reproduction
A page whose content is:

```
BT /F1 12 Tf (secret) Tj ET BI /W 1 /H 1 /BPC 8 /CS /Cs1 ID a<TAB>EI<TAB>q Q
```

```
pdf-dump f.pdf --text    # no text, exit 3
pdftotext f.pdf -        # secret
```

Assert: `--text` prints `secret`, exit 0.

## Notes
`inline_image::decode_content` already rewrites a stream’s bytes for lopdf, to blank an inline image that panics it and to add its end marker.  The scanner there finds this image’s end correctly, so one fix is to normalise the white space after `EI` in the same pass.  The review of the v0.31.4 fixes kept this out of that commit under `minimal-fix.md`; the same fault’s other shapes, such as an `EI` that ends the stream, were fixed there as a side effect of the marker.
