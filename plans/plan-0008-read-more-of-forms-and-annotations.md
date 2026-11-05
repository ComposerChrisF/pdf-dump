# Plan: Read More of Forms and Annotations

## Problem

`--forms` and `--annotations` report `/V`, `/RV`, `/Contents`, `/RC` and `/AP /N`.  A viewer shows more than that, and a caller screening a PDF’s text cannot hold on the rest without holding nearly every file: Obsidian-Brain’s `webapp/deploy/kcs-mail/mailgate.py` says holding on these would hold “every dropdown and every signed comment”.

Four gaps, from Fable’s and Sol’s review of the reading limits (4-Nov-2026), and from Obsidian-Brain’s answer to its request `Obsidian-Brain/r0001`:

1. **Strings a reader sees, not reported.**  An annotation’s `/T` and `/Subj`; a field’s `/TU`, `/DV` and `/Opt` display labels; `/MK`’s `/CA`, `/RC` and `/AC`.  Fable and Sol both asked for some of these, and differ on how many.
2. **Whether an annotation shows.**  Its `/F` flags, Hidden and NoView above all, so that a caller can tell text a viewer shows from text it hides.  Both asked for this.  Sol also wants to know whether `/AP /D` and `/AP /R` exist; Obsidian-Brain wants what they say.
3. **Walks that do not climb.**  A value on a parent reached only through a widget’s `/Parent`, and a comment reached only through a pop-up’s `/Parent`, are not reported.  macOS PDFKit shows both.  A `/Contents`-only comment behind a pop-up passes `mailgate.py` clean today.
4. **Rich text.**  `strip_markup` leaves a named reference such as `&nbsp;` literal with `*_decoded: true`, and turns a tag outside its inline list into a line break, so `123-45-<tt>6789</tt>` loses its shape.

## Proposed Change

**Chris ruled on gaps 1 and 2 on 4-Nov-2026 (`pdf-dump/0004`, option A):** “Both lists, as a plan after the three bugs: each string with its decode status, the flags, and presence of /D and /R”.  So this much is decided:

- `--annotations` gains `author` (`/T`), `subject` (`/Subj`), `flags`, `hidden`, `has_down_appearance` and `has_rollover_appearance`.
- `--forms` gains `tooltip` (`/TU`), `options` (`/Opt`, as `{export, label, decoded}`), and `captions` (`/MK /CA`, as `{text, decoded}`, a list, since each widget of a field has its own).
- Each string carries a `*_decoded` beside it, as the existing ones do, and one that will not decode exits 3.

_Status, 4-Nov-2026: this ruled list landed, with `captions` and `options` items each carrying their own `decoded`, and `tooltip_inherited` and `options_inherited` beside the values.  What follows is what remains of this plan._

**Not ruled on, and still only proposed** (Obsidian-Brain’s asks, which went beyond the two models’ lists):

- `--forms`: `default_value` (`/DV`), and `/MK`’s `/RC` and `/AC`.
- `--annotations`: the text of `/AP /D` and `/AP /R`, where the ruling is presence.
- A walk climbs `/Parent` from a widget or pop-up to an entry nothing else reported, naming how it was reached, with a depth limit and a guard against cycles.
- `strip_markup` decodes the named references XHTML defines, and keeps unknown tags inline, so that text is not split where a viewer would not split it.
- `link_type` and `target` for an action on any annotation: a `/URI` action on a widget or a text note reports neither today, since only a `/Link` is classified.
- An action’s `/Next` chain: a `/GoTo` with `/Next << /S /URI … >>` reports only the `/GoTo`.  It needs a limit and a guard against cycles.

## Implementation Notes

- `src/forms.rs` (`FieldWalk`, `Inherited`, `own_entry`), `src/annotations.rs`, `src/text_string.rs` (`strip_markup`).  `text_string::read_text_value` already reads a string, stream or reference, and flags a wrong type.
- A key’s absence must stay distinct from an empty value (`positive-evidence-of-absence.md`).  New keys only: `mailgate.py` reads `value` as a string and must keep working.
- Graduates into `DEBUGGING_WITH_PDF_DUMP.md` § Mode-specific schemas and § What pdf-dump does not read, whose list shrinks by what lands.
- Tell Obsidian-Brain as each key arrives: its `webapp/README.md` lists these gaps and will read them.

## Why Not a Workaround

A caller’s `--search` can find that a key exists, but not read its decoded text or say which field it belongs to, which is why `mailgate.py` can only hold.  Reading a PDF’s structure is pdf-dump’s domain, and every consumer would otherwise write the same decoding and the same walk.
