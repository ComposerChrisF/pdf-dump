# Feature Plan: Usage-Aware Text-Extraction Reliability

Implementation plan for the three remaining _static-verdict_ limitations documented
in `docs/ROADMAP.md` (“Known limitations: the reliability verdict is static”) and
anchored by `KNOWN LIMITATION` comments in `src/text.rs`.

## Problem

The per-font Reliable/Degraded/Unreliable verdict is assigned **statically**, from a
font’s dictionary alone.  It never looks at which character codes the content
actually shows, nor at how many U+FFFD replacement characters the decode emitted.
Three consequences remain (a fourth, Standard-14 passthrough, was fixed in v0.17.1):

1. **Base-less `/Differences` fonts** — a `/Differences` font whose glyph names all
   resolve via the Adobe Glyph List is reported Reliable even with no recognized
   base `/Encoding`, so its _non_-overridden codes fall to single-byte passthrough
   (accurate only for ASCII).  Anchor: `build_font_decoder`.
2. **Coverage net only watches ToUnicode** — the dynamic `>20% unmapped → Degraded`
   downgrade in `document_verdict` reads the `total`/`unmapped` counters, which are
   incremented _only_ in the ToUnicode branch of `emit_show_string`.  The U+FFFD a
   base-table miss or a byte passthrough emits is invisible to it, so table and
   passthrough fonts get only the coarse static verdict.  Anchor: `document_verdict`.
3. **Variable-width ToUnicode codespace** — a `Variable`/`Unknown` codespace is
   forced to one fixed width (2 for CID, else 1) in `build_font_decoder`, which can
   mis-split a genuinely variable-width font (common in CJK ToUnicode CMaps that mix
   1-byte ASCII with 2-byte codes).  Anchor: the width selection in
   `build_font_decoder`; `split_codes` does the fixed-width chunking.

The three share one corrective shape: a _usage-aware_ verdict that reflects what the
decoder actually could not handle on the bytes the content really shows.

## Assessment — what is worth supporting

- **#2 is the keystone and clearly worth it.**  Fixing it (universal decode
  coverage) is small, low-risk, and structural: it gives table/passthrough fonts the
  same dynamic safety net ToUnicode fonts already have.
- **#1 is mostly _subsumed_ by the #2 fix.**  Once non-overridden passthrough codes
  feed the coverage counters, a base-less `/Differences` font that actually shows
  undecodable codes self-corrects to Degraded.  The residual purely-static
  over-claim only bites when the undecodable codes are never shown — in which case
  the output is fine and the verdict is harmless.  A dedicated static fix is
  therefore _optional and low priority_; it is included as Phase 3 with its trade-off
  spelled out.
- **#3 is worth it but independent.**  It is a real correctness bug for mixed-width
  CJK ToUnicode fonts, orthogonal to coverage.  Moderate effort, isolated to
  `cmap.rs` + `split_codes`.

Recommended order: **Phase 1 (coverage) → Phase 2 (variable width) → Phase 3
(optional `/Differences` static refinement)**.  Each phase ships independently.

---

## Phase 1 — Universal decode coverage (fixes #2, mitigates #1)

> **Status: IMPLEMENTED.**  `push_code` added to `text.rs`; all four decode paths
> in `emit_show_string` feed `total`/`unmapped`; `document_verdict` is now
> usage-aware for every font type.  Five coverage tests added.  The minimum-`total`
> floor (see Edge cases) was deliberately **not** added — it would have masked the
> existing `low_coverage_downgrades_verdict_to_degraded` case (5 codes, 80 % miss),
> and short-but-fully-garbage text should downgrade.  Phases 2 and 3 remain open.

### Goal

Every decode path — ToUnicode, `SimpleTable` (base table + `/Differences`
overrides), and passthrough — contributes to the `total`/`unmapped` counters, so the
existing `document_verdict` low-coverage downgrade becomes a usage-aware safety net
for all fonts, not just ToUnicode ones.

### Changes by module

**`src/text.rs` — `emit_show_string`** (the one hot spot).  Today only the ToUnicode
arm touches the counters.  Extend the other arms:

- `SimpleTable`: for each input byte, `*total += 1`; `*unmapped += 1` when the emitted
  string is the lone replacement char (an unresolved `/Differences` override, or a
  base-table miss `decode(b) == None`).  A resolved override or a real glyph counts
  as mapped, including multi-char ligatures (still one code).
- Passthrough (`_` arm and the `SimpleTable { decode: None }` inner fallback): count
  per input byte — `*total += 1` per byte; `*unmapped += 1` for each byte that the
  lossy UTF-8 decode could not render (i.e. produced U+FFFD).  Implement by walking
  the bytes with `std::str::from_utf8` / `Utf8Error::valid_up_to` rather than
  counting U+FFFD in `from_utf8_lossy` output, so the denominator stays “bytes”, not
  “emitted chars”.

To keep the counting in one place and avoid divergent logic across three arms, factor
a small helper:

```rust
/// Append `s` for one source code; bump counters (U+FFFD == unmapped).
fn push_code(out: &mut String, s: &str, total: &mut u64, unmapped: &mut u64) {
    *total += 1;
    if s == "\u{FFFD}" { *unmapped += 1; }
    out.push_str(s);
}
```

ToUnicode keeps its existing per-code loop (it already counts); `SimpleTable` routes
each byte’s decoded `&str` through `push_code`; passthrough decodes byte-by-byte and
calls `push_code` with either the valid scalar or `"\u{FFFD}"`.

**`src/text.rs` — `document_verdict` / `LOW_COVERAGE_THRESHOLD`.**  No logic change
required — once the counters are universal, the existing `ratio > 0.20` downgrade
applies to all fonts.  Update the `KNOWN LIMITATION` doc comment on `document_verdict`
(the net now watches all paths) and remove the anchor.  Re-confirm the threshold reads
well for table fonts: a WinAnsi font with a handful of undecodable control codes must
not be downgraded (well under 20%); a Symbol-passthrough document (mostly U+FFFD) is
already Degraded statically and stays so.

**Optional refinement (stretch, same phase or later): per-font coverage.**  Thread a
font identifier into `emit_show_string` and accumulate counts per font so the banner
can say `/F2 (XFont, Type1): 47% of codes unmapped`.  Requires carrying per-font
tallies on `FontReliabilityRecord` (or a side map keyed by resource name) and a small
change to the `Tf`-tracking loop.  Improves the banner’s explanation of _why_ a
document was downgraded; not required to fix #2.

### Edge cases

- Source text that genuinely contains U+FFFD (vanishingly rare) would be miscounted
  as unmapped.  Acceptable — real content does not carry replacement chars.
- Empty show-strings and whitespace-only runs: `total` unchanged, no spurious
  downgrade.
- A document with very little text (a few codes) and one stray U+FFFD could cross 20%
  on a tiny denominator.  Mitigation: keep the existing behavior of only downgrading
  `Reliable → Degraded` (never to `Unreliable`), and consider a minimum-`total` floor
  (e.g. ignore the ratio when `total < 20`) so a 1-of-3 miss does not flip a verdict.
  Document the floor if added.

### Tests (in `text.rs`)

- `simple_table_unmapped_codes_feed_coverage` — a WinAnsi font show-string with codes
  that map to `None` (e.g. `0x81`) bumps `unmapped`/`total`.
- `passthrough_invalid_utf8_counts_as_unmapped` — a no-font passthrough run with
  invalid UTF-8 bytes records `unmapped`.
- `simple_table_high_unmapped_ratio_downgrades_to_degraded` — a statically-Reliable
  WinAnsi font whose shown codes are mostly undecodable drops the document verdict to
  Degraded.
- `base_less_differences_self_corrects_when_codes_shown` — the #1 case: a base-less
  `/Differences` font (Reliable on names) that shows non-overridden high bytes now
  downgrades to Degraded via coverage.
- `mostly_ascii_table_font_stays_reliable` — guard against false downgrades.
- `tiny_text_single_miss_does_not_downgrade` — only if the minimum-`total` floor is
  added.

### Version / risk

MINOR bump (it changes the reliability contract: documents that were _falsely_
Reliable will now correctly report Degraded, and exit code stays 0 vs 3 unaffected
since the net never reaches `Unreliable`).  Risk: low — `--text` stdout is byte-for-
byte unchanged; only the stderr banner / JSON `reliability` verdict and the process’
Degraded classification move.  Re-run the full suite; watch the existing
reliability-verdict tests for intended changes.

---

## Phase 2 — Variable-width ToUnicode codespace (fixes #3)

### Goal

Split show-string bytes into codes by honoring the CMap’s codespace ranges instead of
forcing one fixed width, so a mixed 1-byte/2-byte CJK ToUnicode font is decoded
correctly.

### Changes by module

**`src/cmap.rs` — store codespace bounds, not just widths.**  Today
`parse_codespace` pushes only `lo.len()` into `codespace_widths: Vec<u8>`.  Replace
with the full ranges:

```rust
struct Codespace { lo: Vec<u8>, hi: Vec<u8> } // width == lo.len()
// field: codespaces: Vec<Codespace>
```

- `byte_width()` derives `Fixed/Variable/Unknown` from `codespaces` widths (same logic
  as today, computed from `lo.len()` per range).
- New method `next_code(&self, bytes: &[u8]) -> (u32, usize)` returning the matched
  code value and the number of bytes consumed.  Algorithm (best-effort form of the
  PDF 32000-1 §9.7.6.2 code-extraction rule):
  - Among codespace ranges, try by ascending width.  A width-`W` range _matches_ the
    `W`-byte prefix when each byte `b_i` satisfies `lo_i <= b_i <= hi_i`.  Consume the
    first matching range’s width.
  - If no range matches, consume the **shortest** codespace width (per spec, an
    undefined code of the minimum length), yielding a code that `map_code` will miss →
    U+FFFD (and, with Phase 1, counted as unmapped).
  - Guard against running off the end of `bytes` (consume what remains).

**`src/text.rs` — `split_codes` / `FontDecoder::ToUnicode`.**  Carry the codespace
decision in the decoder instead of a bare `width: u8`:

```rust
ToUnicode { cmap: Rc<ToUnicodeCMap>, width: CodeWidth } // was width: u8
```

- In `emit_show_string`’s ToUnicode arm, when `width` is `Fixed(w)` keep the current
  fast path (`split_codes(bytes, w)`); when `Variable`/`Unknown`, iterate with
  `cmap.next_code(remaining)` until the bytes are consumed, calling `push_code`
  (Phase 1 helper) per code.
- `build_font_decoder` stops collapsing `Variable`/`Unknown` to a single byte width;
  it stores the `CodeWidth` and lets the decoder decide.  Retain the CID-vs-simple
  fallback only for `Unknown` (no codespace parsed at all).

### Edge cases

- Overlapping/again-disagreeing codespace ranges: ascending-width-first matching is
  deterministic and matches viewer behavior for the common ASCII-plus-CJK case.
- A byte sequence shorter than the shortest codespace width at end-of-string: consume
  the remainder as one (short) code; `map_code` likely misses → U+FFFD.
- Identity-H (single `<0000><FFFF>`) stays `Fixed(2)` — fast path, zero behavior
  change.  This is the overwhelming common case, so Phase 2 must not regress it.

### Tests

- `cmap.rs`: `next_code_mixed_width_prefers_matching_range` (a `<00><80>` +
  `<8140><FEFE>` codespace splits `41 8140` into `0x41` then `0x8140`);
  `next_code_no_match_consumes_min_width`; `byte_width_unchanged_for_fixed`.
- `text.rs`: `variable_width_cmap_decodes_mixed_one_and_two_byte` (end-to-end through
  `extract_text_from_page_with_warnings`); `identity_h_fixed_width_unchanged`
  (regression guard).

### Version / risk

MINOR bump.  Risk: low-to-moderate, fully contained — the Fixed fast path (virtually
all real PDFs) is untouched; only Variable/Unknown fonts change, and they were
previously mis-split, so any change is an improvement or neutral.

---

## Phase 3 — Base-less `/Differences` static refinement (optional, #1)

### Goal

Reduce the residual purely-static over-claim for nonsymbolic base-less `/Differences`
fonts, beyond what Phase 1 already corrects dynamically.

### Approach (and its trade-off)

Mirror the v0.17.1 Standard-14 fix: when a non-CID font has `/Differences` and **no**
recognized base `/Encoding`, default its base table to `encodings::standard` _iff_ the
font is nonsymbolic (FontDescriptor `/Flags` bit 6 set, bit 3 clear).  This decodes
non-overridden codes through StandardEncoding (the spec’s implicit builtin for a
nonsymbolic simple font) rather than ASCII passthrough.

- New helper `is_nonsymbolic(doc, dict) -> bool` reading `/FontDescriptor /Flags`
  (Symbolic = bit 3 / value 4; Nonsymbolic = bit 6 / value 32).
- In `build_font_decoder`, extend the `base` fallback (currently Standard-14-only) to
  also fire for nonsymbolic base-less `/Differences` fonts.
- Classification: keep it Degraded (not Reliable) for the _guessed_-base case, since
  the true builtin is unknown — but now the non-overridden codes decode through a real
  table, and Phase 1’s coverage net still guards the residual.

**Trade-off (why this is optional / gated on nonsymbolic):** presuming StandardEncoding
for an embedded font whose builtin we cannot read risks turning a secretly-WinAnsi
font’s straight quotes (`0x27`/`0x60`) curly.  Gating on the explicit nonsymbolic flag
and keeping the verdict Degraded contains that risk; without a reliable `/Flags`, do
nothing (status quo passthrough).

### Tests

- `nonsymbolic_base_less_differences_decodes_non_overridden_via_standard`.
- `symbolic_base_less_differences_unchanged` (no Standard presumption).
- `differences_without_flags_unchanged`.

### Version / risk

PATCH bump.  Risk: low, gated on `/Flags`.  Lowest priority — only pursue if real
sample PDFs show the residual mattering after Phase 1.

---

## Out of scope (tracked separately in `docs/ROADMAP.md`)

Predefined CJK CMap resource files, coordinate-based text ordering, and form-XObject
`Do` recursion are independent roadmap items, not part of the static-verdict class.

## Why this belongs in the Rust tool, not a script

This is core `--text` decoding and the reliability contract that `--text`’s exit code
and JSON `reliability` object expose to downstream agents.  It must live in
`pdf-dump` so every invocation — and every tool that shells out to it — gets the same
honest verdict; a one-off script could not participate in the exit-code / banner /
JSON contract or share the `cmap.rs` and `encodings.rs` decode paths.
