# bug-0036: Passthrough coverage counts emitted scalars, not bytes, so a multi-byte invalid run under-reports damage

**Severity:** Medium
**Classification:** CODE bug (coverage safety net biased toward _not_ downgrading; comment states the opposite semantics)
**Status:** Verified (live measurement, v0.24.0 + HEAD `c0b7a1b`: `AAAA` + a truncated 4-byte sequence yields `total=5 unmapped=1 ratio=0.200 verdict=Reliable`; the byte denominator gives `3/7 = 0.43` → Degraded)
**Affected version:** v0.24.0 (HEAD `c0b7a1b`)

## Summary
The passthrough arm of `emit_show_string` counts one `total` per **emitted scalar** of
`String::from_utf8_lossy`, not one per **input byte**.  `from_utf8_lossy` collapses a maximal
invalid subsequence into a _single_ U+FFFD, so a run of _n_ bad bytes contributes `total += 1,
unmapped += 1` instead of `total += n, unmapped += n`.  Valid ASCII bytes still count 1:1.  The
numerator therefore shrinks faster than the denominator, and the `> 20 % unmapped → Degraded`
coverage net in `document_verdict` under-fires — the unsafe direction, since the whole purpose of
the net is to catch text the decoder could not handle.

The arm’s own doc comment claims the byte semantics (“U+FFFD for each byte the lossy decode could
not render”), so code and comment disagree about what is being measured.

## Affected code
- `src/text.rs:514-526` — the `_ =>` passthrough arm.  Note it does **not** call `push_code`; it
  inlines its own counting loop over `String::from_utf8_lossy(bytes).chars()`.
- `src/text.rs:442-454` — `push_code`, which exists precisely to keep the counting identical across
  the decode arms.  The passthrough arm is the one that bypasses it, which is how the divergence
  survived.
- `src/text.rs:1092-1114` — `document_verdict`, the consumer whose ratio is skewed.

## What it does vs. what it should do
Every code that the decoder could not render should count toward `unmapped`, over a denominator of
what the content actually presented.  With no font there are no character codes, so the natural unit
is the byte — which is what the comment says, and what the implementation plan for the usage-aware
net specified (walk with `std::str::from_utf8` / `Utf8Error::valid_up_to` “so the denominator stays
_bytes_, not _emitted chars_”; the plan was `feature-plan-usage-aware-reliability.md`, deleted as
implemented in `c0b7a1b` and recoverable from git).  That prescription is the one part of Phase 1
that did not ship as written.

Measured divergence (`String::from_utf8_lossy` vs. a `valid_up_to` byte walk):

<!-- typo disable -->
| Input bytes | Scalars | Bytes |
|---|---|---|
| `80 81` (two lone continuation bytes) | 2/2 = 100 % | 2/2 = 100 % |
| `93 94 96 41` (WinAnsi-ish high bytes) | 3/4 = 75 % | 3/4 = 75 % |
| `41 E0 A0` (`A` + truncated 3-byte) | 1/2 = 50 % | 2/3 = 67 % |
| `41 41 41 41 F0 9F 98` (`AAAA` + truncated 4-byte) | **1/5 = 20 %** | **3/7 = 43 %** |
<!-- typo enable -->

The first two rows are why this went unnoticed: **lone** high bytes are each their own maximal
subpart, so scalar and byte counting agree exactly — and lone high bytes are the common PDF case (a
WinAnsi smart quote in a font with no recognized encoding).  The divergence needs a byte run that
_looks like_ a truncated UTF-8 lead-plus-continuation sequence, which is why the severity is Medium
rather than High.  The last row straddles the threshold: 20 % is not `> 20 %`, so the document is
certified Reliable where the documented semantics would downgrade it to Degraded.

## Reproduction
No fixture file needed — a fontless content stream is enough (the `_` arm fires when no `Tf` has
selected a decodable font).  As a unit test in `src/text.rs`, mirroring
`passthrough_invalid_utf8_counts_as_unmapped`:

```rust
let mut doc = Document::new();
let c = Stream::new(Dictionary::new(), b"BT (AAAA\xF0\x9F\x98) Tj ET".to_vec());
let c_id = doc.add_object(Object::Stream(c));
let mut page = Dictionary::new();
page.set("Type", Object::Name(b"Page".to_vec()));
page.set("Contents", Object::Reference(c_id));
let p_id = doc.add_object(Object::Dictionary(page));
let result = extract_text_from_page_with_warnings(&doc, p_id);
// measured today: total_codes == 5, unmapped_codes == 1  (ratio 0.200, verdict Reliable)
// expected:       total_codes == 7, unmapped_codes == 3  (ratio 0.43,  verdict Degraded)
```

**The existing test does not pin this either way.**
`passthrough_invalid_utf8_counts_as_unmapped` uses a single `0xFF`, where one bad byte produces
exactly one U+FFFD — a case both semantics agree on.  It will stay green whichever denominator is
chosen, so it is not evidence that the byte semantics hold.  Any fix must add a multi-byte-invalid
case, and that new test should be mutation-checked (revert the fix, confirm it fails).

## Suggested fix
Replace the passthrough arm’s `from_utf8_lossy(bytes).chars()` loop with a byte walk, and route it
through `push_code` so all four arms count in one place:

```rust
let mut rest = bytes;
while !rest.is_empty() {
    match std::str::from_utf8(rest) {
        Ok(v) => {
            for ch in v.chars() { push_code(out, ch.encode_utf8(&mut [0u8; 4]), total, unmapped); }
            break;
        }
        Err(e) => {
            let ok = e.valid_up_to();
            // valid prefix: one push_code per scalar
            // then `e.error_len().unwrap_or(rest.len() - ok)` bad bytes, one push_code("\u{FFFD}") each
            // advance past both and continue
        }
    }
}
```

Emitted text must stay byte-for-byte identical to `from_utf8_lossy` output — that is the
non-negotiable constraint, since `--text` stdout is a stable contract.  Note this is _not_
automatic: emitting one U+FFFD per bad **byte** would change stdout for multi-byte invalid runs,
where `from_utf8_lossy` emits one.  So the counters and the output must be decoupled — count _n_
bad bytes while appending a single U+FFFD.  A `push_code` variant taking an explicit `(text,
code_count, unmapped_count)` is the cleanest shape.

**Alternative resolution (rejected):** fix the _comment_ instead, declaring scalars the intended
unit.  That is cheaper but bakes in the under-counting bias, and it makes the passthrough arm the
only decode path whose denominator is not “one per source code”.  Choose it only with a deliberate
ruling, and then say in the comment why the net is deliberately looser here.

## Why the fix addresses the bug
Counting per input byte restores the invariant the coverage net assumes — one `total` per unit of
source the decoder was asked to render — so a page whose bytes are mostly undecodable crosses the
20 % threshold regardless of whether its garbage happens to form multi-byte-looking runs.  Routing
it through `push_code` removes the divergent counting site that let code and comment drift apart.

## Related
- [[bug-0012-font-dedup-drops-unreliable-verdict]] and
  [[bug-0011-find-text-unreliable-silent-success]] — same reliability verdict, same “certified
  reliable when it should not be” family.  bug-0012 is the higher-severity sibling and should land
  first; both touch `document_verdict`’s inputs.
- [[bug-0014-form-xobject-font-not-inherited]] — also in the `text.rs` reliability cluster; TODO.md
  batches these together.
- `plans/plan-0001-nonsymbolic-differences-base.md` touches the neighboring `build_font_decoder`
  arm, but is independent of this.
