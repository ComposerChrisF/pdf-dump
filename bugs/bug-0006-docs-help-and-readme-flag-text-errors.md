# bug-0006: `--help` / README / CLAUDE.md flag-text and module-table errors

**Severity:** Low
**Classification:** SPEC-DOC bug
**Status:** Verified (empirical)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
Several pieces of flag documentation are inaccurate: the `--page` help describes a mode it no longer
implements, the `--hex` help understates where it applies, the `regex=` search help omits stream
content, the README `--depth` row lists a non-consumer, and the CLAUDE.md module table cites the wrong
files/functions for two modes.

## Affected code / docs
- `src/types.rs:219` — `--page` help.
- `src/types.rs` — `--hex` help.
- `src/types.rs:94+` (`after_long_help`) — the `regex=` description.
- `README.md:106` — the `--depth` row.
- `CLAUDE.md` — the mode → module table.

## What it does vs. what it should do (each a distinct inaccuracy)
1. `--page` help reads “Dump the object tree for a specific page or range” — stale pre-v0.12 wording;
   it shows page info and filters the content modes, and never dumps an object tree.
2. `--hex` help says “use with `--decode`”, but it also works with `--raw` (the debugging guide’s
   `--object 42 --raw --hex` row is correct).
3. `after_long_help`: “`regex=<pattern>` Any key, Name, or String value matches the regex” — regex also
   matches decoded stream content (`src/search.rs:129-152`; `DEBUGGING_WITH_PDF_DUMP.md:59` correctly
   says “keys, values, and streams”).
4. `README.md:106` `--depth` row says “(with `--tree`, `--tags`, `--json`)” — `--json` is not a
   `--depth` consumer; only `tree.rs`/`structure.rs` read it (`--object N --depth 0` is identical to
   `--object N`). `DEBUGGING_WITH_PDF_DUMP.md:46` is right.
5. `CLAUDE.md` mode table: the `--list` row cites `object.rs` but the code is in `summary.rs`
   (`lib.rs:413,454`); the `--inspect` row cites `print_inspect` but the function is `print_info`
   (`inspect.rs:522`).

## Reproduction
Read each doc line against the cited code, or run the commands (e.g. `--object N --depth 0` vs
`--object N`; `--object 42 --raw --hex`).

## Suggested fix
Correct each string: `--page` help → e.g. “Show info for a specific page or range (also filters
`--text`/`--operators`/`--annotations`/`--find-text`)”; `--hex` help → “use with `--decode` or
`--raw`”; `regex=` help → add “or decoded stream”; README `--depth` → “(with `--tree`, `--tags`)”;
CLAUDE.md → fix the two module cells.

## Why the fix addresses the bug
The docs are the discoverable interface for an agent; correcting the flag text stops it from
misdirecting callers about what each flag does and where.

## Related
Broader doc staleness is in [[bug-0007-docs-json-schemas-inaccurate]] and
[[bug-0008-docs-stale-capabilities-and-exit-codes]]. `--page` behavior itself is
[[bug-0025-page-modifier-accepted-but-ignored]].
