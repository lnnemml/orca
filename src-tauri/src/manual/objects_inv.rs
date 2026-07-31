//! Sphinx `objects.inv` (inventory version 2) reader — the **authoritative**
//! label→anchor map for the manual (ADR-013 / `wiki/orca/manual-sources.md`). Our
//! own `predict_anchor` is the independent cross-check asserted against it, never
//! a replacement (rule #9). This file only *parses* the inventory; the cross-check
//! lives at the call site (the corpus gate).
//!
//! Format: four `#`-comment header lines, then a **zlib** stream whose decompressed
//! lines are `name domain:role priority uri dispname`. A `uri` ending in `$` is
//! shorthand for the object name; a `dispname` of `-` means "same as name".

use std::io::Read;

use flate2::read::ZlibDecoder;

/// One inventory record.
#[derive(Debug, Clone)]
pub struct InvEntry {
    pub name: String,
    pub domain_role: String,
    pub priority: i64,
    pub uri: String,
    pub dispname: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InvError {
    #[error("not a Sphinx inventory v2 header: {0}")]
    BadHeader(String),
    #[error("zlib decompress failed: {0}")]
    Inflate(#[source] std::io::Error),
    #[error("no inventory entries parsed")]
    Empty,
}

/// Parse raw `objects.inv` bytes into records.
pub fn parse(bytes: &[u8]) -> Result<Vec<InvEntry>, InvError> {
    // Header: consecutive lines starting with '#'. The zlib stream begins at the
    // first line that does not (its first byte is 0x78, not '#').
    let first_line_end = bytes.iter().position(|&b| b == b'\n').unwrap_or(0);
    let first_line = &bytes[..first_line_end];
    if !first_line.starts_with(b"# Sphinx inventory version 2") {
        return Err(InvError::BadHeader(
            String::from_utf8_lossy(first_line).into_owned(),
        ));
    }

    let mut pos = 0usize;
    while pos < bytes.len() && bytes[pos] == b'#' {
        match bytes[pos..].iter().position(|&b| b == b'\n') {
            Some(nl) => pos += nl + 1,
            None => break,
        }
    }

    let mut text = String::new();
    ZlibDecoder::new(&bytes[pos..])
        .read_to_string(&mut text)
        .map_err(InvError::Inflate)?;

    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(e) = parse_entry(line) {
            out.push(e);
        }
    }
    if out.is_empty() {
        return Err(InvError::Empty);
    }
    Ok(out)
}

/// One inventory line: `name domain:role priority uri dispname`. `name` and `uri`
/// have no spaces; `dispname` is the rest (may contain spaces). The first four
/// fields are whitespace-delimited — no regex needed.
fn parse_entry(line: &str) -> Option<InvEntry> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 5 {
        return None;
    }
    let name = tokens[0].to_string();
    let domain_role = tokens[1].to_string();
    let priority: i64 = tokens[2].parse().ok()?;
    let mut uri = tokens[3].to_string();
    let dispname_raw = tokens[4..].join(" ");

    if let Some(stripped) = uri.strip_suffix('$') {
        uri = format!("{stripped}{name}");
    }
    let dispname = if dispname_raw == "-" { name.clone() } else { dispname_raw };

    Some(InvEntry { name, domain_role, priority, uri, dispname })
}

/// Split an inventory `uri` into `(file_without_.html, fragment)`. E.g.
/// `contents/essentialelements/RI.html#sec-essentialelements-ri` →
/// `("contents/essentialelements/RI", Some("sec-essentialelements-ri"))`.
pub fn uri_file_and_fragment(uri: &str) -> (String, Option<String>) {
    let (path, frag) = match uri.split_once('#') {
        Some((p, f)) => (p, Some(f.to_string())),
        None => (uri, None),
    };
    let file = path.strip_suffix(".html").unwrap_or(path).to_string();
    (file, frag)
}

/// Result of cross-checking our sections' labels against the authoritative
/// inventory. Post-conditions (b) and (c) as library code, not just test asserts:
/// the gate (and later 4.3) call this and act on the counts.
#[derive(Debug, Default)]
pub struct AnchorReport {
    pub inv_entries: usize,
    pub inv_labels: usize,
    pub entries_not_ours: usize,
    pub our_labels: usize,
    pub found_in_inv: usize,
    /// (b) predict_anchor(label) != the inventory fragment. Named, not silently resolved.
    pub anchor_mismatches: Vec<String>,
    /// (c) label bound to section S in file F, but the inventory uri points elsewhere.
    pub binding_mismatches: Vec<String>,
    /// Heading-bound labels absent from the inventory (diagnostic for 4.3: are these
    /// unregistered section labels, or non-section targets our scan over-captured?).
    pub not_found: Vec<String>,
}

/// Cross-check every heading-bound label against `objects.inv`: (b) our independent
/// `predict_anchor` must equal the inventory fragment; (c) the inventory uri must
/// point at the file the label is bound in. Mismatches are collected and NAMED.
pub fn verify_against_inventory(
    sections: &[super::sections::Section],
    entries: &[InvEntry],
) -> AnchorReport {
    use std::collections::{HashMap, HashSet};

    let map: HashMap<&str, &InvEntry> = entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let our: HashSet<&str> = sections
        .iter()
        .flat_map(|s| s.labels.iter().map(String::as_str))
        .collect();

    let mut rep = AnchorReport {
        inv_entries: entries.len(),
        inv_labels: entries.iter().filter(|e| e.domain_role == "std:label").count(),
        entries_not_ours: entries.iter().filter(|e| !our.contains(e.name.as_str())).count(),
        ..Default::default()
    };

    for s in sections {
        for label in &s.labels {
            rep.our_labels += 1;
            let Some(entry) = map.get(label.as_str()) else {
                rep.not_found.push(label.clone());
                continue;
            };
            rep.found_in_inv += 1;
            let (uri_file, frag) = uri_file_and_fragment(&entry.uri);
            let predicted = super::sections::predict_anchor(label);
            if frag.as_deref() != Some(predicted.as_str()) {
                rep.anchor_mismatches
                    .push(format!("{label}: inv #{frag:?} vs predict #{predicted}"));
            }
            if uri_file != s.file {
                rep.binding_mismatches
                    .push(format!("{label}: inv->{uri_file} but bound in {}", s.file));
            }
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    fn make_inv(body: &str) -> Vec<u8> {
        let header = b"# Sphinx inventory version 2\n\
                       # Project: ORCA\n\
                       # Version: 6.1\n\
                       # The remainder of this file is compressed using zlib.\n";
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(body.as_bytes()).unwrap();
        let compressed = enc.finish().unwrap();
        let mut out = header.to_vec();
        out.extend_from_slice(&compressed);
        out
    }

    #[test]
    fn parses_a_label_entry() {
        let inv = make_inv(
            "sec:essentialelements.ri std:label -1 contents/essentialelements/RI.html#sec-essentialelements-ri RI\n",
        );
        let e = parse(&inv).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "sec:essentialelements.ri");
        assert_eq!(e[0].domain_role, "std:label");
        let (file, frag) = uri_file_and_fragment(&e[0].uri);
        assert_eq!(file, "contents/essentialelements/RI");
        assert_eq!(frag.as_deref(), Some("sec-essentialelements-ri"));
    }

    #[test]
    fn expands_dollar_uri_and_dash_dispname() {
        let inv = make_inv("foo:bar std:label -1 contents/x.html#$ -\n");
        let e = parse(&inv).unwrap();
        assert_eq!(e[0].uri, "contents/x.html#foo:bar");
        assert_eq!(e[0].dispname, "foo:bar"); // '-' means same as name
    }

    #[test]
    fn rejects_non_inventory() {
        assert!(parse(b"not an inventory\n\x78\x9c").is_err());
    }
}
