use clap::Parser;
use std::path::PathBuf;

/// Dumps the internal structure of a PDF file.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Args {
    /// Path to the PDF file
    #[arg(required = true)]
    pub file: PathBuf,

    // ── Overview ──────────────────────────────────────────────────────

    /// Print document metadata (version, pages, /Info fields)
    #[arg(short = 'm', long, help_heading = "Overview")]
    pub metadata: bool,

    /// Print a one-line summary of every object
    #[arg(short = 's', long, help_heading = "Overview")]
    pub summary: bool,

    /// Show document statistics (object types, stream sizes, filter usage)
    #[arg(long, help_heading = "Overview")]
    pub stats: bool,

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

    /// Show page resource map (fonts, images, graphics states, color spaces)
    #[arg(long, help_heading = "Content")]
    pub resources: bool,

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

    /// Show tagged PDF logical structure tree
    #[arg(long, help_heading = "Structure")]
    pub structure: bool,

    /// Show optional content groups (layers)
    #[arg(long, alias = "ocg", help_heading = "Structure")]
    pub layers: bool,

    /// Show page labels (logical page numbering)
    #[arg(long, help_heading = "Structure")]
    pub page_labels: bool,

    // ── Annotations & Links ──────────────────────────────────────────

    /// Show annotations (all pages, or filtered with --page)
    #[arg(long, help_heading = "Annotations & Links")]
    pub annotations: bool,

    /// List link annotations with targets (all pages, or filtered with --page)
    #[arg(long, help_heading = "Annotations & Links")]
    pub links: bool,

    /// List form fields (AcroForm)
    #[arg(long, help_heading = "Annotations & Links")]
    pub forms: bool,

    // ── Objects ──────────────────────────────────────────────────────

    /// Print one or more objects by number (e.g. 5, 1,5,12, 3-7, 1,5,10-15)
    #[arg(short = 'o', long, help_heading = "Objects")]
    pub object: Option<String>,

    /// Show a human-readable explanation of an object's role, with full content
    #[arg(long, help_heading = "Objects")]
    pub info: Option<u32>,

    /// Find all objects that reference a given object number
    #[arg(long, help_heading = "Objects")]
    pub refs_to: Option<u32>,

    /// Search for objects matching an expression (e.g. Type=Font, key=MediaBox, value=Hello)
    #[arg(long, help_heading = "Objects")]
    pub search: Option<String>,

    // ── Security & Files ─────────────────────────────────────────────

    /// Show encryption and permission details
    #[arg(long, help_heading = "Security & Files")]
    pub security: bool,

    /// List embedded files (file attachments)
    #[arg(long, help_heading = "Security & Files")]
    pub embedded_files: bool,

    // ── Comparison ───────────────────────────────────────────────────

    /// Compare structurally with a second PDF file
    #[arg(long, help_heading = "Comparison")]
    pub diff: Option<PathBuf>,

    // ── Export ────────────────────────────────────────────────────────

    /// Extract a stream object to a file
    #[arg(long, requires = "output", help_heading = "Export")]
    pub extract_stream: Option<u32>,

    /// Output file for extracted stream
    #[arg(long, requires = "extract_stream", help_heading = "Export")]
    pub output: Option<PathBuf>,

    /// Full depth-first dump of all reachable objects from /Root
    #[arg(long, help_heading = "Export")]
    pub dump: bool,

    // ── Modifiers ────────────────────────────────────────────────────

    /// Dump the object tree for a specific page or range (e.g. 1, 1-3)
    #[arg(long, help_heading = "Modifiers")]
    pub page: Option<String>,

    /// Output as structured JSON
    #[arg(long, help_heading = "Modifiers")]
    pub json: bool,

    /// Decode and print the content of streams
    #[arg(long, help_heading = "Modifiers")]
    pub decode_streams: bool,

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

#[derive(Clone, Copy)]
pub(crate) struct DumpConfig {
    pub decode_streams: bool,
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
