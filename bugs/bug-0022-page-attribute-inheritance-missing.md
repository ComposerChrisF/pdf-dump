# bug-0022: `--page` info omits inherited `MediaBox` / `CropBox` / `Rotate`

**Severity:** High
**Classification:** CODE bug (silent wrong output on a large fraction of real PDFs)
**Status:** Verified (code-trace; asymmetric with the Resources handling in the same function)
**Affected version:** v0.24.0 (HEAD 7353ccb)

## Summary
`collect_page_info` reads `MediaBox`, `CropBox`, and `Rotate` only from the page dictionary directly.
These attributes are inheritable, so a page that omits them and inherits from an ancestor `/Pages`
node — extremely common — is reported as having `MediaBox: -` (and no `Rotate`), wrongly implying it
has no media box.

## Affected code
- `src/page_info.rs:92-100` — reads `MediaBox`/`CropBox`/`Rotate` via `page_dict.get(...)` on the
  page dictionary only, with no parent-chain walk.

## What it does vs. what it should do
PDF 32000-1 §7.7.3.4 (Table 31) lists `MediaBox`, `CropBox`, and `Rotate` as _inheritable_ page
attributes: a `/Page` may omit them and inherit from an ancestor `/Pages` node, and many generators
set `MediaBox` once on the root `Pages` node.  For such a document, `--page N` prints `MediaBox: -` and
the JSON emits `"media_box": "-"`.  The command is internally inconsistent: `resources.rs::resolve_page_resources`
(called at `page_info.rs:103`) _does_ walk the parent chain for `/Resources`, so fonts and XObjects
are shown inherited while the geometry is not.

The box and rotate attributes should be resolved by walking the `/Parent` chain, exactly as Resources
already are.

## Reproduction
Build a document whose `/Page` has no `MediaBox` but whose parent `/Pages` sets
`MediaBox [0 0 612 792]` and `Rotate 90`:

```rust
// page dict: { /Type /Page /Parent <pages> }   (no MediaBox, no Rotate)
// pages dict: { /Type /Pages /Kids [...] /MediaBox [0 0 612 792] /Rotate 90 }
page_info::print_page_info(&mut buf, &doc, &PageSpec::Single(1));
```

Current output shows `MediaBox: -` and no `Rotate:` line; expected `[0 0 612 792]` and `Rotate: 90`.
(The existing tests all set `MediaBox` directly on the page, so this gap is untested.)

## Suggested fix
Add a shared inheritance helper that, for `MediaBox`/`CropBox`/`Rotate`, walks up `/Parent` until the
key is found (or the root is reached), and use it in `collect_page_info`.  Reuse or mirror the
parent-walk already present for Resources.

## Why the fix addresses the bug
Walking the parent chain surfaces the inherited geometry the spec guarantees, making the reported box
and rotation correct and consistent with how the same command already treats Resources.
