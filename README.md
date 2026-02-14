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

Prints a single object by number (generation 0) without following any references.

### `--page N` or `--page N-M`

Dumps the object subtree for a specific page or page range (1-based). Only follows references reachable from each page's dictionary. Examples: `--page 3` for a single page, `--page 1-5` for pages 1 through 5.

### `--search <expr>`

Find objects matching key/value criteria. Conditions are comma-separated and ANDed.

```
pdf-dump file.pdf --search Type=Font              # objects where /Type = /Font
pdf-dump file.pdf --search key=MediaBox            # objects containing a /MediaBox key
pdf-dump file.pdf --search value=Hello             # objects with a string containing "Hello"
pdf-dump file.pdf --search Type=Font,Subtype=Type1 # both conditions must match
```

Combine with `--summary` for compact output: `pdf-dump file.pdf --search Type=Font --summary`

### `--text`

Extracts readable text from page content streams (Tj, TJ, ', " operators). Outputs text page by page.

```
pdf-dump file.pdf --text               # all pages
pdf-dump file.pdf --text --page 3      # just page 3
pdf-dump file.pdf --text --page 1-5    # pages 1 through 5
```

Limitations: no font encoding or ToUnicode mapping; text appears in content stream order, not visual order.

### `--fonts`

Lists every font in the document: object number, BaseFont, Subtype, Encoding, and whether the font is embedded.

### `--images`

Lists every image: object number, dimensions, color space, bits per component, filter, and stream size.

### `--tree`

Shows the object graph as an indented reference tree. Displays object IDs, types, and key paths. Marks revisited nodes to avoid infinite output. Useful for quickly understanding document structure.

```
pdf-dump file.pdf --tree --depth 3    # limit to 3 levels
pdf-dump file.pdf --tree --dot        # GraphViz DOT output
pdf-dump file.pdf --tree --dot --depth 2  # DOT with depth limit
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

### `--bookmarks`

Shows the document's bookmark (outline) tree. Displays titles, destinations (/Dest arrays, /Fit types), and actions (GoTo, URI, etc.) with proper indentation for nested bookmarks.

```
pdf-dump file.pdf --bookmarks
pdf-dump file.pdf --bookmarks --json
```

### `--annotations`

Lists all annotations in the document, grouped by page. Shows subtype, rectangle, and contents. Can be filtered to specific pages.

```
pdf-dump file.pdf --annotations              # all pages
pdf-dump file.pdf --annotations --page 1     # just page 1
pdf-dump file.pdf --annotations --page 1-3   # pages 1-3
pdf-dump file.pdf --annotations --json
```

### `--refs-to N`

Reverse reference lookup. Finds every object that contains a reference to object N, and reports which dictionary key holds that reference.

### `--validate`

Structural health check. Reports broken references, unreachable objects, missing required keys, stream length mismatches, and page tree integrity issues.

### `--extract-object N --output <path>`

Extracts a stream object's decoded content to a file.

### `--diff <file2.pdf>`

Structural comparison of two PDFs. Compares metadata, page dictionaries, resources, content streams, and fonts. Works with `--page N` to compare a single page, and `--json` for structured output.

## Modifier flags

These combine with any mode:

| Flag | Effect |
|------|--------|
| `--json` | Structured JSON output. Works with every mode. |
| `--decode-streams` | Decompress and display stream contents (FlateDecode, ASCII85Decode, ASCIIHexDecode, LZWDecode, RunLengthDecode). |
| `--hex` | Show binary streams as hex dump (use with `--decode-streams`). |
| `--truncate N` | Limit binary stream output to N bytes. |
| `--depth N` | Limit traversal depth (0 = root only). Works with dump, page, tree. |
| `--dot` | Output tree as GraphViz DOT format (use with `--tree`). |

## Examples

Decode all streams and show binary content as hex, truncated:

```
pdf-dump file.pdf --decode-streams --hex --truncate 256
```

Find all image XObjects as JSON:

```
pdf-dump file.pdf --search Subtype=Image --json
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

Show document statistics:

```
pdf-dump file.pdf --stats
```

List bookmarks and annotations:

```
pdf-dump file.pdf --bookmarks
pdf-dump file.pdf --annotations --page 1-3
```

## For Claude Code users

See [DEBUGGING_WITH_PDF_DUMP.md](DEBUGGING_WITH_PDF_DUMP.md) for a guide on using `pdf-dump` as a PDF debugging tool within Claude Code, including common workflows, the JSON schema, and tips for interpreting PDF internals.

## License

This project is provided as-is for PDF inspection and debugging purposes.
