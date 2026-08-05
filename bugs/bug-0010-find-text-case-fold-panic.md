# bug-0010: `--find-text` panics when case-folding changes a string’s byte length

**Severity:** High
**Classification:** CODE bug (panic; violates the exit-code contract — a panic exits 101, none of `0/1/2/3`)
**Status:** Verified (live repro: `pdf-dump ft-panic.pdf --find-text "İ"` panics at `find_text.rs:40`, exit 101)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`find_matches` lowercases both the page text and the search pattern, then mixes byte offsets between
the original and lowercased strings.  Because `to_lowercase()` is not length-preserving for some
Unicode characters, an offset computed in the lowercased text is applied where the code expects the
original length, landing mid-character and panicking on a non-char-boundary slice.  A search whose
pattern (or page text) contains such a character crashes the tool.

## Affected code
- `src/find_text.rs:40` — `while let Some(pos) = lower_text[search_start..].find(&lower_pattern)` —
  the panic site (slicing `lower_text` at a byte index that is not a char boundary).
- `src/find_text.rs:55` — `search_start = abs_pos + pattern.len()` advances by the _original_
  pattern’s byte length, not the lowercased one.
- `src/find_text.rs:43-45` — `ctx_start` / `ctx_end` are `lower_text` offsets applied to `text`,
  garbling the context snippet even for ASCII patterns (no panic — saved by `floor/ceil_char_boundary` — but wrong output).

## What it does vs. what it should do
`to_lowercase()` can change byte length: `U+0130` (İ) is 2 bytes and lowercases to `"i\u{307}"`
(3 bytes); `U+1E9E` (ẞ) is 3 bytes and lowercases to `ß` (2 bytes). `abs_pos` is a byte offset into
`lower_text`, but the loop advances `search_start` by `pattern.len()` (the original pattern length).
When they disagree, `search_start` lands inside a multi-byte character and `lower_text[search_start..]`
panics with `byte index N is not a char boundary`.

The search should be performed entirely in one consistent representation: advance `search_start` by
the matched slice’s length in `lower_text` (i.e. `lower_pattern.len()`), and compute the context
window against `lower_text` (mapping back to `text` with char-boundary care), or iterate
`char_indices` without cross-string offset arithmetic.

## Reproduction
```
pdf-dump ft-panic.pdf --find-text "İ"
# thread 'main' panicked at src/find_text.rs:40:41:
# start byte index 2 is not a char boundary; it is inside '\u{307}' ...
# exit 101
```

Rust test sketch:

```rust
let doc = build_page_doc_with_content(b"BT (\xC4\xB0a) Tj ET"); // passthrough emits "İa"
let mut buf = Vec::new();
find_text::print_find_text(&mut buf, &doc, "İ", None); // currently panics
```

Fixture `ft-panic.pdf` is in the review scratchpad.  Confirmed live.

## Suggested fix
In `find_matches`, keep all offset math in `lower_text` space: advance by `lower_pattern.len()`, and
derive `ctx_start`/`ctx_end` from `lower_text` then map back onto `text` at char boundaries (or build
the snippet from `lower_text`).  Alternatively, use a char-boundary-safe case-insensitive search that
does not mix offsets between the two strings.

## Why the fix addresses the bug
Using offsets that are consistent with the string actually being indexed removes the mid-character
slice that panics, and computing the context window in the same space removes the garbled-snippet
variant.

## Related
A panic is an undocumented exit (101) — see `~/.claude/rules/cli-exit-codes.md`.  The same
byte-offset-mixing root also produces the non-panicking wrong-snippet behavior noted above.
