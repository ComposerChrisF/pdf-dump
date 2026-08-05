# bug-0026: Stream recovery truncates at the first embedded `endstream` and reports the truncation as a successful repair

**Severity:** High
**Classification:** CODE bug (silent wrong output inside the feature whose purpose is making silent loss loud)
**Status:** Verified (agent demonstrated end-to-end against the debug binary)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
The malformed-`/Length` recovery finds the closing `endstream` keyword with a bare first-match search
and no validation.  When a stream body itself contains the bytes `endstream` (a string or comment in
an uncompressed content stream, or a coincidence in binary data), the recovery cuts the body there,
promotes the truncated prefix as the “recovered” stream, and prints a banner asserting the truncated
length as the true body length — with exit 0.  The feature that exists to make silent content loss
loud instead produces silent, wrong content while claiming success.

## Affected code
- `src/recover.rs:187` — `let mut body_end = body_start + find_subslice(&region[body_start..], b"endstream")?;`
  the closing-keyword search: bare first match, no boundary or context check, and searching
  `region` (unbounded by `endobj`) rather than `head`.
- `src/recover.rs:159-198` — `extract_stream_body`.
- `src/recover.rs:229-254` — `recovery_banner`, which reports the truncated length as `actual body N bytes`.

## What it does vs. what it should do
The _opening_ `stream` keyword is validated three ways (a preceding delimiter, that it is not the
tail of `endstream`, and that it is followed by `CR?LF` per PDF 32000-1 §7.3.8.1), and the opening
search is bounded to this object via its `endobj`.  The _closing_ `endstream` search has none of
that: it accepts any occurrence, even mid-token (`xendstreamy`) and even inside the body’s own bytes.
The first embedded `endstream` therefore ends the body prematurely; the truncated prefix is stored as
the stream and the banner asserts its length as fact.

The closing keyword should be chosen with the same rigor: prefer an `endstream` followed (after
optional whitespace/EOL) by `endobj`, the next `N G obj` header, or EOF; require token boundaries;
and bound the search to this object’s region (`head`, not `region`).  When multiple candidate
`endstream`s exist, the banner should say so rather than asserting the first as authoritative.

## Reproduction
Build a page whose content stream embeds the literal bytes `endstream` and has more content after it,
then corrupt its `/Length` so lopdf drops it to a bare dictionary (the technique in
`tests/integration.rs` `create_malformed_length_pdf`, ~line 2641):

```
content = b"BT\n/F1 24 Tf\n72 700 Td\n(before endstream after) Tj\n72 600 Td\n(TAILTEXT) Tj\nET"
```

Set `/Length` to a run of `9`s (overshoot), then run `pdf-dump f.pdf --text`.

- Current: banner reports `actual body 31 bytes` (true body is ~79 bytes); `--text` prints
  `--- Page 1 ---` with no text (the body is cut mid-string-literal so content parsing fails).
- Expected: recovered body length equals the full content length, and stdout contains `TAILTEXT`.

Assert on the recovered body length and that the tail text appears.

## Suggested fix
In `extract_stream_body`, validate the closing `endstream`: require a token boundary and prefer a
candidate followed by `endobj` / the next object header / EOF; bound the search to `head` (this
object) not `region`.  When more than one candidate exists in the region, record that in the recovery
record so the banner can flag the ambiguity rather than silently choosing the first.

## Why the fix addresses the bug
Validating the closing keyword’s context stops the body being cut at a byte coincidence, so the
recovered stream is the real body (or the ambiguity is surfaced) instead of a truncated prefix
mislabeled a successful recovery.

## Related
`~/.claude/rules/positive-evidence-of-absence.md`: a truncated recovery reported as success is an
Unknown dressed as a confident answer.  Same module’s opening-keyword guards are the model to copy.
