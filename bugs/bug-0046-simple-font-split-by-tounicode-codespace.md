# Bug: A Simple Font Whose ToUnicode Declares a 2-Byte Codespace Is Read Two Bytes per Code, So Its Text Comes Out as U+FFFD

**Severity:** major (silent data loss in `--text`; the verdict is honest, but the text is gone)

## What happens

`text.rs` builds a `FontDecoder::ToUnicode` for any font with a usable `/ToUnicode` CMap, and takes the code width from the CMap’s codespace (`cmap.byte_width()`).  Only `CodeWidth::Unknown` falls back to the subtype heuristic (2 for CID, else 1).  So a **simple font** (Type1, TrueType, Type3, MMType1, non-CID) whose ToUnicode declares `<0000> <FFFF>` gets `CodeWidth::Fixed(2)`, and `for_each_code` chunks its show strings two bytes at a time.  `(Hello)` becomes the codes `0x4865`, `0x6C6C`, `0x6F`, none of which is in the CMap (its entries are `<48>`, `<65>`, …), so nearly every character decodes to U+FFFD.

Adobe InDesign writes exactly this shape: every simple TrueType font it embeds carries `1 begincodespacerange <0000> <FFFF> endcodespacerange` followed by one-byte `bfchar` entries.  So pdf-dump cannot read the text of most InDesign PDFs.  On the real-world file below, 2,823 of 3,454 codes (82%) failed; the only lines that came through were set in the document’s Type0 (Identity-H) fonts, where a 2-byte codespace is correct.

## What should happen

For a simple font, each byte of a show string is one character code, always (PDF 32000-1 §9.6, simple fonts; only a Type0 font’s `/Encoding` CMap defines multi-byte codes, §9.7.6.2).  A ToUnicode CMap maps codes to Unicode (§9.10.3); it does not define how a simple font’s string is split.  So when the font is not CID-keyed, the decoder should use one byte per code whatever the ToUnicode codespace says, and look each byte up in the CMap.

Preview and Acrobat do this: they select, copy, and extract the text of both files below correctly.  macOS PDFKit (`PDFPage.string`) returns `Hello, World 2026` for the fixture and the full program text for the real-world file.

## Reproduction

`bugs/bug-0046/simple-font-2byte-codespace.pdf` (made by `make-fixture.py` beside it, by hand, not through pdf-maker): one page, `/Type1 /Helvetica /WinAnsiEncoding` with a ToUnicode of codespace `<0000> <FFFF>` and one-byte `bfchar` entries, showing `(Hello, World 2026)`.

```
$ pdf-dump bugs/bug-0046/simple-font-2byte-codespace.pdf --text
--- Page 1 ---
��������6
[WARN] TEXT EXTRACTION RELIABILITY: DEGRADED
  8 of 9 character codes (89%) could not be mapped.
```

Exit 3.  Expected: `Hello, World 2026`, verdict reliable, exit 0.

## Verifying the fix on real files (not to be committed)

Two private files reproduce it in the wild.  **Read them in place; do not copy them into the repo or into a fixture**: they are a choir’s concert programs, with members’ names.

- `/Users/chris/Downloads/8bd31c9d-d23e-45d0-8a04-a3c0664c7347.pdf` (InDesign 22.0, 4 pages).  Today `--text` exits 3 with 82% unmapped.  After the fix, `--text --page 2` should begin `14th Annual` / `Joy to the World` / `Community Concert` / `Susan McCreary Duprey, Artistic Director`, matching PDFKit.  Page 4’s singers and board lists are a raster image (object 31), so that block correctly yields no text.
- `/Users/chris/Chris/App/Obsidian/KCS-Assistant/KCS-Assistant/inbox/attachments/JTTW Sing-along 1.pdf` (same pattern: simple TrueType Cochin with ToUnicode, 81% unmapped today).  `JTTW Sing-along 2.pdf` and `JTTW Sing-along 3.pdf` beside it are likely the same.

## Notes for the fix

- The decision point is the `match cmap.byte_width()` in `text.rs` (around line 935), which already knows `is_cid`.  For `!is_cid`, the width should be `Fixed(1)`; CID fonts keep today’s codespace-driven behavior.
- Test both spellings a simple font’s ToUnicode can carry: one-byte keys (`<48> <0048>`, the InDesign form) and two-byte keys (`<0048> <0048>`).  Both should map byte `0x48` to `H`, which depends on whether `map_code` looks keys up by numeric value.
- A `Variable` codespace on a simple font is the same fault and should get the same one-byte treatment.
- `--text --layout`, `--find-text`, and the `--forms` / `--annotations` appearance-text paths all go through the same decoder, so each should be checked against the fixture.
- Reported from the KCS-Assistant vault on 24-Nov-2026, while proofreading the program PDF above.  Found by comparing pdf-dump’s output with what Preview showed.
