//! ATX sectioner for the ORCA manual Markdown (ADR-013 (3): Rust, over Markdown,
//! no HTML parser). Cuts each `_sources/*.md.txt` leaf into [`Section`]s.
//!
//! **Section definition (explicit — it is not self-evident).** A section is ONE
//! ATX heading whose body runs to the NEXT heading of ANY level. Bodies are **not
//! nested**: the body of a page-level `#` is only the preamble down to the first
//! `##`. This is what FTS needs (otherwise a parent's text is duplicated in every
//! descendant) and it is exactly what makes **line conservation** checkable — every
//! line of a file belongs to exactly one place.
//!
//! Headings are found **only outside fenced blocks** — a measured necessity: a
//! naive line scan invents ~460 phantom sections because ORCA input examples
//! comment with `#` (`wiki/orca/manual-sources.md`). The fence rule here is the
//! Rust port of the Python `iter_prose_lines` (one rule, ported not reinvented).

use std::collections::HashMap;

/// One ATX section. `line_start` is the heading's line index; the body occupies
/// `line_start+1 ..= line_end` (empty when `line_end == line_start`). Indices are
/// 0-based into the file's `lines()`.
#[derive(Debug, Clone)]
pub struct Section {
    pub file: String,
    pub level: u8,
    pub title: String,
    /// Ancestor titles (nearest-enclosing higher levels), root first; excludes self.
    pub breadcrumb: Vec<String>,
    /// MyST `(name)=` targets stacked directly above the heading, document order.
    pub labels: Vec<String>,
    /// The section's HTML anchor: the slug of the label closest to the heading, or
    /// `None` when unlabelled (Sphinx then auto-generates one from the title slug).
    pub anchor: Option<String>,
    pub body: String,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SectionError {
    /// The central post-condition (rule #9): the sections + preamble must tile the
    /// file's lines exactly once. A gap = a lost paragraph; an overlap = a line in
    /// two sections. Either fails loudly, naming the file and the offending indices.
    #[error("line conservation failed in {file}: {detail}")]
    LineConservation { file: String, detail: String },
}

// --- The fence rule (ported from Python iter_prose_lines) ------------------

/// If `line` opens a fenced block, return its `(fence_char, run_length)`. Backtick
/// CODE fences (```` ```orca ````) AND colon directives (`:::{table}`), any length
/// ≥ 3, leading whitespace tolerated.
fn fence_open(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start();
    let c = t.chars().next()?;
    if c != '`' && c != ':' {
        return None;
    }
    let run = t.chars().take_while(|&x| x == c).count();
    (run >= 3).then_some((c, run))
}

/// A closing fence line: only the fence char, run ≥ the opening run.
fn is_fence_close(line: &str, ch: char, len: usize) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().count() >= len && t.chars().all(|x| x == ch)
}

/// Mark which lines are prose (OUTSIDE any fenced block). Fence delimiter lines and
/// fenced interiors are `false`. Single-active-fence model — nesting is handled by
/// the longer-outer / shorter-inner convention (an inner shorter fence never
/// satisfies the outer close), the same as `parse_toctrees` / the Python original.
pub(crate) fn prose_mask(lines: &[&str]) -> Vec<bool> {
    let mut prose = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if let Some((ch, len)) = fence_open(lines[i]) {
            i += 1;
            while i < lines.len() && !is_fence_close(lines[i], ch, len) {
                i += 1;
            }
            if i < lines.len() {
                i += 1; // consume the closing fence
            }
            continue;
        }
        prose[i] = true;
        i += 1;
    }
    prose
}

// --- ATX headings, titles, labels, anchors ---------------------------------

/// The ATX level of a heading line (`#`…`######` then whitespace then content), or
/// `None`. Requires the `#` run at column 0 and a space before a non-empty title.
pub(crate) fn atx_level(line: &str) -> Option<u8> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    match rest.chars().next() {
        Some(' ') | Some('\t') => {}
        _ => return None,
    }
    (!rest.trim().is_empty()).then_some(hashes as u8)
}

fn heading_title(line: &str) -> String {
    line.trim_start_matches('#')
        .trim()
        .trim_end_matches('#')
        .trim_end()
        .to_string()
}

/// A MyST target line `(name)=` → `name`, else `None`.
fn parse_label(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix('(')?.strip_suffix(")=")?;
    if inner.is_empty() || inner.contains('(') || inner.contains(')') {
        return None;
    }
    Some(inner.to_string())
}

/// The MyST labels that bind to the heading at `heading_idx`: consecutive `(name)=`
/// lines immediately above it, blank lines skipped, stopping at the first other line.
/// Returned in document order.
fn labels_above(lines: &[&str], heading_idx: usize) -> Vec<String> {
    let mut labels = Vec::new();
    let mut j = heading_idx;
    while j > 0 {
        j -= 1;
        let t = lines[j].trim();
        if t.is_empty() {
            continue;
        }
        match parse_label(t) {
            Some(name) => labels.push(name),
            None => break,
        }
    }
    labels.reverse();
    labels
}

/// `(label)` → `#slug`: lowercase, every run of non-alphanumerics → one `-`, trimmed.
/// The independent check on the authoritative `objects.inv` (rule #9). Measured to
/// match real HTML ids 46/46; all corpus labels are ASCII (`manual-sources.md`).
pub(crate) fn predict_anchor(label: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

// --- Sectioning ------------------------------------------------------------

/// Cut `text` (one manual leaf, identified by `file`) into sections, then verify
/// **line conservation** before returning. A conservation failure is an error, not
/// a silent partial result.
pub fn sectionize(file: &str, text: &str) -> Result<Vec<Section>, SectionError> {
    let lines: Vec<&str> = text.lines().collect();
    let prose = prose_mask(&lines);

    let heading_idxs: Vec<usize> = (0..lines.len())
        .filter(|&i| prose[i] && atx_level(lines[i]).is_some())
        .collect();

    let mut sections = Vec::with_capacity(heading_idxs.len());
    let mut stack: Vec<(u8, String)> = Vec::new();

    for (k, &hidx) in heading_idxs.iter().enumerate() {
        let level = atx_level(lines[hidx]).expect("filtered to headings");
        let title = heading_title(lines[hidx]);

        while stack.last().is_some_and(|&(l, _)| l >= level) {
            stack.pop();
        }
        let breadcrumb: Vec<String> = stack.iter().map(|(_, t)| t.clone()).collect();
        stack.push((level, title.clone()));

        let next = heading_idxs.get(k + 1).copied().unwrap_or(lines.len());
        let line_end = next.saturating_sub(1).max(hidx);
        let body = if line_end > hidx {
            lines[hidx + 1..=line_end].join("\n")
        } else {
            String::new()
        };

        let labels = labels_above(&lines, hidx);
        let anchor = labels.last().map(|l| predict_anchor(l));

        sections.push(Section {
            file: file.to_string(),
            level,
            title,
            breadcrumb,
            labels,
            anchor,
            body,
            line_start: hidx,
            line_end,
        });
    }

    verify_line_conservation(file, lines.len(), heading_idxs.first().copied(), &sections)?;
    Ok(sections)
}

/// Rule #9, in our terms: independently re-derive which section (or the preamble)
/// owns each line, and assert the whole file `[0, n)` is covered exactly once.
fn verify_line_conservation(
    file: &str,
    n_lines: usize,
    first_heading: Option<usize>,
    sections: &[Section],
) -> Result<(), SectionError> {
    const PREAMBLE: usize = usize::MAX;
    let mut owner: Vec<Option<usize>> = vec![None; n_lines];

    let mut claim = |idx: usize, who: usize| -> Result<(), SectionError> {
        match owner[idx] {
            None => {
                owner[idx] = Some(who);
                Ok(())
            }
            Some(other) => Err(SectionError::LineConservation {
                file: file.to_string(),
                detail: format!(
                    "line {idx} claimed by two owners ({} and {})",
                    owner_name(other),
                    owner_name(who)
                ),
            }),
        }
    };

    // Preamble: everything before the first heading.
    let pre_end = first_heading.unwrap_or(n_lines);
    for i in 0..pre_end {
        claim(i, PREAMBLE)?;
    }
    // Each section owns its heading line plus its body range.
    for (si, s) in sections.iter().enumerate() {
        claim(s.line_start, si)?;
        for i in (s.line_start + 1)..=s.line_end {
            if i > s.line_start {
                claim(i, si)?;
            }
        }
    }

    let lost: Vec<usize> = (0..n_lines).filter(|&i| owner[i].is_none()).collect();
    if !lost.is_empty() {
        return Err(SectionError::LineConservation {
            file: file.to_string(),
            detail: format!("{} line(s) belong to no section: {:?}", lost.len(), preview(&lost)),
        });
    }
    Ok(())
}

fn owner_name(who: usize) -> String {
    if who == usize::MAX {
        "preamble".to_string()
    } else {
        format!("section#{who}")
    }
}

fn preview(v: &[usize]) -> Vec<usize> {
    v.iter().take(20).copied().collect()
}

/// How many of a slice of label-less sections would collide on their title slug,
/// **per file** — several `## Keywords` in one page slug to the same anchor. A
/// helper the corpus gate uses; collisions must be known before an anchor is a key.
pub(crate) fn title_slug_collisions(sections: &[Section]) -> usize {
    let mut per_file: HashMap<(&str, String), usize> = HashMap::new();
    for s in sections.iter().filter(|s| s.labels.is_empty()) {
        *per_file.entry((s.file.as_str(), predict_anchor(&s.title))).or_insert(0) += 1;
    }
    per_file.values().filter(|&&c| c > 1).map(|&c| c).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\
# Top

preamble text

(sec:alpha)=
## Alpha

alpha body line 1
alpha body line 2

```orca
# not a heading
#### not a heading either
Basis def2-TZVP # inline
```

### Alpha child

child body

## Beta

beta body";

    #[test]
    fn fenced_hashes_are_not_headings() {
        let secs = sectionize("f", DOC).unwrap();
        let titles: Vec<_> = secs.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Top", "Alpha", "Alpha child", "Beta"]);
    }

    #[test]
    fn body_is_not_nested_and_labels_bind_upward() {
        let secs = sectionize("f", DOC).unwrap();
        let alpha = &secs[1];
        assert_eq!(alpha.title, "Alpha");
        assert_eq!(alpha.labels, ["sec:alpha"]);
        assert_eq!(alpha.anchor.as_deref(), Some("sec-alpha"));
        // Alpha's body stops at "### Alpha child" and INCLUDES the code block.
        assert!(alpha.body.contains("alpha body line 2"));
        assert!(alpha.body.contains("# not a heading")); // the fenced hash lives in a body
        assert!(!alpha.body.contains("child body")); // not nested into the child
        assert_eq!(alpha.breadcrumb, ["Top"]);
        assert_eq!(secs[2].breadcrumb, ["Top", "Alpha"]); // ### under ## under #
        assert_eq!(secs[3].breadcrumb, ["Top"]); // ## Beta is a sibling of Alpha
    }

    #[test]
    fn line_conservation_holds_on_a_real_shape() {
        // The whole point: every line owned exactly once (no panic = OK).
        sectionize("f", DOC).unwrap();
    }

    #[test]
    fn two_h1_pages_are_siblings() {
        let secs = sectionize("f", "# One\n\nbody\n\n# Two\n\nbody2").unwrap();
        assert_eq!(secs.len(), 2);
        assert!(secs[0].breadcrumb.is_empty());
        assert!(secs[1].breadcrumb.is_empty()); // second H1 pops the first
    }

    #[test]
    fn empty_body_section_owns_only_its_heading() {
        let secs = sectionize("f", "# A\n## B\nb").unwrap();
        assert_eq!(secs[0].title, "A");
        assert_eq!(secs[0].line_start, 0);
        assert_eq!(secs[0].line_end, 0); // no body: "## B" is the next heading
        assert!(secs[0].body.is_empty());
    }

    #[test]
    fn stacked_labels_all_bind() {
        let secs = sectionize("f", "(sec:a)=\n(sec:b)=\n## H\n\nx").unwrap();
        assert_eq!(secs[0].labels, ["sec:a", "sec:b"]);
        assert_eq!(secs[0].anchor.as_deref(), Some("sec-b")); // closest label
    }

    #[test]
    fn predict_anchor_matches_the_rule() {
        assert_eq!(predict_anchor("sec:essentialelements.ri.rijcosx"), "sec-essentialelements-ri-rijcosx");
        assert_eq!(predict_anchor("table:CPCM-features"), "table-cpcm-features");
    }

    #[test]
    fn nested_fences_do_not_leak() {
        // A 4-backtick block containing a 3-backtick block; inner ``` must not close outer.
        let doc = "# T\n\n````{note}\n```python\n# x\n```\n## fake inside note\n````\n\n## Real";
        let secs = sectionize("f", doc).unwrap();
        let titles: Vec<_> = secs.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["T", "Real"]); // "## fake inside note" is inside the note
    }
}
