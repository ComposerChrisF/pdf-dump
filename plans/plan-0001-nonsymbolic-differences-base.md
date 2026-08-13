# Plan: Presume StandardEncoding for Nonsymbolic Base-Less `/Differences` Fonts

## Problem

`--text` classifies a simple font carrying `/Encoding /Differences` as **Reliable** as soon
as every one of its glyph names resolves through the Adobe Glyph List — whether or not a base
encoding was recognized.  When none was (`build_font_decoder`’s `base` is `None`), the font’s
_non_-overridden codes fall through to single-byte UTF-8 passthrough, which is accurate only
for ASCII.  A non-ASCII non-overridden byte therefore extracts as U+FFFD beneath a “reliable”
banner.

Anchors: the `KNOWN LIMITATION` comment on the `/Differences` arm of `build_font_decoder`
(`src/text.rs`), and `docs/ROADMAP.md` § “Known limitations: the reliability verdict is
static”.

**This is the residual left after the usage-aware coverage net shipped in v0.18.0.**  Every
decode path now feeds the `total`/`unmapped` counters, so a base-less `/Differences` font that
_actually shows_ undecodable codes already self-corrects to Degraded in `document_verdict`.
What remains is the purely static over-claim: a font whose undecodable codes are never shown.
In exactly that case the extracted text is correct and the verdict is merely generous — which
is why this is an optional refinement and not a defect.

## Proposed Change

Mirror the v0.17.1 Standard-14 fix one step further out.  When a non-CID font has
`/Differences` and **no** recognized base `/Encoding`, default its base table to
`encodings::standard` _iff_ the font is nonsymbolic, so non-overridden codes decode through
StandardEncoding — the spec’s implicit builtin for a nonsymbolic simple font — instead of
ASCII passthrough.

Nonsymbolic means the `/FontDescriptor /Flags` Nonsymbolic bit (bit 6, value 32) is set and
the Symbolic bit (bit 3, value 4) is clear.  With no readable `/Flags`, do nothing: the status
quo passthrough stands.

The verdict for a _guessed_ base stays **Degraded**, not Reliable — the true builtin is still
unknown, the non-overridden codes now merely decode through a real table, and the coverage net
continues to guard the residual.

## Implementation Notes

- All of the work is in `src/text.rs`, in the `!is_cid` arm of `build_font_decoder`.  That arm
  currently computes `base` as `simple_table_for(font_base_encoding(doc, dict))`, with an
  `.or_else` fallback that fires only for `STANDARD_14_TEXT` base fonts.  Extend that
  `or_else` with the nonsymbolic case; nothing else in the decode path changes, because
  `FontDecoder::SimpleTable { decode, overrides }` already consults `overrides` first and
  falls back to `decode`.
- New helper `is_nonsymbolic(doc, dict) -> bool`, reading `/FontDescriptor /Flags` via
  `helpers::resolve_dict`.  Returns `false` when the descriptor or `/Flags` is absent or
  unreadable — absence must not be read as evidence of nonsymbolic.
- The classification decision sits a few lines below, in the `if !diff.map.is_empty()` block:
  today it is `Reliable` when `diff.unresolved == 0`.  A guessed base must degrade it, so the
  branch needs to know whether `base` came from `font_base_encoding` or from the presumption.
  Carry that as a small enum or a `bool guessed_base` rather than re-deriving it.
- Update the `KNOWN LIMITATION` comment in the same edit — it is the anchor this plan cites,
  and leaving it describing fixed behavior is the stale-restatement failure mode.

**Trade-off (this is why the change is gated, and why it is optional).**  Presuming
StandardEncoding for an embedded font whose builtin cannot be read risks turning a
secretly-WinAnsi font’s straight quotes (`0x27`/`0x60`) into curly ones.  Gating on the
explicit nonsymbolic flag and holding the verdict at Degraded contains that risk.  Only pursue
this if real sample PDFs show the residual mattering — it was deliberately deferred once
already, on the grounds that the coverage net makes the harmful case self-correcting.

Version: PATCH.  Risk: low, gated on `/Flags`.

### Tests

- `nonsymbolic_base_less_differences_decodes_non_overridden_via_standard`
- `symbolic_base_less_differences_unchanged` — no Standard presumption.
- `differences_without_flags_unchanged` — absent `/Flags` keeps passthrough.
- A verdict assertion that the guessed-base font reports Degraded, not Reliable.

## Why Not a Workaround

This is the `--text` decode path itself, and the reliability verdict it feeds is a contract
that `--text` publishes through its exit code, its stderr banner, and the JSON `reliability`
object.  A post-processing pass over extracted text could not participate in any of those, nor
reach `/FontDescriptor /Flags`, nor share the `encodings.rs` tables — and every tool that
shells out to `pdf-dump` must get the same verdict from the same run.
