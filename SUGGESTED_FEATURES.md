# Suggested Features

Feature review focused on gaps that would benefit an AI agent (e.g. Claude Code) trying to debug or understand a PDF file.

> **Status:** All Tier 1 and Tier 2 features have been implemented. Some were later restructured: `--links` was merged into `--annotations` (which now shows link type and target for Link annotations), and `--structure` was renamed to `--tags`. See CLAUDE.md for the current CLI reference.

---

## Tier 1 — High-Impact Additions (all implemented)

### 1. `--embedded-files` — List/extract file attachments [DONE]

PDFs commonly embed other files (invoices, XML, original source docs). Lists Name Trees under `/Names/EmbeddedFiles`, showing filename, MIME type, size, and the object number of the `EmbeddedFile` stream (which `--extract-stream` can extract).

### 2. `--links` — Extract all hyperlinks and GoTo actions [DONE — merged into --annotations]

Link information is now part of `--annotations`. For Link annotations, the output includes link type (URI, GoTo, GoToR, Named, Launch) and target. Combines with `--page` filter.

### 3. `--page-labels` — Physical-to-logical page number mapping [DONE]

Shows the label-to-physical-page mapping table, including label style (decimal, roman, alphabetic), prefix, and starting value for each range.

### 4. `--security` / `--encryption` — Encryption and permissions info [DONE]

Shows the `/Encrypt` dictionary: algorithm, key length, revision, and permissions bitfield decoded to human-readable flags.

### 5. Enhanced `--search` with regex support [DONE]

Added `regex=<pattern>` condition type: `--search "regex=D:20(23|24)"`.

### 6. Enhanced `--validate` — More structural checks [DONE]

Validation now covers 10+ checks including all items listed below:

- Content stream syntax validation (can the operators be parsed?)
- Font requirements (does each font have a `/BaseFont`? Is `/FirstChar`/`/LastChar` consistent with `/Widths` array length?)
- Page `/MediaBox` presence (required, often inherited — detect when it's truly missing)
- Circular reference detection in page tree `/Parent` chains
- `/Names` tree structure validation
- Duplicate object detection

---

## Tier 2 — Solid Value (all implemented)

### 7. `--layers` / `--ocg` — Optional Content Groups [DONE]

Lists OCGs with name, default visibility state (ON/OFF), and which pages reference them.

### 8. `--tags` (was `--structure`) — Tagged PDF logical structure tree [DONE]

Shows the `/StructTreeRoot` hierarchy with role types, page associations, marked content IDs, titles, and alt text. Supports `--depth N`.

### 9. `--raw` — View undecoded stream bytes [DONE]

Added as a modifier for `--object`. Shows raw pre-decode bytes in-terminal. Combines with `--hex` and `--truncate`.

### 10. Font encoding diagnostics (enhance `--fonts`) [DONE]

`--fonts` now shows:

- Whether a `/ToUnicode` CMap exists and its object number
- `/FirstChar`, `/LastChar`, and `/Widths` array length
- Encoding details beyond just the name (custom `/Differences` array entries)
- For Type0/CID fonts: the descendant font's CIDSystemInfo

---

## Tier 3 — Nice to Have (all implemented)

### 11. `--info N` — Human-readable object role explanation [DONE]

Classifies the object, shows domain-specific details, page associations, and both forward and reverse references. Includes full object content dump.

---

## Enhancements to Existing Features

### 12. `--text` improvements

The current text extraction is raw content-stream order with no encoding awareness. Two pragmatic improvements:

- **Use `/ToUnicode` CMaps when available** — this is the single biggest improvement for text accuracy, and most modern PDFs include them.
- **Basic page-order output** — use `Tm`/`Td`/`TD` coordinates to sort text blocks top-to-bottom, left-to-right within a page, rather than pure stream order.

### 13. `--search` + `--list` should include matched key/value [DONE]

When using `--search` with `--list`, the one-line output now includes the matched key/value in the summary line.

---

## Features NOT Recommended

- **Full PDF/A compliance validation** — too large a scope, better served by dedicated tools.
- **Visual rendering/comparison** — out of scope for a structural tool.
- **Decryption** — significant complexity, and `lopdf` has limited support; better to note the encryption and let the user provide a decrypted version.
- **OCR** — entirely different domain.

---

## Remaining Enhancement

The only unimplemented item is `--text` improvements (#12): ToUnicode CMap support and coordinate-based text ordering. All other suggested features have been implemented.
