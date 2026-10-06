# bug-0038: a `/ToUnicode` CMap with holes for codes the page shows is rated `reliable`

**Severity:** Medium (silent text corruption under a clean verdict — the failure the verdict exists to prevent)
**Classification:** CODE BUG — contradicts the documented contract
**Status:** Verified (synthetic fixture, and the real-world file it was found in)
**Affected version:** v0.29.0 (HEAD 74e3e6a)

## Summary

A font whose `/ToUnicode` CMap omits codes the content stream actually shows decodes those codes to U+FFFD, yet is classified `reliable`, and the document verdict stays `reliable` with exit 0 unless more than 20% of all codes are unmapped.  Mid-word gaps therefore corrupt words — `Administra�ve`, `Commi�ee` — while `--text` exits 0, `--find-text` reports a clean search, and `--json` says `"verdict": "reliable"`.

This contradicts the contract in `README.md` and `--help`: the verdict is `degraded` for “a font whose encoding is only partly known”.  A ToUnicode map that lacks entries for codes in use is exactly a partly known encoding.

## Where it was found

Board-meeting agendas exported from Apple Pages in 2023 (Calibri, `Producer: macOS Version 11.6 … Quartz PDFContext`).  Quartz gives the `ti` and `tt` ligature glyphs their own codes (`<2a>`, `<35>`) and writes no ToUnicode entry for them, while `fi` is mapped correctly (`<4b>` → U+FB01).  One page: 1,254 codes, 21 unmapped (1.7%), verdict `reliable`, exit 0 — and roughly one word in seven damaged (“administra”, “commi”, “a�endance”).  The real files are an organisation’s board papers and are not committed; the synthetic fixture below reproduces the fault exactly.

**Second symptom, real file only (not reproduced by the fixture):** in the Pages export, each U+FFFD is emitted on a line of its own, so a damaged word is also split across three lines (`'5)   Administra\n�\nve Support Specialist'`).  `--find-text "Administra"` still finds the fragment, but any search spanning the word cannot.  The fixture shows a single `Tj` and keeps the word on one line, so this probably comes from the Quartz file’s glyph positioning (an unmapped glyph with no known advance width?) and may be a separate defect.

## Reproduction

`bugs/bug-0038/ligature-gap.pdf`, built by `bugs/bug-0038/make-fixture.sh` (hand-written, uncompressed, correct xref; `pdf-dump --validate` finds no issues).  One TrueType font; its ToUnicode CMap maps `<20>–<29>` and `<2B>–<7E>`, so code `0x2A` alone is unmapped.  The page shows `(Administra*ve Commi*ee Report)`: 2 of 29 codes unmapped (7%).

```
$ pdf-dump bugs/bug-0038/ligature-gap.pdf --text
--- Page 1 ---
Administra�ve Commi�ee Report
$ echo $?            # 0 — expected 3
$ pdf-dump bugs/bug-0038/ligature-gap.pdf --text --json | jq .reliability
  … "total_codes": 29, "unmapped_codes": 2, "verdict": "reliable",
    fonts[0].classification: "reliable"
```

**Expected:** verdict `degraded`, exit 3, the stderr banner naming the font and the unmapped codes.  The text is still printed, as for any `degraded` result.

## Affected code

- `src/text.rs:772–777`: a font with a parsed `/ToUnicode` CMap is recorded `Reliability::Reliable` statically, before any code is decoded.
- `src/text.rs:1251–1266`, `document_verdict`: the only usage-aware downgrade is the document-wide `LOW_COVERAGE_THRESHOLD = 0.20` (`src/text.rs:14`).  A handful of unmapped codes in a long page never reaches it.  The function’s own comment argues that “a suppressed [downgrade] costs silently wrong text”, which is what happens here below the threshold.

## Suggested fix

Make the ToUnicode classification usage-aware, per font: a font decoded through ToUnicode that emitted **any** U+FFFD for a code the content showed is `degraded`, with a reason such as “ToUnicode has no entry for N code(s) shown: 2A”.  The per-decode counters `emit_show_string` already keeps are the natural source.  Leave the 20% document-wide net in place for the other decode paths, or revisit it separately; that is a design question, and this fix does not depend on it.

**Pin:** this fixture → verdict `degraded`, exit 3, for `--text` and for `--find-text`, which shares the verdict (bug-0011).  Plus a guard that a fully mapped ToUnicode font stays `reliable`.  Confirm the pin fails with the fix reverted.

## Why it matters

The README’s case for the verdict is that “an extraction that silently mangled the account-number line looks exactly like a clean one” under other tools.  Here it looks exactly like a clean one under this tool.  A downstream consumer that trusts `reliable` — one comparing a PDF’s text with its Word master, for instance, which is how this was found — gets corrupted words with no warning.
