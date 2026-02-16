# DESIGN.md — pdf-dump Feature Design

> **Note:** This is the original design document. All P0 and P1 features have been implemented, along with most P2 features. Some flag names have since changed: `--summary` is now `--list`, `--metadata` has been removed (its content is part of the default overview), `--links` has been merged into `--annotations`, and `--structure` is now `--tags`. See CLAUDE.md for the current CLI reference.

## Motivation

When debugging, inspecting, or comparing PDF files, Claude Code currently falls back to writing throwaway Python scripts (PyPDF2, pdfplumber, etc.). `pdf-dump` should replace that workflow entirely — providing targeted queries, structured output, and comparison capabilities that make it the fastest path to understanding a PDF's internals.

Design goals:
- **Targeted queries** — Ask specific questions without dumping everything
- **Machine-parseable output** — JSON mode so Claude Code can reason about structured data
- **Human-readable by default** — Retains the current readable text format
- **Comparison workflows** — Diff two PDFs structurally
- **Self-contained** — No need to write scripts; one CLI invocation answers the question

---

## P0 — Critical Features

### 1. `--search <expr>` — Object Search & Filter

The single most important feature for replacing Python scripts. Instead of `for obj in pdf.objects: if obj.Type == 'Font': print(obj)`, run:

```
pdf-dump file.pdf --search Type=Font
pdf-dump file.pdf --search Subtype=Image
pdf-dump file.pdf --search key=MediaBox
pdf-dump file.pdf --search Type=Font,Subtype=Type1    # AND (all conditions must match)
```

**Search expression syntax:**
- `Key=Value` — Match objects where dictionary key `/Key` has name value `/Value`
- `key=Key` — Match objects that contain dictionary key `/Key` (any value)
- `value=Text` — Match objects containing a string value matching `Text`
- Multiple conditions separated by `,` are ANDed together
- Case-insensitive matching on values

**Output:** For each matching object, print the object header and full contents (same format as `--object N`). Print a count summary at the end: `Found N matching objects.`

**Interaction with other flags:**
- `--decode-streams` works as normal to show stream content
- `--json` wraps results in JSON (see JSON section)
- `--summary` style output if `--search` and `--summary` are combined: one-line-per-match table

**Implementation notes:**
- Iterate `doc.objects`, for each object extract the dictionary (from `Object::Dictionary` or `Object::Stream`), check filter conditions against dict entries
- For `key=X`: `dict.has(X)`
- For `Key=Value`: `dict.get(Key)` matches `Object::Name(Value)`
- Non-dictionary/stream objects never match (they have no keys)
- `--search` is a mode flag (mutually exclusive with `--object`, `--summary`, etc.)

---

### 2. `--json` — Structured JSON Output

JSON output mode for all existing and new modes. This is what makes pdf-dump a proper Claude Code tool — structured data is dramatically easier for an LLM to parse and reason about than formatted text.

**Flag:** `--json` (modifier, works with any mode)

**JSON representation of PDF objects:**

```json
{
  "type": "dictionary",
  "entries": {
    "Type": {"type": "name", "value": "Page"},
    "MediaBox": {"type": "array", "items": [
      {"type": "integer", "value": 0},
      {"type": "integer", "value": 0},
      {"type": "integer", "value": 612},
      {"type": "integer", "value": 792}
    ]},
    "Contents": {"type": "reference", "object_number": 5, "generation": 0}
  }
}
```

**Object type mappings:**
| lopdf Object | JSON `type` field | Value field(s) |
|---|---|---|
| `Null` | `"null"` | (none) |
| `Boolean(b)` | `"boolean"` | `"value": true/false` |
| `Integer(i)` | `"integer"` | `"value": 42` |
| `Real(r)` | `"real"` | `"value": 3.14` |
| `Name(n)` | `"name"` | `"value": "Page"` |
| `String(s, _)` | `"string"` | `"value": "Hello"` (UTF-8 lossy) |
| `Array(a)` | `"array"` | `"items": [...]` |
| `Dictionary(d)` | `"dictionary"` | `"entries": {...}` |
| `Stream(s)` | `"stream"` | `"dict": {...}`, optionally `"content"` |
| `Reference(id)` | `"reference"` | `"object_number": N, "generation": G` |

**Mode-specific JSON structures:**

**Default dump:**
```json
{
  "trailer": { ... },
  "objects": {
    "1:0": { ... },
    "2:0": { ... }
  }
}
```

**`--object N`:**
```json
{
  "object_number": 5,
  "generation": 0,
  "object": { ... }
}
```

**`--summary`:**
```json
{
  "version": "1.4",
  "object_count": 42,
  "objects": [
    {"object_number": 1, "generation": 0, "kind": "Dictionary", "type": "Catalog", "detail": "5 keys"},
    {"object_number": 2, "generation": 0, "kind": "Stream", "type": "XObject", "detail": "1024 bytes, FlateDecode"}
  ]
}
```

**`--metadata`:**
```json
{
  "version": "1.4",
  "object_count": 42,
  "page_count": 3,
  "info": {
    "Title": "My Document",
    "Author": "Jane Doe",
    "CreationDate": "D:20250101120000"
  },
  "catalog": {
    "PageLayout": "/SinglePage",
    "Lang": "en-US"
  }
}
```

**`--search`:**
```json
{
  "query": "Type=Font",
  "match_count": 3,
  "matches": [
    {"object_number": 5, "generation": 0, "object": { ... }},
    {"object_number": 12, "generation": 0, "object": { ... }}
  ]
}
```

**Implementation notes:**
- Use `serde_json` crate for serialization
- Add `serde_json = "1.0"` to dependencies
- Create a parallel set of serialization functions (or a `to_json_value` function for `Object`)
- `--json` flag is a modifier on `DumpConfig`, not a mode

---

### 3. `--text [--page N]` — Text Extraction

Extract readable text from page content streams. This is one of the most common reasons Claude Code writes Python scripts.

```
pdf-dump file.pdf --text                # All pages
pdf-dump file.pdf --text --page 2       # Just page 2
```

**Output (human-readable, default):**
```
--- Page 1 ---
Hello World
This is a PDF document.

--- Page 2 ---
Second page content here.
```

**What operators are extracted:**
- `Tj` — Show string: `(Hello) Tj`
- `TJ` — Show strings with positioning: `[(H) 20 (ello)] TJ`
- `'` — Move to next line and show string
- `"` — Set spacing, move to next line, show string

**Implementation approach:**
- Iterate pages via `doc.get_pages()`
- For each page, find `/Contents` (single ref or array of refs)
- Decode stream(s) using existing `decode_stream()`
- Parse with `lopdf::content::Content::decode()`
- Walk operations, extract text from text-showing operators
- Concatenate string operands, insert spaces between TJ array segments
- Insert newline on `Td`/`TD`/`T*`/`'`/`"` operators when the y-offset is negative (indicating line break)

**Limitations (acceptable for v1):**
- No font encoding/ToUnicode mapping — displays raw bytes as UTF-8
- No reordering by position — text appears in content stream order
- No handling of Type 3 fonts or CIDFonts with complex mappings

**`--text` is a mode flag** (mutually exclusive with other modes).

---

### 4. `--diff <file2.pdf>` — Structural PDF Comparison

Compare two PDFs structurally. Essential for "I modified this PDF and now it's broken — what changed?"

```
pdf-dump file1.pdf --diff file2.pdf
pdf-dump file1.pdf --diff file2.pdf --page 1     # Compare page 1 only
pdf-dump file1.pdf --diff file2.pdf --json        # JSON diff output
```

**Approach: Page-level semantic comparison**

Raw object-ID comparison is fragile (different generators produce different numbering). Instead, compare at the page level:

1. **Metadata comparison** — version, page count, /Info fields
2. **Per-page comparison** — For each page:
   - Compare page dictionary entries (MediaBox, CropBox, Rotate, etc.)
   - Compare resource dictionaries (fonts, images, color spaces)
   - Compare decoded content streams (text-level diff)
3. **Font comparison** — List fonts unique to each file, fonts that differ
4. **Object count** — Total objects in each file

**Output (human-readable):**
```
--- Metadata ---
  Pages: 3 vs 4
  Producer: "LibreOffice" vs "Chrome"

--- Page 1 ---
  MediaBox: [0 0 612 792] vs [0 0 595 842]
  Fonts: identical
  Content stream: differs
    - BT /F1 12 Tf (Hello) Tj ET
    + BT /F1 14 Tf (Hello) Tj ET

--- Page 2 ---
  (identical)

--- Fonts ---
  Only in file1.pdf: /Helvetica-Bold (obj 12)
  Only in file2.pdf: /Arial (obj 15)
```

**`--diff` is a modifier flag** that takes a second PDF path. It works with:
- Default mode: full comparison
- `--page N`: compare only page N
- `--json`: structured diff output

**Implementation notes:**
- Load both documents
- Build comparable representations (normalize by page structure, not object ID)
- For content streams, compare decoded text operations
- For resources, compare by name/value rather than object ID
- Use a simple line-based diff for content stream text

---

## P1 — Very Useful Features

### 5. `--refs-to N` — Reverse Reference Lookup

"What objects reference object N?" — essential for understanding object relationships.

```
pdf-dump file.pdf --refs-to 42
```

**Output:**
```
Objects referencing 42 0:
  Object  3 0: Dictionary (/Type=Pages)     — via /Kids
  Object  7 0: Dictionary (/Type=Page)      — via /Resources
Found 2 objects referencing 42 0.
```

**Implementation:**
- Scan all objects in `doc.objects`
- For each object, recursively walk its structure looking for `Object::Reference((42, 0))`
- Track which dictionary key contained the reference
- This is a mode flag (mutually exclusive with other modes)

---

### 6. `--fonts` — Font Listing

Shortcut for the common query "what fonts does this PDF use?"

```
pdf-dump file.pdf --fonts
```

**Output:**
```
Fonts (5 found):

  Obj#  BaseFont              Subtype    Encoding         Embedded
     5  Helvetica             Type1      WinAnsiEncoding  No
     8  TimesNewRoman         TrueType   Identity-H       Yes (obj 9)
    12  CourierNew             Type1      WinAnsiEncoding  No
    15  Arial,Bold            TrueType   Identity-H       Yes (obj 16)
    20  Symbol                Type1      -                No
```

**Implementation:**
- Search all objects for dictionaries/streams where `/Type` = `Font` or `/Subtype` is a font type
- Also check resource dictionaries for font references
- Extract: BaseFont, Subtype, Encoding, and whether a FontDescriptor with FontFile/FontFile2/FontFile3 exists (embedded)
- This is a mode flag

---

### 7. `--images` — Image Listing

Shortcut for "what images are in this PDF?"

```
pdf-dump file.pdf --images
```

**Output:**
```
Images (3 found):

  Obj#  Width  Height  ColorSpace    BPC  Filter        Size
     6   800    600    DeviceRGB       8  DCTDecode     45,230
    14   200    200    DeviceGray      8  FlateDecode   12,100
    22  1024    768    ICCBased        8  JPXDecode     98,400
```

**Implementation:**
- Search for stream objects where `/Subtype` = `Image` (these are XObject images)
- Extract: Width, Height, ColorSpace, BitsPerComponent, Filter, content length
- This is a mode flag

---

### 8. `--validate` — PDF Health Check

Quick structural validation. Answers "is this PDF well-formed?"

```
pdf-dump file.pdf --validate
```

**Output:**
```
Validation Results:

  [WARN]  Object 42 0: referenced but does not exist (from obj 5, key /Font)
  [WARN]  Object 18 0: unreachable (not referenced from /Root tree)
  [WARN]  Page 3: missing required /MediaBox (not inherited)
  [INFO]  Object 7 0: stream /Length (1024) != actual length (1020)
  [OK]    No broken reference cycles detected
  [OK]    All pages reachable from /Pages tree

Summary: 0 errors, 3 warnings, 1 info
```

**Checks to perform:**
1. **Broken references** — References to nonexistent object IDs
2. **Unreachable objects** — Objects not reachable from /Root (potential garbage)
3. **Missing required keys** — Pages without /MediaBox (considering inheritance), Catalog without /Pages
4. **Stream length mismatches** — /Length value vs actual stream byte count
5. **Page tree integrity** — All pages reachable, /Count matches actual
6. **Reference cycles** — Report (not an error, just info)
7. **Empty streams** — Streams with 0 bytes (may indicate corruption)

**Implementation:**
- Walk from /Root, track reachable objects
- Compare reachable set against all objects
- Check each object against PDF spec requirements for its /Type
- This is a mode flag

---

### 9. `--hex` — Hex Dump for Binary Streams

When `--decode-streams` shows binary content, display a proper hex dump instead of lossy UTF-8.

```
pdf-dump file.pdf --object 42 --decode-streams --hex
```

**Output:**
```
Stream content (decoded, 256 bytes):
  00000000  89 50 4e 47 0d 0a 1a 0a  00 00 00 0d 49 48 44 52  |.PNG........IHDR|
  00000010  00 00 03 20 00 00 02 58  08 06 00 00 00 9a 76 82  |... ...X......v.|
  00000020  70 00 00 00 01 73 52 47  42 00 ae ce 1c e9 00 00  |p....sRGB.......|
```

**Implementation:**
- `--hex` is a modifier flag (works with `--decode-streams`)
- When displaying stream content, if `--hex` is set AND content `is_binary_stream()`, use hex dump format
- Standard hex dump: offset, 16 hex bytes (split 8+8), ASCII printable representation
- Non-printable bytes shown as `.` in ASCII column
- If `--hex` is not set, maintain current behavior (lossy UTF-8)

---

## P2 — Nice to Have

### 10. Additional Stream Filter Decoders

Currently only `FlateDecode` is supported. Add:

- **ASCII85Decode** — Base-85 encoding, common in PostScript-derived PDFs. Decode `<~ ... ~>` sequences.
- **ASCIIHexDecode** — Hex-encoded streams. Decode pairs of hex digits.
- **LZWDecode** — LZW compression (legacy, but still encountered). Use `weezl` or `lzw` crate.

**Implementation:**
- Extend `decode_stream()` to handle a pipeline of filters (apply in order)
- Currently only checks for `FlateDecode` presence; need to iterate the filter array in order and apply each supported filter sequentially
- For unsupported filters in a pipeline, stop decoding and return what we have with a warning

**New dependency:** `weezl = "0.1"` for LZW (if included)

---

### 11. `--tree` — Reference Tree View

Show the object graph as an indented tree (IDs and types only, no full content).

```
pdf-dump file.pdf --tree
```

**Output:**
```
Trailer
  /Root -> 1 0 (Catalog)
    /Pages -> 2 0 (Pages)
      /Kids[0] -> 3 0 (Page)
        /Contents -> 5 0 (Stream, 1024 bytes)
        /Resources -> 6 0 (Dictionary)
          /Font/F1 -> 7 0 (Font)
          /Font/F2 -> 8 0 (Font)
      /Kids[1] -> 4 0 (Page)
        /Contents -> 9 0 (Stream, 512 bytes)
        /Resources -> 6 0 (visited)
  /Info -> 10 0 (Dictionary)
```

**Implementation:**
- Similar to dump mode but only prints the reference skeleton
- Shows key path, object ID, type label, and basic info (stream size)
- Marks revisited nodes as `(visited)` to prevent infinite output
- This is a mode flag

---

### 12. Decode Failure Warnings

Currently, when `ZlibDecoder` fails, `decode_stream()` silently returns the raw content. Add a visible warning.

**Change:** Return a result type or add a warning callback:
```
Stream content (raw, 1024 bytes) [WARNING: FlateDecode decompression failed]:
```

**Implementation:**
- Change `decode_stream` to return `(Cow<[u8]>, Option<String>)` where the second element is an optional warning message
- Display the warning in `print_stream_content` and `print_content_data`

---

### 13. Configurable Truncation

Replace the hardcoded 100-byte limit with a user-specified value.

**Change:** `--truncate-binary-streams` becomes `--truncate <N>` (default: no truncation, specifying `--truncate` without a value uses 100).

```
pdf-dump file.pdf --decode-streams --truncate 500
pdf-dump file.pdf --decode-streams --truncate        # defaults to 100
```

**Implementation:**
- Change `truncate_binary_streams: bool` to `truncate: Option<usize>` in DumpConfig
- Use the value instead of hardcoded 100
- Backward compatibility: could keep `--truncate-binary-streams` as alias for `--truncate 100`

---

### 14. `--depth N` — Traversal Depth Limit

Limit how deep the dump traversal goes. Useful for large PDFs where you only want the top-level structure.

```
pdf-dump file.pdf --depth 2    # Only follow references 2 levels deep
```

**Implementation:**
- Add depth counter to `dump_object_and_children`
- Stop following references when depth exceeds N
- Print `(depth limit reached)` annotation on truncated references
- Default: unlimited (current behavior)

---

## Implementation Priority Order

For making pdf-dump a complete Claude Code PDF debugging tool, implement in this order:

| Order | Feature | Effort | Impact |
|-------|---------|--------|--------|
| 1 | `--json` | Medium | Unlocks machine-readable output for all modes |
| 2 | `--search` | Medium | Targeted queries replace most Python scripts |
| 3 | `--text` | Medium | Most common "why write Python" scenario |
| 4 | `--fonts` | Small | Very common query, easy with `--search` infra |
| 5 | `--images` | Small | Very common query, easy with `--search` infra |
| 6 | `--refs-to` | Small | Essential for understanding object relationships |
| 7 | `--validate` | Medium | Quick health check replaces manual inspection |
| 8 | `--hex` | Small | Proper binary inspection |
| 9 | `--diff` | Large | PDF comparison is complex but very valuable |
| 10 | `--tree` | Small | Quick structural overview |
| 11 | Decode warnings | Small | Better UX |
| 12 | Configurable truncation | Small | Better UX |
| 13 | Additional filters | Medium | Broader PDF coverage |
| 14 | `--depth N` | Small | Useful for large PDFs |

---

## Files to Modify

> **Note:** The codebase has since been split into ~29 source files in `src/`. See CLAUDE.md for the current module layout.

- **`src/`** — Feature implementations split across modules
- **`Cargo.toml`** — Dependencies: `serde_json`, `weezl` (LZW), `flate2` (zlib), etc.
- **`tests/integration.rs`** — Integration tests for all modes
- **`CLAUDE.md`** — CLI reference

---

## Verification Plan

For each feature, verify with:

1. **Unit tests** — Test the core logic function with synthetic `lopdf::Document` objects (following existing test patterns in `src/main.rs`)
2. **Integration tests** — Run the CLI against real PDF files and verify output format
3. **Manual testing** — Test against a variety of real-world PDFs (simple, complex, large, broken)
4. **Cross-mode testing** — Verify `--json` works correctly with every mode
5. **Error cases** — Bad search expressions, nonexistent pages, corrupt PDFs
