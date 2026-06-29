//! Adobe Glyph List (AGL) glyph-name → Unicode resolution for Tier 1 text
//! extraction.
//!
//! Simple fonts that remap character codes through an `/Encoding /Differences`
//! array name each code's glyph (e.g. `/Lcommaaccent`, `/uni00E9`, `/f_f_i`)
//! rather than pointing at a Unicode value. Without resolving those names every
//! overridden code extracts as U+FFFD. This module turns a glyph name into the
//! string it represents using two mechanisms:
//!
//! 1. **The Adobe Glyph List** (`glyphlist.txt`, embedded verbatim — it is
//!    freely redistributable under Adobe's BSD-style license). ~4,300 entries
//!    map a name to one or more Unicode scalar values (all BMP, 4 hex digits),
//!    e.g. `Lcommaaccent;013B` or the multi-codepoint `dalethatafpatah;05D3 05B2`.
//! 2. **Algorithmic forms** that need no table (per the AGL specification):
//!    - `uniXXXX` — one or more 4-hex UTF-16 code units (so surrogate pairs like
//!      `uniD834DD1E` → U+1D11E decode correctly via `String::from_utf16`).
//!    - `uXXXX`..`uXXXXXX` — a single 4-to-6-hex Unicode scalar value.
//!    - A `.suffix` (`a.sc`, `one.oldstyle`) is dropped before resolution.
//!    - Underscore-joined ligature names (`f_f_i`) resolve each component and
//!      concatenate.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Adobe's canonical glyph-name → Unicode list, embedded at build time.
const GLYPHLIST_TXT: &str = include_str!("glyphlist.txt");

/// Parse `glyphlist.txt` once into a `name -> string` map. Keys are slices of
/// the embedded `&'static str` (no allocation); values are the decoded Unicode
/// (owned, since they are assembled from hex code points).
fn agl() -> &'static HashMap<&'static str, String> {
    static MAP: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for line in GLYPHLIST_TXT.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, codes)) = line.split_once(';') else {
                continue;
            };
            let mut value = String::new();
            let mut ok = true;
            for cp in codes.split(' ') {
                match u32::from_str_radix(cp, 16).ok().and_then(char::from_u32) {
                    Some(c) => value.push(c),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && !value.is_empty() {
                map.insert(name, value);
            }
        }
        map
    })
}

/// Resolve a glyph name to the string it represents, or `None` if no mapping is
/// known. Handles `.suffix` stripping and underscore-joined ligature components;
/// each component is resolved against the AGL table or the `uni`/`u` algorithmic
/// forms.
pub(crate) fn glyph_name_to_string(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    // Drop a `.suffix` (variant marker): `a.sc` → `a`. A leading dot (`.notdef`)
    // leaves an empty base, which has no mapping.
    let base = name.split('.').next().unwrap_or("");
    if base.is_empty() {
        return None;
    }

    // Underscore-joined ligature: resolve every component, concatenate. Without
    // an underscore this is a single component (the whole `base`).
    let mut out = String::new();
    for component in base.split('_') {
        out.push_str(&resolve_component(component)?);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Resolve a single (suffix-stripped, underscore-free) glyph-name component.
fn resolve_component(component: &str) -> Option<String> {
    if component.is_empty() {
        return None;
    }
    // The AGL table takes precedence over the algorithmic forms.
    if let Some(s) = agl().get(component) {
        return Some(s.clone());
    }
    // `uniXXXX...` — one or more 4-hex UTF-16 code units (surrogate pairs allowed).
    if let Some(hex) = component.strip_prefix("uni")
        && hex.len() >= 4
        && hex.len().is_multiple_of(4)
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let mut units = Vec::with_capacity(hex.len() / 4);
        for chunk in hex.as_bytes().chunks(4) {
            let s = std::str::from_utf8(chunk).ok()?;
            units.push(u16::from_str_radix(s, 16).ok()?);
        }
        return String::from_utf16(&units).ok();
    }
    // `uXXXX`..`uXXXXXX` — a single 4-to-6-hex Unicode scalar value.
    if let Some(hex) = component.strip_prefix('u')
        && (4..=6).contains(&hex.len())
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let cp = u32::from_str_radix(hex, 16).ok()?;
        return char::from_u32(cp).map(|c| c.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_agl_names() {
        assert_eq!(glyph_name_to_string("A"), Some("A".to_string()));
        assert_eq!(glyph_name_to_string("ampersand"), Some("&".to_string()));
        // Lcommaaccent;013B → U+013B (LATIN CAPITAL LETTER L WITH CEDILLA).
        assert_eq!(
            glyph_name_to_string("Lcommaaccent"),
            Some("\u{013B}".to_string())
        );
    }

    #[test]
    fn multi_codepoint_agl_entry() {
        // dalethatafpatah;05D3 05B2 → two BMP scalars concatenated.
        assert_eq!(
            glyph_name_to_string("dalethatafpatah"),
            Some("\u{05D3}\u{05B2}".to_string())
        );
    }

    #[test]
    fn uni_form_bmp() {
        assert_eq!(glyph_name_to_string("uni0041"), Some("A".to_string()));
        assert_eq!(glyph_name_to_string("uni00E9"), Some("é".to_string()));
    }

    #[test]
    fn uni_form_surrogate_pair() {
        // uniD834DD1E: UTF-16 surrogate pair D834 DD1E → U+1D11E (𝄞 G CLEF).
        assert_eq!(
            glyph_name_to_string("uniD834DD1E"),
            Some("\u{1D11E}".to_string())
        );
    }

    #[test]
    fn uni_form_multiple_units() {
        // Two BMP code units back to back: A then é.
        assert_eq!(glyph_name_to_string("uni004100E9"), Some("Aé".to_string()));
    }

    #[test]
    fn u_form_astral() {
        // uXXXXX: a single 4-to-6-hex scalar, here U+1F600 (😀).
        assert_eq!(
            glyph_name_to_string("u1F600"),
            Some("\u{1F600}".to_string())
        );
        assert_eq!(glyph_name_to_string("u0041"), Some("A".to_string()));
    }

    #[test]
    fn suffix_is_stripped() {
        // a.sc → resolve "a"; A.alt → "A".
        assert_eq!(glyph_name_to_string("a.sc"), Some("a".to_string()));
        assert_eq!(glyph_name_to_string("A.alt"), Some("A".to_string()));
        // uni-form with a suffix still resolves.
        assert_eq!(glyph_name_to_string("uni00E9.sc"), Some("é".to_string()));
    }

    #[test]
    fn underscore_ligature_concatenates_components() {
        // f_f_i → f + f + i = "ffi" (the ASCII components, not the U+FB03 ligature).
        assert_eq!(glyph_name_to_string("f_f_i"), Some("ffi".to_string()));
        assert_eq!(glyph_name_to_string("f_i"), Some("fi".to_string()));
        // Mixed component kinds: AGL name + uni form.
        assert_eq!(glyph_name_to_string("f_uni0069"), Some("fi".to_string()));
    }

    #[test]
    fn unresolved_names_are_none() {
        assert_eq!(glyph_name_to_string("g13"), None);
        assert_eq!(glyph_name_to_string("notarealglyphname"), None);
        assert_eq!(glyph_name_to_string(""), None);
        // Leading-dot names (e.g. .notdef) have an empty base → no mapping.
        assert_eq!(glyph_name_to_string(".notdef"), None);
        // A ligature whose component is unresolvable fails as a whole.
        assert_eq!(glyph_name_to_string("f_bogusname"), None);
    }

    #[test]
    fn non_hex_uni_falls_through_to_table_or_none() {
        // "uhungarumlaut" starts with 'u' but the remainder is not hex, so it is
        // looked up in the AGL (where it exists) rather than parsed as a scalar.
        assert!(glyph_name_to_string("uhungarumlaut").is_some());
        // "union" likewise: a real AGL name (∪), not a hex form.
        assert_eq!(glyph_name_to_string("union"), Some("\u{222A}".to_string()));
        // A 'u'-prefixed name that is neither hex nor in the AGL has no mapping.
        assert_eq!(glyph_name_to_string("uglyphnotreal"), None);
    }
}
