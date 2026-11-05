# bug-0042: A link target that will not decode comes back as hex with exit 0

**Severity:** Medium
**Classification:** CODE bug
**Status:** Verified (invented fixture run through v0.32.0, 4-Nov-2026)
**Affected version:** v0.32.0

## Summary
`--annotations` exits 3 when a comment will not decode, and `--help` says so.  A link’s `/URI` that will not decode is reported as hex in `target` with exit 0, and no `target_decoded` says which happened.  A caller reading `target` cannot tell an unreadable address from an address that looks odd, so an unread string passes as clean.

## Reproduction
A page whose one annotation is:

```
<</Type/Annot/Subtype/Link/Rect[0 0 9 9]/A<</S/URI/URI <FEFF00>>>>>
```

```
pdf-dump f.pdf --annotations --json    # "link_type": "URI", "target": "<FEFF00>", exit 0
```

The same bytes in `/Contents` give `contents_decoded: false` and exit 3.

Assert: `target_decoded: false` and exit 3, and `true` with exit 0 for a sound target.

## Notes
- `annotations::classify_link` reads `/URI`, and the `/F` of GoToR and Launch and the `/N` of Named, through `helpers::obj_to_text`, which keeps `decode_text_string(bytes).text` and drops its `decoded` flag.  That is the fault to fix.  The list comes from reading `classify_link`; `obj_to_text`’s callers in other modes were not walked, and each wants checking for the same loss.
- A value of the wrong type reads as `?` with exit 0 in the same arms, which is the same fault.
- ISO 32000-1 Table 206 makes `/URI` an ASCII string, not a text string, so `FE FF` has no standing there.  Whether a URI should go through the text-string decoder at all belongs to the fix.
- Reported by Obsidian-Brain’s gate, which reads `target`.
