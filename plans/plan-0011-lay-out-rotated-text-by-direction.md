# Plan: Lay Out Rotated Text by Direction, Like the Grid

## Problem

Under `--text --layout`, text that is not left-to-right horizontal in visual space follows the grid under a `[non-horizontal text]` line, **in content order, one line per show-string**.  That reads well when a rotated label is one `Tj`.  It reads badly when a generator draws each glyph in its own `BT … Tm … Tj ET`, as some brokerage statements do: a rotated block then prints one glyph per line, and a rotated block of several lines (a name and address printed up the margin) cannot keep its lines apart, because content order knows nothing of baselines.

A consumer that matches identifiers in this text (id-redact) cannot match a number printed one glyph per line, and refuses such a file rather than passing it.  Banks do print identifiers in rotated margin strips.

## Proposed Change

The non-horizontal glyphs are laid out by geometry, as the grid is, one direction at a time:

- Each such glyph keeps its baseline direction in visual space.  Glyphs are grouped by that direction, quantized to whole degrees, so text at 90°, 180°, 270° and any other angle each form a group.
- Each group is mapped into its own baseline frame (rotated so its text runs left to right) and laid out by the same chunk, line and grid machinery as the page: chunks never split, lines by baseline, columns by position, and the group’s own left margin shifted out.
- Under the `[non-horizontal text]` marker each group prints as a small grid of its own, in the order of its first glyph.  So a three-line rotated address keeps its three lines, and a glyph-by-glyph axis label reads `Dollars ($)`.
- **Content order stays the fallback** for glyphs that fit no group: vertical writing mode (`WMode 1`), and mirrored text (a frame whose glyphs are left-handed, as with a negative `Tz` alone).  They print after the groups, one line per show-string, as today.
- The reliability rules are unchanged: a glyph placed without a known advance degrades its font whether it lands on the grid or in a group.  `non_horizontal_glyphs` in `--json` still counts every glyph under the marker.

Nothing changes for horizontal text, for off-page text, or for plain `--text`.

## Implementation Notes

- `layout.rs::show` already computes each glyph’s baseline vector `(dx, dy)` in visual space and its up vector.  For a non-horizontal glyph it records the quantized angle and the glyph’s origin, end and baseline rotated into that angle’s frame (or nothing, for the fallback cases).
- `render_grid`’s line-building half (chunks → lines → `render_line`, margin trim) is factored out so the page grid and each group call the same code.  Each group is translated so its leftmost glyph is at x = 0, since `render_line` clamps negative columns to 0.  The cell is `--layout-cell` when given, else the group’s own median advance.
- Tests (synthetic, Helvetica glyph by glyph, `0 1 -1 0 x y Tm`): a three-line block rotated 90° counterclockwise keeps its lines and words, with one horizontal line for contrast; the same at 270°; and the mirrored and vertical fallbacks stay in content order.

## Why Not a Workaround

The positions exist only inside pdf-dump’s text-state machine.  A consumer could re-derive them only by re-implementing that machine (CTM, text matrix, font metrics, `/Rotate`) over `--operators` output.  The grid’s chunk and line rules are already the contract id-redact depends on (runs never split), and rotated text should obey the same rules rather than a second set built beside them.
