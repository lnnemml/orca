//! Manual index ingest + search (Phase 4.3, ADR-013). Reads the fetched corpus,
//! sectionises it (unit 4.2), resolves each anchor against `objects.inv`, and writes
//! `manual_sections` (+ the external-content `manual_fts`) with **content-preserving
//! post-conditions** (rule #9): the risk here is a search index that answers
//! everything and stays silent about what it dropped, so the guards check that the
//! stored body survives the storage boundary byte-for-byte, not merely that N rows
//! went in.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

use super::objects_inv::{self, uri_file_and_fragment, InvEntry};
use super::sections::{self, predict_anchor, Section};
use crate::error::AppError;

/// Bumped when the sectioning that produced the stored rows changes (like
/// `parser_version` for results) — provenance for a rebuild decision.
pub const SECTIONER_VERSION: i64 = 1;

/// The outcome of an ingest — returned by the command and asserted by the gate.
#[derive(Debug, Clone, Serialize)]
pub struct IngestReport {
    pub orca_version: String,
    pub section_count: usize,
    pub anchors_verified: usize,
    pub null_anchors: usize,
    pub body_bytes: u64,
    pub corpus_hash: String,
}

/// One search hit. `anchor` is `None` when UNDETERMINED — the link then lands on the
/// page without a fragment rather than on a guessed, non-existent one.
#[derive(Debug, Clone, Serialize)]
pub struct ManualHit {
    pub id: i64,
    pub file: String,
    pub breadcrumb: Vec<String>,
    pub title: String,
    pub anchor: Option<String>,
    pub snippet: String,
    pub rank: f64,
}

/// A full section for display — everything the panel/`SectionView` needs.
#[derive(Debug, Clone, Serialize)]
pub struct ManualSection {
    pub id: i64,
    pub file: String,
    pub level: u8,
    pub title: String,
    pub breadcrumb: Vec<String>,
    pub anchor: Option<String>,
    pub anchor_source: String,
    pub body_md: String,
}

/// Snippet match delimiters. `[`/`]` (the obvious choice, and the first cut) occur
/// **1905 / 1903** times in the 4 MB corpus (measured) — every `[link](…)` and MyST
/// role — so a highlighter splitting on them would paint ~1900 phantom matches. These
/// two Private-Use-Area codepoints occur **0** times in the corpus (measured), so a
/// split on them is exact. The frontend turns them into `<mark>`.
pub const SNIP_OPEN: &str = "\u{E000}";
pub const SNIP_CLOSE: &str = "\u{E001}";

/// Walk `dir` for `*.md.txt`, pushing `(file_id, contents)`. `file_id` is the path
/// relative to `base` with `.md.txt` stripped and forward slashes — the form an
/// `objects.inv` uri uses (`contents/essentialelements/RI`).
pub(crate) fn collect_leaves(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_leaves(&p, base, out);
        } else if p.to_string_lossy().ends_with(".md.txt") {
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let id = rel.strip_suffix(".md.txt").unwrap_or(&rel).to_string();
            if let Ok(text) = std::fs::read_to_string(&p) {
                out.push((id, text));
            }
        }
    }
}

/// Query text → an FTS5 MATCH of OR'd, quoted tokens (so `%`, `/`, `-` can't be read
/// as operators), minus a tiny stoplist and 1-char tokens. Empty query → empty MATCH
/// → empty result (the `output_search` contract). This is the ONE query builder,
/// shared by `search_manual` and the retrieval gate — so the gate predicts production.
pub(crate) fn to_fts_match(query: &str) -> String {
    const STOP: &[&str] = &[
        "how", "do", "for", "the", "to", "up", "of", "is", "in", "set", "what", "and",
        "with", "me", "my", "does", "can", "an",
    ];
    let toks: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2 && !STOP.contains(t))
        .map(|t| format!("\"{t}\""))
        .collect();
    toks.join(" OR ")
}

/// The verified anchor for a section, or `(None, "undetermined")`. Filled ONLY when
/// the section's closest label is in `objects.inv` with a matching file and slug —
/// the same cross-check as `verify_against_inventory`, per section.
fn resolve_anchor(section: &Section, inv: &HashMap<String, &InvEntry>) -> (Option<String>, &'static str) {
    if let Some(label) = section.labels.last() {
        if let Some(entry) = inv.get(&objects_inv::normalize_label(label)) {
            let (uri_file, frag) = uri_file_and_fragment(&entry.uri);
            let predicted = predict_anchor(label);
            if uri_file == section.file && frag.as_deref() == Some(predicted.as_str()) {
                return (Some(predicted), "objects_inv");
            }
        }
    }
    (None, "undetermined")
}

/// A deterministic, dependency-free content hash of the sectioned corpus (sorted by
/// file+body) — provenance for change detection, like `parser_version`. Not crypto.
fn corpus_hash(sections: &[Section]) -> String {
    use std::hash::{Hash, Hasher};
    let mut items: Vec<(&str, &str)> =
        sections.iter().map(|s| (s.file.as_str(), s.body.as_str())).collect();
    items.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (f, b) in items {
        f.hash(&mut h);
        b.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn manifest_field(manual_root: &Path, key: &str) -> Option<String> {
    let s = std::fs::read_to_string(manual_root.join("manifest.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get(key).and_then(|x| x.as_str()).map(String::from)
}

/// Ingest the corpus at `manual_root/<version>/` into the manual index tables,
/// replacing any prior rows for `version` (idempotent). All content-preserving
/// post-conditions run INSIDE the transaction, so a failed check rolls back rather
/// than persisting a silently-lossy index.
pub fn build_index(
    conn: &mut Connection,
    manual_root: &Path,
    version: &str,
) -> Result<IngestReport, AppError> {
    let version_dir = manual_root.join(version);
    if !version_dir.is_dir() {
        return Err(AppError::NotFound(format!(
            "manual corpus not found at {}",
            version_dir.display()
        )));
    }

    // 1. Sectionise every leaf (line conservation runs inside sectionize).
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    if leaves.is_empty() {
        return Err(AppError::NotFound(format!("no .md.txt under {}", version_dir.display())));
    }
    let mut sections: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        let secs = sections::sectionize(file, text)
            .map_err(|e| AppError::Internal(format!("sectionize {file}: {e}")))?;
        sections.extend(secs);
    }

    // 2. objects.inv → the anchor cross-check map (absent → all anchors undetermined).
    let inv_entries: Vec<InvEntry> = std::fs::read(version_dir.join("objects.inv"))
        .ok()
        .and_then(|b| objects_inv::parse(&b).ok())
        .unwrap_or_default();
    let inv: HashMap<String, &InvEntry> =
        inv_entries.iter().map(|e| (objects_inv::normalize_label(&e.name), e)).collect();

    // 3. Provenance + expected content totals.
    let expected_bytes: u64 = sections.iter().map(|s| s.body.len() as u64).sum();
    let hash = corpus_hash(&sections);
    let base_url = manifest_field(manual_root, "base_url");
    let collected_at = manifest_field(manual_root, "generated_at");

    // 4. Write + verify inside one transaction.
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM manual_sections WHERE orca_version = ?1", params![version])?;

    let mut verified = 0usize;
    {
        let mut ins = tx.prepare(
            "INSERT INTO manual_sections
                (orca_version, file, level, title, breadcrumb, labels, anchor,
                 anchor_source, body_md, line_start, line_end)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )?;
        for s in &sections {
            let (anchor, source) = resolve_anchor(s, &inv);
            if anchor.is_some() {
                verified += 1;
            }
            let breadcrumb = serde_json::to_string(&s.breadcrumb)
                .map_err(|e| AppError::Internal(format!("serialize breadcrumb: {e}")))?;
            let labels = serde_json::to_string(&s.labels)
                .map_err(|e| AppError::Internal(format!("serialize labels: {e}")))?;
            ins.execute(params![
                version, s.file, s.level as i64, s.title, breadcrumb, labels, anchor, source,
                s.body, s.line_start as i64, s.line_end as i64,
            ])?;
        }
    }
    // External-content FTS: rebuild the index from the base table.
    tx.execute_batch("INSERT INTO manual_fts(manual_fts) VALUES('rebuild');")?;

    tx.execute(
        "INSERT INTO manual_provenance
            (orca_version, base_url, corpus_collected_at, corpus_hash, sectioner_version,
             section_count, anchors_verified, indexed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7, datetime('now'))
         ON CONFLICT(orca_version) DO UPDATE SET
            base_url=?2, corpus_collected_at=?3, corpus_hash=?4, sectioner_version=?5,
            section_count=?6, anchors_verified=?7, indexed_at=datetime('now')",
        params![
            version, base_url, collected_at, hash, SECTIONER_VERSION,
            sections.len() as i64, verified as i64,
        ],
    )?;

    // --- Post-conditions IN OUR TERMS (rule #9), before commit ---
    // (a) row count == sections produced.
    let rows: i64 = tx.query_row(
        "SELECT COUNT(*) FROM manual_sections WHERE orca_version = ?1",
        params![version],
        |r| r.get(0),
    )?;
    if rows as usize != sections.len() {
        return Err(AppError::Internal(format!(
            "row count {rows} != {} sections", sections.len()
        )));
    }
    // (b)+(c) full read-back: every body_md returns byte-for-byte, in order. This
    // subsumes the byte-sum check and catches silent truncation.
    let mut stored_bytes: u64 = 0;
    {
        let mut q = tx.prepare(
            "SELECT body_md FROM manual_sections WHERE orca_version = ?1 ORDER BY id",
        )?;
        let bodies: Vec<String> = q
            .query_map(params![version], |r| r.get::<_, String>(0))?
            .filter_map(Result::ok)
            .collect();
        if bodies.len() != sections.len() {
            return Err(AppError::Internal("read-back row count changed".into()));
        }
        for (i, body) in bodies.iter().enumerate() {
            if body.as_bytes() != sections[i].body.as_bytes() {
                return Err(AppError::Internal(format!(
                    "body_md round-trip differs at section {i} ({})", sections[i].file
                )));
            }
            stored_bytes += body.len() as u64;
        }
    }
    if stored_bytes != expected_bytes {
        return Err(AppError::Internal(format!(
            "stored body bytes {stored_bytes} != sectioned {expected_bytes}"
        )));
    }
    // (d) FTS row count == table row count.
    let fts_rows: i64 = tx.query_row("SELECT COUNT(*) FROM manual_fts", [], |r| r.get(0))?;
    if fts_rows != rows {
        return Err(AppError::Internal(format!("FTS rows {fts_rows} != table rows {rows}")));
    }
    // (e) NULL-anchor count == section_count - verified.
    let nulls: i64 = tx.query_row(
        "SELECT COUNT(*) FROM manual_sections WHERE orca_version = ?1 AND anchor IS NULL",
        params![version],
        |r| r.get(0),
    )?;
    let expected_nulls = sections.len() - verified;
    if nulls as usize != expected_nulls {
        return Err(AppError::Internal(format!(
            "NULL-anchor count {nulls} != {expected_nulls} (section_count - verified)"
        )));
    }

    tx.commit()?;

    Ok(IngestReport {
        orca_version: version.to_string(),
        section_count: sections.len(),
        anchors_verified: verified,
        null_anchors: expected_nulls,
        body_bytes: stored_bytes,
        corpus_hash: hash,
    })
}

/// Full-text search over the manual index. Empty query → empty result (not an error).
/// `ORDER BY bm25 ASC`, title-weighted (10/5/1 over title/breadcrumb/body_md).
pub fn search_manual(conn: &Connection, query: &str, limit: usize) -> Result<Vec<ManualHit>, AppError> {
    let m = to_fts_match(query);
    if m.is_empty() {
        return Ok(Vec::new());
    }
    // Snippet markers are PUA codepoints (measured 0 in the corpus), not `[`/`]`
    // (measured 1905/1903) — so the frontend can split on them without false matches.
    let snip = format!("snippet(manual_fts, 2, '{SNIP_OPEN}', '{SNIP_CLOSE}', '…', 12)");
    let mut stmt = conn.prepare(&format!(
        "SELECT s.id, s.file, s.breadcrumb, s.title, s.anchor,
                {snip},
                bm25(manual_fts, 10.0, 5.0, 1.0)
         FROM manual_fts JOIN manual_sections s ON s.id = manual_fts.rowid
         WHERE manual_fts MATCH ?1
         ORDER BY bm25(manual_fts, 10.0, 5.0, 1.0)
         LIMIT ?2",
    ))?;
    let hits = stmt
        .query_map(params![m, limit as i64], |r| {
            let breadcrumb_json: String = r.get(2)?;
            Ok(ManualHit {
                id: r.get(0)?,
                file: r.get(1)?,
                breadcrumb: serde_json::from_str(&breadcrumb_json).unwrap_or_default(),
                title: r.get(3)?,
                anchor: r.get(4)?,
                snippet: r.get(5)?,
                rank: r.get(6)?,
            })
        })?
        .filter_map(Result::ok)
        .collect();
    Ok(hits)
}

/// Whether the manual index has rows, and the ingest tallies if so — so the panel can
/// show a "Build index" state instead of an empty result list (which reads as "nothing
/// found"). `None` = not built (no rows).
#[derive(Debug, Clone, Serialize)]
pub struct ManualStatus {
    pub orca_version: String,
    pub section_count: i64,
    pub anchors_verified: i64,
    pub null_anchors: i64,
}

/// The index status from `manual_provenance` — `None` when nothing is indexed yet.
pub fn index_status(conn: &Connection) -> Result<Option<ManualStatus>, AppError> {
    let row = conn.query_row(
        "SELECT orca_version, section_count, anchors_verified
         FROM manual_provenance ORDER BY indexed_at DESC LIMIT 1",
        [],
        |r| {
            let section_count: i64 = r.get(1)?;
            let anchors_verified: i64 = r.get(2)?;
            Ok(ManualStatus {
                orca_version: r.get(0)?,
                section_count,
                anchors_verified,
                null_anchors: section_count - anchors_verified,
            })
        },
    );
    match row {
        Ok(s) if s.section_count > 0 => Ok(Some(s)),
        Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AppError::from(e)),
    }
}

/// Resolve a `keywords.json` section descriptor `(file, breadcrumb, title, nth)` to a
/// `manual_sections` row id. `keywords.json` references sections by index into its OWN
/// `sections` array (a different space from the DB's synthetic id), so the hover needs
/// this bridge. `nth` is the ordinal among identical `(file, breadcrumb, title)` triples
/// in document order (`line_start`) — it exists because that triple collides once
/// (mreom). **Post-condition (rule #9): a descriptor resolves to EXACTLY one row.** 0
/// matches (the manual moved) or an out-of-range `nth` is a `NotFound` error, never a
/// pick-first — a wrong section shown confidently is the failure the whole layer guards.
pub fn resolve_descriptor(
    conn: &Connection,
    version: &str,
    file: &str,
    breadcrumb: &[String],
    title: &str,
    nth: usize,
) -> Result<i64, AppError> {
    let bc = serde_json::to_string(breadcrumb)
        .map_err(|e| AppError::Internal(format!("serialize breadcrumb: {e}")))?;
    let mut stmt = conn.prepare(
        "SELECT id FROM manual_sections
         WHERE orca_version = ?1 AND file = ?2 AND breadcrumb = ?3 AND title = ?4
         ORDER BY line_start",
    )?;
    let ids: Vec<i64> = stmt
        .query_map(params![version, file, bc, title], |r| r.get::<_, i64>(0))?
        .filter_map(Result::ok)
        .collect();
    ids.get(nth).copied().ok_or_else(|| {
        AppError::NotFound(format!(
            "manual descriptor {file} :: {} > {title} [nth {nth}] resolved to {} row(s)",
            breadcrumb.join(" > "),
            ids.len()
        ))
    })
}

/// A cross-reference target — the section a `[..](sec:…)` / `{ref}`…`` label points to.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnchorTarget {
    pub file: String,
    pub section_id: i64,
}

/// Resolve manual cross-reference LABELS to sections through the `anchor` column. The
/// slugify rule is the sectioner's `predict_anchor` (rule #9 — the SAME transform that
/// built the stored anchor, reused, never a second normalization). A label whose slug
/// matches no anchor → `None` (UNDETERMINED: the target is one of the ~518 NULL-anchor
/// sections, or a genuinely unregistered label) — the frontend then keeps the link as
/// verbatim text, not a dead click. Batch (one call per page), order-preserving.
pub fn resolve_anchors(
    conn: &Connection,
    labels: &[String],
) -> Result<Vec<Option<AnchorTarget>>, AppError> {
    let version = index_status(conn)?
        .ok_or_else(|| AppError::NotFound("manual index not built".into()))?
        .orca_version;
    let mut stmt = conn.prepare(
        "SELECT file, id FROM manual_sections
         WHERE orca_version = ?1 AND anchor = ?2
         ORDER BY line_start LIMIT 1",
    )?;
    let mut out = Vec::with_capacity(labels.len());
    for label in labels {
        let slug = predict_anchor(label);
        if slug.is_empty() {
            out.push(None);
            continue;
        }
        match stmt.query_row(params![version, slug], |r| {
            Ok(AnchorTarget { file: r.get(0)?, section_id: r.get(1)? })
        }) {
            Ok(t) => out.push(Some(t)),
            Err(rusqlite::Error::QueryReturnedNoRows) => out.push(None),
            Err(e) => return Err(AppError::from(e)),
        }
    }
    Ok(out)
}

/// One section's line-bounds within its file — enough for the frontend to scroll to and
/// highlight it inside the full page. The body itself is NOT here: the page carries the
/// whole file text, and a section is a line range into it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageSection {
    pub id: i64,
    pub level: u8,
    pub title: String,
    pub anchor: Option<String>,
    pub line_start: usize,
    pub line_end: usize,
}

/// A full manual page: the file's complete text (read from disk) plus the line-bounds of
/// every section indexed from it, in line order. The display unit — a section indexes,
/// a page shows.
#[derive(Debug, Clone, Serialize)]
pub struct ManualPage {
    pub file: String,
    pub orca_version: String,
    pub text: String,
    pub sections: Vec<PageSection>,
}

/// Read the full text of `file` off disk and pair it with the line-bounds of every
/// section the index holds for it. **Source of truth is the FILE ON DISK** — not the
/// stored `body_md` — because the preamble (lines before the first heading) is verified
/// by the sectioner's coverage check but never stored, and the heading lines cannot be
/// reproduced byte-for-byte from `title`+`level`.
///
/// **Post-condition (rule #9, in our terms):** the file read now must match what was
/// indexed. Checked the cheapest sufficient way — the file's line count equals
/// `max(line_end)+1` over its sections, and each section's `line_start` line begins with
/// exactly `level` `#` and contains its `title`. A mismatch (the corpus drifted on disk
/// while the index is stale — a refresh, a partial reload) is an **explicit error**, not
/// a silent page whose scroll target lands on the wrong section.
pub fn get_page(conn: &Connection, manual_root: &Path, file: &str) -> Result<ManualPage, AppError> {
    // Serve the most-recently-built version's index (the one `manual_index_status` reports).
    let version = index_status(conn)?
        .ok_or_else(|| AppError::NotFound("manual index not built".into()))?
        .orca_version;

    let mut stmt = conn.prepare(
        "SELECT id, level, title, anchor, line_start, line_end
         FROM manual_sections
         WHERE orca_version = ?1 AND file = ?2
         ORDER BY line_start",
    )?;
    let sections: Vec<PageSection> = stmt
        .query_map(params![version, file], |r| {
            Ok(PageSection {
                id: r.get(0)?,
                level: r.get::<_, i64>(1)? as u8,
                title: r.get(2)?,
                anchor: r.get(3)?,
                line_start: r.get::<_, i64>(4)? as usize,
                line_end: r.get::<_, i64>(5)? as usize,
            })
        })?
        .filter_map(Result::ok)
        .collect();
    if sections.is_empty() {
        return Err(AppError::NotFound(format!(
            "no indexed sections for manual file {file}"
        )));
    }

    let path = manual_root.join(&version).join(format!("{file}.md.txt"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| AppError::NotFound(format!("manual page {} not readable: {e}", path.display())))?;

    verify_page_matches_index(file, &text, &sections)?;

    Ok(ManualPage {
        file: file.to_string(),
        orca_version: version,
        text,
        sections,
    })
}

/// The get_page post-condition, factored out so it is testable without the DB or the
/// filesystem. Splits `text` the SAME way the sectioner did (`str::lines`), so line
/// indices align. Any drift → an `Internal` error naming the file and the mismatch.
fn verify_page_matches_index(
    file: &str,
    text: &str,
    sections: &[PageSection],
) -> Result<(), AppError> {
    let lines: Vec<&str> = text.lines().collect();
    // (1) the file's line count == max(line_end)+1 over its sections. Because the last
    // section's line_end is the file's last line index (the sectioner's tiling), this
    // catches truncation or growth on disk.
    let max_end = sections.iter().map(|s| s.line_end).max().unwrap_or(0);
    if lines.len() != max_end + 1 {
        return Err(AppError::Internal(format!(
            "page on disk does not match the index for {file}: {} lines on disk, index \
             expects {} (max line_end + 1); rebuild the index",
            lines.len(),
            max_end + 1
        )));
    }
    // (2) each section's heading line is where and what the index says it is: exactly
    // `level` leading `#`, and it contains the stored `title`.
    for s in sections {
        let line = lines.get(s.line_start).ok_or_else(|| {
            AppError::Internal(format!(
                "page on disk does not match the index for {file}: section {} line_start {} \
                 out of range ({} lines); rebuild the index",
                s.id,
                s.line_start,
                lines.len()
            ))
        })?;
        let hashes = line.chars().take_while(|&c| c == '#').count();
        if hashes != s.level as usize || !line.contains(&s.title) {
            return Err(AppError::Internal(format!(
                "page on disk does not match the index for {file}: line {} is {line:?}, \
                 expected a level-{} heading containing {:?}; rebuild the index",
                s.line_start, s.level, s.title
            )));
        }
    }
    Ok(())
}

/// Fetch one section in full for display. A missing id is a `NotFound` error, never an
/// empty section (the caller must be able to tell "no such section" from "empty body").
pub fn get_section(conn: &Connection, id: i64) -> Result<ManualSection, AppError> {
    conn.query_row(
        "SELECT id, file, level, title, breadcrumb, anchor, anchor_source, body_md
         FROM manual_sections WHERE id = ?1",
        params![id],
        |r| {
            let breadcrumb_json: String = r.get(4)?;
            Ok(ManualSection {
                id: r.get(0)?,
                file: r.get(1)?,
                level: r.get::<_, i64>(2)? as u8,
                title: r.get(3)?,
                breadcrumb: serde_json::from_str(&breadcrumb_json).unwrap_or_default(),
                anchor: r.get(5)?,
                anchor_source: r.get(6)?,
                body_md: r.get(7)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => AppError::NotFound(format!("manual section {id}")),
        other => AppError::from(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::create_manual_tables(&conn).unwrap();
        conn
    }

    fn section(file: &str, title: &str, labels: &[&str], body: &str) -> Section {
        Section {
            file: file.into(),
            level: 2,
            title: title.into(),
            breadcrumb: vec!["Root".into()],
            labels: labels.iter().map(|s| s.to_string()).collect(),
            anchor: None,
            body: body.into(),
            line_start: 0,
            line_end: 2,
        }
    }

    /// Insert sections directly (bypassing the corpus) to exercise search + the
    /// external-content FTS round-trip without files.
    fn insert(conn: &mut Connection, secs: &[Section]) {
        let tx = conn.transaction().unwrap();
        {
            let mut ins = tx.prepare(
                "INSERT INTO manual_sections
                    (orca_version, file, level, title, breadcrumb, labels, anchor,
                     anchor_source, body_md, line_start, line_end)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            ).unwrap();
            for s in secs {
                ins.execute(params![
                    "6.1", s.file, s.level as i64, s.title,
                    serde_json::to_string(&s.breadcrumb).unwrap(),
                    serde_json::to_string(&s.labels).unwrap(),
                    Option::<String>::None, "undetermined", s.body,
                    s.line_start as i64, s.line_end as i64,
                ]).unwrap();
            }
        }
        tx.execute_batch("INSERT INTO manual_fts(manual_fts) VALUES('rebuild');").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn empty_query_is_empty_not_error() {
        let conn = mem_db();
        assert!(search_manual(&conn, "", 5).unwrap().is_empty());
        assert!(search_manual(&conn, "   ", 5).unwrap().is_empty());
    }

    #[test]
    fn search_ranks_and_snippets() {
        let mut conn = mem_db();
        insert(&mut conn, &[
            section("contents/essentialelements/RI", "RIJCOSX", &[],
                    "The RIJCOSX approximation accelerates hybrid DFT."),
            section("contents/essentialelements/solvationmodels", "CPCM", &[],
                    "The conductor-like polarizable continuum model for implicit water solvation."),
        ]);
        let hits = search_manual(&conn, "RIJCOSX", 5).unwrap();
        assert_eq!(hits.first().map(|h| h.title.as_str()), Some("RIJCOSX"));
        assert!(hits[0].rank < 0.0, "bm25 is negative: {}", hits[0].rank);
        assert!(hits[0].snippet.to_lowercase().contains("rijcosx"));
        // The match is wrapped in PUA markers, not `[`/`]` (which occur in the corpus).
        assert!(hits[0].snippet.contains(SNIP_OPEN) && hits[0].snippet.contains(SNIP_CLOSE));
        assert!(!hits[0].snippet.contains('['));

        let water = search_manual(&conn, "CPCM water", 5).unwrap();
        assert_eq!(water.first().map(|h| h.title.as_str()), Some("CPCM"));
        assert!(water[0].anchor.is_none()); // undetermined here
    }

    #[test]
    fn get_section_returns_full_body_or_notfound() {
        let mut conn = mem_db();
        insert(&mut conn, &[section(
            "contents/essentialelements/RI", "RIJCOSX", &[],
            "The RIJCOSX approximation accelerates hybrid DFT.\n\n```orca\n! RIJCOSX\n```",
        )]);
        let id = search_manual(&conn, "RIJCOSX", 1).unwrap()[0].id;
        let sec = get_section(&conn, id).unwrap();
        assert_eq!(sec.title, "RIJCOSX");
        assert_eq!(sec.breadcrumb, vec!["Root".to_string()]);
        assert!(sec.body_md.contains("```orca")); // full body, not a snippet
        assert_eq!(sec.anchor_source, "undetermined");
        // A missing id is an error, not an empty section.
        assert!(matches!(get_section(&conn, 999_999), Err(AppError::NotFound(_))));
    }

    #[test]
    fn to_fts_match_ors_tokens_and_drops_noise() {
        assert_eq!(to_fts_match("RIJCOSX"), "\"rijcosx\"");
        assert_eq!(to_fts_match("how do I set up CPCM for water"), "\"cpcm\" OR \"water\"");
        assert_eq!(to_fts_match(""), "");
    }

    // ── Page display (a section indexes, a page shows) ──────────────────────────

    /// A small corpus file whose sectioner output we know: a preamble, then three
    /// headings at levels 1/2/3. Line indices below are 0-based into `.lines()`.
    const PAGE: &str = "\
# Top

preamble prose

## Alpha

alpha body

### Alpha child

child body

## Beta

beta body";

    fn page_sections() -> Vec<PageSection> {
        // Derived by hand from PAGE, matching what `sectionize` produces:
        //  0:# Top  1:  2:preamble  3:  4:## Alpha  5:  6:alpha body  7:
        //  8:### Alpha child  9:  10:child body  11:  12:## Beta  13:  14:beta body
        vec![
            PageSection { id: 1, level: 1, title: "Top".into(), anchor: None, line_start: 0, line_end: 3 },
            PageSection { id: 2, level: 2, title: "Alpha".into(), anchor: None, line_start: 4, line_end: 7 },
            PageSection { id: 3, level: 3, title: "Alpha child".into(), anchor: None, line_start: 8, line_end: 11 },
            PageSection { id: 4, level: 2, title: "Beta".into(), anchor: None, line_start: 12, line_end: 14 },
        ]
    }

    #[test]
    fn page_post_condition_accepts_a_matching_file() {
        // Independent confidence the hand-derived bounds are right: sectionize agrees.
        let secs = sections::sectionize("page", PAGE).unwrap();
        let derived: Vec<PageSection> = secs
            .iter()
            .enumerate()
            .map(|(i, s)| PageSection {
                id: i as i64 + 1,
                level: s.level,
                title: s.title.clone(),
                anchor: None,
                line_start: s.line_start,
                line_end: s.line_end,
            })
            .collect();
        assert_eq!(derived, page_sections());
        verify_page_matches_index("page", PAGE, &page_sections()).unwrap();
    }

    #[test]
    fn page_post_condition_catches_a_line_count_drift() {
        // The corpus grew on disk (an extra trailing line) but the index is stale.
        let grown = format!("{PAGE}\nan extra line the index never saw");
        let err = verify_page_matches_index("page", &grown, &page_sections()).unwrap_err();
        assert!(matches!(err, AppError::Internal(_)));
        assert!(err.to_string().contains("does not match the index"));
    }

    #[test]
    fn page_post_condition_catches_a_shifted_heading() {
        // Same line count, but a heading moved/changed under a stale index: line 4 is no
        // longer "## Alpha". A silent page would scroll the wrong place — this must fail.
        let shifted = PAGE.replace("## Alpha\n", "## Gamma\n");
        let err = verify_page_matches_index("page", &shifted, &page_sections()).unwrap_err();
        assert!(matches!(err, AppError::Internal(_)));
        assert!(err.to_string().contains("does not match the index"));
    }

    #[test]
    fn get_page_round_trips_a_temp_corpus() {
        use std::io::Write;
        // A throwaway corpus on disk: <tmp>/6.1/contents/demo.md.txt.
        let tmp = std::env::temp_dir().join(format!("orca_page_test_{}", std::process::id()));
        let vdir = tmp.join("6.1").join("contents");
        std::fs::create_dir_all(&vdir).unwrap();
        let mut f = std::fs::File::create(vdir.join("demo.md.txt")).unwrap();
        f.write_all(PAGE.as_bytes()).unwrap();
        drop(f);

        let mut conn = mem_db();
        // Full ingest path writes provenance, so index_status/get_page can resolve.
        build_index(&mut conn, &tmp, "6.1").unwrap();

        let page = get_page(&conn, &tmp, "contents/demo").unwrap();
        assert_eq!(page.orca_version, "6.1");
        assert_eq!(page.text, PAGE); // the FILE on disk, byte-for-byte
        let titles: Vec<&str> = page.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Top", "Alpha", "Alpha child", "Beta"]);
        // Sections are in line order and the last one's line_end is the file's last line.
        assert_eq!(page.sections.last().unwrap().line_end, PAGE.lines().count() - 1);

        // A file the index has no sections for is NotFound, not an empty page.
        assert!(matches!(get_page(&conn, &tmp, "contents/nope"), Err(AppError::NotFound(_))));

        std::fs::remove_dir_all(&tmp).ok();
    }
}
