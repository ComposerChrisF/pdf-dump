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

pub(crate) enum StandaloneMode {
    Object { nums: Vec<u32> },
    Inspect { obj_num: u32 },
    Search { expr: String, list_modifier: bool },
    ExtractStream { obj_num: u32, output: PathBuf },
}

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
  pdf-dump file.pdf --inspect 5          Explain object 5
  pdf-dump file.pdf --search Type=Font   Find all font objects
  pdf-dump file.pdf --validate --json    Validation results as JSON
  pdf-dump file.pdf --fonts --images     Combine multiple modes
  pdf-dump file.pdf --list               One-line listing of every object
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

    /// Decode and print the content of streams
    #[arg(long, help_heading = "Modifiers")]
    pub decode: bool,

    /// Inline-expand references to show target summaries (use with --object or --page)
    #[arg(long, help_heading = "Modifiers")]
    pub deref: bool,

    /// Limit traversal depth (0 = root only, 1 = root + immediate refs, etc.)
    #[arg(long, help_heading = "Modifiers")]
    pub depth: Option<usize>,

    /// Display binary stream content as hex dump (use with --decode-streams)
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

        if let Some(obj_num) = self.extract_stream {
            let output = self.output.clone().unwrap();
            standalone.push(StandaloneMode::ExtractStream { obj_num, output });
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
        if self.validate { doc_modes.push(DocMode::Validate); }
        if self.fonts { doc_modes.push(DocMode::Fonts); }
        if self.images { doc_modes.push(DocMode::Images); }
        if self.forms { doc_modes.push(DocMode::Forms); }
        if self.bookmarks { doc_modes.push(DocMode::Bookmarks); }
        if self.annotations { doc_modes.push(DocMode::Annotations); }
        if self.text { doc_modes.push(DocMode::Text); }
        if self.operators { doc_modes.push(DocMode::Operators); }
        if self.tags { doc_modes.push(DocMode::Tags); }
        if self.tree { doc_modes.push(DocMode::Tree); }
        if self.find_text.is_some() { doc_modes.push(DocMode::FindText); }
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

pub(crate) enum PageSpec {
    Single(u32),
    Range(u32, u32),
}

impl PageSpec {
    pub fn parse(s: &str) -> Result<PageSpec, String> {
        if let Some((start_s, end_s)) = s.split_once('-') {
            let start: u32 = start_s.trim().parse()
                .map_err(|_| format!("Invalid page range start: '{}'", start_s.trim()))?;
            let end: u32 = end_s.trim().parse()
                .map_err(|_| format!("Invalid page range end: '{}'", end_s.trim()))?;
            if start == 0 || end == 0 {
                return Err("Page numbers must be >= 1".to_string());
            }
            if start > end {
                return Err(format!("Invalid page range: {} > {}", start, end));
            }
            Ok(PageSpec::Range(start, end))
        } else {
            let num: u32 = s.trim().parse()
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
        }
    }

    pub fn pages(&self) -> Vec<u32> {
        match self {
            PageSpec::Single(n) => vec![*n],
            PageSpec::Range(start, end) => (*start..=*end).collect(),
        }
    }
}

pub(crate) fn parse_object_spec(s: &str) -> Result<Vec<u32>, String> {
    let mut result = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        if let Some((start_s, end_s)) = part.split_once('-') {
            let start: u32 = start_s.trim().parse()
                .map_err(|_| format!("Invalid object number: '{}'", start_s.trim()))?;
            let end: u32 = end_s.trim().parse()
                .map_err(|_| format!("Invalid object number: '{}'", end_s.trim()))?;
            if start > end {
                return Err(format!("Invalid object range: {} > {}", start, end));
            }
            result.extend(start..=end);
        } else {
            let num: u32 = part.parse()
                .map_err(|_| format!("Invalid object number: '{}'", part))?;
            result.push(num);
        }
    }
    if result.is_empty() {
        return Err("Empty object specification".to_string());
    }
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

}
