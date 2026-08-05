# bug-0031: `--detail security` reads a key length from the next object (raw encrypt-dict parse is unbounded)

**Severity:** High
**Classification:** CODE bug (silent wrong output — an absent key read from a neighbor)
**Status:** Verified (live repro: `pdf-dump enc-trad.pdf --detail security` prints `Key Length: 35 bits`; correct is 40)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
When lopdf has consumed the encrypt dictionary during decryption, `--detail security` falls back to
parsing the encrypt object out of the raw file bytes.  That fallback takes a fixed ~1024-byte window
and never bounds it at the encrypt dictionary’s closing `>>`, so `extract_int_after_key` scans on
into the _following_ object.  Any key absent from the encrypt dictionary is silently satisfied by the
next object’s value — reporting a wrong key length (and, in a related case, wrong permissions).

## Affected code
- `src/security.rs:203-237` — `parse_encrypt_from_raw_file`; the fixed-size window at `:213-220`
  (`window = &data[start..start+1024]`, `dict_bytes = &window[dict_start..]`) with no upper bound at
  the dict’s `>>`.
- `src/security.rs:241-272` — `extract_int_after_key`, which scans the whole unbounded slab and also
  matches a key as a raw byte substring with no delimiter check.

## What it does vs. what it should do
After locating `{n} 0 obj`, the code finds `<<` and treats everything from there to the end of the
1024-byte window as the dictionary.  For an RC4/V1 encrypt dictionary that has no `/Length` (the V1
default key length is 40 bits), the first `/Length` in the window belongs to the _next_ object (an
XRef stream’s `/Length 35`), so the tool reports `Key Length: 35 bits`.  It should bound `dict_bytes`
at the matching `>>` (or the first `endobj`) so that an absent key returns `None` and the caller
falls back to the correct V-based default.

Related (same function, lower confidence, not separately reproduced): `extract_int_after_key`
matches `/P` as a raw substring, so it can match the `/P` prefix of `/Perms` in R6/AES-256 encrypt
dictionaries; if `/Perms` precedes `/P`, the permission bits default to 0 (all-denied).

## Reproduction
Create an empty-user-password RC4/V1 PDF whose encrypt object lopdf consumes on decrypt (so the raw
fallback runs) and whose encrypt dictionary has no `/Length`:

```
<< /Filter /Standard /V 1 /R 2 /O (…) /U (…) /P -4 >>
```

followed by an XRef stream carrying `/Length 35`.  Then:

```
pdf-dump enc-trad.pdf --detail security
# Key Length: 35 bits      (WRONG — should be 40)
```

Fixture `enc-trad.pdf` is in the review scratchpad.  Assert the reported key length is 40.

## Suggested fix
In `parse_encrypt_from_raw_file`, after finding `<<`, scan forward tracking `<<`/`>>` nesting and set
`dict_bytes` to end at the matching `>>` (or clamp at the first `endobj`), before calling
`extract_int_after_key`.  Add a word-boundary/delimiter check in `extract_int_after_key` so `/P` does
not match `/Perms`.

## Why the fix addresses the bug
Bounding the dictionary makes “key absent” resolve to the correct default (from `/V`) rather than to
the next object’s field, and the delimiter check prevents the `/P`-vs-`/Perms` mismatch.

## Related
`~/.claude/rules/positive-evidence-of-absence.md`: an unreadable/absent key must not be read as a
neighbor’s value.
