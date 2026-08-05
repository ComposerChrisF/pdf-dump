# bug-0011: `--find-text` reports “No matches” with exit 0 on a document whose text extraction is unreliable

**Severity:** High
**Classification:** CODE bug (contract gap; partial SPEC-DOC scope gap)
**Status:** Verified (live: on a CID-without-ToUnicode doc, `--find-text` prints “No matches”, exit 0, no banner, while `--text` banners and exits 3)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`--find-text` extracts page text through the same font-aware pipeline as `--text`, but discards the
per-page reliability data.  On a document whose extraction is Unreliable (a CID/Type0 font without a
`/ToUnicode` map), `--text` prints a loud banner and exits 3, while `--find-text "word"` searches the
same garbage text, prints `No matches for "word".`, and exits 0 with no banner — the silent,
plausible, wrong-output shape the tool’s own contract exists to prevent.

## Affected code
- `src/find_text.rs:13-67` — extracts via `extract_text_from_page_with_warnings` but throws away
  `result.fonts` and the reliability counters.
- `src/lib.rs:424-431` and `src/lib.rs:488-496` — `DocMode::FindText` hard-codes `had_issues = false`,
  so the run can never exit 3 from `--find-text`.

## What it does vs. what it should do
The per-page reliability is already computed and available; `--find-text` just does not consult it.  A
caller who runs `--find-text` on an undecodable document concludes the word is absent, when the truth
is that the text could not be decoded at all.  Per this repo’s exit-code rules and the
positive-evidence-of-absence principle, searching undecodable text should print the reliability
banner (as `--text` does) and exit 3, rather than returning a confident “No matches”.

`DEBUGGING_WITH_PDF_DUMP.md` currently promises reliability signaling only for `--text`, so part of
the remedy is documenting that `--find-text` now shares it.

## Reproduction
Reuse the CID-without-ToUnicode fixture from [[bug-0012-font-dedup-drops-unreliable-verdict]]
(`dedup.pdf`, or any doc with a Type0 font lacking `/ToUnicode`):

```
pdf-dump dedup.pdf --find-text "A"
# currently: "No matches for \"A\".", exit 0, no banner
# expected:  reliability banner on stderr, exit 3 (search over undecodable text is not authoritative)
```

Assert the run exits 3 and prints the banner when the document verdict is Unreliable.

## Suggested fix
Thread the per-page reliability into `find_text` (reuse `document_verdict`); when the verdict is
Unreliable, print the reliability banner as `--text` does and return `had_issues = true` so `run()`
exits 3.  Update `DEBUGGING_WITH_PDF_DUMP.md` to state that `--find-text` shares the reliability
signaling.

## Why the fix addresses the bug
Reusing the already-computed verdict makes `--find-text` honor the same “unreliable → loud + exit 3”
contract as `--text`, so a search over undecodable text is no longer reported as a confident absence.

## Related
Depends on [[bug-0012-font-dedup-drops-unreliable-verdict]] (the verdict must be correct first).
`~/.claude/rules/cli-exit-codes.md`, `~/.claude/rules/positive-evidence-of-absence.md` (an
unreadable text is not an absent word).
