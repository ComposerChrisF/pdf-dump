# Plan: Adopt cli-contract

## Problem

pdf-dump hand-rolls three things that `cli-contract` now implements and tests once for the whole portfolio:

- **BrokenPipe:** the `wln!` and `w!` macros in `src/lib.rs` exit 0 on a closed pipe, and exit 1 on any other write error.
- **Exit codes:** bare literals throughout, such as `std::process::exit(2)` for a usage fault in `run` and `exit(3)` for findings.
- **One output file:** `--extract-stream N --output PATH` writes with `std::fs::write` (`src/lib.rs:302`).  It has no up-front check of the output’s parent, so a missing directory fails only after the document has been parsed.  It also replaces an existing file without a word.

Until 31-Oct-2026, pdf-dump could not adopt the crate, because a crates.io release cannot depend on a path or git dependency.  cli-contract 0.6.0 is now on crates.io, and making this adoption possible was the main reason it was published (cli-contract-history’s plan-0003 § Phase 6, step 5).

## Proposed Change

Depend on `cli-contract = "0.6"` from crates.io, with no `path`.  Then:

- **Exit codes:** `cli_contract::{OK, TOOL_ERROR, USAGE, FINDINGS}` in place of the literals.  pdf-dump’s own exit table in `after_long_help` (`src/types.rs:94`) stays, since it says what each code means in pdf-dump’s terms.  Check it against `cli_contract::HELP_BLOCK` so that the two cannot disagree.
- **BrokenPipe:** every pdf-dump mode is read-only, which is the case `cli_contract::reset_sigpipe()` exists for: call it once in `run`, and the macros reduce to a plain `writeln!` with the non-pipe error path.  Or keep the macros and have them use `cli_contract::is_broken_pipe`.  Either way, a `pdf-dump … | head -1` test pins exit 0 and an empty stderr.
- **`--extract-stream --output`:** run `cli_contract::check_output_file` before the document is loaded, and `prepare_output_file` before the write, both with `Clobber::Allow`: an existing file is overwritten, today’s behavior.  Chris ruled this on 2-Nov-2026.  The file is named explicitly on the command line, which is when tools commonly replace without asking, and a stream can always be extracted again.

## Implementation Notes

- Read cli-contract’s 0.6.0 `CHANGELOG.md` and its `pipe` and `paths` module docs first.
- **The input PDF path:** check whether its existing probe already gives three answers before replacing it with `require_input_path`.  If it does, the change is the message wording only.
- **Exit codes:** pdf-dump’s exit-1 cases for a missing object (`src/types.rs:125`–`128`) are tool errors under the table and stay 1.
- **Plan-0005** (export images as viewable files) will write output files too.  Land this plan first, so that plan builds on `prepare_output_file` rather than a second `fs::write`.

## Why Not a Workaround

A workaround is what exists today: hand-written copies of a contract that the portfolio has already centralised, one of them (`--output`) missing the up-front check.  `cli-contract` is the deterministic home for it (`strategy-defaults.md`, home 4: a contract between tools), and since 0.6.0 it is on crates.io, so pdf-dump can use it without giving up its own publishing.
