//! Pure search / status filtering over the job list (Phase 4.7.4, closes Phase 4.7).
//!
//! Frontend-only, composed **after** the 4.7.3 group filter: the rendered rows are
//! always `filterJobsBySearch(filterJobsByGroup(jobs, selection, groups), query,
//! statuses)`. Search narrows WITHIN the selected group's subtree — it never
//! re-widens to all jobs. Both are pure functions over `Job[]`, so they compose and
//! are unit-testable in isolation. Plain case-insensitive **substring** match (the
//! roadmap's "LIKE / column filter" intent) — not FTS5, not fuzzy/tokenized.

import type { Job, JobStatus } from "../types";

/**
 * The searchable "method" token of a job: the content of its FIRST `!` line (the
 * ORCA keyword line), with the `!` stripped and trimmed — e.g.
 * `"! r2SCAN-3c Opt Freq"` → `"r2SCAN-3c Opt Freq"`. No `!` line → `""`.
 *
 * Deliberately dumb: a display/search token, NOT a semantic parse. Whatever the `!`
 * line says is the method string (an xtb/GOAT input's first keyword is fine too).
 */
export function parseMethodLine(input: string): string {
  for (const line of input.split("\n")) {
    const trimmed = line.trim();
    if (trimmed.startsWith("!")) {
      return trimmed.slice(1).trim();
    }
  }
  return "";
}

/**
 * Filter jobs by a free-text query AND a status set. Both are ANDed:
 * - **query** matches iff it is empty, OR `title` contains it, OR the parsed method
 *   line contains it (both compared case-insensitively).
 * - **status** matches iff `statuses` is empty (no status filter = all), OR
 *   `statuses` contains the job's status.
 *
 * Empty query + empty statuses ⇒ **identity** (returns the input list, drops nothing).
 */
export function filterJobsBySearch(
  jobs: Job[],
  query: string,
  statuses: Set<JobStatus>,
): Job[] {
  const q = query.trim().toLowerCase();
  if (q === "" && statuses.size === 0) return jobs; // identity fast-path

  return jobs.filter((job) => {
    const statusOk = statuses.size === 0 || statuses.has(job.status);
    if (!statusOk) return false;
    if (q === "") return true;
    const title = job.title.toLowerCase();
    const method = parseMethodLine(job.input_content).toLowerCase();
    return title.includes(q) || method.includes(q);
  });
}
