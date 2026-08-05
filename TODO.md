# TODO — pdf-dump

## Deep bug hunt 2026-07-15 (v0.24.0, HEAD 7353ccb)

A portfolio-style adversarial review found **35 bugs**, filed as `bugs/bug-0001`…`bugs/bug-0035`.
Each report carries a reproduction (test-ready), a suggested fix, and why the fix works.
Several are **SPEC DECISIONs** — a downstream instance must NOT code-fix them blindly; resolve
the decision first (they are gathered in Phase 0 and gate their dependent code fixes).

**Start here (a fresh Opus instance):** work top-down through the phases below.  Within a phase,
the order is deliberate — later items in a phase may depend on earlier ones (noted inline).  Read
the linked `bugs/bug-NNNN-*.md` before touching code.  When a bug is fixed, delete its report in the
fixing commit and name the bug id in the message (per `~/.claude/rules/bug-reports.md`); move a
one-line stub here into a `## Done` section.

Severity legend: **[CRIT]** crash from a normal invocation · **[HIGH]** silent wrong output / DoS /
contract violation · **[MED]** correctness/robustness on unusual input · **[LOW]** minor / docs.

---

### Phase 0 — SPEC DECISIONS (resolve with Chris before the dependent code fix)

These need a decision from Chris; the report lists the options and a recommendation.  Do the
decision here, then implement in the phase noted. **Do not just “fix the code”.**

- [ ] **bug-0019** [HIGH] Missing-object modes exit 0 (should `--object`/`--inspect` on a missing
      object exit 1 like `--extract-stream`/`--page`? and should `--object 0` be a usage error?).
      Decision gates the Phase 2 code fix. `bugs/bug-0019-missing-object-modes-exit-zero.md`
- [ ] **bug-0020** [MED] Object generation not addressable (fallback lookup vs a `--object N:G`
      syntax?).  Gates Phase 3 fix. `bugs/bug-0020-object-generation-not-addressable.md`
- [ ] **bug-0004** [HIGH] `/DecodeParms` predictor ignored (apply the PNG/TIFF predictor, or emit a
      “predictor not applied” warning?).  Gates Phase 2 fix. `bugs/bug-0004-decodeparms-predictor-ignored.md`
- [ ] **bug-0033** [MED] UTF-16BE / non-UTF-8 text strings mishandled in display + search (decode
      per spec, and/or lossless byte fallback — scope?).  Gates Phase 2 fix.
      `bugs/bug-0033-text-string-utf16be-mishandled.md`
- [ ] **bug-0009** [LOW] `--extract-stream` ignores `--json` (emit a JSON summary, or scope the
      docs?).  Gates the Phase 4 doc/behavior fix. `bugs/bug-0009-extract-stream-ignores-json.md`
- [ ] **bug-0025** [LOW] `--page` accepted-but-ignored, yet range-validated (reject/warn, or
      document + stop validating in modes that ignore it?). `bugs/bug-0025-page-modifier-accepted-but-ignored.md`
- [ ] **bug-0028** [MED] `--search value=`/`regex=` only match top-level dict values (recurse into
      nested containers, or narrow the docs?). `bugs/bug-0028-search-nested-values-not-matched.md`
- [ ] **bug-0017** [LOW] `is_garbled_text` flags correctly-extracted non-ASCII as non-extractable
      (drop the ASCII heuristic for the Unicode-aware verdict?).  Test-pinned — confirm intent.
      `bugs/bug-0017-is-garbled-heuristic-flags-valid-unicode.md`

---

### Phase 1 — Crashes & denial-of-service (fix first; mostly no spec decision)

- [ ] **bug-0013** [CRIT] Form-field `/Kids` cycle → stack-overflow abort, reachable from the
      **default** command (`pdf-dump file.pdf`).  Add a visited-set to `collect_field_recursive`.
      **This is the single most urgent fix.** `bugs/bug-0013-form-field-kids-cycle-stack-overflow.md`
- [ ] **bug-0010** [HIGH] `--find-text` panics (exit 101) when case-folding changes byte length
      (Turkish İ, ẞ). `bugs/bug-0010-find-text-case-fold-panic.md`
- [ ] **bug-0018** [HIGH] Large `--object`/`--page` range materialized in memory before validation
      (~17 GB alloc / multi-second hang).  Intersect with the real document instead of enumerating.
      Exit-code for “range matches nothing” depends on **bug-0019**. `bugs/bug-0018-large-range-materialized-before-validation.md`
- [ ] **bug-0024** [MED] Page-label roman numerals: unbounded loop on a crafted `/St` (DoS).  Cap
      the value. `bugs/bug-0024-page-label-roman-unbounded-loop.md`
- [ ] **bug-0005** [MED] LZW/RunLength decompression-bomb cap enforced after-the-fact / not at all.
      Cap during decode. `bugs/bug-0005-decompression-bomb-lzw-runlength-uncapped.md`

---

### Phase 2 — Silent wrong output (the tool’s core sin)

- [ ] **bug-0026** [HIGH] Recovery truncates at the first embedded `endstream` byte-match and
      reports the truncation as a successful repair. `bugs/bug-0026-recovery-truncates-at-embedded-endstream.md`
- [ ] **bug-0031** [HIGH] `--detail security` reads a key-length from the _next object_ (reports
      35-bit instead of 40) — raw encrypt-dict parse is unbounded. `bugs/bug-0031-security-keylength-reads-past-dict.md`
- [ ] **bug-0034** [HIGH] `--validate` false negative: a dangling `/Root` in the trailer → “0
      errors”, exit 0. `bugs/bug-0034-validate-dangling-root-false-negative.md`
      _(share the trailer-walk with bug-0027; do together.)_
- [ ] **bug-0012** [HIGH] `dedup_font_records` drops a conflicting Unreliable record → garbage
      extraction certified “reliable”, exit 0. **Do before bug-0011** (0011 reuses this verdict).
      `bugs/bug-0012-font-dedup-drops-unreliable-verdict.md`
- [ ] **bug-0011** [HIGH] `--find-text` on an unreliable document → silent “No matches”, exit 0, no
      banner.  Depends on bug-0012. `bugs/bug-0011-find-text-unreliable-silent-success.md`
- [ ] **bug-0003** [HIGH] Multiple content streams concatenated with no separator → tokens fuse
      across boundaries (`ETBT`).  Push a newline between segments. `bugs/bug-0003-content-streams-joined-without-separator.md`
- [ ] **bug-0015** [HIGH] Indirect `/Filter` (a reference) silently treated as unfiltered →
      `--extract-stream` writes still-compressed bytes, exit 0. `bugs/bug-0015-indirect-filter-silently-unfiltered.md`
- [ ] **bug-0029** [HIGH] `--search stream=`/`regex=` search raw bytes when decode fails → false
      hits and false “not found”, warning discarded. `bugs/bug-0029-search-raw-bytes-on-decode-failure.md`
      _(related family with bug-0004: a decode “success” that is not the decoded content.)_
- [ ] **bug-0022** [HIGH] `--page` info omits inherited `MediaBox`/`CropBox`/`Rotate` (prints `-`)
      on the many PDFs that set them on an ancestor `/Pages`. `bugs/bug-0022-page-attribute-inheritance-missing.md`
- [ ] **bug-0021** [MED] `--object N` on an indirect-reference object shows the _target’s_ content
      under N’s header.  Display the stored value, not the deref. `bugs/bug-0021-object-indirect-reference-masquerade.md`
- [ ] **bug-0019** [HIGH] (code fix, after Phase 0 decision) Route missing-object misses to exit 1;
      move `--inspect`’s error to stderr. `bugs/bug-0019-missing-object-modes-exit-zero.md`
- [ ] **bug-0004** [HIGH] (code fix, after Phase 0 decision) Apply predictor or warn on
      `/DecodeParms`. `bugs/bug-0004-decodeparms-predictor-ignored.md`
- [ ] **bug-0033** [MED] (code fix, after Phase 0 decision) Decode UTF-16BE text strings / lossless
      fallback, in display and search. `bugs/bug-0033-text-string-utf16be-mishandled.md`

---

### Phase 3 — Correctness & robustness (medium)

- [ ] **bug-0002** [MED] CMap parser skips its section-end keyword on a malformed pair, destroying
      later valid sections. `bugs/bug-0002-cmap-malformed-pair-destroys-later-sections.md`
- [ ] **bug-0027** [MED] Reverse-reference lookup never scans the trailer → `--inspect <catalog>`
      says “Referenced by: (none)”. **Do with bug-0034** (shared trailer walk).
      `bugs/bug-0027-reverse-refs-skip-trailer.md`
- [ ] **bug-0035** [MED] `--validate` false positive: every `/Type /ObjStm` flagged “unreachable”
      (extend the XRef-stream skip). `bugs/bug-0035-validate-objstm-false-positive.md`
- [ ] **bug-0014** [MED] Form XObject text does not inherit the caller’s active font (`Tf`); `q`/`Q`
      not modeled.  Interacts with the reliability verdict. `bugs/bug-0014-form-xobject-font-not-inherited.md`
- [ ] **bug-0020** [MED] (code fix, after Phase 0 decision) Address objects by their real
      generation. `bugs/bug-0020-object-generation-not-addressable.md`
- [ ] **bug-0032** [MED] `--strict` (and default recovery) silently skip on a re-read I/O error →
      exit 0 on a malformed file.  Report + non-zero. `bugs/bug-0032-strict-reread-error-silently-ignored.md`
- [ ] **bug-0016** [LOW] `--inspect --deref` honored in JSON, ignored in text. `bugs/bug-0016-inspect-deref-ignored-in-text-mode.md`
- [ ] **bug-0001** [LOW] ASCII85 overflow group (`> 2^32−1`) silently truncated; malformed 1-char
      final group. `bugs/bug-0001-ascii85-overflow-group-silent-truncation.md`
- [ ] **bug-0023** [LOW] Page-label alpha style wrong past 26 (`AB` instead of `BB`; spec says
      repeated letters). `bugs/bug-0023-page-label-alpha-repetition-wrong.md`

---

### Phase 4 — Documentation & discoverability (do last; reflect the FINAL behavior)

These are pure “fixed in code, doc not updated” (safe anytime) EXCEPT where a Phase 0/2 behavior
change must be documented as part of that fix.  Do the schema/capability ones (0007, 0008) after the
behavior-changing fixes land so they describe the shipped behavior.

- [ ] **bug-0007** [MED] DEBUGGING JSON schemas for `--object`/`--inspect`/`--list` are wrong (and
      never were right). `bugs/bug-0007-docs-json-schemas-inaccurate.md`
- [ ] **bug-0008** [MED] Stale docs: phantom “caution tier”, understated `--text` encoding
      capability, no exit-code table in the AI reference, reliability-always wording, CHANGELOG
      mis-nesting. `bugs/bug-0008-docs-stale-capabilities-and-exit-codes.md`
- [ ] **bug-0006** [LOW] `--help`/README/CLAUDE.md flag-text errors (stale `--page` help,
      `--hex` scope, `regex=` scope, `--depth` row, CLAUDE.md module cells). `bugs/bug-0006-docs-help-and-readme-flag-text-errors.md`
- [ ] **bug-0030** [LOW] `--search` comma-AND syntax + key case-sensitivity undocumented; no comma
      escape. `bugs/bug-0030-search-syntax-comma-and-case-undocumented.md`
- [ ] **bug-0009** [LOW] `--extract-stream --json` (Phase 0 decision → implement or scope docs).
- [ ] **bug-0025** [LOW] `--page` accepted-but-ignored (Phase 0 decision → reject/warn or document).

---

### Lower-tier follow-ups (surfaced by the hunt; not filed as bugs)

Feature gaps and cleanups — file as bugs or feature-plans if they graduate:

- Named destinations (a `/Dest` that is a Name/String) are not resolved through the catalog
  `/Names /Dests` name tree in bookmarks/annotations (`bookmarks.rs::format_dest_value`).  Feature gap.
- Layer→page attribution (`layers.rs::scan_page_for_ocgs`) inherits `/Resources` only one level and
  only scans marked-content OCGs (misses XObject/annotation `/OC`).  Under-reports `pages`.
- `structure.rs` inline-`Dictionary` `/K` elements don’t recurse and drop `page`/`title`/`alt`;
  `/RoleMap` is not applied.  Incompleteness.
- Inline images (`BI`/`ID`/`EI`) are not counted by `--images` (nothing claims they are).
- **Test-suite repairs (do alongside the relevant fix):** `tests/integration.rs` `object_flag_nonexistent_object_fails` asserts only stderr text, not the exit status (bug-0019); revisit
  `parse_object_spec_zero` and `multi_object_missing_reports_error_in_json` (bug-0019); the existing
  `garbled_text_unicode_chars` pins bug-0017; `int_to_alpha_basic` pins bug-0023 only up to 27.
- Trivial: `src/page_info.rs:401` doc-comment says “exit-3 signal” but the path exits 1 (bug-0019
  area).  Usage-error precedence nit: a malformed `--page`/`--search` is detected only after a
  successful load, so `pdf-dump missing.pdf --page 0` exits 1, not 2.

---

## Suggested implementation sequence (why this order)

1. **Phase 0 spec decisions first.** Fixing code to match a wrong or undecided contract is wasted
   work (`~/.claude/rules/strategy-defaults.md`: spec first). bug-0019, 0004, 0033, 0020, 0009,
   0025, 0028, 0017 each need a call before their code fix.
2. **Phase 1 crashes/DoS next** — bug-0013 aborts the _default_ command, so it is the first code
   change regardless of everything else.  The other DoS items are independent one-liners/guards.
3. **Phase 2 silent-wrong-output** — highest user harm after crashes.  Note the intra-phase
   dependencies: bug-0012 before bug-0011 (shared reliability verdict); bug-0034 with bug-0027
   (shared trailer walk); bug-0004 and bug-0033 land here once their Phase 0 decisions are made.
4. **Phase 3 correctness/robustness.**
5. **Phase 4 docs last**, so they describe the behavior that actually shipped.  The pure-stale doc
   fixes (0006, 0007, 0008) are safe to start early but must be reconciled with any behavior change.

Batch by touched file to minimize churn: `validate.rs` (0034, 0027, 0035), `stream.rs` (0004, 0005,
0015, 0001), `text.rs`/reliability (0012, 0011, 0014), `search.rs` (0029, 0028, 0030, 0033-search),
`types.rs`/`lib.rs` exit codes (0019, 0018, 0025).
