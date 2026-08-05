# bug-0008: Docs stale on capabilities and exit codes — phantom “caution tier”, understated `--text` coverage, missing exit-code table

**Severity:** Medium
**Classification:** SPEC-DOC bug
**Status:** Verified (empirical + git)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary

Five distinct documentation-staleness defects, each of which misleads an agent about what the tool can do or how to interpret its output.  The CHANGELOG announces a “caution tier” that exists nowhere in the code, tests, or help text; README and the AI reference understate the `--text` encoding coverage (naming two of the four base tables and omitting Adobe-Glyph-List `/Differences` resolution entirely); the AI reference claims completeness yet carries no exit-code table; the README implies the `reliability` JSON object is conditional when it is always present; and the CHANGELOG misfiles 0.12.8 content under `[0.12.7]` and defines no released-version link references.  These are all “fixed in code, doc not updated” cases — the code is the ground truth and the docs must change, not the code.

## Affected code

Documentation sites (the defects):

- `CHANGELOG.md:12` — the v0.24.0 entry reads `Migrate to the canonical portfolio exit-code table and add a “caution” tier.`  No caution tier exists anywhere.
- `README.md:56` — the `--text` row says it decodes “`/ToUnicode` CMaps and WinAnsi/MacRoman encodings”, naming two of four base tables.
- `README.md:77` — the reliability paragraph repeats the two-table understatement and says the `reliability` object appears “When extraction is not fully trustworthy”.
- `DEBUGGING_WITH_PDF_DUMP.md:3` — opens claiming the guide “covers everything needed to use the tool and interpret its output”, yet the file has no exit-code table.
- `DEBUGGING_WITH_PDF_DUMP.md:120` — repeats the WinAnsi/MacRoman-only encoding claim.
- `CHANGELOG.md:57-60` — the `[0.12.7]` section body includes 0.12.8’s clippy fixes; only `[Unreleased]` has a link definition (`CHANGELOG.md:71`), so `[0.24.0]`, `[0.23.1]`, etc. are undefined references.

Ground truth (the code the docs must match):

- `src/types.rs:120-133` — the canonical exit-code table (0/1/2/3) in clap’s `after_long_help`; no caution tier, and validation severities are ERROR/WARN/INFO.
- `src/encodings.rs` — four base tables: `winansi`, `macroman`, `standard`, `macexpert`.
- `src/glyphlist.rs` — Adobe Glyph List resolution of `/Encoding /Differences` names.
- `src/text.rs:1064` — the `reliability` object is unconditionally present in `--text --json` output.

## What it does vs. what it should do

1. **Phantom “caution tier”.**  `CHANGELOG.md:12` and the title of commit `f1bb413` announce a caution tier as part of the v0.24.0 exit-code migration.  `grep -rin caution src/ tests/ *.md` finds the word only in the CHANGELOG, and the full `f1bb413` diff contains it only in the commit title.  `--help` documents exit codes 0/1/2/3 only; validation issue severities are ERROR/WARN/INFO.  Either the CHANGELOG line is simply wrong, or a planned feature was silently dropped during the migration and should be tracked — Chris should confirm which before the line is edited, since the two answers imply different follow-up (delete the words vs. open a feature plan).
2. **Understated `--text` encoding coverage.**  `README.md:56`, `README.md:77`, and `DEBUGGING_WITH_PDF_DUMP.md:120` say `--text` decodes via `/ToUnicode` CMaps and WinAnsi/MacRoman tables.  The actual coverage (CHANGELOG 0.15.0–0.17.0, `src/encodings.rs`, `src/glyphlist.rs`) is four base tables — WinAnsi, MacRoman, Standard, and MacExpert — plus Adobe-Glyph-List resolution of `/Encoding /Differences` glyph names.  An agent reading the docs may wrongly distrust correct output from a Standard/MacExpert/`Differences` font, or waste effort “working around” a gap that does not exist.
3. **No exit-code table in the AI reference.**  `DEBUGGING_WITH_PDF_DUMP.md:3` claims the guide covers everything needed to interpret the tool’s output, but the document nowhere presents the exit-code contract — only scattered inline mentions of exit 3 (lines 101, 125, 209).  The complete, empirically-accurate table lives only in `--help` (`src/types.rs:120-133`).  For a tool whose consumers are agents branching mechanically on exit codes (the whole point of the `cli-exit-codes` portfolio rule), the AI reference must carry the table.
4. **Reliability object wrongly described as conditional.**  `README.md:77` says that when extraction is not fully trustworthy the tool “in `--json` mode, adds a top-level `reliability` object”.  In fact `reliability` is always present in `--text --json` output, including for a fully clean document (`src/text.rs:1064`).  `DEBUGGING_WITH_PDF_DUMP.md:118` documents the schema correctly.  A consumer written from the README would treat the key’s presence as a warning signal, which it is not.
5. **CHANGELOG structural errors.**  The `[0.12.7]` section body (`CHANGELOG.md:57-60`) describes clippy 1.95.0 lint fixes explicitly attributed to “(0.12.8)” — content misfiled under the wrong version heading.  Additionally, Keep a Changelog version headings are Markdown link references, and only `[Unreleased]` is defined (`CHANGELOG.md:71`); `[0.24.0]`, `[0.23.1]`, `[0.23.0]`, `[0.22.0]`, `[0.21.0]`, `[0.20.1]`, `[0.20.0]`, and `[0.12.6]` have no link definitions.

## Reproduction

Each item is checkable mechanically from the repo root:

```bash
# 1. Phantom caution tier: only the CHANGELOG mentions it.
grep -rin caution src/ tests/ *.md
git show f1bb413 --stat            # and: git show f1bb413 | grep -i caution
pdf-dump --help                    # exit-code section lists 0/1/2/3 only

# 2. Encoding coverage: four tables plus AGL, not two.
grep -n "pub(crate) fn" src/encodings.rs
ls src/glyphlist.rs

# 3. No exit-code table in the AI reference.
grep -n "Exit code" DEBUGGING_WITH_PDF_DUMP.md    # no table section exists

# 4. Reliability object always present (use any clean PDF).
pdf-dump clean.pdf --text --json   # output contains "reliability" with verdict "reliable"

# 5. CHANGELOG: sed -n '57,60p' CHANGELOG.md shows 0.12.8 content under [0.12.7];
#    grep -n '^\[' CHANGELOG.md shows only the [Unreleased] link definition.
```

Current behavior: each doc statement contradicts the command output next to it.  Correct behavior: the docs read exactly what the code does.

## Suggested fix

All five fixes are documentation edits (plus one question for Chris):

1. Ask Chris whether a caution tier was planned.  If not, delete “and add a ‘caution’ tier” from `CHANGELOG.md:12` (and note the commit title cannot be amended — the CHANGELOG is the record that can).  If yes, keep a corrected line and open a tracking item (feature plan or `TODO.md`) so the dropped feature is not lost silently.
2. Update `README.md:56`, `README.md:77`, and `DEBUGGING_WITH_PDF_DUMP.md:120` to name all four base tables (WinAnsi, MacRoman, Standard, MacExpert) and the Adobe-Glyph-List resolution of `/Encoding /Differences`.
3. Add the canonical exit-code table — copied from `src/types.rs:120-133`, which is the tested source of truth — as a section of `DEBUGGING_WITH_PDF_DUMP.md`, and point the inline exit-3 mentions at it.
4. Reword `README.md:77` so the always-present `reliability` object is described accurately (the banner is conditional; the JSON object is not), matching `DEBUGGING_WITH_PDF_DUMP.md:118`.
5. Move the 0.12.8 content out of the `[0.12.7]` body (either a proper `[0.12.8]` section or reworded attribution), and add link definitions for every released version heading alongside the existing `[Unreleased]` one.

## Why the fix addresses the bug

The docs are the discoverable interface an agent reads before (or instead of) the code; aligning each stale sentence with the tested behavior removes the misdirection at its source.  Copying the exit-code table from `--help` rather than paraphrasing it keeps a single authoritative wording.

## Related

- [[bug-0006-docs-help-and-readme-flag-text-errors]] and [[bug-0007-docs-json-schemas-inaccurate]] — sibling doc-accuracy findings in this batch; fix them in the same documentation pass.
- `cli-exit-codes` portfolio rule — the exit-code contract must be documented where its consumers look; item 3 is that rule applied to the AI reference.
- No existing test pins any of these doc claims; consider a lightweight doc-consistency check only if item 1 turns out to be a dropped feature (otherwise these are one-time edits).
