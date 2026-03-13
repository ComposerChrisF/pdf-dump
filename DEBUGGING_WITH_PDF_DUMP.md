# pdf-dump: AI Reference Guide

Use `pdf-dump` instead of writing Python/shell scripts to inspect PDF internals. This guide covers everything needed to use the tool and interpret its output.

## Quick lookup: "I need to..." → command

| Goal | Command |
|------|---------|
| Overview of unknown PDF | `pdf-dump f.pdf` |
| Overview with compression ratios | `pdf-dump f.pdf --decode` |
| Page dimensions, resources, text preview | `pdf-dump f.pdf --page 3` (or `--page 1-5`) |
| Extract text | `pdf-dump f.pdf --text` (or `--text --page 3`) |
| Search for text across pages | `pdf-dump f.pdf --find-text "word"` |
| List all fonts | `pdf-dump f.pdf --fonts` |
| List all images | `pdf-dump f.pdf --images` |
| List form fields | `pdf-dump f.pdf --forms` |
| Show bookmarks | `pdf-dump f.pdf --bookmarks` |
| Show annotations/links | `pdf-dump f.pdf --annotations` (or `--annotations --page 1`) |
| Show tagged structure | `pdf-dump f.pdf --tags` (or `--tags --depth 3`) |
| Object graph tree | `pdf-dump f.pdf --tree` (or `--tree --depth 3`) |
| Structural validation | `pdf-dump f.pdf --validate` |
| Encryption/permissions | `pdf-dump f.pdf --detail security` |
| Embedded files | `pdf-dump f.pdf --detail embedded` |
| Page numbering scheme | `pdf-dump f.pdf --detail labels` |
| Layers (OCGs) | `pdf-dump f.pdf --detail layers` |
| One-line listing of all objects | `pdf-dump f.pdf --list` |
| Print specific object | `pdf-dump f.pdf --object 42` |
| Print object with decoded stream | `pdf-dump f.pdf --object 42 --decode` |
| Print object with refs expanded | `pdf-dump f.pdf --object 42 --deref` |
| Raw undecoded stream bytes | `pdf-dump f.pdf --object 42 --raw --hex` |
| Explain object's role and refs | `pdf-dump f.pdf --inspect 42` |
| Find objects by criteria | `pdf-dump f.pdf --search Type=Font` |
| Find objects, compact output | `pdf-dump f.pdf --search Type=Font --list` |
| Search decoded stream content | `pdf-dump f.pdf --search "stream=text"` |
| Regex search | `pdf-dump f.pdf --search "regex=D:20(23\|24)"` |
| Extract stream to file | `pdf-dump f.pdf --extract-stream 12 --output out.bin` |
| GraphViz DOT graph | `pdf-dump f.pdf --tree --dot` |
| Any of the above as JSON | Add `--json` to any command |
| Combine document-level modes | `pdf-dump f.pdf --fonts --images --validate` |

## Combinability rules

- **Document-level modes** combine freely: `--list`, `--validate`, `--fonts`, `--images`, `--forms`, `--bookmarks`, `--annotations`, `--text`, `--operators`, `--tags`, `--tree`, `--find-text`, `--detail`. Multi-mode text output gets `=== Mode Name ===` headers; multi-mode JSON wraps values in `{"mode_name": ...}`.
- **Standalone modes** are mutually exclusive (pick one): `--object`, `--inspect`, `--search`, `--extract-stream`. Cannot combine with document-level modes.
- **`--page N` or `--page N-M`**: always a modifier. Filters `--text`, `--operators`, `--annotations`, `--find-text` to specific pages. When used alone, shows page info.
- **Modifier flags**: `--json` (all modes), `--decode` (with `--object`; in overview enables decoded stats), `--deref` (with `--object`), `--depth N` (with `--tree`, `--tags`), `--hex` (with `--decode` or `--raw`), `--raw` (with `--object`, conflicts with `--decode`), `--truncate N` (limit binary output), `--dot` (with `--tree`).

## Search syntax

Conditions are ANDed. Case-insensitive on values.

| Condition | Example | Matches |
|-----------|---------|---------|
| `Type=Value` | `Type=Font` | Objects where `/Type` = `/Font` |
| `Key=Value` | `Subtype=Image` | Any dict key matching |
| `key=Name` | `key=MediaBox` | Objects containing key `/MediaBox` |
| `value=Text` | `value=Helvetica` | Any Name or String value containing text |
| `stream=Text` | `stream=BT` | Decoded stream content containing text |
| `regex=Pattern` | `regex=D:20(23\|24)` | Regex match across keys, values, and streams |

## Validation checks (10)

Broken references, unreachable objects, missing required keys (`/MediaBox`, `/Pages`), stream length mismatches, page tree integrity, content stream syntax, font requirements (`/BaseFont`, `/Widths` consistency), page tree cycles, names tree structure, duplicate objects. Severity levels: `[ERROR]`, `[WARN]`, `[INFO]`, `[OK]`.

## PDF structure primer

| Concept | Key |  Role |
|---------|-----|-------|
| Trailer | `/Root`, `/Info` | Entry point; catalog ref + metadata |
| Catalog | `/Pages`, `/AcroForm`, `/Outlines`, `/Names`, `/OCProperties`, `/StructTreeRoot`, `/PageLabels` | Document-level structures |
| Page | `/MediaBox`, `/Contents`, `/Resources`, `/Annots` | Dimensions, drawing instructions, fonts/images, annotations |
| Content stream | `BT`/`ET`, `Tf`, `Tj`/`TJ`, `Td`, `Do` | Drawing operators (text, graphics, images) |
| Objects | `N G R` (e.g. `5 0 R`) | Numbered; generation usually 0; pdf-dump assumes gen 0 for all CLI args |

Object types: null, boolean, integer, real, name (`/Type`), string (`(Hello)`), array, dictionary, stream (dict + bytes), reference.

## JSON output schemas

Add `--json` to any mode. All schemas below show the structure of a single mode's JSON output.

### Object type mapping (used in `--object --json`, `--search --json`, `--inspect --json`)

| PDF type | `"type"` | Value fields |
|----------|----------|-------------|
| Null | `"null"` | — |
| Boolean | `"boolean"` | `"value"` |
| Integer | `"integer"` | `"value"` |
| Real | `"real"` | `"value"` |
| Name | `"name"` | `"value"` (no `/` prefix) |
| String | `"string"` | `"value"` (UTF-8 lossy) |
| Array | `"array"` | `"items": [...]` |
| Dictionary | `"dictionary"` | `"keys": {"Key": {...}}` |
| Stream | `"stream"` | `"dict"`, optional `"content"` / `"content_hex"` / `"content_binary"` / `"decode_warning"` |
| Reference | `"reference"` | `"object": "N M"`. With `--deref`: adds `"resolved": {...}` |

### Mode-specific schemas

**Overview** (`pdf-dump f.pdf --json`):
`{version, object_count, page_count, encrypted, info: {Title, Author, ...}, catalog: {PageLayout, ...}, object_types: {dictionaries: N, streams: N, ...}, validation: {error_count, warning_count, info_count, issues: [{level, message}]}, streams: {count, total_bytes, total_decoded_bytes, filters: {name: N}, largest: [{object_number, bytes}]}, features: {bookmark_count, form_field_count, layer_count, embedded_file_count, page_labels: bool, tagged_structure: bool}}`

**Page info** (`--page N --json`):
`{pages: [{page_number, object_number, generation, media_box, crop_box, rotate, fonts: [str], image_count, ext_gstate_count, annotation_count, annotation_subtypes: [{subtype, count}], content_stream_count, content_stream_bytes, text, resources: {fonts, xobjects, ext_gstate, color_spaces}, text_extractable?}]}`

**Object** (`--object N --json`): Direct object using type mapping above.

**List** (`--list --json`):
`{objects: [{object_number, generation, kind, type, details}]}`

**Search** (`--search <expr> --json`):
`{query, match_count, matches: [{object_number, generation, object: {...}}]}`

**Inspect** (`--inspect N --json`):
`{role, description, details: [[key, value]], domain_details: [str], page_associations: [N], forward_references: [{key, object, summary}], reverse_references: [{from_object, key_path}], object: {...}}`

**Text** (`--text --json`):
`{pages: [{page_number, text, warnings?: [str]}]}`

**Operators** (`--operators --json`):
`{pages: [{page_number, operation_count, operations: [{operator, operands: [str]}], warnings?: [str]}]}`

**Find text** (`--find-text "x" --json`):
`{pattern, match_count, pages: [{page_number, matches: [str]}]}`

**Fonts** (`--fonts --json`):
`{font_count, fonts: [{object_number, generation, base_font, subtype, encoding, embedded: null|{object_number, generation}, to_unicode?, first_char?, last_char?, widths_count?, encoding_differences?, cid_system_info?}]}`

**Images** (`--images --json`):
`{image_count, images: [{object_number, generation, width, height, color_space, bits_per_component, filter, size}]}`

**Validate** (`--validate --json`):
`{error_count, warning_count, info_count, issues: [{level, message}]}`

**Forms** (`--forms --json`):
`{acroform_object, need_appearances, field_count, fields: [{object_number, generation, field_name, field_type, value, flags, page_number}]}`

**Bookmarks** (`--bookmarks --json`):
`{bookmark_count, bookmarks: [{object_number, title, destination, children: [...]}]}`

**Annotations** (`--annotations --json`):
`{annotation_count, annotations: [{page_number, object_number, generation, subtype, rect, contents, link_type?, target?}]}`

**Tree** (`--tree --json`):
`{tree: {node: "Trailer", children: [{key, object, label, status?, children: [...]}]}}`

**Tags** (`--tags --json`):
`{tagged, element_count, structure: [{role, object_number?, generation?, page?, mcid?, title?, alt?, children: [...], children_count?}]}`

**Detail security** (`--detail security --json`):
`{encrypted, algorithm, version, revision, key_length, permissions_raw, permissions: {Print, Modify, "Copy/extract text", Annotate, "Fill forms", "Accessibility extract", Assemble, "Print high quality"}, encrypt_object}`

**Detail embedded** (`--detail embedded --json`):
`{embedded_file_count, embedded_files: [{name, filename, mime_type, size, object_number, filespec_object, filespec_generation}]}`

**Detail labels** (`--detail labels --json`):
`{page_count, page_labels: [{physical_page, label, style, prefix, start}]}`

**Detail layers** (`--detail layers --json`):
`{layer_count, layers: [{object_number, generation, name, default_state, pages: [N]}]}`

## Debugging decision tree

| Symptom | Start with | If needed |
|---------|-----------|-----------|
| Garbled text | `--fonts` (check encoding, ToUnicode) | `--operators --page N`, `--inspect <font_obj>`, `--validate` |
| Blank page | `--page N` (check MediaBox, contents) | `--object <contents_id> --decode`, `--detail layers` |
| Missing image | `--images`, `--page N` | `--operators --page N` (look for `Do`) |
| Corrupted PDF | `--validate` | `--list` |
| Broken form fields | `--forms` | `--object <field_id>` (check `/AP`, `/V`, `/FT`, `/Ff`) |
| Wrong link target | `--annotations --page N` | `--detail labels` (physical vs logical page) |
| Missing content | `--detail layers`, `--detail embedded` | `--page N`, `--annotations --page N` |
| PDF too large | `--images`, `pdf-dump f.pdf` (stream stats) | `--list`, `--validate` (unreachable objs), `--detail embedded` |
| Page numbers wrong | `--detail labels` | Physical page ≠ logical label |
| Restricted permissions | `--detail security` | — |

## Stream filters

Supported (applied sequentially in pipelines): FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, RunLengthDecode.
Unsupported (reported as warnings, decoding stops): DCTDecode, JPXDecode, JBIG2Decode, CCITTFaxDecode.

## Content stream operators

| Op | Meaning | Op | Meaning |
|----|---------|----|---------|
| `BT`/`ET` | Begin/end text block | `q`/`Q` | Save/restore graphics state |
| `Tf` | Set font+size | `cm` | Concatenate matrix |
| `Tj` | Show string | `Do` | Paint XObject (image/form) |
| `TJ` | Show with kerning | `re` | Rectangle path |
| `Td`/`TD` | Move text position | `m`/`l`/`c` | moveto/lineto/curveto |
| `Tm` | Set text matrix | `f`/`S` | Fill/stroke path |
| `T*` | Next line | `W` | Clip path |
| `g`/`G` | Set gray fill/stroke | `rg`/`RG` | Set RGB fill/stroke |
| `k`/`K` | Set CMYK fill/stroke | `cs`/`CS`/`sc`/`SC` | Set color space/color |

## Tips

- Use `--json` when parsing output programmatically.
- Use `--depth 2` with `--tree` or `--tags` for large files, increase as needed.
- `--decode --truncate 200 --hex` for a quick peek at binary streams.
- `--search` is case-insensitive on values. Use `regex=(?i)pattern` for case-insensitive regex.
- `--text` garbage → almost always `Identity-H` encoding without ToUnicode. Check `--fonts`.
- `--inspect N` replaces multiple `--object` + manual ref chasing.
- `--deref` with `--object` shows ref targets inline.
- Combine document-level modes freely: `--fonts --images --validate`.
- JPEG images (DCTDecode) extracted via `--extract-stream` are valid JPEG files.
