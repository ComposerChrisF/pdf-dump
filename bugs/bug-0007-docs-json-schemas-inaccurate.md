# bug-0007: DEBUGGING JSON schemas for `--object`, `--inspect`, and `--list` are wrong

**Severity:** Medium
**Classification:** SPEC-DOC bug
**Status:** Verified (empirical; the schemas were wrong from the moment the doc was written at v0.12.5)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
The JSON schemas documented in `DEBUGGING_WITH_PDF_DUMP.md` for `--object`, `--inspect`, and `--list`
do not match the actual output.  A JSON consumer written from this document breaks: the field names,
the wrapper shape, and several inspect keys are all wrong.

## Affected code / docs
- `DEBUGGING_WITH_PDF_DUMP.md` — “Object type mapping” (~lines 81-95), §Object (line 106), §Inspect
  (lines 114-115), §List (108-109).
- Code (authoritative): `src/object.rs:427,478,364-399`; `src/inspect.rs:640-680`; `src/summary.rs`
  (`list_json_value`).

## What it does vs. what it should do (each confirmed by running the binary)
1. Dictionary documented as `"keys": {...}` → actual field is `"entries"` (`object.rs:427`).
2. Reference documented as `"object": "N M"` → actual is
   `{"type":"reference","object_number":N,"generation":G}` (plus `"resolved"` with `--deref`)
   (`object.rs:478`).
3. `--object N --json` documented as a bare “type mapping” → actual is a wrapper
   `{object_number, generation, object:{...}}`; multiple numbers yield `{"objects":[...]}`; a missing
   object yields `{"error":"..."}`.
4. `--inspect N --json` documented keys
   `{role, description, details:[[key,value]], domain_details, page_associations, forward_references,
   reverse_references, object}` → actual keys
   `{role, description, kind, object_number, generation, details:{map}, page_associations,
   references:[{object_number,generation,path,summary}],
   referenced_by:[{object_number,generation,kind,type,via_keys}], object}`.  The documented
   `domain_details`, `forward_references`, `reverse_references`, and pair-array `details` do not exist.
5. `--list --json` documented as `{objects:[{...,details}]}` → actual is
   `{version, object_count, objects:[...]}` and the per-item field is `detail` (singular).

## Reproduction
```
pdf-dump two.pdf --object 1 --json
pdf-dump two.pdf --inspect 1 --json
pdf-dump two.pdf --list --json
```

Compare each against the documented schema.

## Suggested fix
Rewrite these four schema blocks in `DEBUGGING_WITH_PDF_DUMP.md` to match the actual `*_json_value`
output.  The code is tested and internally consistent, so the documentation changes, not the code.  Do
this after any behavior-changing fixes land so the schemas describe the shipped output (note the
missing-object error shape may change with [[bug-0019-missing-object-modes-exit-zero]]).

## Why the fix addresses the bug
These are the machine-facing schemas from which agents generate parsers; aligning them with the actual
output stops those parsers from breaking.

## Related
[[bug-0008-docs-stale-capabilities-and-exit-codes]], [[bug-0006-docs-help-and-readme-flag-text-errors]],
[[bug-0019-missing-object-modes-exit-zero]] (may change the JSON error shape).
