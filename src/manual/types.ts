//! Types + constants shared by the manual panel (mirrors `src-tauri/src/manual/index.rs`).

/** One search hit. `anchor` is null when UNDETERMINED (link lands on the page, no fragment). */
export interface ManualHit {
  id: number;
  file: string;
  breadcrumb: string[];
  title: string;
  anchor: string | null;
  snippet: string;
  rank: number;
}

/** A full section for display — everything `SectionView` needs. */
export interface ManualSection {
  id: number;
  file: string;
  level: number;
  title: string;
  breadcrumb: string[];
  anchor: string | null;
  anchor_source: string;
  body_md: string;
}

/** Result of an author-run `build_manual_index`. */
export interface IngestReport {
  orca_version: string;
  section_count: number;
  anchors_verified: number;
  null_anchors: number;
  body_bytes: number;
  corpus_hash: string;
}

/** `manual_index_status` — null when the index has no rows. */
export interface ManualStatus {
  orca_version: string;
  section_count: number;
  anchors_verified: number;
  null_anchors: number;
}

/**
 * Snippet match delimiters — Private-Use-Area codepoints, chosen because they occur
 * **0** times in the 4 MB corpus (measured; `[`/`]` occur 1905/1903, so splitting on
 * those would paint phantom highlights). Must match `SNIP_OPEN`/`SNIP_CLOSE` in Rust.
 */
export const SNIP_OPEN = "\uE000";
export const SNIP_CLOSE = "\uE001";
