use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub(crate) enum DetailSub {
    /// Encryption and permission details
    Security,
    /// Embedded file attachments
    Embedded,
    /// Page label numbering scheme
    Labels,
    /// Optional content groups (layers)
    Layers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DocMode {
    List,
    Validate,
    Fonts,
    Images,
    Forms,
    Bookmarks,
    Annotations,
    Text,
    Operators,
    Tags,
    Tree,
    FindText,
    Detail(DetailSub),
}

impl DocMode {
    pub fn label(&self) -> &'static str {
        match self {
            DocMode::List => "List",
            DocMode::Validate => "Validate",
            DocMode::Fonts => "Fonts",
            DocMode::Images => "Images",
            DocMode::Forms => "Forms",
            DocMode::Bookmarks => "Bookmarks",
            DocMode::Annotations => "Annotations",
            DocMode::Text => "Text",
            DocMode::Operators => "Operators",
            DocMode::Tags => "Tags",
            DocMode::Tree => "Tree",
            DocMode::FindText => "Find Text",
            DocMode::Detail(DetailSub::Security) => "Security",
            DocMode::Detail(DetailSub::Embedded) => "Embedded Files",
            DocMode::Detail(DetailSub::Labels) => "Page Labels",
            DocMode::Detail(DetailSub::Layers) => "Layers",
        }
    }

    pub fn json_key(&self) -> &'static str {
        match self {
            DocMode::List => "list",
            DocMode::Validate => "validate",
            DocMode::Fonts => "fonts",
            DocMode::Images => "images",
            DocMode::Forms => "forms",
            DocMode::Bookmarks => "bookmarks",
            DocMode::Annotations => "annotations",
            DocMode::Text => "text",
            DocMode::Operators => "operators",
            DocMode::Tags => "tags",
            DocMode::Tree => "tree",
            DocMode::FindText => "find_text",
            DocMode::Detail(DetailSub::Security) => "security",
            DocMode::Detail(DetailSub::Embedded) => "embedded_files",
            DocMode::Detail(DetailSub::Labels) => "page_labels",
            DocMode::Detail(DetailSub::Layers) => "layers",
        }
    }
}

#[derive(Debug)]
pub(crate) enum StandaloneMode {
    Object { nums: Vec<u32> },
    Inspect { obj_num: u32 },
    Search { expr: String, list_modifier: bool },
    ExtractStream { obj_num: u32, output: PathBuf },
}

#[derive(Debug)]
pub(crate) enum ResolvedMode {
    Default,
    Combined(Vec<DocMode>),
    Standalone(StandaloneMode),
}

/// Dumps the internal structure of a PDF file.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, after_long_help = "\
Common workflows:
  pdf-dump file.pdf                      Overview (metadata + validation + stats)
  pdf-dump file.pdf --text               Extract all text
  pdf-dump file.pdf --text --page 3      Extract text from page 3
  pdf-dump file.pdf --find-text \"word\"   Search for text across pages
  pdf-dump file.pdf --page 3             Page 3 info (dimensions, resources, text preview)
  pdf-dump file.pdf --page 5-            Pages 5 to last
  pdf-dump file.pdf --inspect 5          Explain object 5
  pdf-dump file.pdf --search Type=Font   Find all font objects
  pdf-dump file.pdf --validate --json    Validation results as JSON
  pdf-dump file.pdf --fonts --images     Combine multiple modes
  pdf-dump file.pdf --list               One-line listing of every object

Search expression syntax (--search):
  <KeyName>=<value>   Dictionary has /KeyName equal to <value>
  key=<name>          Dictionary has a key named <name>
  value=<text>        Any Name/String value contains <text> (case-insensitive)
  stream=<text>       Decoded stream content contains <text> (case-insensitive)
  regex=<pattern>     Any key, Name, or String value matches the regex

Exit codes:
  0   Success (or validation passed with no errors)
  1   Runtime error (file not found, IO failure, invalid argument value)
  2   Argument parse error (clap; e.g. unknown flag, missing required arg)
  3   Tool ran successfully but the input had problems
      (--validate found errors, or --page was out of range)
")]
pub(crate) struct Args {
    /// Path to the PDF file
    #[arg(required = true)]
    pub file: PathBuf,

    // ── Overview ──────────────────────────────────────────────────────
    /// Print a one-line listing of every object
    #[arg(short = 's', long, help_heading = "Overview")]
    pub list: bool,

    /// Run structural validation checks on the PDF
    #[arg(long, help_heading = "Overview")]
    pub validate: bool,

    // ── Content ──────────────────────────────────────────────────────
    /// Extract readable text from page content streams
    #[arg(long, help_heading = "Content")]
    pub text: bool,

    /// Show content stream operators (all pages, or filtered with --page)
    #[arg(long, help_heading = "Content")]
    pub operators: bool,

    /// Search for text across pages (case-insensitive substring match)
    #[arg(long, help_heading = "Content")]
    pub find_text: Option<String>,

    /// List all fonts in the document
    #[arg(long, help_heading = "Content")]
    pub fonts: bool,

    /// List all images in the document
    #[arg(long, help_heading = "Content")]
    pub images: bool,

    // ── Structure ────────────────────────────────────────────────────
    /// Show the object graph as an indented reference tree
    #[arg(long, help_heading = "Structure")]
    pub tree: bool,

    /// Show document bookmarks (outline tree)
    #[arg(long, help_heading = "Structure")]
    pub bookmarks: bool,

    /// Show tagged PDF structure tree (accessibility tags)
    #[arg(long, help_heading = "Structure")]
    pub tags: bool,

    // ── Annotations & Forms ──────────────────────────────────────────
    /// Show annotations with link targets (all pages, or filtered with --page)
    #[arg(long, help_heading = "Annotations & Forms")]
    pub annotations: bool,

    /// List form fields (AcroForm)
    #[arg(long, help_heading = "Annotations & Forms")]
    pub forms: bool,

    // ── Objects ──────────────────────────────────────────────────────
    /// Print one or more objects by number (e.g. 5, 1,5,12, 3-7, 1,5,10-15)
    #[arg(short = 'o', long, help_heading = "Objects")]
    pub object: Option<String>,

    /// Show a human-readable explanation of an object's role, with full content
    #[arg(long, help_heading = "Objects")]
    pub inspect: Option<u32>,

    /// Search for objects matching an expression (e.g. Type=Font, key=MediaBox, value=Hello)
    #[arg(long, help_heading = "Objects")]
    pub search: Option<String>,

    // ── Detail ────────────────────────────────────────────────────────
    /// Show detail: security, embedded, labels, layers
    #[arg(long, value_enum, help_heading = "Detail")]
    pub detail: Vec<DetailSub>,

    // ── Export ────────────────────────────────────────────────────────
    /// Extract a stream object to a file
    #[arg(long, requires = "output", help_heading = "Export")]
    pub extract_stream: Option<u32>,

    /// Output file for extracted stream
    #[arg(long, requires = "extract_stream", help_heading = "Export")]
    pub output: Option<PathBuf>,

    // ── Modifiers ────────────────────────────────────────────────────
    /// Dump the object tree for a specific page or range (e.g. 1, 1-3)
    #[arg(long, help_heading = "Modifiers")]
    pub page: Option<String>,

    /// Output as structured JSON
    #[arg(long, help_heading = "Modifiers")]
    pub json: bool,

    /// Decode and print the content of streams (also enables decoded byte counts in overview)
    #[arg(long, help_heading = "Modifiers")]
    pub decode: bool,

    /// Inline-expand references to show target summaries (use with --object)
    #[arg(long, help_heading = "Modifiers")]
    pub deref: bool,

    /// Limit traversal depth (0 = root only, 1 = root + immediate refs, etc.)
    #[arg(long, help_heading = "Modifiers")]
    pub depth: Option<usize>,

    /// Display binary stream content as hex dump (use with --decode)
    #[arg(long, help_heading = "Modifiers")]
    pub hex: bool,

    /// Truncate binary streams to the first N bytes
    #[arg(long, help_heading = "Modifiers")]
    pub truncate: Option<usize>,

    /// Show raw undecoded stream bytes (use with --object)
    #[arg(long, help_heading = "Modifiers")]
    pub raw: bool,

    /// Output tree as GraphViz DOT format (use with --tree)
    #[arg(long, requires = "tree", help_heading = "Modifiers")]
    pub dot: bool,
}

impl Args {
    pub fn resolve_mode(&self) -> Result<ResolvedMode, String> {
        // Collect standalone modes
        let mut standalone: Vec<StandaloneMode> = Vec::new();

        if let (Some(obj_num), Some(output)) = (self.extract_stream, self.output.as_ref()) {
            standalone.push(StandaloneMode::ExtractStream {
                obj_num,
                output: output.clone(),
            });
        }
        if let Some(ref spec) = self.object {
            let nums = parse_object_spec(spec)?;
            standalone.push(StandaloneMode::Object { nums });
        }
        if let Some(obj_num) = self.inspect {
            standalone.push(StandaloneMode::Inspect { obj_num });
        }
        if let Some(ref expr) = self.search {
            standalone.push(StandaloneMode::Search {
                expr: expr.clone(),
                list_modifier: self.list,
            });
        }

        // Collect document-level modes
        let mut doc_modes: Vec<DocMode> = Vec::new();

        // When --search is active, --list is consumed as a search modifier, not a DocMode
        if self.list && self.search.is_none() {
            doc_modes.push(DocMode::List);
        }
        if self.validate {
            doc_modes.push(DocMode::Validate);
        }
        if self.fonts {
            doc_modes.push(DocMode::Fonts);
        }
        if self.images {
            doc_modes.push(DocMode::Images);
        }
        if self.forms {
            doc_modes.push(DocMode::Forms);
        }
        if self.bookmarks {
            doc_modes.push(DocMode::Bookmarks);
        }
        if self.annotations {
            doc_modes.push(DocMode::Annotations);
        }
        if self.text {
            doc_modes.push(DocMode::Text);
        }
        if self.operators {
            doc_modes.push(DocMode::Operators);
        }
        if self.tags {
            doc_modes.push(DocMode::Tags);
        }
        if self.tree {
            doc_modes.push(DocMode::Tree);
        }
        if self.find_text.is_some() {
            doc_modes.push(DocMode::FindText);
        }
        for sub in &self.detail {
            doc_modes.push(DocMode::Detail(*sub));
        }

        // Validate: can't mix standalone + document modes
        if !standalone.is_empty() && !doc_modes.is_empty() {
            return Err("Cannot combine standalone mode (--object, --inspect, --search, --extract-stream) with document-level modes.".to_string());
        }

        // Validate: at most one standalone mode
        if standalone.len() > 1 {
            return Err("Only one standalone mode may be used at a time (--object, --inspect, --search, --extract-stream).".to_string());
        }

        if let Some(mode) = standalone.into_iter().next() {
            Ok(ResolvedMode::Standalone(mode))
        } else if !doc_modes.is_empty() {
            Ok(ResolvedMode::Combined(doc_modes))
        } else {
            Ok(ResolvedMode::Default)
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DumpConfig {
    pub decode: bool,
    pub truncate: Option<usize>,
    pub json: bool,
    pub hex: bool,
    pub depth: Option<usize>,
    pub deref: bool,
    pub raw: bool,
}

#[derive(Debug)]
pub(crate) enum PageSpec {
    Single(u32),
    Range(u32, u32),
    /// Open-ended range: `start-` means "from `start` to the last page".
    /// Resolution against the document's page count happens at the call site.
    OpenRange(u32),
}

impl PageSpec {
    pub fn parse(s: &str) -> Result<PageSpec, String> {
        if let Some((start_s, end_s)) = s.split_once('-') {
            let start: u32 = start_s
                .trim()
                .parse()
                .map_err(|_| format!("Invalid page range start: '{}'", start_s.trim()))?;
            if start == 0 {
                return Err("Page numbers must be >= 1".to_string());
            }
            let end_s = end_s.trim();
            if end_s.is_empty() {
                return Ok(PageSpec::OpenRange(start));
            }
            let end: u32 = end_s
                .parse()
                .map_err(|_| format!("Invalid page range end: '{}'", end_s))?;
            if end == 0 {
                return Err("Page numbers must be >= 1".to_string());
            }
            if start > end {
                return Err(format!("Invalid page range: {} > {}", start, end));
            }
            Ok(PageSpec::Range(start, end))
        } else {
            let num: u32 = s
                .trim()
                .parse()
                .map_err(|_| format!("Invalid page number: '{}'", s.trim()))?;
            if num == 0 {
                return Err("Page numbers must be >= 1".to_string());
            }
            Ok(PageSpec::Single(num))
        }
    }

    pub fn contains(&self, page: u32) -> bool {
        match self {
            PageSpec::Single(n) => page == *n,
            PageSpec::Range(start, end) => page >= *start && page <= *end,
            PageSpec::OpenRange(start) => page >= *start,
        }
    }

    /// Returns the explicit page numbers for `Single`/`Range`.
    /// `OpenRange` returns an empty Vec — callers that need to enumerate it
    /// must filter `doc.get_pages()` via `contains()` (see `helpers::build_page_list`).
    pub fn pages(&self) -> Vec<u32> {
        match self {
            PageSpec::Single(n) => vec![*n],
            PageSpec::Range(start, end) => (*start..=*end).collect(),
            PageSpec::OpenRange(_) => Vec::new(),
        }
    }
}

pub(crate) fn parse_object_spec(s: &str) -> Result<Vec<u32>, String> {
    let mut result = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start_s, end_s)) = part.split_once('-') {
            let start: u32 = start_s
                .trim()
                .parse()
                .map_err(|_| format!("Invalid object number: '{}'", start_s.trim()))?;
            let end: u32 = end_s
                .trim()
                .parse()
                .map_err(|_| format!("Invalid object number: '{}'", end_s.trim()))?;
            if start > end {
                return Err(format!("Invalid object range: {} > {}", start, end));
            }
            result.extend(start..=end);
        } else {
            let num: u32 = part
                .parse()
                .map_err(|_| format!("Invalid object number: '{}'", part))?;
            result.push(num);
        }
    }
    if result.is_empty() {
        return Err("Empty object specification".to_string());
    }
    result.sort_unstable();
    result.dedup();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn page_spec_parse_single() {
        let spec = PageSpec::parse("5").unwrap();
        assert!(matches!(spec, PageSpec::Single(5)));
    }

    #[test]
    fn page_spec_parse_range() {
        let spec = PageSpec::parse("1-5").unwrap();
        assert!(matches!(spec, PageSpec::Range(1, 5)));
    }

    #[test]
    fn page_spec_parse_invalid() {
        assert!(PageSpec::parse("abc").is_err());
        assert!(PageSpec::parse("0").is_err());
        assert!(PageSpec::parse("5-3").is_err()); // start > end
        assert!(PageSpec::parse("0-5").is_err()); // zero
        assert!(PageSpec::parse("1-0").is_err()); // zero
    }

    #[test]
    fn page_spec_parse_open_range() {
        let spec = PageSpec::parse("5-").unwrap();
        assert!(matches!(spec, PageSpec::OpenRange(5)));

        let spec = PageSpec::parse(" 2 - ").unwrap();
        assert!(matches!(spec, PageSpec::OpenRange(2)));
    }

    #[test]
    fn page_spec_parse_open_range_invalid() {
        assert!(PageSpec::parse("0-").is_err());
        assert!(PageSpec::parse("-").is_err());
    }

    #[test]
    fn page_spec_open_range_contains() {
        let spec = PageSpec::OpenRange(3);
        assert!(!spec.contains(1));
        assert!(!spec.contains(2));
        assert!(spec.contains(3));
        assert!(spec.contains(100));
    }

    #[test]
    fn page_spec_open_range_pages_empty() {
        // OpenRange::pages() returns empty — callers must use contains() with doc context.
        let spec = PageSpec::OpenRange(5);
        assert!(spec.pages().is_empty());
    }

    #[test]
    fn page_spec_contains() {
        let single = PageSpec::Single(3);
        assert!(single.contains(3));
        assert!(!single.contains(4));

        let range = PageSpec::Range(2, 5);
        assert!(!range.contains(1));
        assert!(range.contains(2));
        assert!(range.contains(3));
        assert!(range.contains(5));
        assert!(!range.contains(6));
    }

    #[test]
    fn page_spec_pages() {
        let single = PageSpec::Single(3);
        assert_eq!(single.pages(), vec![3]);

        let range = PageSpec::Range(2, 5);
        assert_eq!(range.pages(), vec![2, 3, 4, 5]);
    }

    #[test]
    fn parse_object_spec_single() {
        let result = parse_object_spec("5").unwrap();
        assert_eq!(result, vec![5]);
    }

    #[test]
    fn parse_object_spec_multiple() {
        let result = parse_object_spec("1,5,12").unwrap();
        assert_eq!(result, vec![1, 5, 12]);
    }

    #[test]
    fn parse_object_spec_range() {
        let result = parse_object_spec("3-7").unwrap();
        assert_eq!(result, vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn parse_object_spec_mixed() {
        let result = parse_object_spec("1,5,10-12").unwrap();
        assert_eq!(result, vec![1, 5, 10, 11, 12]);
    }

    #[test]
    fn parse_object_spec_invalid() {
        assert!(parse_object_spec("abc").is_err());
        assert!(parse_object_spec("").is_err());
        assert!(parse_object_spec("5-3").is_err());
    }

    // ── PageSpec edge cases ─────────────────────────────────────────

    #[test]
    fn page_spec_parse_whitespace() {
        let spec = PageSpec::parse("  5  ").unwrap();
        assert!(matches!(spec, PageSpec::Single(5)));

        let spec = PageSpec::parse(" 1 - 3 ").unwrap();
        assert!(matches!(spec, PageSpec::Range(1, 3)));
    }

    #[test]
    fn page_spec_parse_single_page_boundary() {
        // Minimum valid page
        let spec = PageSpec::parse("1").unwrap();
        assert!(matches!(spec, PageSpec::Single(1)));

        // Large page number
        let spec = PageSpec::parse("999999").unwrap();
        assert!(matches!(spec, PageSpec::Single(999999)));
    }

    #[test]
    fn page_spec_parse_same_start_end_range() {
        // Range where start == end is valid (equivalent to single)
        let spec = PageSpec::parse("5-5").unwrap();
        assert!(matches!(spec, PageSpec::Range(5, 5)));
        assert_eq!(spec.pages(), vec![5]);
    }

    #[test]
    fn page_spec_error_messages() {
        let err = PageSpec::parse("abc").unwrap_err();
        assert!(err.contains("Invalid page number"), "got: {}", err);

        let err = PageSpec::parse("0").unwrap_err();
        assert!(err.contains(">= 1"), "got: {}", err);

        let err = PageSpec::parse("5-3").unwrap_err();
        assert!(err.contains("5 > 3"), "got: {}", err);

        let err = PageSpec::parse("a-5").unwrap_err();
        assert!(err.contains("Invalid page range start"), "got: {}", err);

        let err = PageSpec::parse("1-b").unwrap_err();
        assert!(err.contains("Invalid page range end"), "got: {}", err);
    }

    #[test]
    fn page_spec_contains_boundary() {
        let range = PageSpec::Range(1, 1);
        assert!(range.contains(1));
        assert!(!range.contains(0));
        assert!(!range.contains(2));
    }

    // ── parse_object_spec edge cases ────────────────────────────────

    #[test]
    fn parse_object_spec_deduplicates() {
        let result = parse_object_spec("1,1,5,5").unwrap();
        assert_eq!(result, vec![1, 5]);
    }

    #[test]
    fn parse_object_spec_sorts() {
        let result = parse_object_spec("10,5,1").unwrap();
        assert_eq!(result, vec![1, 5, 10]);
    }

    #[test]
    fn parse_object_spec_trailing_comma() {
        // Empty parts from trailing comma should be skipped
        let result = parse_object_spec("1,5,").unwrap();
        assert_eq!(result, vec![1, 5]);
    }

    #[test]
    fn parse_object_spec_whitespace() {
        let result = parse_object_spec(" 1 , 5 , 10 - 12 ").unwrap();
        assert_eq!(result, vec![1, 5, 10, 11, 12]);
    }

    #[test]
    fn parse_object_spec_overlap_deduped() {
        // Ranges that overlap should be deduped
        let result = parse_object_spec("1-3,2-4").unwrap();
        assert_eq!(result, vec![1, 2, 3, 4]);
    }

    #[test]
    fn parse_object_spec_single_element_range() {
        let result = parse_object_spec("5-5").unwrap();
        assert_eq!(result, vec![5]);
    }

    #[test]
    fn parse_object_spec_reversed_range_error() {
        let err = parse_object_spec("10-5").unwrap_err();
        assert!(err.contains("10 > 5"), "got: {}", err);
    }

    #[test]
    fn parse_object_spec_non_numeric_in_range() {
        let err = parse_object_spec("a-5").unwrap_err();
        assert!(err.contains("Invalid object number"), "got: {}", err);
    }

    // ── DocMode label/json_key ──────────────────────────────────────

    #[test]
    fn doc_mode_label_all_variants() {
        assert_eq!(DocMode::List.label(), "List");
        assert_eq!(DocMode::Validate.label(), "Validate");
        assert_eq!(DocMode::Fonts.label(), "Fonts");
        assert_eq!(DocMode::Images.label(), "Images");
        assert_eq!(DocMode::Forms.label(), "Forms");
        assert_eq!(DocMode::Bookmarks.label(), "Bookmarks");
        assert_eq!(DocMode::Annotations.label(), "Annotations");
        assert_eq!(DocMode::Text.label(), "Text");
        assert_eq!(DocMode::Operators.label(), "Operators");
        assert_eq!(DocMode::Tags.label(), "Tags");
        assert_eq!(DocMode::Tree.label(), "Tree");
        assert_eq!(DocMode::FindText.label(), "Find Text");
        assert_eq!(DocMode::Detail(DetailSub::Security).label(), "Security");
        assert_eq!(
            DocMode::Detail(DetailSub::Embedded).label(),
            "Embedded Files"
        );
        assert_eq!(DocMode::Detail(DetailSub::Labels).label(), "Page Labels");
        assert_eq!(DocMode::Detail(DetailSub::Layers).label(), "Layers");
    }

    #[test]
    fn doc_mode_json_key_all_variants() {
        assert_eq!(DocMode::List.json_key(), "list");
        assert_eq!(DocMode::Validate.json_key(), "validate");
        assert_eq!(DocMode::Fonts.json_key(), "fonts");
        assert_eq!(DocMode::Images.json_key(), "images");
        assert_eq!(DocMode::Forms.json_key(), "forms");
        assert_eq!(DocMode::Bookmarks.json_key(), "bookmarks");
        assert_eq!(DocMode::Annotations.json_key(), "annotations");
        assert_eq!(DocMode::Text.json_key(), "text");
        assert_eq!(DocMode::Operators.json_key(), "operators");
        assert_eq!(DocMode::Tags.json_key(), "tags");
        assert_eq!(DocMode::Tree.json_key(), "tree");
        assert_eq!(DocMode::FindText.json_key(), "find_text");
        assert_eq!(DocMode::Detail(DetailSub::Security).json_key(), "security");
        assert_eq!(
            DocMode::Detail(DetailSub::Embedded).json_key(),
            "embedded_files"
        );
        assert_eq!(DocMode::Detail(DetailSub::Labels).json_key(), "page_labels");
        assert_eq!(DocMode::Detail(DetailSub::Layers).json_key(), "layers");
    }

    // ── Args::resolve_mode ─────────────────────────────────────────

    #[test]
    fn resolve_mode_no_flags_returns_default() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf"]);
        let mode = args.resolve_mode().unwrap();
        assert!(matches!(mode, ResolvedMode::Default));
    }

    #[test]
    fn resolve_mode_single_doc_mode() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--fonts"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                assert_eq!(modes.len(), 1);
                assert_eq!(modes[0], DocMode::Fonts);
            }
            other => panic!(
                "Expected Combined, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn resolve_mode_multiple_doc_modes() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--fonts", "--images"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                assert_eq!(modes.len(), 2);
                assert!(modes.contains(&DocMode::Fonts));
                assert!(modes.contains(&DocMode::Images));
            }
            other => panic!(
                "Expected Combined, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn resolve_mode_standalone_object() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--object", "5"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Standalone(StandaloneMode::Object { nums }) => {
                assert_eq!(nums, vec![5]);
            }
            _ => panic!("Expected Standalone(Object)"),
        }
    }

    #[test]
    fn resolve_mode_standalone_inspect() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--inspect", "7"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Standalone(StandaloneMode::Inspect { obj_num }) => {
                assert_eq!(obj_num, 7);
            }
            _ => panic!("Expected Standalone(Inspect)"),
        }
    }

    #[test]
    fn resolve_mode_standalone_search() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--search", "Type=Font"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Standalone(StandaloneMode::Search {
                expr,
                list_modifier,
            }) => {
                assert_eq!(expr, "Type=Font");
                assert!(!list_modifier);
            }
            _ => panic!("Expected Standalone(Search)"),
        }
    }

    #[test]
    fn resolve_mode_standalone_plus_doc_mode_error() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--object", "5", "--fonts"]);
        match args.resolve_mode() {
            Err(err) => assert!(
                err.contains("Cannot combine standalone mode"),
                "got: {}",
                err
            ),
            Ok(_) => panic!("Expected error for standalone + doc mode"),
        }
    }

    #[test]
    fn resolve_mode_multiple_standalone_error() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--object", "5", "--inspect", "3"]);
        match args.resolve_mode() {
            Err(err) => assert!(err.contains("Only one standalone mode"), "got: {}", err),
            Ok(_) => panic!("Expected error for multiple standalone modes"),
        }
    }

    #[test]
    fn resolve_mode_search_with_list_modifier() {
        use clap::Parser;
        // When --search is active, --list should be consumed as search modifier, not DocMode
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--search", "Type=Font", "--list"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Standalone(StandaloneMode::Search {
                expr,
                list_modifier,
            }) => {
                assert_eq!(expr, "Type=Font");
                assert!(list_modifier);
            }
            _ => panic!("Expected Standalone(Search) with list_modifier=true"),
        }
    }

    #[test]
    fn resolve_mode_list_without_search_is_doc_mode() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--list"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                assert_eq!(modes.len(), 1);
                assert_eq!(modes[0], DocMode::List);
            }
            other => panic!(
                "Expected Combined([List]), got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn resolve_mode_single_detail() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--detail", "security"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                assert_eq!(modes.len(), 1);
                assert_eq!(modes[0], DocMode::Detail(DetailSub::Security));
            }
            other => panic!(
                "Expected Combined([Detail(Security)]), got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn resolve_mode_multiple_details() {
        use clap::Parser;
        let args = Args::parse_from([
            "pdf-dump", "test.pdf", "--detail", "security", "--detail", "embedded", "--detail",
            "layers",
        ]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                assert_eq!(modes.len(), 3);
                assert!(modes.contains(&DocMode::Detail(DetailSub::Security)));
                assert!(modes.contains(&DocMode::Detail(DetailSub::Embedded)));
                assert!(modes.contains(&DocMode::Detail(DetailSub::Layers)));
            }
            other => panic!(
                "Expected Combined with 3 details, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn resolve_mode_detail_mixed_with_doc_modes() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--fonts", "--detail", "labels"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                assert_eq!(modes.len(), 2);
                assert!(modes.contains(&DocMode::Fonts));
                assert!(modes.contains(&DocMode::Detail(DetailSub::Labels)));
            }
            other => panic!(
                "Expected Combined with Fonts + Detail, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn resolve_mode_all_doc_modes_at_once() {
        use clap::Parser;
        let args = Args::parse_from([
            "pdf-dump",
            "test.pdf",
            "--list",
            "--validate",
            "--fonts",
            "--images",
            "--forms",
            "--bookmarks",
            "--annotations",
            "--text",
            "--operators",
            "--tags",
            "--tree",
            "--find-text",
            "hello",
            "--detail",
            "security",
            "--detail",
            "embedded",
            "--detail",
            "labels",
            "--detail",
            "layers",
        ]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                // 12 base doc modes + 4 detail modes = 16
                assert_eq!(modes.len(), 16);
            }
            other => panic!(
                "Expected Combined with 16 modes, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn resolve_mode_standalone_extract_stream() {
        use clap::Parser;
        let args = Args::parse_from([
            "pdf-dump",
            "test.pdf",
            "--extract-stream",
            "12",
            "--output",
            "out.bin",
        ]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Standalone(StandaloneMode::ExtractStream { obj_num, output }) => {
                assert_eq!(obj_num, 12);
                assert_eq!(output, PathBuf::from("out.bin"));
            }
            _ => panic!("Expected Standalone(ExtractStream)"),
        }
    }

    #[test]
    fn resolve_mode_object_with_multi_spec() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--object", "1,5,10-12"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Standalone(StandaloneMode::Object { nums }) => {
                assert_eq!(nums, vec![1, 5, 10, 11, 12]);
            }
            _ => panic!("Expected Standalone(Object) with multiple nums"),
        }
    }

    #[test]
    fn resolve_mode_object_invalid_spec_error() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--object", "abc"]);
        match args.resolve_mode() {
            Err(err) => assert!(err.contains("Invalid object number"), "got: {}", err),
            Ok(_) => panic!("Expected error for invalid object spec"),
        }
    }

    // ── DumpConfig ─────────────────────────────────────────────────

    #[test]
    fn dump_config_construction() {
        let config = DumpConfig {
            decode: true,
            truncate: Some(1024),
            json: true,
            hex: false,
            depth: Some(3),
            deref: true,
            raw: false,
        };
        assert!(config.decode);
        assert_eq!(config.truncate, Some(1024));
        assert!(config.json);
        assert!(!config.hex);
        assert_eq!(config.depth, Some(3));
        assert!(config.deref);
        assert!(!config.raw);
    }

    #[test]
    fn dump_config_defaults_style() {
        let config = DumpConfig {
            decode: false,
            truncate: None,
            json: false,
            hex: false,
            depth: None,
            deref: false,
            raw: false,
        };
        assert!(!config.decode);
        assert_eq!(config.truncate, None);
        assert!(!config.json);
        assert!(!config.hex);
        assert_eq!(config.depth, None);
        assert!(!config.deref);
        assert!(!config.raw);
    }

    #[test]
    fn dump_config_is_copy() {
        let config = DumpConfig {
            decode: true,
            truncate: None,
            json: false,
            hex: true,
            depth: Some(5),
            deref: false,
            raw: true,
        };
        let copy = config; // Copy
        // Both should still be usable (Copy semantics)
        assert_eq!(config.decode, copy.decode);
        assert_eq!(config.hex, copy.hex);
        assert_eq!(config.raw, copy.raw);
    }

    // ── DocMode ordering ───────────────────────────────────────────

    #[test]
    fn doc_mode_ordering() {
        assert!(DocMode::List < DocMode::Validate);
        assert!(DocMode::Validate < DocMode::Fonts);
        assert!(DocMode::Fonts < DocMode::Images);
        assert!(DocMode::Images < DocMode::Forms);
        assert!(DocMode::Forms < DocMode::Bookmarks);
        assert!(DocMode::Bookmarks < DocMode::Annotations);
        assert!(DocMode::Annotations < DocMode::Text);
        assert!(DocMode::Text < DocMode::Operators);
        assert!(DocMode::Operators < DocMode::Tags);
        assert!(DocMode::Tags < DocMode::Tree);
        assert!(DocMode::Tree < DocMode::FindText);
        assert!(DocMode::FindText < DocMode::Detail(DetailSub::Security));
    }

    #[test]
    fn doc_mode_detail_sub_ordering_within_detail() {
        assert!(DocMode::Detail(DetailSub::Security) < DocMode::Detail(DetailSub::Embedded));
        assert!(DocMode::Detail(DetailSub::Embedded) < DocMode::Detail(DetailSub::Labels));
        assert!(DocMode::Detail(DetailSub::Labels) < DocMode::Detail(DetailSub::Layers));
    }

    #[test]
    fn doc_mode_sorting() {
        let mut modes = vec![
            DocMode::Tree,
            DocMode::Fonts,
            DocMode::List,
            DocMode::Detail(DetailSub::Layers),
            DocMode::Detail(DetailSub::Security),
        ];
        modes.sort();
        assert_eq!(
            modes,
            vec![
                DocMode::List,
                DocMode::Fonts,
                DocMode::Tree,
                DocMode::Detail(DetailSub::Security),
                DocMode::Detail(DetailSub::Layers),
            ]
        );
    }

    // ── DetailSub ordering ─────────────────────────────────────────

    #[test]
    fn detail_sub_ordering() {
        assert!(DetailSub::Security < DetailSub::Embedded);
        assert!(DetailSub::Embedded < DetailSub::Labels);
        assert!(DetailSub::Labels < DetailSub::Layers);
    }

    #[test]
    fn detail_sub_sorting() {
        let mut subs = vec![
            DetailSub::Layers,
            DetailSub::Security,
            DetailSub::Labels,
            DetailSub::Embedded,
        ];
        subs.sort();
        assert_eq!(
            subs,
            vec![
                DetailSub::Security,
                DetailSub::Embedded,
                DetailSub::Labels,
                DetailSub::Layers,
            ]
        );
    }

    #[test]
    fn detail_sub_equality() {
        assert_eq!(DetailSub::Security, DetailSub::Security);
        assert_ne!(DetailSub::Security, DetailSub::Embedded);
    }

    // ── parse_object_spec additional edge cases ────────────────────

    #[test]
    fn parse_object_spec_large_range() {
        let result = parse_object_spec("1-100").unwrap();
        assert_eq!(result.len(), 100);
        assert_eq!(result[0], 1);
        assert_eq!(result[99], 100);
    }

    #[test]
    fn parse_object_spec_only_commas() {
        let err = parse_object_spec(",,,").unwrap_err();
        assert!(err.contains("Empty object specification"), "got: {}", err);
    }

    #[test]
    fn parse_object_spec_zero() {
        // Zero is technically valid for parse_object_spec (unlike PageSpec)
        let result = parse_object_spec("0").unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn parse_object_spec_large_numbers() {
        let result = parse_object_spec("999999").unwrap();
        assert_eq!(result, vec![999999]);
    }

    #[test]
    fn parse_object_spec_leading_comma() {
        let result = parse_object_spec(",1,5").unwrap();
        assert_eq!(result, vec![1, 5]);
    }

    // ── PageSpec additional edge cases ─────────────────────────────

    #[test]
    fn page_spec_range_pages_large() {
        let spec = PageSpec::Range(1, 10);
        let pages = spec.pages();
        assert_eq!(pages.len(), 10);
        assert_eq!(pages, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn page_spec_single_pages_returns_one_element() {
        let spec = PageSpec::Single(42);
        let pages = spec.pages();
        assert_eq!(pages, vec![42]);
    }

    #[test]
    fn page_spec_single_contains_only_self() {
        let spec = PageSpec::Single(1);
        assert!(spec.contains(1));
        assert!(!spec.contains(0));
        assert!(!spec.contains(2));
        assert!(!spec.contains(u32::MAX));
    }

    #[test]
    fn page_spec_parse_negative_like_string() {
        // Something like "-5" would be parsed as a range with empty start
        let err = PageSpec::parse("-5").unwrap_err();
        assert!(err.contains("Invalid page range start"), "got: {}", err);
    }

    #[test]
    fn page_spec_parse_double_dash() {
        // "1-2-3" — split_once on '-' gives "1" and "2-3", "2-3" is not a valid u32
        let err = PageSpec::parse("1-2-3").unwrap_err();
        assert!(err.contains("Invalid page range end"), "got: {}", err);
    }

    // ── Args modifier flags ────────────────────────────────────────

    #[test]
    fn args_page_modifier_alone_is_default() {
        use clap::Parser;
        // --page alone (no mode flags) should resolve to Default
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--page", "3"]);
        let mode = args.resolve_mode().unwrap();
        assert!(matches!(mode, ResolvedMode::Default));
        assert_eq!(args.page, Some("3".to_string()));
    }

    #[test]
    fn args_json_modifier_alone_is_default() {
        use clap::Parser;
        // --json alone (no mode flags) should resolve to Default
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--json"]);
        let mode = args.resolve_mode().unwrap();
        assert!(matches!(mode, ResolvedMode::Default));
        assert!(args.json);
    }

    #[test]
    fn args_find_text_is_doc_mode() {
        use clap::Parser;
        let args = Args::parse_from(["pdf-dump", "test.pdf", "--find-text", "hello"]);
        let mode = args.resolve_mode().unwrap();
        match mode {
            ResolvedMode::Combined(modes) => {
                assert_eq!(modes.len(), 1);
                assert_eq!(modes[0], DocMode::FindText);
            }
            other => panic!(
                "Expected Combined([FindText]), got {:?}",
                std::mem::discriminant(&other)
            ),
        }
        assert_eq!(args.find_text, Some("hello".to_string()));
    }
}
