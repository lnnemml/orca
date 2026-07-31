//! Search projection (variant B for the retrieval gate). A cleaned view of a
//! section body for FTS: strip MyST directive/target syntax and LaTeX math, but
//! **keep the contents of code fences** (` ```orca ` input examples) — in a quantum
//! chemistry manual an input line is often a better search target than the prose
//! around it (measured: 42.7 % of corpus bytes are inside fences,
//! `wiki/orca/manual-sources.md`). Whether this beats indexing the raw body is
//! decided by the retrieval gate, not by taste.
//!
//! The fence walk reuses `sections::{fence_open_info, is_fence_close}` — one fence
//! rule, not a third copy.

use super::sections::{fence_open_info, is_fence_close};

/// Produce the cleaned, searchable text of a section body.
pub fn search_projection(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some((ch, len, info)) = fence_open_info(lines[i]) {
            i += 1;
            let mut interior: Vec<&str> = Vec::new();
            while i < lines.len() && !is_fence_close(lines[i], ch, len) {
                interior.push(lines[i]);
                i += 1;
            }
            if i < lines.len() {
                i += 1; // consume the closing fence
            }
            let info = info.trim();
            if info.starts_with('{') {
                // MyST directive. Drop math wholesale; otherwise keep the interior
                // prose (note/admonition/table text), minus directive option lines.
                let name = info.trim_start_matches('{').trim_end_matches('}');
                if name == "math" || name == "eval-rst" {
                    continue;
                }
                for l in interior {
                    if is_directive_option(l) {
                        continue;
                    }
                    let c = clean_inline(l);
                    if !c.trim().is_empty() {
                        out.push(c);
                    }
                }
            } else {
                // Code fence (```orca, ```python, bare ```): keep the code verbatim.
                for l in interior {
                    out.push(l.to_string());
                }
            }
            continue;
        }
        let c = clean_inline(lines[i]);
        if !c.trim().is_empty() {
            out.push(c);
        }
        i += 1;
    }
    out.join("\n")
}

/// A MyST directive option line, e.g. `:maxdepth: 2`, `:name:`, `:widths:`.
fn is_directive_option(line: &str) -> bool {
    let t = line.trim_start();
    let mut chars = t.char_indices();
    if chars.next().map(|(_, c)| c) != Some(':') {
        return false;
    }
    // `:word:` — a second colon after ≥1 word char.
    let rest = &t[1..];
    match rest.find(':') {
        Some(j) if j >= 1 => rest[..j].chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
        _ => false,
    }
}

/// Strip a single prose line to its searchable text: drop MyST target lines,
/// LaTeX `$…$`, MyST roles, and reduce `[text](target)` to `text`.
fn clean_inline(line: &str) -> String {
    let t = line.trim();
    // MyST target: `(sec:foo)=`
    if t.starts_with('(') && t.ends_with(")=") {
        return String::new();
    }
    let mut s = line.to_string();
    s = strip_between(&s, '$'); // LaTeX inline/blocks $…$, $$…$$
    s = strip_myst_roles(&s); // {ref}`x`, {math}`x`, {doc}`x` → drop the `x`
    s = markdown_link_text(&s); // [text](target) → text
    s
}

/// Remove text between paired `delim` characters (LaTeX `$…$`). Unpaired trailing
/// delimiter is left as-is.
fn strip_between(s: &str, delim: char) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        if c == delim {
            inside = !inside;
            continue;
        }
        if !inside {
            out.push(c);
        }
    }
    if inside {
        // Unbalanced: we dropped a tail we shouldn't have — recover by returning original.
        return s.to_string();
    }
    out
}

/// Drop MyST inline roles `{name}` immediately before a backtick span:
/// `` {ref}`sec:foo` `` → removed (the target id is noise, not prose).
fn strip_myst_roles(s: &str) -> String {
    // Cheap, allocation-light pass: whenever we see `}` `` ` `` right after a `{...}`,
    // skip the role. Fall back to a simple regex-free scan.
    let bytes = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        if bytes[i] == b'{' {
            if let Some(close) = s[i..].find('}') {
                let after = i + close + 1;
                if s[after..].starts_with('`') {
                    if let Some(tick) = s[after + 1..].find('`') {
                        i = after + 1 + tick + 1; // skip `{role}` and the backtick span
                        continue;
                    }
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `[text](target)` → `text` (keep the human words, drop the link target).
fn markdown_link_text(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        match (tail.find(']'), tail.find("](")) {
            (Some(close), Some(paren)) if close == paren => {
                out.push_str(&tail[..close]); // the link text
                if let Some(end) = tail[close + 2..].find(')') {
                    rest = &tail[close + 2 + end + 1..];
                } else {
                    rest = &tail[close + 1..];
                }
            }
            _ => {
                out.push('[');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_orca_code_blocks() {
        let body = "Set up RIJCOSX:\n\n```orca\n! RIJCOSX def2/J\n%pal nprocs 4 end\n```\n\nDone.";
        let p = search_projection(body);
        assert!(p.contains("RIJCOSX def2/J"), "orca block kept: {p:?}");
        assert!(p.contains("nprocs 4"));
        assert!(!p.contains("```"));
    }

    #[test]
    fn drops_math_directive_and_latex() {
        let body = "The energy $E = mc^2$ is exact.\n\n```{math}\nΨ = φ\n```\n\nNext.";
        let p = search_projection(body);
        assert!(p.contains("The energy"));
        assert!(p.contains("is exact"));
        assert!(!p.contains("mc^2"));
        assert!(!p.contains("Ψ"));
    }

    #[test]
    fn keeps_note_prose_drops_options_and_target() {
        let body = "(sec:foo)=\n## H\n\n:::{note}\n:name: mynote\nUse TightOpt for accuracy.\n:::";
        let p = search_projection(body);
        assert!(p.contains("Use TightOpt for accuracy"));
        assert!(!p.contains("mynote"));
        assert!(!p.contains("sec:foo"));
    }

    #[test]
    fn link_text_survives_target_dropped() {
        let p = search_projection("See [the CPCM section](sec:solv.cpcm) for water.");
        assert!(p.contains("the CPCM section"));
        assert!(p.contains("for water"));
        assert!(!p.contains("sec:solv.cpcm"));
    }
}
