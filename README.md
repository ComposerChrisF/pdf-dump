# pdf-dump

A Rust CLI tool that dumps the internal object structure of a PDF file. It lets you inspect, search, compare, and validate PDF internals without writing throwaway scripts or reaching for a hex editor.

## Why

PDF is a container format built from numbered objects — dictionaries, streams, arrays, references — stitched together by a cross-reference table. When something goes wrong (rendering glitch, missing font, broken form, corrupted page), the answers are inside that object graph. But PDF is binary enough that `cat` won't help, and high-level libraries like PyPDF2 or pdfplumber hide the raw structure behind abstractions.

`pdf-dump` sits at the right level: it shows you exactly what the PDF contains, in the terms the PDF spec uses, without requiring you to write code. One command replaces an afternoon of scripting.

## Install

Requires a Rust toolchain that supports edition 2024.

```
cargo install --path .
```

Or build and run directly:

```
cargo build --release
./target/release/pdf-dump <file.pdf>
```

## Quick start

Dump the full object tree (trailer, catalog, pages, resources, streams):

```
pdf-dump file.pdf
```

Get a one-line summary of every object:

```
pdf-dump file.pdf --summary
```

Show document metadata (version, page count, /Info fields):

```
pdf-dump file.pdf --metadata
```

Inspect a single object by number:

```
pdf-dump file.pdf --object 42
```

## Modes

Each invocation runs in one mode. Only one mode flag at a time (with a few noted exceptions).

### Default dump

No flags. Prints the trailer, then walks the object graph depth-first from `/Root`, printing each object's full contents and following all references.

### `--summary` / `-s`

One-line-per-object table showing object number, kind, `/Type`, and a detail string (key count, stream size, filter).

### `--metadata` / `-m`

Prints PDF version, object count, page count, `/Info` dictionary fields (Title, Author, Producer, etc.), and catalog-level properties (PageLayout, Lang).

### `--object N` / `-o N`

Prints one or more objects by number (generation 0) without following references. Accepts single numbers, comma-separated lists, ranges, or mixed: `--object 5`, `--object 1,5,10`, `--object 10-15`, `--object 1,5,10-15`.

### `--page N` or `--page N-M`

Dumps the object subtree for a specific page or page range (1-based). Only follows references reachable from each page's dictionary. Examples: `--page 3` for a single page, `--page 1-5` for pages 1 through 5.

### `--search <expr>`

Find objects matching key/value/stream/regex criteria. Conditions are comma-separated and ANDed.

```
pdf-dump file.pdf --search Type=Font              # objects where /Type = /Font
pdf-dump file.pdf --search key=MediaBox            # objects containing a /MediaBox key
pdf-dump file.pdf --search value=Hello             # objects with a string containing "Hello"
pdf-dump file.pdf --search stream=text             # search inside decoded stream content
pdf-dump file.pdf --search "regex=D:20(23|24)"     # regex match across values
pdf-dump file.pdf --search Type=Font,Subtype=Type1 # both conditions must match
```

Combine with `--summary` for compact output (includes matched key/value): `pdf-dump file.pdf --search Type=Font --summary`

### `--text`

Extracts readable text from page content streams (Tj, TJ, ', " operators). Outputs text page by page. Uses `Td`/`TD` positioning to insert line breaks where the PDF moves to a new line.

```
pdf-dump file.pdf --text               # all pages
pdf-dump file.pdf --text --page 3      # just page 3
pdf-dump file.pdf --text --page 1-5    # pages 1 through 5
```

Limitations: no font encoding or ToUnicode mapping; text order follows content stream with basic positioning, not full visual reflow.

### `--operators`

Shows all content stream operators for each page. Useful for seeing the exact drawing instructions.

```
pdf-dump file.pdf --operators              # all pages
pdf-dump file.pdf --operators --page 3     # just page 3
pdf-dump file.pdf --operators --json
```

### `--resources`

Shows page resource maps: fonts, XObjects, ExtGState, ColorSpaces with details. Handles resource inheritance from parent pages.

```
pdf-dump file.pdf --resources              # all pages
pdf-dump file.pdf --resources --page 1     # just page 1
pdf-dump file.pdf --resources --json
```

### `--fonts`

Lists every font in the document with encoding diagnostics: object number, BaseFont, Subtype, Encoding, embedded status, ToUnicode CMap presence, FirstChar/LastChar/Widths consistency, Differences entries, and CIDSystemInfo for CID fonts.

```
pdf-dump file.pdf --fonts
pdf-dump file.pdf --fonts --json
```

### `--images`

Lists every image: object number, dimensions, color space, bits per component, filter, and stream size.

### `--forms`

Lists AcroForm fields with fully qualified names, field types (Tx/Btn/Ch/Sig), values, flags, and page numbers. Walks hierarchical field trees.

```
pdf-dump file.pdf --forms
pdf-dump file.pdf --forms --json
```

### `--links`

Extracts all hyperlinks and GoTo actions. Lists every URI, GoTo, GoToR, and Named action with source page number and target. Can be filtered to specific pages.

```
pdf-dump file.pdf --links                  # all pages
pdf-dump file.pdf --links --page 1         # just page 1
pdf-dump file.pdf --links --json
```

### `--annotations`

Lists all annotations in the document, grouped by page. Shows subtype, rectangle, and contents. Can be filtered to specific pages.

```
pdf-dump file.pdf --annotations              # all pages
pdf-dump file.pdf --annotations --page 1     # just page 1
pdf-dump file.pdf --annotations --page 1-3   # pages 1-3
pdf-dump file.pdf --annotations --json
```

### `--bookmarks`

Shows the document's bookmark (outline) tree. Displays titles, destinations (/Dest arrays, /Fit types), and actions (GoTo, URI, etc.) with proper indentation for nested bookmarks.

```
pdf-dump file.pdf --bookmarks
pdf-dump file.pdf --bookmarks --json
```

### `--embedded-files`

Lists file attachments embedded in the PDF. Shows filename, MIME type, size, and the object number of the EmbeddedFile stream (which `--extract-object` can extract).

```
pdf-dump file.pdf --embedded-files
pdf-dump file.pdf --embedded-files --json
```

### `--layers` / `--ocg`

Lists Optional Content Groups (layers) with name, default visibility (ON/OFF), and page references. Reads `/OCProperties` from the catalog. Useful for debugging hidden content in engineering drawings, maps, or multi-language documents.

```
pdf-dump file.pdf --layers
pdf-dump file.pdf --ocg --json
```

### `--structure`

Shows the tagged PDF logical structure tree from `/StructTreeRoot`. Displays element roles (heading, paragraph, table, figure), page refs, MCIDs, titles, and alt text. Supports `--depth N` to limit tree depth. Cycle detection prevents infinite output.

```
pdf-dump file.pdf --structure
pdf-dump file.pdf --structure --depth 3
pdf-dump file.pdf --structure --json
```

### `--page-labels`

Shows the physical-to-logical page number mapping defined by the `/PageLabels` number tree. Displays label style (decimal, roman, alphabetic), prefix, and starting value for each range.

```
pdf-dump file.pdf --page-labels
pdf-dump file.pdf --page-labels --json
```

### `--security`

Shows encryption and permissions info from the `/Encrypt` dictionary: algorithm, key length, revision, and permissions bitfield decoded to human-readable flags (print, copy, modify, etc.).

```
pdf-dump file.pdf --security
pdf-dump file.pdf --security --json
```

### `--tree`

Shows the object graph as an indented reference tree. Displays object IDs, types, and key paths. Marks revisited nodes to avoid infinite output. Useful for quickly understanding document structure.

```
pdf-dump file.pdf --tree --depth 3    # limit to 3 levels
pdf-dump file.pdf --tree --dot        # GraphViz DOT output
pdf-dump file.pdf --tree --dot --depth 2  # DOT with depth limit
pdf-dump file.pdf --tree --json
```

### `--stats`

Shows document statistics: page count, object count, object type breakdown, stream statistics (total raw/decoded bytes, filter usage histogram), and the top 10 largest streams.

```
pdf-dump file.pdf --stats
pdf-dump file.pdf --stats --json
```

### `--xref`

Shows a cross-reference table listing every object in the document with its number, generation, kind, and `/Type`.

```
pdf-dump file.pdf --xref
pdf-dump file.pdf --xref --json
```

### `--refs-to N`

Reverse reference lookup. Finds every object that contains a reference to object N, and reports which dictionary key holds that reference.

### `--info N`

Human-readable object role explanation. Classifies the object (Font, Page, Image, Catalog, etc.) with domain-specific details, shows page associations, and lists both forward and reverse references. Think of it as a one-stop "tell me about this object" command.

```
pdf-dump file.pdf --info 42
pdf-dump file.pdf --info 42 --json
```

### `--validate`

Structural health check. Runs 10 validation checks:

- **Broken references**: objects referenced but not present in the file
- **Unreachable objects**: objects that exist but aren't reachable from `/Root`
- **Missing required keys**: pages without `/MediaBox` (considering inheritance), catalog without `/Pages`
- **Stream length mismatches**: declared `/Length` vs actual byte count
- **Page tree integrity**: `/Count` matches actual page count, all pages reachable
- **Content stream syntax**: can the operators be parsed without errors?
- **Font requirements**: `/BaseFont` present, `/FirstChar`/`/LastChar` consistent with `/Widths` array length
- **Page tree cycles**: circular reference detection in `/Parent` chains
- **Names tree structure**: validates `/Names` tree nodes are well-formed
- **Duplicate objects**: detects multiple objects sharing the same object number

Severity levels: `[ERROR]`, `[WARN]`, `[INFO]`, `[OK]`

### `--extract-object N --output <path>`

Extracts a stream object's decoded content to a file.

### `--diff <file2.pdf>`

Structural comparison of two PDFs. Compares metadata, page dictionaries, resources, content streams, and fonts. Works with `--page N` to compare a single page, and `--json` for structured output.

## Modifier flags

These combine with mode flags:

| Flag | Effect |
|------|--------|
| `--json` | Structured JSON output. Works with every mode. |
| `--decode-streams` | Decompress and display stream contents (FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, RunLengthDecode). |
| `--hex` | Show binary streams as hex dump (use with `--decode-streams` or `--raw`). |
| `--truncate N` | Limit binary stream output to N bytes. |
| `--depth N` | Limit traversal depth (0 = root only). Works with dump, page, tree, structure. |
| `--dot` | Output tree as GraphViz DOT format (use with `--tree`). |
| `--deref` | Inline-expand references to show target summaries (use with `--object` or `--page`). |
| `--raw` | Show raw undecoded stream bytes (use with `--object`; conflicts with `--decode-streams`). |
| `--context` | Show bidirectional reference context (use with `--object`). |

## Examples

Decode all streams and show binary content as hex, truncated:

```
pdf-dump file.pdf --decode-streams --hex --truncate 256
```

Find all image XObjects as JSON:

```
pdf-dump file.pdf --search Subtype=Image --json
```

Search with a regex pattern:

```
pdf-dump file.pdf --search "regex=D:20(23|24)"
```

Compare two versions of a PDF, page 1 only:

```
pdf-dump old.pdf --diff new.pdf --page 1
```

Validate a PDF and get machine-readable results:

```
pdf-dump file.pdf --validate --json
```

See the object tree two levels deep:

```
pdf-dump file.pdf --tree --depth 2
```

Generate a DOT graph for visualization:

```
pdf-dump file.pdf --tree --dot | dot -Tpng -o tree.png
```

Get a human-readable explanation of what an object is:

```
pdf-dump file.pdf --info 42
```

See an object with full bidirectional reference context:

```
pdf-dump file.pdf --object 42 --context
```

Inspect an object with references expanded inline:

```
pdf-dump file.pdf --object 42 --deref
```

View raw undecoded stream bytes as hex:

```
pdf-dump file.pdf --object 42 --raw --hex
```

List embedded file attachments:

```
pdf-dump file.pdf --embedded-files
```

Show page label mapping:

```
pdf-dump file.pdf --page-labels
```

Check encryption and permissions:

```
pdf-dump file.pdf --security
```

Show document statistics:

```
pdf-dump file.pdf --stats
```

List bookmarks, annotations, links, and forms:

```
pdf-dump file.pdf --bookmarks
pdf-dump file.pdf --annotations --page 1-3
pdf-dump file.pdf --links
pdf-dump file.pdf --forms
```

Show layers and tagged structure:

```
pdf-dump file.pdf --layers
pdf-dump file.pdf --structure --depth 3
```

## For Claude Code users

See [DEBUGGING_WITH_PDF_DUMP.md](DEBUGGING_WITH_PDF_DUMP.md) for a guide on using `pdf-dump` as a PDF debugging tool within Claude Code, including common workflows, the JSON schema, and tips for interpreting PDF internals.

## License

This project is provided as-is for PDF inspection and debugging purposes.
