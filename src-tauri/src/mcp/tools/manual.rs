//! `search_manual` (ADR-027 decision 7; M2 amendment, item 7): the manual's full-text search with a
//! capped limit and capped fields. Each hit keeps its section id, a source for ADR-014 (1a)
//! retrieval.
//!
//! The index's snippet marks a match with two private-use code points (`manual::index::SNIP_OPEN`
//! and `SNIP_CLOSE`), chosen for the app's panel. Here they become `«` and `»`, which a model reads.

use rusqlite::Connection;
use serde::Serialize;

use super::{clip, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::manual::index::{search_manual as index_search, SNIP_CLOSE, SNIP_OPEN};

pub const MANUAL_DEFAULT_HITS: usize = 10;
pub const MANUAL_MAX_HITS: usize = 20;
pub const MANUAL_QUERY_MAX_CHARS: usize = 500;
pub const SNIPPET_MAX_BYTES: usize = 400;
/// The title, the file and the anchor are each cut to this many serialised bytes.
pub const FIELD_MAX_BYTES: usize = 160;
/// At most this many breadcrumb entries, each cut to [`CRUMB_MAX_BYTES`].
pub const CRUMBS_MAX: usize = 6;
pub const CRUMB_MAX_BYTES: usize = 80;

/// One hit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManualHitView {
    /// The `manual_sections` row id: the section to cite or open.
    pub section_id: i64,
    pub file: String,
    pub anchor: Option<String>,
    pub title: String,
    pub breadcrumb: Vec<String>,
    /// The breadcrumb had more than [`CRUMBS_MAX`] entries; the first ones are shown.
    pub breadcrumb_truncated: bool,
    /// About 12 words around the match, the match between `«` and `»`.
    pub snippet: String,
    /// SQLite FTS5 bm25, title-weighted: lower (more negative) is a better match. Dimensionless.
    pub bm25_score: f64,
}

/// `search_manual`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManualResults {
    pub query: String,
    pub limit: usize,
    pub hits: Vec<ManualHitView>,
    /// The page is full, so more hits may exist; ask a narrower query.
    pub more_may_exist: bool,
}

fn view_of(h: crate::manual::index::ManualHit) -> ManualHitView {
    let snippet = h.snippet.replace(SNIP_OPEN, "«").replace(SNIP_CLOSE, "»");
    ManualHitView {
        section_id: h.id,
        file: clip(&h.file, FIELD_MAX_BYTES).0,
        anchor: h.anchor.map(|a| clip(&a, FIELD_MAX_BYTES).0),
        title: clip(&h.title, FIELD_MAX_BYTES).0,
        breadcrumb: h.breadcrumb.iter().take(CRUMBS_MAX).map(|c| clip(c, CRUMB_MAX_BYTES).0).collect(),
        breadcrumb_truncated: h.breadcrumb.len() > CRUMBS_MAX,
        snippet: clip(&snippet, SNIPPET_MAX_BYTES).0,
        bm25_score: h.rank,
    }
}

/// `search_manual`: at most `limit` hits for `query` (default [`MANUAL_DEFAULT_HITS`], at most
/// [`MANUAL_MAX_HITS`]). An empty query gives no hits, as the panel's search does.
pub fn search_manual(conn: &Connection, query: &str, limit: Option<usize>) -> Result<ManualResults, ToolError> {
    if query.chars().count() > MANUAL_QUERY_MAX_CHARS {
        return Err(ToolError::refused(format!("the query is longer than {MANUAL_QUERY_MAX_CHARS} characters")));
    }
    let limit = limit.unwrap_or(MANUAL_DEFAULT_HITS).clamp(1, MANUAL_MAX_HITS);
    let hits: Vec<ManualHitView> = index_search(conn, query, limit).map_err(ToolError::App)?.into_iter().map(view_of).collect();
    let results = ManualResults { query: clip(query, 200).0, limit, more_may_exist: hits.len() == limit, hits };
    within_cap(results, RESULT_CAP_BYTES)
}
