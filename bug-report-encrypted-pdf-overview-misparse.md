# Bug Report: overview/`--json` path mis-parses encrypted PDFs (reports `encrypted: false`, 0 objects, 0 pages)

**Severity:** High — silent incorrect output.  For a valid, fully-readable
encrypted PDF, the default overview and `--json` summary report it as **not
encrypted**, with **1 object**, **0 pages**, and **0 streams** — and emit **no
error or warning**.  A consumer that trusts these fields gets wrong answers with
no signal that anything failed.

**Component:** `pdf-dump` — the document overview / summary aggregation path
(the one that fills `encrypted`, `object_count`, `object_types`, `page_count`,
`streams`, and the human-readable overview).  The `--detail security` path is
**not** affected and reads the same file correctly.

**Observed in:** `pdf-dump 0.21.0` (the binary currently on `PATH`).  Repo HEAD
is `v0.22.0`; see “Version note / please verify at HEAD” below.

**Status:** Reproduced with strong evidence and an internal self-contradiction
that localizes the fault.  Root cause not pinned (needs a bisect); likely
related to recent lenient-recovery work and/or the lopdf 0.36 → 0.39 bump.

---

## Summary

Given a valid AES-128 encrypted PDF (classic xref table, `/V 4 /R 4`, `AESV2`),
the overview/`--json` path returns a degraded result:

| field | reported | correct |
|---|---|---|
| `encrypted` | `false` | `true` |
| `object_count` | `1` | `13` |
| `object_types` | `{dictionaries: 1}` | full set |
| `page_count` | `0` | `3` |
| `streams.count` | `0` | several |
| `validation.warning_count` | `0` | (n/a) — no warning that parsing degraded |

Crucially, **`pdf-dump <file> --detail security` reads the very same file
correctly** — it reports `Encryption: Yes, Algorithm: AES-128, Version 4,
Revision 4, Encrypt Object: 13`.  So one code path locates the `/Encrypt`
dictionary at object 13 (and therefore can walk the xref to at least object 13),
while the overview path collapses to a single object and concludes the file is
not encrypted.  That internal contradiction is the key clue: the xref/trailer is
readable; it is the **overview’s object-graph walk** that is bailing out.

## Reproduction

`pdf-maker` and `pdf-dump` are both on `PATH`.

```bash
# Build a small, valid 3-page PDF, then an AES-128 encrypted copy.
pdf-maker -o plain.pdf --blank-page letter
pdf-maker -o plain.pdf plain.pdf all plain.pdf all plain.pdf all
pdf-maker -o enc.pdf plain.pdf all \
  --user-password user --owner-password owner --encryption-algorithm aes128

# Confirm enc.pdf really is encrypted (independent of pdf-dump):
grep -aoE '/Encrypt|/Filter/Standard|/V [0-9]|/R [0-9]|AESV2' enc.pdf | sort | uniq -c
#   2 /Encrypt
#   1 /Filter/Standard
#   1 /R 4
#   1 /V 4
#   1 AESV2

# Overview path — WRONG:
pdf-dump enc.pdf --json | python3 -c \
  "import sys,json; d=json.load(sys.stdin); print('encrypted=',d['encrypted'],'objects=',d['object_count'],'pages=',d['page_count'])"
#   encrypted= False objects= 1 pages= 0

# Security-detail path — CORRECT, same file:
pdf-dump enc.pdf --detail security
#   Encryption: Yes
#   Algorithm:  AES-128
#   Version:    4
#   Revision:   4
#   Key Length: 128 bits
#   Encrypt Object: 13
#   Permissions ...

# Control: the unencrypted source parses fine, proving the structure is sound.
pdf-dump plain.pdf --json | python3 -c \
  "import sys,json; d=json.load(sys.stdin); print('objects=',d['object_count'],'pages=',d['page_count'])"
#   objects= 13 pages= 3
```

## Evidence

`enc.pdf` is a 1,888-byte, `%PDF-1.7`, classic-xref, AES-128 encrypted file
produced by `pdf-maker` (which uses traditional `save()` for encrypted output).

- Independent confirmation it is encrypted: the raw bytes contain `/Encrypt`,
  `/Filter/Standard`, `/V 4`, `/R 4`, `AESV2`.
- `--detail security` parses all of that correctly and names object 13 as the
  `/Encrypt` dict — so the xref is walkable to at least obj 13.
- The same file via `--json` / default overview: `encrypted: false`,
  `object_count: 1`, `page_count: 0`, `streams.count: 0`, and the human overview
  prints `Encrypted: no`, `Objects: 1`, `Pages: 0`.
- The identical-structure **unencrypted** `plain.pdf` reports `object_count: 13`,
  `page_count: 3` — correct.  So the difference is encryption, not structure.

The full `--json` for `enc.pdf`:

```json
{
  "encrypted": false,
  "object_count": 1,
  "object_types": { "dictionaries": 1 },
  "page_count": 0,
  "streams": { "count": 0, "filters": {}, "largest": [], "total_bytes": 0, "total_decoded_bytes": null },
  "validation": { "error_count": 0, "info_count": 0, "issues": [], "warning_count": 0 },
  "version": "1.7"
}
```

## Likely cause (needs a bisect)

Two recent changes are the prime suspects; a `git bisect` between them would
settle it:

- **`v0.21.0` — “Lenient stream recovery for PDFs with wrong /Length (silently
  dropped content).”**  This is the most probable culprit.  In an encrypted PDF,
  stream bodies are ciphertext; before decryption their `/Length` and contents
  are meaningless to a plaintext reader.  If the lenient-recovery path treats an
  encrypted stream as a malformed-`/Length` stream and “recovers” it, it can
  derail the object-graph walk — which matches the collapse to `object_count: 1`
  and `encrypted: false` (the walk never reaches/recognizes the `/Encrypt`
  dict).  Notably this is the same tolerance class that (helpfully) recovers
  other malformed PDFs, but here it appears to **mis-fire on legitimately
  encrypted streams**.
- **`v0.20.1` — lopdf 0.36.0 → 0.39.0 bump.**  An alternative or compounding
  cause if 0.39 changed encrypted-xref/stream handling.

The decisive symptom is that `--detail security` still works while the overview
does not: the trailer/`/Encrypt` are reachable, so the regression is in the
**summary/object-enumeration path**, not in xref parsing per se.

## Impact

- Any caller relying on the overview/`--json` fields (`encrypted`, `page_count`,
  `object_count`, `streams`) for encrypted PDFs gets wrong values **with no
  error and no validation warning** — the worst failure mode for a tool whose
  job is faithful inspection.
- Concretely, it broke a downstream consumer: `pdf-maker`’s encryption
  regression tests (`tests/lopdf_save_modern_bug.rs`) shelled out to
  `pdf-dump --json` to count pages of encrypted PDFs and began failing — not
  because the code under test regressed, but because `pdf-dump` started
  reporting 0 pages.  (Those tests have since been rewritten to be
  self-contained, but the `pdf-dump` regression remains.)

## Suggested fix / hardening

1. **Don’t let lenient stream recovery run on encrypted documents** (or run it
   only after decryption).  Encrypted stream bodies are expected to be
   un-decodable as plaintext; the recovery heuristic should recognize an
   `/Encrypt`-protected document and not treat ciphertext streams as
   malformed-`/Length` content to “recover”.
2. **Detect `/Encrypt` before the object walk** so `encrypted` is set correctly
   regardless of how far enumeration gets.  Reporting `encrypted: false` for a
   file that has a `/Encrypt` trailer entry is the most clearly wrong single
   field, and the `--detail security` path already knows how to find it.
3. **Never silently degrade.**  If the overview path can only enumerate 1 of N
   objects (or cannot decode an encrypted body), emit a validation
   warning/notice (or, with `--strict`, an error) rather than returning
   `object_count: 1 / page_count: 0` as if authoritative.  The page-tree
   dictionaries in a standard encrypted PDF are _not_ encrypted, so page_count
   should be recoverable even without the password.
4. **Optional but valuable:** a `--password`/`--user-password` flag so encrypted
   stream/string content can be decrypted for `--text`, `--object`, etc.  (None
   exists today; `--password` errors as an unexpected argument.)  This is a
   feature, separate from the regression above.

## Version note / please verify at HEAD

The misbehaving binary is `pdf-dump 0.21.0` (on `PATH`).  The repo HEAD is
`v0.22.0` (its title adds observable recovery and a strict-mode gate for
malformed `/Length` streams), which post-dates the suspected `v0.21.0` change
and may already alter this behavior.  Before fixing, **rebuild from HEAD and re-run the reproduction**:

- If HEAD still reports `encrypted: false` / `object_count: 1` / `page_count: 0`
  for `enc.pdf`, apply the fixes above.
- If HEAD already reports it correctly, this regression is fixed-but-unreleased;
  cut a release so the installed binary picks it up.

## Test plan

- Add a fixture: a small, valid AES-128 (and an AES-256, and an RC4) encrypted
  PDF.  Assert the overview/`--json` reports `encrypted: true`, the correct
  `page_count`, and `object_count > 1` — i.e. the summary path agrees with
  `--detail security`.
- Regression assertion tying the two paths together: for any input,
  `json.encrypted` must equal `(--detail security reports encryption)`.  They
  must never contradict.
- A lenient-recovery guard: ensure the recovery heuristic does not fire on
  encrypted stream bodies (or only fires post-decryption).
