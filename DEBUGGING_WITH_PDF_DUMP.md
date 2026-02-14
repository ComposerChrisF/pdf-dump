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

## PDF structure primer

A PDF file consists of:

- **Objects** identified by `(object_number, generation)` pairs, e.g., `5 0` means object 5, generation 0. `pdf-dump` assumes generation 0 for all command-line arguments.
- **Object types**: null, boolean, integer, real, name (`/Type`), string (`(Hello)`), array (`[...]`), dictionary (`<< /Key Value >>`), stream (dictionary + byte content), and reference (`5 0 R`).
- **Trailer**: the entry point. Contains `/Root` (the document catalog) and optionally `/Info` (metadata).
- **Catalog** (`/Root`): points to `/Pages` (page tree), `/AcroForm` (forms), `/Outlines` (bookmarks), etc.
- **Page tree**: `/Pages` dict with `/Kids` array pointing to individual `/Page` objects (or nested `/Pages` nodes).
- **Page**: dictionary with `/MediaBox` (dimensions), `/Contents` (content stream reference), `/Resources` (fonts, images, etc.).
- **Content streams**: instructions that draw the page. Use operators like `BT`/`ET` (text blocks), `Tf` (set font), `Tj`/`TJ` (show text), `cm` (transform matrix), `Do` (draw XObject).
- **Streams**: any object with both a dictionary and binary content. Streams can be compressed with filters (FlateDecode, LZWDecode, ASCII85Decode, ASCIIHexDecode). Multiple filters can be chained in a pipeline.

## Recommended workflows

### 1. First look at an unknown PDF

Start with metadata and summary to get the lay of the land:

```bash
pdf-dump file.pdf --metadata
pdf-dump file.pdf --summary
```

Metadata gives you version, page count, producer, and catalog properties. Summary gives a one-line-per-object table so you can see what types of objects exist and how large they are.

### 2. Understanding document structure

Use `--tree` for a quick visual map of the object graph:

```bash
pdf-dump file.pdf --tree
pdf-dump file.pdf --tree --depth 3    # limit depth for large files
```

This shows the reference tree from the trailer downward — which objects point to which, via which keys. Revisited nodes are marked to prevent infinite output.

### 3. Finding specific objects

Search by type, key, or value:

```bash
pdf-dump file.pdf --search Type=Font
pdf-dump file.pdf --search Type=Annot
pdf-dump file.pdf --search key=AcroForm
pdf-dump file.pdf --search Subtype=Image
pdf-dump file.pdf --search value=Helvetica
```

Multiple conditions are ANDed: `--search Type=Font,Subtype=Type1`

For compact results, add `--summary`:

```bash
pdf-dump file.pdf --search Type=Font --summary
```

### 4. Inspecting a specific object

Once you know an object number (from summary, search, or tree output):

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

### 5. Debugging a specific page

Dump only the objects reachable from page N:

```bash
pdf-dump file.pdf --page 1
pdf-dump file.pdf --page 1 --decode-streams
```

Extract text from that page:

```bash
pdf-dump file.pdf --text --page 1
```

### 6. Font problems

List all fonts with embedding status:

```bash
pdf-dump file.pdf --fonts
```

Output columns: Obj#, BaseFont, Subtype (Type1, TrueType, Type0, CIDFontType2), Encoding, and whether the font program is embedded (with the object number of the font file if so).

Common issues:
- **Missing embedded font**: the font shows `Embedded: No` but the viewer doesn't have it installed. Look for `/FontDescriptor` and check if `/FontFile`, `/FontFile2`, or `/FontFile3` exists.
- **Wrong encoding**: text appears garbled. Check the `/Encoding` value and whether a `/ToUnicode` CMap exists.
- **Subset font name**: names like `ABCDEF+Helvetica` indicate a subset. The prefix before `+` is arbitrary.

### 7. Image problems

List all images:

```bash
pdf-dump file.pdf --images
```

Output columns: Obj#, Width, Height, ColorSpace, BPC (bits per component), Filter, and stream Size.

To extract an image's raw stream data:

```bash
pdf-dump file.pdf --extract-object 42 --output image_raw.bin
```

The extracted data is the decoded stream. For JPEG images (DCTDecode filter), the output is a valid JPEG file. For FlateDecode images, the output is raw pixel data.

### 8. Comparing two PDFs

When code generates a PDF and the output changes:

```bash
pdf-dump old.pdf --diff new.pdf
pdf-dump old.pdf --diff new.pdf --page 1     # focus on one page
pdf-dump old.pdf --diff new.pdf --json        # structured output
```

The diff compares metadata, page dictionaries, resource dictionaries, decoded content streams, and fonts. It works at the semantic level (by page structure) rather than by raw object IDs, so it handles renumbered objects correctly.

### 9. Structural validation

Check for common PDF problems:

```bash
pdf-dump file.pdf --validate
```

Checks performed:
- **Broken references**: objects referenced but not present in the file
- **Unreachable objects**: objects that exist but aren't reachable from `/Root`
- **Missing required keys**: pages without `/MediaBox` (considering inheritance), catalog without `/Pages`
- **Stream length mismatches**: declared `/Length` vs actual byte count
- **Page tree integrity**: `/Count` matches actual page count, all pages reachable
- **Empty streams**: streams with 0 bytes (possible corruption)

Severity levels: `[ERROR]`, `[WARN]`, `[INFO]`, `[OK]`

### 10. Reverse reference lookup

"What points to object 42?"

```bash
pdf-dump file.pdf --refs-to 42
```

Reports every object containing a reference to the target, along with the dictionary key path. Useful for understanding how an object fits into the document structure.

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
| Reference | `"reference"` | `"object_number": N, "generation": G` |

### Mode-specific JSON structures

**Default dump** (`--json`):
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

**Summary** (`--summary --json`):
```json
{
  "version": "1.4",
  "object_count": 42,
  "objects": [
    {"object_number": 1, "generation": 0, "kind": "Dictionary", "type": "Catalog", "detail": "5 keys"}
  ]
}
```

**Metadata** (`--metadata --json`):
```json
{
  "version": "1.4",
  "object_count": 42,
  "page_count": 3,
  "info": {"Title": "...", "Author": "..."},
  "catalog": {"PageLayout": "/SinglePage"}
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

## Stream filter support

`pdf-dump` decodes these stream filters (applied sequentially for filter pipelines):

| Filter | Description |
|--------|------------|
| FlateDecode | zlib/deflate compression (most common) |
| ASCII85Decode | Base-85 text encoding |
| ASCIIHexDecode | Hex-encoded bytes |
| LZWDecode | LZW compression (legacy) |

Unsupported filters (DCTDecode/JPEG, JPXDecode/JPEG2000, JBIG2Decode, CCITTFaxDecode) are reported as warnings. When an unsupported filter is encountered in a pipeline, decoding stops and returns what was decoded so far.

## Content stream operators quick reference

When viewing decoded content streams (`--decode-streams` or `--text`), these are the most common operators:

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

1. Check what font is being used: `pdf-dump file.pdf --fonts`
2. Look at the font's `/Encoding` and whether `/ToUnicode` exists
3. If the font uses `Identity-H` encoding without a ToUnicode CMap, the glyph IDs are raw CID values, not Unicode — text extraction will produce garbage
4. Check the content stream to see the raw operator calls: `pdf-dump file.pdf --page N --decode-streams`

### "Page appears blank"

1. Check the page has a `/Contents` reference: `pdf-dump file.pdf --page N`
2. Verify the content stream isn't empty: `pdf-dump file.pdf --object <contents_id> --decode-streams`
3. Check that the content stream decodes successfully (look for `[WARNING: ...]` in output)
4. The drawing commands might be off-page — check `/MediaBox` dimensions vs the coordinates used in the content stream

### "Image doesn't appear"

1. Find the image object: `pdf-dump file.pdf --images`
2. Check the page's `/Resources` dictionary for an `/XObject` entry referencing the image
3. Verify the content stream contains a `Do` operator referencing the right name
4. Check the image stream's filter — if using an unsupported filter, it may not be the cause of the rendering problem

### "PDF won't open / is corrupted"

1. Run validation: `pdf-dump file.pdf --validate`
2. Check the summary for anomalies: `pdf-dump file.pdf --summary`
3. Look for broken references, missing required keys, or stream length mismatches
4. If the file won't load at all, the problem is likely in the cross-reference table or file structure, which is below pdf-dump's parsing layer (lopdf handles that)

### "Two PDFs look different but should be the same"

1. Use diff mode: `pdf-dump old.pdf --diff new.pdf`
2. Focus on a specific page: `pdf-dump old.pdf --diff new.pdf --page 1`
3. For programmatic analysis: `pdf-dump old.pdf --diff new.pdf --json`

### "Form fields aren't working"

1. Find the AcroForm: `pdf-dump file.pdf --search key=AcroForm`
2. Find field objects: `pdf-dump file.pdf --search Type=Annot`
3. Look at a specific field's properties: `pdf-dump file.pdf --object <field_id>`
4. Check for `/AP` (appearance streams), `/V` (value), `/FT` (field type)

### "PDF is too large"

1. List images to find large ones: `pdf-dump file.pdf --images`
2. Check the summary for unusually large stream objects: `pdf-dump file.pdf --summary`
3. Look for unreachable objects wasting space: `pdf-dump file.pdf --validate`

## Tips

- Always use `--json` when you need to parse the output programmatically or pass it to another tool.
- Use `--depth N` with the default dump or `--tree` to avoid drowning in output from large files. Start with `--depth 2` and increase as needed.
- Combine `--decode-streams --truncate 200 --hex` when inspecting binary streams — you get enough to identify the format without flooding the terminal.
- Object numbers are stable within a single file but meaningless across different files. Use `--diff` instead of comparing object numbers between two PDFs.
- The `--search` syntax is case-insensitive on values. `Type=font` matches `/Type /Font`.
- When `--text` produces garbage, the issue is almost always font encoding (CID fonts without ToUnicode), not a bug in pdf-dump.
