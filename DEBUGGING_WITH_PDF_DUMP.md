# pdf-dump: AI Reference Guide

Use `pdf-dump` instead of writing Python/shell scripts to inspect PDF internals.  This guide covers everything needed to use the tool and interpret its output.

## Quick lookup: “I need to...” → command

| Goal | Command |
|------|---------|
| Overview of unknown PDF | `pdf-dump f.pdf` |
| Overview with compression ratios | `pdf-dump f.pdf --decode` |
| Page dimensions, resources, text preview | `pdf-dump f.pdf --page 3` (or `--page 1-5`) |
| Extract text | `pdf-dump f.pdf --text` (or `--text --page 3`) |
| Extract a table with columns aligned | `pdf-dump f.pdf --text --layout` (`--layout-cell <pt>` to set the grid) |
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
| Explain object’s role and refs | `pdf-dump f.pdf --inspect 42` |
| Find objects by criteria | `pdf-dump f.pdf --search Type=Font` |
| Find objects, compact output | `pdf-dump f.pdf --search Type=Font --list` |
| Search decoded stream content | `pdf-dump f.pdf --search "stream=text"` |
| Regex search | `pdf-dump f.pdf --search "regex=D:20(23\|24)"` |
| Extract stream to file | `pdf-dump f.pdf --extract-stream 12 --output out.bin` (the directory must exist; an existing file is replaced) |
| Write the text to a file, never through stdout | `pdf-dump f.pdf --text --output f.txt` (also with `--layout`, `--page`; atomic, nothing written on exit 1, still written on exit 3; not with `--json`) |
| Fail when a page’s words are outlined or scanned | `pdf-dump f.pdf --text --require-text` (exit 1 naming each page that draws but shows no text; nothing written) |
| Write an image a viewer or OCR engine opens | `pdf-dump f.pdf --extract-image 12 --output a.png`, or `--extract-inline-image 3:1` for an inline image (see `--images --json`) |
| GraphViz DOT graph | `pdf-dump f.pdf --tree --dot` |
| Any of the above as JSON | Add `--json` to any command |
| Combine document-level modes | `pdf-dump f.pdf --fonts --images --validate` |

## Combinability rules

- **Document-level modes** combine freely: `--list`, `--validate`, `--fonts`, `--images`, `--forms`, `--bookmarks`, `--annotations`, `--text`, `--operators`, `--tags`, `--tree`, `--find-text`, `--detail`.  Multi-mode text output gets `=== Mode Name ===` headers; multi-mode JSON wraps values in `{"mode_name": ...}`.
- **Standalone modes** are mutually exclusive (pick one): `--object`, `--inspect`, `--search`, `--extract-stream`, `--extract-image`, `--extract-inline-image`.  Cannot combine with document-level modes.
- **`--page N` or `--page N-M`**: always a modifier.  Filters `--text`, `--operators`, `--annotations`, `--find-text` to specific pages.  When used alone, shows page info.
- **Modifier flags**: `--json` (all modes), `--decode` (with `--object`; in overview enables decoded stats), `--deref` (with `--object`), `--depth N` (with `--tree`, `--tags`), `--hex` (with `--decode` or `--raw`), `--raw` (with `--object`, conflicts with `--decode`), `--truncate N` (limit binary output), `--dot` (with `--tree`).

## Search syntax

Conditions are ANDed.  Case-insensitive on values.

| Condition | Example | Matches |
|-----------|---------|---------|
| `Type=Value` | `Type=Font` | Objects where `/Type` = `/Font` |
| `Key=Value` | `Subtype=Image` | Any dict key matching |
| `key=Name` | `key=MediaBox` | Objects containing key `/MediaBox` |
| `value=Text` | `value=Helvetica` | Any Name or String value containing text, in what it says or in its bytes |
| `stream=Text` | `stream=BT` | Decoded stream content containing text |
| `regex=Pattern` | `regex=D:20(23\|24)` | Regex match across keys, values, and streams |

## Text strings

`--forms`, `--annotations`, `--detail`, `--bookmarks`, `--tags`, `--object`, `--inspect`, `--tree`, and the overview’s metadata decode a PDF text string as the standard says: by its UTF-16BE (`FE FF`) or UTF-8 (`EF BB BF`) mark, otherwise as PDFDocEncoding, with any language escape removed.  So raw UTF-8 with no mark, which is invalid, reads as mojibake here, as it does in a viewer.  A string that cannot be decoded prints as lossless hex (`<FEFF00>`), never as replacement characters.  `--forms`, `--annotations`, `--detail embedded` and `--object` also flag it in `--json`; the others show only the hex.  `--object` adds the encoding when it matters, so `(Hello) [UTF-16BE]` can be told from ASCII, and raw UTF-8 reads as mojibake marked `[PDFDocEncoding]`; one that cannot be read shows `[not decodable]`.  A hex string stays as written, with what it says added when it has a mark.  Text output escapes what could rewrite a terminal, such as `\u{d}`; JSON has the text as it is.  `--search` matches what a string says and its raw bytes, either one.  `--operators` does not decode: an operand’s bytes mean what its font says.  `decoded` says only whether the bytes read under the encoding: PDF does not mark a byte string such as `/ID` as one, so it can read as `decoded: true` mojibake, with its bytes in `hex`.

## Validation checks (10)

Broken references, unreachable objects, missing required keys (`/MediaBox`, `/Pages`), stream length mismatches, page tree integrity, content stream syntax, font requirements (`/BaseFont`, `/Widths` consistency), page tree cycles, names tree structure, duplicate objects.  Severity levels: `[ERROR]`, `[WARN]`, `[INFO]`, `[OK]`.

## PDF structure primer

| Concept | Key |  Role |
|---------|-----|-------|
| Trailer | `/Root`, `/Info` | Entry point; catalog ref + metadata |
| Catalog | `/Pages`, `/AcroForm`, `/Outlines`, `/Names`, `/OCProperties`, `/StructTreeRoot`, `/PageLabels` | Document-level structures |
| Page | `/MediaBox`, `/Contents`, `/Resources`, `/Annots` | Dimensions, drawing instructions, fonts/images, annotations |
| Content stream | `BT`/`ET`, `Tf`, `Tj`/`TJ`, `Td`, `Do` | Drawing operators (text, graphics, images) |
| Objects | `N G R` (e.g. `5 0 R`) | Numbered; generation usually 0.  A number on the command line means the object’s only generation, or, with several, the one the cross-reference table says is live, and the output names it.  If nothing says which, it exits 1 naming them, in a range too |

Object types: null, boolean, integer, real, name (`/Type`), string (`(Hello)`), array, dictionary, stream (dict + bytes), reference.

## JSON output schemas

Add `--json` to any mode.  All schemas below show the structure of a single mode’s JSON output.

### Object type mapping (used in `--object --json`, `--search --json`, `--inspect --json`)

| PDF type | `"type"` | Value fields |
|----------|----------|-------------|
| Null | `"null"` | — |
| Boolean | `"boolean"` | `"value"` |
| Integer | `"integer"` | `"value"` |
| Real | `"real"` | `"value"` |
| Name | `"name"` | `"value"` (no `/` prefix) |
| String | `"string"` | `"value"` (decoded, or lossless hex when it cannot be), `"decoded"`, `"encoding"`, `"format"` (`literal` or `hexadecimal`), and `"hex"`, the bytes, unless they are plain ASCII |
| Array | `"array"` | `"items": [...]` |
| Dictionary | `"dictionary"` | `"keys": {"Key": {...}}` |
| Stream | `"stream"` | `"dict"`, optional `"content"` / `"content_hex"` / `"content_binary"` / `"decode_warning"` |
| Reference | `"reference"` | `"object": "N M"`.  With `--deref`: adds `"resolved": {...}` |

### Mode-specific schemas

**Overview** (`pdf-dump f.pdf --json`):
`{version, object_count, page_count, encrypted, decrypted?, info: {Title, Author, ...}, catalog: {PageLayout, ...}, object_types: {dictionaries: N, streams: N, ...}, validation: {error_count, warning_count, info_count, issues: [{level, message}]}, streams: {count, total_bytes, total_decoded_bytes, filters: {name: N}, largest: [{object_number, bytes}]}, features: {bookmark_count, form_field_count, layer_count, embedded_file_count, page_labels: bool, tagged_structure: bool}}`

`encrypted` is authoritative (set from the trailer `/Encrypt` as well as lopdf’s decryption state).  `decrypted` is present only when `encrypted` is true: `false` means the file is password-protected and was not decrypted — `object_count`/`page_count`/`streams` are then incomplete, a validation warning is recorded, a loud banner prints to **stderr**, and the tool exits **3**.  Supply `--password <PASSWORD>` (user or owner password) to decrypt and read it fully; a wrong password exits **1**.

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
`{pages: [{page_number, text, warnings?: [str]}], reliability: {verdict: "reliable"|"degraded"|"unreliable", total_codes, unmapped_codes, unmapped_ratio, fonts: [{name, base_font, subtype, classification, has_to_unicode, reason}]}, pages_without_text: [{page_number, paths, images, unread}]}`

`pages_without_text` lists each page that shows no text (no `Tj`, `TJ`, `'` or `"` with a non-empty string, in the page or any form it invokes) yet fills or strokes a path, paints a shading, or draws an image, or whose content could not be read to its end (`unread`).  Its words, if any, are outlined or scanned, and only rendering and OCR read them.  A blank page is not listed, and the list is empty when there is none.  It does not change the verdict or the exit code.  A stderr note names the same pages.  `--require-text` turns any such page into exit 1, with nothing written; it fails as well on a document that could not be decrypted or has no pages.

Text extraction is font-aware: show-strings are decoded through each font’s `/ToUnicode` CMap (the fix for CID/Type0 mojibake) and through WinAnsiEncoding/MacRomanEncoding tables for simple fonts lacking ToUnicode; undecodable fonts fall back to raw byte passthrough.  Line breaks and word spaces come from where the glyphs land, by the positions `--layout` computes: a new line where the baseline moves, a space where two strings are a word gap apart, in content order.  Where a glyph’s width is unknown, the operators decide instead (`BT`, `T*` and a downward `Td` start a line; a `TJ` gap past 100 is a space).  A loud reliability banner prints to **stderr** (stdout stays clean for piping) when a document is not fully reliable, and `--text` exits **3** whenever the verdict is not `reliable`: `unreliable` (a CID/Type0 font with no ToUnicode map) or `degraded` (partly-known encoding, more than 20 % of shown codes undecodable, or a content stream not read to its end).  The text is still printed and `--json` still emits on exit 3.  Through v0.24.x only `unreliable` exited 3.

**Recovery** (any mode, `--json`, present only when a malformed stream was found):
`{repaired: bool, strict: bool, count: N, streams: [{object, generation, file_offset, declared_length: int|null, actual_length}]}`

When a content stream declares a wrong `/Length`, a strict parser drops its body and the page content silently vanishes.  By default pdf-dump recovers the body (scanning to `endstream`), warns loudly on **stderr**, adds this `recovery` object to every `--json` mode, and keeps exit code **0**.  The key is absent for well-formed PDFs.  Pass `--strict` to refuse the repair instead: the affected content stays missing (as a spec-conformant reader would see it), `recovery.repaired` is `false`, and the tool exits **3** — a hard gate for CI.

**Operators** (`--operators --json`):
`{pages: [{page_number, operation_count, operations: [{operator, operands: [str]}], warnings?: [str]}]}`

**Layout text** (`--text --layout --json`):
`{layout: true, pages: [{page_number, text, rotate, crop_box: [x0, y0, x1, y1], cell, non_horizontal_glyphs, off_page_glyphs, warnings?: [str]}], reliability: {…same as --text…}, pages_without_text: […same as --text…]}`

`text` is the character grid (lines top to bottom in visual space; column 0 is the page’s leftmost text; rotated or vertical text after a `[non-horizontal text]` line, each rotation laid out as a small grid of its own along its baseline, and vertical or mirrored text in content order; text outside the CropBox after `[off-page text]`).  `rotate` is the page’s normalized `/Rotate`; `crop_box` is the effective CropBox (CropBox ∩ MediaBox, inherited) in default user space; `cell` is the grid column width in points.  A glyph whose width is unknown makes its font’s verdict `degraded`, which exits 3.  That covers a font without `/Widths` (a Standard-14 font takes its built-in AFM widths instead, and is unknown only for a glyph its AFM lacks), `/Widths` without `/FirstChar`, a non-numeric or missing entry, an out-of-range code with no explicit `/MissingWidth`, a malformed CID `/W`, a Type0 font with a non-Identity `/Encoding`, and text shown with no usable font.

**Find text** (`--find-text "x" --json`):
`{pattern, match_count, pages: [{page_number, matches: [str]}], reliability}`

`reliability` is the same object `--text --json` carries, computed over the pages searched.  Since v0.29.0 `--find-text` shares `--text`’s reliability signaling: when the searched text is degraded or unreliable it prints the stderr banner and exits **3**, whether or not anything matched, since “no matches” in undecodable text is not evidence that the word is absent.

**Fonts** (`--fonts --json`):
`{font_count, fonts: [{object_number, generation, base_font, subtype, encoding, embedded: null|{object_number, generation}, to_unicode?, first_char?, last_char?, widths_count?, encoding_differences?, cid_system_info?}]}`

**Images** (`--images --json`):
`{image_count, images: [{object_number, generation, width, height, color_space, bits_per_component, filter, size, export_format}], inline_image_count, inline_images: [{page_number, index, source, source_object, offset, width, height, color_space, bits_per_component, filter, size, export_format}], inline_image_warnings: [text]}`

- `export_format` is what `--extract-image` or `--extract-inline-image` writes, `png`, `jpeg` or `jp2`, which tells a caller the extension to ask for; `null` when the filters cannot be read.  It does not promise the export will succeed.
- An inline image is addressed as `page_number:index`.  `index` counts from 1 on its page, in walking order.  `source` is `page`, `form`, `pattern` or `appearance`, with `source_object` for the last three, and `offset` is where `BI` falls in that source’s decoded content; a page’s content streams count as one, joined by newlines.
- `images` holds every `/Subtype /Image` stream, and any stream one of them names as its `/SMask` or `/Mask`, whatever that stream’s `/Subtype`.
- Walked for inline images: a page’s contents, the form XObjects (`Do`), tiling patterns (`scn`) and soft-mask groups (`gs`) it invokes, and its annotations’ normal appearances.  Not walked: Type 3 glyph procedures, whose bitmap glyphs are inline images, and forms and patterns nothing on the page invokes.  `color_space` is the dictionary’s text, so a name from the resources stays a name.
- `inline_image_warnings` names anything that could not be walked: a stream that would not decode, nesting too deep, an inline image with no `EI` or whose end had to be guessed, a page tree declaring more pages than could be walked, or a scan stopped after 1 GiB of decoded content.  Any of them makes the run exit **3**, since each is a place an image may be.

**Extract image** (`--extract-image N --output f --json`, `--extract-inline-image PAGE:INDEX --output f --json`):
`{object_number, generation, path, format, width, height, color_space, bits_per_component, filter, mask, mask_applied}`, the first two `null` for an inline image

`width` and `height` are the file’s pixels, which a mask larger than its image raises.  For a JPEG they are checked against its frame; for a JPEG 2000 they are the dictionary’s.  `mask` is `none`, `smask`, `explicit` or `color_key`.  `mask_applied` is false only for a JPEG or JPEG 2000 written as it is, which also warns on stderr and exits 3, with the file still written: the picture as drawn needs that mask, so a caller reading the file must read the mask too, which `--images` lists as an image of its own.  Masked JPEGs are common, so read `mask_applied` before treating that 3 as a problem.

**Validate** (`--validate --json`):
`{error_count, warning_count, info_count, issues: [{level, message}]}`

**Forms** (`--forms --json`):
`{acroform_object, need_appearances, xfa, xfa_packets: [name], xfa_datasets_text, xfa_decoded, field_count, fields: [{object_number, generation, field_name, field_name_decoded, field_type, value, values, value_decoded, inherited, rich_value, rich_value_inherited, rich_value_decoded, tooltip, tooltip_decoded, tooltip_inherited, options: null|[{export, label, decoded}], options_decoded, options_inherited, captions: [{text, decoded}], captions_decoded, has_field_kids, flags, page_number}]}`

- `value` is always one string: `"text"` quoted, `/Name`, a number, `[…]` for a list, `<<…>>` for a dictionary, `<FEFF00>` hex for a string that would not decode, `(empty)`, or, flagged, `(unresolvable reference)` or `(nested too deeply to read)`.  A signature’s `<<…>>` leaves out `/ByteRange`, `/Cert`, `/Contents` and `/Reference`.  `values` is a list as a JSON array of decoded items, else `null`.
- `inherited` is true when `/V` came from an ancestor, and `rich_value_inherited` when `/RV` did.  A parent field holding a value of its own is an entry too, with `has_field_kids: true`, so a value is seen even when every kid overrides it.
- `rich_value` is `/RV` with its markup removed; malformed markup comes back whole and flagged.  `xfa_datasets_text` is the same for an XFA form’s datasets: the character data of every datasets packet or element.
- `tooltip` is `/TU` and `options` is `/Opt`, each the field’s own or its nearest ancestor’s, and `null` when there is none; `tooltip_inherited` and `options_inherited` say which.  The standard marks only a button’s `/Opt` as inheritable, so an inherited one is reported so that it is not lost, not because a viewer is sure to use it.
- A plain string in `/Opt` is both `export` and `label`.  Each item keeps its place, since `/I` counts by position, and one that cannot be read is flagged with `decoded: false`, not left out.  Past 10,000 items, a last entry says so, flagged.
- `captions` is `/MK /CA` of each of the field’s widgets, itself first, since a radio button’s widgets each have their own; `[]` when there is none.  Its order does not map to `/Kids`: a widget with no caption adds nothing.
- `options_decoded` and `captions_decoded` are true when every item decoded.
- **What this does not read.**  The walk starts from `/Fields`, so a widget that no field lists is not an entry: `--annotations` lists it, but its `/T`, `/TU`, `/Opt` and `/MK` are not read.  A parent whose kids are fields is an entry only for a `/V` or `/RV` of its own, so its `/MK`, and a `/TU` or `/Opt` that every kid overrides, are not reported.  When all of a field’s kids are widgets, their `/TU` and `/Opt` are not read.  `/DV` and `/MK`’s `/RC` and `/AC` are not read.  Of XFA, attribute values and the template and form packets are not read, so `xfa: true` with `xfa_decoded: true` does not say the form was read in full; `xfa_packets` is empty when `/XFA` is one stream.
- `object_number` and `generation` are `null` for a field written directly in `/Fields` or `/Kids`.
- A `*_decoded: false` on a value, a rich value, a tooltip, the options, the captions, or the XFA data makes the run exit **3**, with a stderr warning; the JSON still emits.  What cannot be decoded shows as hex, and a value of the wrong type as it stands, so `<FEFF00>` alone does not say which: read the flag.  `field_name_decoded: false` is only a flag.

**Bookmarks** (`--bookmarks --json`):
`{bookmark_count, bookmarks: [{object_number, title, destination, children: [...]}]}`

**Annotations** (`--annotations --json`):
`{annotation_count, annotations: [{page_number, object_number, generation, subtype, rect, contents, contents_decoded, rich_contents, rich_contents_decoded, author, author_decoded, subject, subject_decoded, flags, hidden, has_down_appearance, has_rollover_appearance, link_type, target, appearance_text, appearance_state, appearance_reliability, appearance_warnings}]}`

- Every entry in `/Annots` is listed, direct or indirect; `object_number` and `generation` are `null` for a direct one.
- `rich_contents` is `/RC` with its markup removed.
- `author` is `/T` and `subject` is `/Subj`, `null` when absent and `""` when empty.  `/T` is the author on a markup annotation and a title on a Screen or Movie.  On a widget it is the field’s name, which `--forms` reports for a field it reaches, so `author` is `null` there.
- `flags` is `/F`, 0 when absent.  `hidden` is true when its Hidden or NoView bit is set.  It reports those two bits and is not a verdict on whether a viewer shows the annotation: with ToggleNoView a NoView annotation appears on hover, Invisible hides one of an unknown type, and `/OC` can hide any.  The bits are in `flags`; `/OC` is not read.  An `/F` that is not an integer, is wider than 32 bits, or will not resolve gives `null` for both and exits 3: not known, rather than shown.
- `has_down_appearance` and `has_rollover_appearance` say `/AP /D` and `/AP /R` exist.  Their text is not read.
- `appearance_text` is what `/AP /N` shows, read by the extractor `--text` uses; `null` when there is none.  The text of `/D` and `/R` is not read, and a hidden annotation’s appearance is read like any other.  `appearance_reliability` has the shape of `--text`’s `reliability`, except that only the fonts the appearance selects count, and it is `unreliable` when a stream could not be read, or text is shown in a font no `Tf` named, or a `gs` sets one.  For a state dictionary `appearance_state` is the `/AS` selected; with no `/AS`, every state is read, since nothing says which one shows.  A widget with no `/Resources` uses the form’s `/DR`; any other annotation, the page’s.
- The run exits **3**, with a stderr warning, on `contents_decoded: false`, `rich_contents_decoded: false`, `author_decoded: false`, `subject_decoded: false`, `flags: null`, or an appearance verdict other than `reliable`.  A checkbox’s or radio button’s state is excused when all it fails on is Symbol or ZapfDingbats, since it draws a mark, not text.

**Tree** (`--tree --json`):
`{tree: {node: "Trailer", children: [{key, object, label, status?, children: [...]}]}}`

**Tags** (`--tags --json`):
`{tagged, element_count, structure: [{role, object_number?, generation?, page?, mcid?, title?, alt?, children: [...], children_count?}]}`

**Detail security** (`--detail security --json`):
`{encrypted, algorithm, version, revision, key_length, permissions_raw, permissions: {Print, Modify, "Copy/extract text", Annotate, "Fill forms", "Accessibility extract", Assemble, "Print high quality"}, encrypt_object}`

**Detail embedded** (`--detail embedded --json`):
`{embedded_file_count, embedded_files: [{name, filename, filename_decoded, mime_type, size, object_number, filespec_object, filespec_generation, reached_from}]}`

Every file, however it is reached: `reached_from` is `name_tree`; `filespec` (a filespec with `/EF` anywhere else, such as behind a `Launch` or `GoToE` action or an annotation’s `/FS`); or `orphan` (a `/Type /EmbeddedFile` stream no `/EF` names).  A stream reached twice is one file, and each distinct stream in one `/EF` or `/RF` is its own file.  `filespec_object` is `null` for a direct filespec or an orphan.

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

`--extract-stream` and `--object --decode` undo a `/Predictor` and honour `/EarlyChange`.  When a parameter is unusable or does not fit the data, they warn and hand back the bytes as the filter left them.  A `/Filter` by reference is followed, and one that cannot be used, such as an array entry that is not a name, is a warning with the bytes as they are stored.

## Exporting images

`--extract-image` writes the picture as drawn, in its stored orientation: the page’s rotation and the image’s placement are not applied, so a scan stored upside down comes out upside down.

- **PNG for decoded pixels; the stream as it is for DCTDecode (`.jpg`, `.jpeg`) and JPXDecode (`.jp2`).**  Filters before them are undone.
- **Exit codes.**  An extension that is none of those is **2**, wrong whatever the document holds.  An extension for another format than this image is written in is **1**, naming the right one: only the document can tell, so it is a mismatch with the world, not with the command.  `export_format` says which to ask for.
- **Colour:** grey, RGB, CMYK (converted naively), ICCBased by `/N` then `/Alternate`, CalGray and CalRGB as grey and RGB, and Indexed through its palette.  Separation, DeviceN and Lab are refused by name, since each needs a tint transform or a real conversion.
- **Samples:** 1, 2, 4, 8 and 16 bits, with `/Decode`.  16 bits reduce to 8.  The output is always 8-bit grey or RGB.
- **Filters:** all of the above, with `/Predictor` and `/EarlyChange`, and CCITTFaxDecode and JBIG2Decode.  Both bilevel codecs decode strictly, because a lenient decode reports success over a damaged line or missing globals.  That catches a damaged fax line, a page missing its globals, and a stream cut short of its declared length.  It cannot catch damaged arithmetic-coded JBIG2 data, which decodes to something from any bytes.
- **Masks are composited onto white,** the colour of paper: `/SMask`, which overrides `/Mask`; `/Mask` as a stencil or a colour key; and `/ImageMask`, which paints black.  The file takes the larger of the image’s and the mask’s size, since a mixed-raster scan keeps its words in a full-size mask over a tiny colour image.  An image with `/SMask` keeps `/Matte`’s tint at the edges.
- **Refused, exit 1, nothing written:** what is above, plus data shorter than the image needs, a palette index past the palette, a `/Decode` that would change a JPEG or JPEG 2000, an Indexed JPEG, a JPEG whose frame is not the image’s size, a mask pdf-dump cannot decode, a light image whose mask would leave a blank file on white, data in an external file (`/F`), a `/DecodeParms` value of the wrong type, and anything past 256 MiB of decoded pixels or samples.

## What pdf-dump does not read

pdf-dump reads a PDF’s structure and draws nothing.  Exit 0 means a mode did what it says over what it reads, not that every word a viewer shows was read; `--help` lists what exit 3 reports.  This list gathers the limits named above and adds the first three.  It is not exhaustive.

- **Text in pixels:** a scan’s words are in an image.  Export it with `--extract-image` and OCR the file.
- **Lettering drawn as vector paths:** it is neither text nor an image, so `--text` and `--images` come back empty with exit 0.  Only rendering the page and OCR reads it.  `--text` names a page that draws but shows no text (`pages_without_text`), and `--require-text` makes it exit 1.  A page holding both text and outlined or scanned words is not named.
- **Whether text shows:** `--text` reports the text in a page’s content, which can include text a viewer would not show: invisible, clipped, covered, off the page, or on a hidden layer.
- **Images:** a JPEG’s or JPEG 2000’s mask stays unapplied in its file, though the mask exports on its own, and a JPEG’s `/ColorTransform` is not applied, so a viewer of the file follows the file’s own markers.  Inline images in Type 3 glyph procedures, and in forms and patterns nothing invokes, are not listed.  Exports keep their stored orientation.
- **Forms:** only fields `/AcroForm /Fields` reaches, and of those not `/DV`, `/MK`’s `/RC` and `/AC`, or a parent’s `/MK` and overridden `/TU` and `/Opt`, or `/TU` and `/Opt` on a widget.  Of XFA, only the character data of the datasets packets.
- **Annotations:** the text of `/AP /N` only: `/AP /D` and `/AP /R` are reported as present, not read.  `hidden` reports two bits of `/F`; `/OC` is not read.
- **Attachments:** `--detail embedded` lists them; what they hold is not read.

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
- `--search` is case-insensitive on values.  Use `regex=(?i)pattern` for case-insensitive regex.
- `--text` garbage → almost always `Identity-H` encoding without ToUnicode.  `--text` now flags this loudly (stderr banner + `reliability.verdict: "unreliable"` + exit 3); confirm with `--fonts`.
- `--inspect N` replaces multiple `--object` + manual ref chasing.
- `--deref` with `--object` shows ref targets inline.
- Combine document-level modes freely: `--fonts --images --validate`.
- JPEG images (DCTDecode) extracted via `--extract-stream` are valid JPEG files.
