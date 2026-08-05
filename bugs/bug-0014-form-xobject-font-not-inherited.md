# bug-0014: Form XObject text does not inherit the caller’s active font; `q`/`Q` not modeled

**Severity:** Medium
**Classification:** SPEC-CODE mismatch
**Status:** Verified (live: `form-tf.pdf --text` yields `�` instead of `–`, with a Degraded banner)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
When `--text` recurses into a form XObject (`Do`), it rebuilds the form’s font-decoder table but
resets the active font to `None`.  A form that draws text without its own `Tf` legally uses the font
selected by its caller; because the caller’s font is dropped, such text falls to raw passthrough and
decodes wrong.  The graphics-state save/restore operators `q`/`Q` are also not modeled.

## Affected code
- `src/text.rs:244` — `current_font` is reset to `None` at the start of each `process_content`.
- `src/text.rs:335-341` and `src/text.rs:426` — the form recursion passes the form’s resources but
  not the caller’s text state.

## What it does vs. what it should do
PDF 32000-1 §8.10.1 — a form XObject executes within the current graphics state; the text font and
size (§9.3.1, part of the graphics state) persist into it.  A form drawing text without its own `Tf`
must use the caller’s selected font.  The code should seed the form recursion with the caller’s active
decoder as the initial `current_font`.  Related (evident from the code, not separately reproduced): the
operator walk ignores `q`/`Q`, so a font set inside a saved state incorrectly persists after `Q`.

Note this interacts with the reliability verdict: fixing it turns Degraded into Reliable for such
documents, because the previously-undecodable inherited-font text now decodes.

## Reproduction
Page content `/F1 12 Tf /Fm0 Do` where `/F1` is a WinAnsi font and the form (`/Fm0`, no `/Resources`,
no `Tf`) draws `(\x96)` (WinAnsi en dash):

```
pdf-dump form-tf.pdf --text
# currently: "�" (U+FFFD)  — should be "–"
```

Fixture `form-tf.pdf` is in the review scratchpad.  Assert the extracted text contains the en dash.

## Suggested fix
Pass the caller’s active decoder into the form recursion as the initial `current_font` (so a form
without its own `Tf` inherits it).  Optionally model a `q`/`Q` stack of the text state so a font set
inside a saved state is restored on `Q`.

## Why the fix addresses the bug
Seeding the form with the inherited font matches the spec’s graphics-state persistence, so
inherited-font text decodes correctly instead of falling to passthrough.

## Related
Touches the reliability machinery shared with [[bug-0012-font-dedup-drops-unreliable-verdict]] and
[[bug-0011-find-text-unreliable-silent-success]].  Form-XObject recursion was added in v0.20.0.
