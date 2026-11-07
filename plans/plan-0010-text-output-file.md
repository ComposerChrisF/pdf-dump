# Plan: `--text --output <file>`, Written Atomically

## Problem

`--text` (and `--text --layout`) prints only to stdout.  A build tool that runs pdf-dump as a step often cannot redirect a step’s stdout to a file: it captures it instead, echoes it in verbose mode, and copies it into an error log when the step fails.  When the text is sensitive (account numbers, on its way to a redaction stage), it must never pass through a captured stream; it has to go straight from pdf-dump to a file.

Asked for by request `id-redact/r0003` (6-Nov-2026), for a pipeline of the shape `file.pdf` → `file.pdf.txt` → redaction.

## Proposed Change

- **`--text [--layout] [--layout-cell PT] [--page …] --output <file>`** writes the text to the file, byte for byte what stdout carries without `--output`, and prints none of it to stdout.  The reliability banner and warnings stay on stderr.
- **The exit code is unchanged.**  A degraded or unreliable verdict, or an encrypted file that could not be decrypted, still exits 3 and still writes the file, as stdout gets the text today.
- **The write is atomic**: the text goes to a new temporary file in the output’s directory (`.<name>.pdf-dump-<pid>.tmp`, created with `create_new`, so it never clobbers anything), is synced, and is renamed over the output.  A run killed part way leaves the output as it was, never a partial file under its name — a build tool deciding staleness by mtime would otherwise take a partial file as up to date.  A failure to write or rename removes the temporary file this run created and exits 1.
- **Exit 1 writes nothing.**  Every exit-1 path (unreadable PDF, wrong password, `--page` beyond the end, a failed write) runs before the rename.
- **The output directory must exist** (exit 1 otherwise, checked before the document is read, through `cli_contract::check_output_file` with `CreateMode::None`), and an existing file is replaced — what `--output` already means for streams and images.
- **Scope**: `--output` combines with `--text` alone.  With `--json`, or with any other document-level mode beside `--text`, it is a usage error (exit 2).  The pipeline needs only the plain-text form; JSON to a file can be added later if a caller asks.

## Implementation Notes

- `src/types.rs`: `--output` currently `requires = "export"`; it gains a group naming the three extract flags and `--text`, so `--output` alone (or with `--fonts`) is still clap’s usage error.  Help text for `--output` and the exit-code table grow the new cases.
- `src/lib.rs` `run()`: resolve the text output path beside the standalone one, run the same pre-load path check, and, for the text mode, dispatch into a `Vec<u8>` and write it atomically after dispatch.  Writing to a buffer also means no stdout write error can cut the text short.
- An existing output that is a symlink is replaced by a regular file (rename replaces the link, not its target).  Acceptable for a generated text file; noted in `--help`.
- Tests in `tests/integration.rs`: bytes equal stdout’s (plain and `--layout --page`), stdout empty; degraded verdict exits 3 with the file written; missing directory exits 1 before the load and creates nothing; `--page` beyond the end exits 1 and leaves an existing output untouched; replacing an existing file leaves no temporary behind; `--json` or another mode with `--output` exits 2.

## Why Not a Workaround

A shell wrapper (`sh -c 'pdf-dump … > file'`) would move the redirect into a script the build tool must carry for every step, would not be atomic (a killed run leaves a truncated file with a fresh mtime), and would let the exit code and the write disagree.  The atomic, nothing-on-exit-1 contract belongs in the tool that knows when its output is complete.
