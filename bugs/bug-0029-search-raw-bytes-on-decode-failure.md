# bug-0029: `--search stream=` silently matches raw bytes when stream decoding fails

**Severity:** High
**Classification:** CODE bug + SPEC-CODE mismatch
**Status:** Verified (live repro — fixture with a non-zlib `/FlateDecode` body containing `SECRET`: `--search stream=SECRET` printed “Found 1 matching objects.”, exit 0, empty stderr)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary

`--help` and the debugging guide promise that `stream=<text>` matches _decoded_ stream content.  When decoding fails — corrupt Flate data, or an unsupported filter such as `DCTDecode` or `JBIG2Decode` — `decode_stream` returns the raw (still-encoded) bytes together with a warning, and `object_matches` discards the warning and searches those raw bytes anyway.  The failure is silent in both directions: garbage matches against compressed bytes surface as false hits, and content present only in decoded form is missed with a confident “Found 0 matching objects.”.  No stderr warning, no JSON field, exit 0.

## Affected code

- `src/search.rs:85-94` — `object_matches` lazily decodes the stream; the discard is at line 87: `let (decoded, _) = decode_stream(stream);`.  The warning that says “these bytes are not decoded content” is thrown away.  (The brief cited `:85-90`; the block actually spans `:85-94`, with the discard at `:87`.)
- `src/stream.rs:204-215` — `decode_stream`’s contract: on an unsupported filter (`:204-210`) or a per-filter decode failure (`:212-215`) it returns the input bytes plus `Some(warning)`.  The contract is fine; the search caller ignores the second tuple element.
- `src/search.rs:237-258` — `search_json_value` matches through the same `object_matches` and emits no warning field, so `--json` consumers get no signal either.
- `src/types.rs:112` — the `--help` claim being violated: `stream=<text>` “Decoded stream content contains `<text>` (case-insensitive)”.  Same claim at `DEBUGGING_WITH_PDF_DUMP.md:34`.

## What it does vs. what it should do

The matcher does this:

```rust
let decoded_content = if needs_stream {
    if let Object::Stream(stream) = obj {
        let (decoded, _) = decode_stream(stream);   // warning discarded
        Some(decoded)
    } else {
        None
    }
} else {
    None
};
```

When `decode_stream` fails, `decoded` is the _raw_ stream body — zlib bytes, JPEG data, whatever the file holds — and `StreamContains` / `RegexMatch` (`src/search.rs:119-128` and `:148-154`) then run a case-insensitive byte search over it as though it were the decoded content the help promised.  Every _other_ decode consumer in the crate (`print_stream_content`, `object_to_json`, `read_content_streams`) propagates the warning; the search path is the one consumer that drops it.

Correct behavior: bytes the tool itself flagged as undecoded must never be searched as if they were decoded content.  An object whose stream could not be decoded is _Unknown_ with respect to a `stream=` condition — not a match, and not a provable non-match either.  Reporting “Found 0” after never actually examining the decoded content is a false absence claim (the positive-evidence-of-absence rule applies directly).

**SPEC DECISION REQUIRED** — the surfacing contract needs Chris’s call before coding; do not blindly fix:

- **Skip-and-warn, exit 0:** objects with a decode warning never match `stream=`/`regex=`; a stderr note and a JSON `warnings` array name each skipped object and its filter/message.
- **Skip-and-warn, exit 3 (recommended):** as above, and any skipped object makes the run findings (exit 3) per the `cli-exit-codes` rule, because the search did not fully run over the data it was asked to search.

Either way the fix must not silently `return false` — that converts the current false hits into more silent false misses, which is the worse direction.

## Reproduction

Shell (as verified live): build a PDF whose object 5 is a stream declaring `/Filter /FlateDecode` but whose body is _not_ zlib and contains the bytes `SECRET`.  Then:

```
pdf-dump fixture.pdf --search stream=SECRET
```

Current (buggy): the object prints as a match, “Found 1 matching objects.”, exit 0, empty stderr.  Correct: no silent hit — a warning naming object 5 and the decode failure, the object skipped (or explicitly flagged), and the exit code per the decision above.  The fixture used for the live verification is `fixture.pdf` in the session scratchpad.

Unit-test sketch (in the `src/search.rs` tests module, helpers from `crate::test_utils`):

```rust
#[test]
fn stream_contains_does_not_silently_match_raw_bytes() {
    // Filter says FlateDecode, but the body is NOT zlib, so decode_stream
    // returns the raw bytes plus Some(warning).
    let stream = make_stream(
        Some(Object::Name(b"FlateDecode".to_vec())),
        b"xxSECRETxx".to_vec(),
    );
    let conds = parse_search_expr("stream=secret").unwrap();
    // CURRENT (buggy): true — the raw bytes matched.
    // CORRECT: no silent hit (assertion shape follows the decided contract).
    assert!(!object_matches(&Object::Stream(stream), &conds));
}
```

The false-miss direction can be pinned at the `search_objects`/`search_json_value` level: a document containing only such an object must not answer a plain “Found 0 matching objects.” with empty stderr — the JSON must carry a warning entry for the undecodable object.

## Suggested fix

In `object_matches` (`src/search.rs:85-94`), keep the warning: `let (decoded, warning) = decode_stream(stream);`.  When `warning.is_some()`, do not evaluate `StreamContains`/`RegexMatch` against the returned bytes; instead surface the warning.  That requires widening the function’s return (or adding an out-parameter) so `search_objects` (`:159-220`) can print per-object stderr notes and `search_json_value` (`:237-258`) can emit a `warnings` field, and so `run()` can map “objects were skipped” onto the decided exit code.  Implement only after the **SPEC DECISION** above is made (recommended: skip-and-warn with exit 3).

## Why the fix addresses the bug

Not matching against bytes the tool itself flagged as undecoded eliminates both failure directions at the root: no false hits against compressed garbage, and no confident “not found” over content that was never actually examined — the warning the tool already computes finally reaches the caller.

## Related

- [[bug-0004-decodeparms-predictor-ignored]] — the same search path is also defeated by predictor-coded streams, where decoding “succeeds” (no warning at all) yet the bytes are still not the decoded content.
- Portfolio rule `positive-evidence-of-absence` — a stream the tool could not decode is Unknown, never “does not contain the text”; “Found 0” must mean “looked and it was not there”.
- Portfolio rule `cli-exit-codes` — findings (the tool ran but could not fully examine the data) belong on exit 3, never silent 0.
- No existing test pins the buggy behavior: every `StreamContains` test in `src/search.rs` (`search_stream_contains_matches` at `:614`, `search_stream_contains_case_insensitive` at `:630`, `search_stream_no_match` at `:641`, `search_stream_and_key_combined` at `:664`) uses an _unfiltered_ stream, so the decode-failure path is entirely uncovered.
