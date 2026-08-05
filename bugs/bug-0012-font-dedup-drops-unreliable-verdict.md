# bug-0012: Font deduplication drops a conflicting Unreliable record, certifying garbage extraction as “reliable”

**Severity:** High
**Classification:** CODE bug (silent garbage certified reliable, exit 0)
**Status:** Verified (live repro: `pdf-dump dedup.pdf --text --json` reports `"verdict":"reliable"` though page 2 is CID-without-ToUnicode)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`dedup_font_records` keys deduplication on `name|base_font|subtype`, excluding the reliability
classification.  Two different font objects that share a resource name, BaseFont, and Subtype collapse
to whichever was seen first.  When the Reliable instance is first, the Unreliable record — the only
thing that drives the exit-3 “unreliable” verdict — is silently discarded, so a document whose text
is genuinely undecodable is certified reliable with exit 0.

## Affected code
- `src/text.rs:1070-1080` — the dedup key (`name|base_font|subtype`) omits `classification`.
- `src/text.rs:1092-1104` — the document verdict consumes the deduped list, so a dropped Unreliable
  record cannot influence it.

## What it does vs. what it should do
The same subset font emitted twice by a writer, with one instance missing its `/ToUnicode`, is
entirely realistic.  With the current key, the two records dedup to one; if the Reliable one wins, the
Unreliable classification vanishes and the verdict is `reliable`.  The coverage safety net (the
“> 20% of shown codes unmapped → Degraded” downgrade) does not catch it when the undecoded bytes
happen to be valid UTF-8 (e.g. `\x00A` produces zero `U+FFFD`).

Deduplication should preserve the _worst_ classification per key (any Unreliable instance survives),
or include `classification`/reason in the key so conflicting instances are not merged.

## Reproduction
Two pages, both referencing `/F1` with BaseFont `ABCDEF+Custom` and Subtype `Type0`.  Page 1’s font
has a valid `/ToUnicode`; page 2’s font is a separate object with none (so page 2 emits raw CID bytes
like `\x00A`).

```
pdf-dump dedup.pdf --text --json
# currently: one font entry, "has_to_unicode": true, "verdict": "reliable", exit 0
```

Fixture `dedup.pdf` is in the review scratchpad.  Assert the verdict is `unreliable` (and the run
exits 3) because page 2’s font is CID-without-ToUnicode.

## Suggested fix
Change `dedup_font_records` to keep the worst classification per key (fold Unreliable over Degraded
over Reliable), or add `classification` to the dedup key so a Reliable duplicate cannot mask an
Unreliable one.

## Why the fix addresses the bug
Preserving the worst per-key classification ensures any Unreliable font instance survives dedup and
drives the document verdict, so undecodable text is reported as unreliable (exit 3) instead of clean.

## Related
[[bug-0011-find-text-unreliable-silent-success]] uses the same reliability verdict — fix this one
first so the verdict `--find-text` reuses is correct.  See the reliability design in `docs/ROADMAP.md`.
