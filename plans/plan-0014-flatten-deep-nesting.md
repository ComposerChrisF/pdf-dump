# Plan: Flatten the Four Blocks Nested More Than Six Deep

## Problem

The lint set’s `clippy.toml` sets `excessive-nesting-threshold = 6` (adopted 16-Nov-2026), which switches on clippy’s `excessive_nesting`: a block more than six deep in one function is flagged.  The function body is one block, and each `if`/`else`, loop, braced `match` arm, closure, and the `impl` around a method adds one.  Four sites in `src/` go deeper.  Each line is the flagged block’s, as of the adoption commit:

| File | Line | Function | Depth | The deep block |
|---|---|---|---|---|
| `src/inline_image.rs` | 274 | `PageWalk::scan_stream` | 7 | `if read.ambiguous { … }`, the warning for a guessed data end |
| `src/inline_image.rs` | 292 | `PageWalk::scan_stream` | 7 | `if keep { … } else { … }`, inside the `InlineImage` pushed for an image read |
| `src/inline_image.rs` | 522 | `Tokens::read_literal_string` | 7 | the `Some(d @ b'0'..=b'7')` arm reading an octal escape’s next digit in a literal string (one the scanner skips in content, or one in an inline image’s dictionary) |
| `src/layout.rs` | 723 | `process_content` | 7 | `if let Some(n) = number(other) { … }`, the `TJ` adjustment that moves the text matrix |

The code is correct, so this is a refactor, not a bug.  Because CI denies warnings, the adoption marks each site with `#[expect(clippy::excessive_nesting, reason = "plan-0014: …")]` on the narrowest item that compiles: the `if` statement at the first site, the `match` arm at the third, and the `if let` at the fourth.  The second site’s `if` is a struct field’s value, so its expect sits on the enclosing `self.scan.images.push(…)` statement.  An attribute on the field would compile too, but it is too long for rustfmt, which then silently skips the whole statement.  This plan is what removes them.

## Proposed Change

Bring each site to six blocks or fewer, with no change in behavior:

- **`PageWalk::scan_stream`, both sites:** move the `Ok(read)` arm’s body (the warning and the push) into a method such as `record_image(&mut self, read, offset, resources, source, source_object)`.
- **`Tokens::read_literal_string`:** move the escape handling (everything after the backslash) into a method such as `read_escape(&mut self, out: &mut Vec<u8>)`, or read the octal digits in a small helper that returns the value.
- **`process_content`:** move the `TJ` arm into a helper that walks the array, or turn the `if let` into an early `continue` in the loop over items.

## Implementation Notes

- Remove each `#[expect(clippy::excessive_nesting, …)]` with the site it covers.  `unfulfilled_lint_expectations` flags any left behind, and `cargo clippy --all-targets` must show no `excessive_nesting` warning afterwards.
- **Pin each site before reworking it.**  `an_end_that_had_to_be_guessed_is_a_warning` and `a_plain_inline_image_is_found_with_its_data_and_place` in `inline_image.rs` cover the first two sites.  The other two are not fully covered:
  - No test reads an octal escape through this tokenizer.  Add one before reworking `read_literal_string`.
  - `tj_adjustments_space_words_but_not_kerning` in `layout.rs` runs the `TJ` adjustment, but it still passes if the adjustment’s sign is flipped, if the vertical branch’s axes are swapped, or if `* gs.h_scale` is dropped.  Before that site is reworked, add a `TJ` test with a vertical (Identity-V) font and one under `Tz`.
- The sites can land one at a time; each commit removes the expects it covers.

## Why Not a Workaround

The fix is in the tool’s own code; there is nothing to work around.  Keeping the expects for good would leave the deep blocks in place with the lint silenced.
