# Debugging PDFs with pdf-dump (Claude Code guide)

This document describes how to use `pdf-dump` effectively when debugging PDF files and PDF-generating code. It covers common workflows, output interpretation, the JSON schema, and enough PDF internals to make the output meaningful.

## When to use pdf-dump

Use `pdf-dump` instead of writing Python scripts whenever you need to:

- Understand why a PDF renders incorrectly
- Find what fonts or images a PDF contains
- Compare two versions of a PDF to see what changed
- Check if a PDF is structurally well-formed
- Track down a specific object (form field, annotation, image, font)
- Understand the object graph and reference structure
- Extract raw stream content (content streams, embedded images, etc.)
- Debug hyperlinks and navigation actions
- Investigate encryption and permission restrictions
- Examine page labeling and numbering schemes
- Find embedded file attachments
- Inspect layers (Optional Content Groups) and tagged structure
- Diagnose font encoding problems causing garbled text

## PDF structure primer

A PDF file consists of:

- **Objects** identified by `(object_number, generation)` pairs, e.g., `5 0` means object 5, generation 0. `pdf-dump` assumes generation 0 for all command-line arguments.
- **Object types**: null, boolean, integer, real, name (`/Type`), string (`(Hello)`), array (`[...]`), dictionary (`<< /Key Value >>`), stream (dictionary + byte content), and reference (`5 0 R`).
- **Trailer**: the entry point. Contains `/Root` (the document catalog) and optionally `/Info` (metadata).
- **Catalog** (`/Root`): points to `/Pages` (page tree), `/AcroForm` (forms), `/Outlines` (bookmarks), `/Names` (name trees for embedded files, destinations, etc.), `/OCProperties` (layers), `/StructTreeRoot` (tagged structure), `/PageLabels` (page numbering), etc.
- **Page tree**: `/Pages` dict with `/Kids` array pointing to individual `/Page` objects (or nested `/Pages` nodes).
- **Page**: dictionary with `/MediaBox` (dimensions), `/Contents` (content stream reference), `/Resources` (fonts, images, etc.), `/Annots` (annotations including links).
- **Content streams**: instructions that draw the page. Use operators like `BT`/`ET` (text blocks), `Tf` (set font), `Tj`/`TJ` (show text), `cm` (transform matrix), `Do` (draw XObject).
- **Streams**: any object with both a dictionary and binary content. Streams can be compressed with filters (FlateDecode, LZWDecode, ASCII85Decode, ASCIIHexDecode, RunLengthDecode). Multiple filters can be chained in a pipeline.

## Recommended workflows

### 1. First look at an unknown PDF

Start with the default overview to get the lay of the land:

```bash
pdf-dump file.pdf
```

This shows version, page/object counts, encryption status, all /Info fields, catalog properties, validation summary, stream stats, and feature indicators (bookmarks, forms, layers, embedded files, page labels, tagged structure).

For a one-line-per-object listing:

```bash
pdf-dump file.pdf --list
```

For a quick structural health check:

```bash
pdf-dump file.pdf --validate
```

### 2. Understanding document structure

Use `--tree` for a quick visual map of the object graph:

```bash
pdf-dump file.pdf --tree
pdf-dump file.pdf --tree --depth 3    # limit depth for large files
```

This shows the reference tree from the trailer downward — which objects point to which, via which keys. Revisited nodes are marked to prevent infinite output.

For a human-readable explanation of any specific object:

```bash
pdf-dump file.pdf --info 42
```

This classifies the object (Font, Page, Image, Catalog, etc.), shows domain-specific details, page associations, and both forward and reverse references.

### 3. Finding specific objects

Search by type, key, value, stream content, or regex:

```bash
pdf-dump file.pdf --search Type=Font
pdf-dump file.pdf --search Type=Annot
pdf-dump file.pdf --search key=AcroForm
pdf-dump file.pdf --search Subtype=Image
pdf-dump file.pdf --search value=Helvetica
pdf-dump file.pdf --search stream=text           # search inside decoded streams
pdf-dump file.pdf --search "regex=D:20(23|24)"   # regex across values
```

Multiple conditions are ANDed: `--search Type=Font,Subtype=Type1`

For compact results, add `--list` (includes matched key/value in output):

```bash
pdf-dump file.pdf --search Type=Font --list
```

### 4. Inspecting a specific object

Once you know an object number (from list, search, or tree output):

```bash
pdf-dump file.pdf --object 42
```

This prints the full object without following its references. To also see decoded stream content:

```bash
pdf-dump file.pdf --object 42 --decode-streams
```

For binary streams, add `--hex`:

```bash
pdf-dump file.pdf --object 42 --decode-streams --hex
```

To see references expanded inline (showing target summaries instead of bare `N 0 R`):

```bash
pdf-dump file.pdf --object 42 --deref
```

To see the raw undecoded stream bytes (useful for debugging broken filter chains):

```bash
pdf-dump file.pdf --object 42 --raw
pdf-dump file.pdf --object 42 --raw --hex
pdf-dump file.pdf --object 42 --raw --truncate 256
```

Inspect multiple objects at once:

```bash
pdf-dump file.pdf --object 1,5,10-15
```

### 5. Debugging a specific page

Get structured page info (dimensions, resources, annotations, text preview):

```bash
pdf-dump file.pdf --page 1
pdf-dump file.pdf --page 1 --json
```

Extract text from that page:

```bash
pdf-dump file.pdf --text --page 1
```

See the exact drawing operators:

```bash
pdf-dump file.pdf --operators --page 1
```

See the page's resource maps (fonts, images, graphics state):

```bash
pdf-dump file.pdf --resources --page 1
```

### 6. Font problems

List all fonts with full encoding diagnostics:

```bash
pdf-dump file.pdf --fonts
```

Output includes: Obj#, BaseFont, Subtype (Type1, TrueType, Type0, CIDFontType2), Encoding, embedded status, and encoding diagnostics:

- **ToUnicode**: whether a `/ToUnicode` CMap exists and its object number
- **FirstChar/LastChar**: the character code range and `/Widths` array length
- **Differences**: custom encoding entries from the `/Differences` array (first 5 shown with total count)
- **CIDSystemInfo**: for Type0/CID fonts, the Registry-Ordering-Supplement values

Common issues:
- **Missing embedded font**: the font shows `Embedded: No` but the viewer doesn't have it installed. Look for `/FontDescriptor` and check if `/FontFile`, `/FontFile2`, or `/FontFile3` exists.
- **Wrong encoding**: text appears garbled. Check the `/Encoding` value and whether `/ToUnicode` exists.
- **Subset font name**: names like `ABCDEF+Helvetica` indicate a subset. The prefix before `+` is arbitrary.
- **Width mismatches**: if the `/Widths` array length doesn't match `LastChar - FirstChar + 1`, glyph metrics will be wrong. The `--validate` check catches this.
- **CID fonts without ToUnicode**: `Identity-H` encoding without a ToUnicode CMap means glyph IDs are raw CID values — text extraction will produce garbage.

### 7. Image problems

List all images:

```bash
pdf-dump file.pdf --images
```

Output columns: Obj#, Width, Height, ColorSpace, BPC (bits per component), Filter, and stream Size.

To extract an image's raw stream data:

```bash
pdf-dump file.pdf --extract-stream 42 --output image_raw.bin
```

The extracted data is the decoded stream. For JPEG images (DCTDecode filter), the output is a valid JPEG file. For FlateDecode images, the output is raw pixel data.

### 8. Hyperlinks and navigation

Annotations include link information. List all annotations (including links with their targets):

```bash
pdf-dump file.pdf --annotations
pdf-dump file.pdf --annotations --page 1     # just page 1
```

For Link annotations, the output includes the link type (URI, GoTo, GoToR, Named, Launch) and target URL or destination.

### 9. Comparing two PDFs

When code generates a PDF and the output changes:

```bash
pdf-dump old.pdf --diff new.pdf
pdf-dump old.pdf --diff new.pdf --page 1     # focus on one page
pdf-dump old.pdf --diff new.pdf --json        # structured output
```

The diff compares metadata, page dictionaries, resource dictionaries, decoded content streams, and fonts. It works at the semantic level (by page structure) rather than by raw object IDs, so it handles renumbered objects correctly.

### 10. Structural validation

Check for common PDF problems:

```bash
pdf-dump file.pdf --validate
```

Checks performed (10 total):
- **Broken references**: objects referenced but not present in the file
- **Unreachable objects**: objects that exist but aren't reachable from `/Root`
- **Missing required keys**: pages without `/MediaBox` (considering inheritance), catalog without `/Pages`
- **Stream length mismatches**: declared `/Length` vs actual byte count
- **Page tree integrity**: `/Count` matches actual page count, all pages reachable
- **Empty streams**: streams with 0 bytes (possible corruption)
- **Content stream syntax**: can the operators be parsed without errors?
- **Font requirements**: `/BaseFont` present, `/FirstChar`/`/LastChar` consistent with `/Widths` array length
- **Page tree cycles**: circular reference detection in `/Parent` chains
- **Names tree structure**: validates `/Names` tree nodes are well-formed
- **Duplicate objects**: detects multiple objects sharing the same object number

Severity levels: `[ERROR]`, `[WARN]`, `[INFO]`, `[OK]`

### 11. Reverse reference lookup

"What points to object 42?"

```bash
pdf-dump file.pdf --refs-to 42
```

Reports every object containing a reference to the target, along with the dictionary key path. Useful for understanding how an object fits into the document structure.

### 12. Form fields

List all form fields with their properties:

```bash
pdf-dump file.pdf --forms
```

Shows fully qualified field names, field types (Tx=text, Btn=button, Ch=choice, Sig=signature), current values, flags, and page numbers. Walks hierarchical field trees so nested fields appear with their full path.

For deeper inspection, find the AcroForm:

```bash
pdf-dump file.pdf --search key=AcroForm
```

Then inspect individual field objects:

```bash
pdf-dump file.pdf --object <field_id>
```

Check for `/AP` (appearance streams), `/V` (value), `/FT` (field type), `/Ff` (field flags).

### 13. Annotations

List all annotations grouped by page:

```bash
pdf-dump file.pdf --annotations
pdf-dump file.pdf --annotations --page 1-3
```

Shows subtype (Link, Widget, Text, Highlight, etc.), rectangle, contents, and for Link annotations: link type and target.

### 14. Bookmarks

Show the document outline tree:

```bash
pdf-dump file.pdf --bookmarks
```

Displays titles, destinations, and actions with indentation for nested bookmarks.

### 15. Embedded file attachments

List files embedded in the PDF:

```bash
pdf-dump file.pdf --embedded-files
```

Shows filename, MIME type, size, and the object number of the EmbeddedFile stream. To extract an embedded file:

```bash
pdf-dump file.pdf --extract-stream <stream_obj_number> --output attachment.bin
```

### 16. Layers (Optional Content Groups)

List layers and their visibility:

```bash
pdf-dump file.pdf --layers
```

Shows each OCG's name, default visibility (ON/OFF), and which pages reference it. When content appears to be "missing" visually, hidden layers are often the cause.

### 17. Tagged structure

Show the logical structure tree for tagged (accessible) PDFs:

```bash
pdf-dump file.pdf --tags
pdf-dump file.pdf --tags --depth 3
```

Displays element roles (Document, H1, P, Table, Figure, etc.), page refs, MCIDs, titles, and alt text. One of the richest signals for understanding document layout without visual rendering.

### 18. Page labels

Show the page numbering scheme:

```bash
pdf-dump file.pdf --page-labels
```

Displays the physical-to-logical page number mapping (e.g., pages 1-4 use roman numerals `i, ii, iii, iv`, pages 5+ use decimal starting at 1). Essential when "page 5" means different things to the user vs the PDF.

### 19. Encryption and permissions

Check security restrictions:

```bash
pdf-dump file.pdf --security
```

Shows the encryption algorithm, key length, revision, and decoded permission flags (print: yes/no, copy: yes/no, modify: yes/no, etc.). Useful for understanding why a PDF can't be fully parsed or why certain operations are restricted.

## Using JSON mode

Add `--json` to any mode for structured output. This is the best approach when you need to parse or reason about the results programmatically.

### JSON type schema

Every PDF object maps to a JSON object with a `type` field:

| PDF type | JSON `type` | Value fields |
|----------|-------------|-------------|
| Null | `"null"` | (none) |
| Boolean | `"boolean"` | `"value": true/false` |
| Integer | `"integer"` | `"value": 42` |
| Real | `"real"` | `"value": 3.14` |
| Name | `"name"` | `"value": "Page"` (no leading `/`) |
| String | `"string"` | `"value": "Hello"` (UTF-8 lossy) |
| Array | `"array"` | `"items": [...]` |
| Dictionary | `"dictionary"` | `"entries": {"Key": ...}` |
| Stream | `"stream"` | `"dict": {...}`, optional `"content"` (text), `"content_hex"` (hex), `"content_binary"` (binary summary), `"decode_warning"` |
| Reference | `"reference"` | `"object_number": N, `"generation": G`. With `--deref`, gains a `"resolved"` field. |

### Mode-specific JSON structures

**Default overview** (`--json`):
```json
{
  "version": "1.4",
  "object_count": 42,
  "page_count": 3,
  "encrypted": false,
  "info": {"Title": "...", "Author": "...", "Producer": "..."},
  "catalog": {"PageLayout": "/SinglePage"},
  "features": {
    "bookmark_count": 12,
    "form_field_count": 5,
    "layer_count": 3,
    "embedded_file_count": 0,
    "page_labels": false,
    "tagged_structure": true
  },
  "validation": {...},
  "streams": {...}
}
```

**Dump** (`--dump --json`):
```json
{
  "trailer": { "type": "dictionary", "entries": {...} },
  "objects": { "1:0": {...}, "2:0": {...} }
}
```

**Single object** (`--object N --json`):
```json
{
  "object_number": N,
  "generation": 0,
  "object": { "type": "...", ... }
}
```

**List** (`--list --json`):
```json
{
  "version": "1.4",
  "object_count": 42,
  "objects": [
    {"object_number": 1, "generation": 0, "kind": "Dictionary", "type": "Catalog", "detail": "5 keys"}
  ]
}
```

**Page info** (`--page N --json`):
```json
{
  "page_number": 1,
  "object_number": 5,
  "generation": 0,
  "media_box": "[0 0 612 792]",
  "fonts": ["Helvetica", "TimesRoman"],
  "image_count": 2,
  "annotation_count": 3,
  "content_stream_count": 1,
  "content_stream_bytes": 4523,
  "text": "Full page text..."
}
```

**Search** (`--search <expr> --json`):
```json
{
  "query": "Type=Font",
  "match_count": 3,
  "matches": [
    {"object_number": 5, "generation": 0, "object": {...}}
  ]
}
```

**Text** (`--text --json`):
```json
{
  "pages": [
    {"page_number": 1, "text": "Hello World\n..."}
  ]
}
```

**Validate** (`--validate --json`):
```json
{
  "issues": [...],
  "summary": {"errors": 0, "warnings": 2, "info": 1}
}
```

**Fonts** (`--fonts --json`):
```json
{
  "font_count": 3,
  "fonts": [
    {"object_number": 5, "base_font": "Helvetica", "subtype": "Type1", "encoding": "WinAnsiEncoding", "embedded": false}
  ]
}
```

**Images** (`--images --json`):
```json
{
  "image_count": 2,
  "images": [
    {"object_number": 6, "width": 800, "height": 600, "color_space": "DeviceRGB", "bits_per_component": 8, "filter": "DCTDecode", "size": 45230}
  ]
}
```

**Tree** (`--tree --json`):
```json
{
  "tree": [
    {"key": "/Root", "object_number": 1, "generation": 0, "type_label": "Catalog", "children": [...]}
  ]
}
```

**Diff** (`--diff file2.pdf --json`):
```json
{
  "file1": "old.pdf",
  "file2": "new.pdf",
  "metadata": [...],
  "pages": [...],
  "fonts": {"only_in_first": [...], "only_in_second": [...]},
  "object_count": {"file1": 40, "file2": 42}
}
```

**Info** (`--info N --json`):
```json
{
  "object_number": N,
  "role": "Font",
  "details": {...},
  "page_associations": [...],
  "forward_refs": [...],
  "reverse_refs": [...]
}
```

## Stream filter support

`pdf-dump` decodes these stream filters (applied sequentially for filter pipelines):

| Filter | Description |
|--------|------------|
| FlateDecode | zlib/deflate compression (most common) |
| ASCII85Decode | Base-85 text encoding |
| ASCIIHexDecode | Hex-encoded bytes |
| LZWDecode | LZW compression (legacy) |
| RunLengthDecode | Run-length encoded data |

Unsupported filters (DCTDecode/JPEG, JPXDecode/JPEG2000, JBIG2Decode, CCITTFaxDecode) are reported as warnings. When an unsupported filter is encountered in a pipeline, decoding stops and returns what was decoded so far.

## Content stream operators quick reference

When viewing decoded content streams (`--decode-streams`, `--operators`, or `--text`), these are the most common operators:

### Text operators
| Operator | Meaning |
|----------|---------|
| `BT` / `ET` | Begin/end text block |
| `Tf` | Set font and size: `/F1 12 Tf` |
| `Tj` | Show string: `(Hello) Tj` |
| `TJ` | Show strings with kerning: `[(H) -20 (ello)] TJ` |
| `'` | Move to next line and show string |
| `"` | Set word/char spacing, move to next line, show string |
| `Td` / `TD` | Move text position (dx, dy) |
| `Tm` | Set text matrix |
| `T*` | Move to start of next line |

### Graphics operators
| Operator | Meaning |
|----------|---------|
| `q` / `Q` | Save/restore graphics state |
| `cm` | Concatenate matrix to CTM |
| `Do` | Paint XObject (image or form): `/Im1 Do` |
| `re` | Rectangle path |
| `m` / `l` / `c` | moveto / lineto / curveto |
| `f` / `f*` | Fill path |
| `S` / `s` | Stroke path |
| `W` / `W*` | Clip path |

### Color operators
| Operator | Meaning |
|----------|---------|
| `g` / `G` | Set gray (fill/stroke) |
| `rg` / `RG` | Set RGB (fill/stroke) |
| `k` / `K` | Set CMYK (fill/stroke) |
| `cs` / `CS` | Set color space |
| `sc` / `SC` | Set color |

## Common debugging scenarios

### "Text is garbled or shows wrong characters"

1. Check what fonts are being used and their encoding diagnostics: `pdf-dump file.pdf --fonts`
2. Look at the font's `/Encoding`, whether `/ToUnicode` exists, and whether `/Widths` is consistent with `/FirstChar`/`/LastChar`
3. If the font uses `Identity-H` encoding without a ToUnicode CMap, the glyph IDs are raw CID values, not Unicode — text extraction will produce garbage
4. Check the content stream to see the raw operator calls: `pdf-dump file.pdf --operators --page N`
5. For a quick overview of the object: `pdf-dump file.pdf --info <font_obj_number>`
6. Run validation to catch font issues automatically: `pdf-dump file.pdf --validate`

### "Page appears blank"

1. Check the page info: `pdf-dump file.pdf --page N`
2. If the page has a content stream, verify it isn't empty: `pdf-dump file.pdf --object <contents_id> --decode-streams`
3. Check that the content stream decodes successfully (look for `[WARNING: ...]` in output)
4. The drawing commands might be off-page — check `/MediaBox` dimensions vs the coordinates used in the content stream
5. Check if content is on a hidden layer: `pdf-dump file.pdf --layers`

### "Image doesn't appear"

1. Find the image object: `pdf-dump file.pdf --images`
2. Check the page's `/Resources` dictionary for an `/XObject` entry referencing the image: `pdf-dump file.pdf --resources --page N`
3. Verify the content stream contains a `Do` operator referencing the right name: `pdf-dump file.pdf --operators --page N`
4. Check the image stream's filter — if using an unsupported filter, it may not be the cause of the rendering problem

### "PDF won't open / is corrupted"

1. Run validation: `pdf-dump file.pdf --validate`
2. Check the listing for anomalies: `pdf-dump file.pdf --list`
3. Look for broken references, missing required keys, stream length mismatches, content stream syntax errors, or page tree cycles
4. If the file won't load at all, the problem is likely in the cross-reference table or file structure, which is below pdf-dump's parsing layer (lopdf handles that)

### "Two PDFs look different but should be the same"

1. Use diff mode: `pdf-dump old.pdf --diff new.pdf`
2. Focus on a specific page: `pdf-dump old.pdf --diff new.pdf --page 1`
3. For programmatic analysis: `pdf-dump old.pdf --diff new.pdf --json`

### "Form fields aren't working"

1. List all form fields: `pdf-dump file.pdf --forms`
2. Check field types, values, and flags in the output
3. For deeper inspection: `pdf-dump file.pdf --object <field_id>`
4. Check for `/AP` (appearance streams), `/V` (value), `/FT` (field type), `/Ff` (field flags)
5. Run validation to check for structural issues: `pdf-dump file.pdf --validate`

### "Link doesn't work / goes to wrong place"

1. List annotations to see link targets: `pdf-dump file.pdf --annotations`
2. Filter to the page in question: `pdf-dump file.pdf --annotations --page N`
3. Check the link type (URI, GoTo, GoToR, Named, Launch) and target in the output
4. For GoTo actions, verify the destination page exists: `pdf-dump file.pdf --page-labels` to reconcile physical vs logical page numbers

### "Content is missing but the file seems fine"

1. Check for hidden layers: `pdf-dump file.pdf --layers`
2. Check if content is in an embedded file: `pdf-dump file.pdf --embedded-files`
3. Look at the page's resources to see what's available: `pdf-dump file.pdf --resources --page N`
4. Check annotations (some content lives in annotation appearance streams): `pdf-dump file.pdf --annotations --page N`

### "PDF is too large"

1. Check document statistics: `pdf-dump file.pdf --stats`
2. List images to find large ones: `pdf-dump file.pdf --images`
3. Check the listing for unusually large stream objects: `pdf-dump file.pdf --list`
4. Look for unreachable objects wasting space: `pdf-dump file.pdf --validate`
5. Check for embedded files: `pdf-dump file.pdf --embedded-files`

### "Page numbers don't match"

1. Check the page label mapping: `pdf-dump file.pdf --page-labels`
2. PDFs often use roman numerals for preface pages, then restart numbering at 1 — the physical page number (what pdf-dump uses for `--page N`) may differ from the logical label shown in the viewer

### "PDF has restricted permissions"

1. Check encryption and permissions: `pdf-dump file.pdf --security`
2. The output shows which operations are allowed (print, copy, modify, etc.)
3. If the PDF is encrypted and can't be fully parsed, this tells you why

## Tips

- Always use `--json` when you need to parse the output programmatically or pass it to another tool.
- Use `--depth N` with `--dump`, `--tree`, or `--tags` to avoid drowning in output from large files. Start with `--depth 2` and increase as needed.
- Combine `--decode-streams --truncate 200 --hex` when inspecting binary streams — you get enough to identify the format without flooding the terminal.
- Object numbers are stable within a single file but meaningless across different files. Use `--diff` instead of comparing object numbers between two PDFs.
- The `--search` syntax is case-insensitive on values. `Type=font` matches `/Type /Font`.
- Use `regex=<pattern>` in search for powerful pattern matching: `--search "regex=D:20(23|24)"` finds date-stamped objects.
- When `--text` produces garbage, the issue is almost always font encoding (CID fonts without ToUnicode). Use `--fonts` to check encoding diagnostics.
- Use `--info N` for a quick human-readable explanation of any object — saves multiple round trips of `--object` + `--refs-to`.
- Use `--deref` with `--object` to see reference targets inline, avoiding follow-up `--object` calls.
- Use `--raw` to see undecoded stream bytes when debugging broken filter chains ("is the compressed data itself corrupt?").
- Start with `--validate` when investigating any "broken PDF" report — it catches 10 categories of structural problems automatically.
- Use `--operators --page N` instead of `--object <contents_id> --decode-streams` when you only need to see the drawing instructions without the full object dump.
