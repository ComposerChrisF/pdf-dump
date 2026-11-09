# Bug: Text Placed Glyph by Glyph Loses Its Lines and Words, With a Reliable Verdict

**Severity:** high.  The output looks plausible, the verdict says `reliable`, and `--find-text` reports “No matches” with exit 0 for a word that is on the page.

## Symptom

Some generators place every glyph on its own, each in its own `BT … ET` block, and separate words by position, with no space glyph.  One brokerage’s statement generator does this.  On real statements of that kind (not shareable, so no sample exists), plain `--text` wrote 70–93% of all lines as a single character, with no spaces between words.  The verdict named only some unrelated `/Differences` names.  Nothing said that the line structure was lost.

`--find-text` reads the same text, so a word drawn this way is never found.  Its `--help` says that “no matches” is not evidence of absence only when the text was not reliably decodable.  Here it was decodable, so exit 0 with “No matches” claims an absence that is false.

## Reproduction

`bug-0044/make-fixtures.py <dir>` writes three synthetic one-page PDFs: Courier with `/Widths`, two lines of words with word gaps and no space glyphs.  The only difference between them is how each glyph is positioned (pdf-dump v0.34.0):

| Fixture | Each glyph placed by | Plain `--text` | `--find-text Statement` | `--text --layout` |
|---|---|---|---|---|
| `bt-per-glyph.pdf` | its own `BT /F1 12 Tf 1 0 0 1 x y Tm (c) Tj ET` | one glyph per line | **No matches**, exit 0 | `Statement  Period` / `Account 1234  5678` |
| `tm-per-glyph.pdf` | one `BT`, a `Tm` per glyph | `StatementPeriodAccount12345678` (one line) | found; phrase with a space not found | correct |
| `td-per-glyph.pdf` | one `BT`, a relative `Td` per glyph | `StatementPeriod` / `Account12345678` | found; phrase with a space not found | correct |

Every plain run exits 0 with nothing on stderr.

## Cause

`text.rs::process_content` decides lines and spaces from operators alone, never from positions:

- **`BT` always starts a new line** (unless the text already ends in one).  A `BT` per glyph is therefore a line per glyph.
- **`Tm` is ignored**, so a `Tm` that moves to a new line joins it to the last one, and one that moves along the line adds no space.
- **`Td`/`TD` break a line only when `ty < 0`**, and never add a space for a horizontal gap.
- **A space appears only for a `TJ` offset below −100.**  A word gap made by positioning is invisible.

`--layout` tracks the text matrix, the CTM and glyph advances, groups runs into lines by baseline, and places them by x.  So it reads all three fixtures correctly.

## Describing a Case Without the Document

This prints only operator names and their counts per page, never an operand.  It lets someone who may not share a file describe its pattern; `BT` ≈ `Tj` is the signature of the first fixture:

```
pdf-dump f.pdf --operators --json | jq -c '.pages[] | {page: .page_number, ops: ([.operations[].operator] | group_by(.) | map({(.[0]): length}) | add)}'
```

It covers page content streams only, not form XObjects.

## Fix

The root fault is that the extractor decides lines and word breaks without looking at where glyphs land.  The fix belongs in the extractor that `--text` and `--find-text` share.  There are two directions, and they change plain `--text` for every caller:

- **A. Lines and spaces from geometry.**  Break a line when the baseline moves, never merely because a `BT` begins, and add a space where the gap to the previous glyph is wide.  This is the machinery `--layout` already has (`layout.rs`), but plain mode keeps content order and draws no grid.  It needs glyph advances, so a font without widths falls back to today’s operator rules for spacing.  Output changes wherever a document splits one visual line across `BT` blocks, which is common, and that is mostly an improvement.
- **B. Plain stays operator-driven, and says so.**  Count line breaks forced by `BT` or `Tm` while the baseline did not move, and make the verdict `degraded` (exit 3) past a threshold, so a caller can tell.  `--find-text` then exits 3 too.  Output is unchanged, but the text is still unusable for such files, and callers are pointed at `--layout`.

Either way, the test pins all three fixtures: plain lines and words (A) or the verdict (B), and `--find-text Statement` on `bt-per-glyph.pdf` never exits 0 with no match.
