import { describe, it, expect } from "vitest";
import type { Group, Job, JobStatus } from "../types";
import { parseMethodLine, filterJobsBySearch } from "./search";
import { filterJobsByGroup } from "./tree";

/** Minimal Job carrying the fields the search/group filters read. */
function j(
  id: string,
  title: string,
  input_content: string,
  status: JobStatus = "draft",
  group_id: string | null = null,
): Job {
  return {
    id,
    title,
    input_content,
    status,
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: null,
    created_at: "2026-01-01",
    started_at: null,
    completed_at: null,
    scene_json: null,
    scene_log_json: null,
    pathway_id: null,
    group_id,
  };
}

function g(id: string, name: string, parent_id: string | null): Group {
  return { id, name, parent_id, created_at: "2026-01-01" };
}

describe("parseMethodLine — the `!` keyword line as a search token", () => {
  it("returns the first `!` line content, `!` stripped and trimmed", () => {
    expect(parseMethodLine("! r2SCAN-3c Opt Freq\n* xyz 0 1\n")).toBe("r2SCAN-3c Opt Freq");
  });

  it("skips leading blank / comment lines before the `!` line", () => {
    expect(parseMethodLine("\n# a comment\n   ! B3LYP def2-SVP\n")).toBe("B3LYP def2-SVP");
  });

  it("returns '' when there is no `!` line", () => {
    expect(parseMethodLine("* xyz 0 1\nC 0 0 0\n*\n")).toBe("");
  });

  it("finds the `!` line even when it is not line 0", () => {
    expect(parseMethodLine("%pal nprocs 4 end\n! XTB GOAT\n")).toBe("XTB GOAT");
  });
});

describe("filterJobsBySearch — query AND status", () => {
  const jobs = [
    j("a", "water opt", "! r2SCAN-3c Opt", "completed"),
    j("b", "benzene freq", "! B3LYP def2-TZVP Freq", "failed"),
    j("c", "TS search", "! XTB GOAT", "running"),
  ];
  const none = new Set<JobStatus>();

  it("empty query + empty status = identity", () => {
    expect(filterJobsBySearch(jobs, "", none)).toBe(jobs); // same reference (fast-path)
    expect(filterJobsBySearch(jobs, "   ", none).map((x) => x.id)).toEqual(["a", "b", "c"]);
  });

  it("matches a title substring, case-insensitively", () => {
    expect(filterJobsBySearch(jobs, "WATER", none).map((x) => x.id)).toEqual(["a"]);
  });

  it("matches via the method line (input_content), not just the title", () => {
    // "r2scan" is in job a's `!` line, not its title.
    expect(filterJobsBySearch(jobs, "r2scan", none).map((x) => x.id)).toEqual(["a"]);
    // "b3lyp" only appears in job b's method.
    expect(filterJobsBySearch(jobs, "b3lyp", none).map((x) => x.id)).toEqual(["b"]);
  });

  it("filters by status alone (query empty)", () => {
    expect(
      filterJobsBySearch(jobs, "", new Set<JobStatus>(["failed", "running"])).map((x) => x.id),
    ).toEqual(["b", "c"]);
  });

  it("ANDs query and status together", () => {
    // "freq" matches b's title+method; status 'failed' keeps it.
    expect(
      filterJobsBySearch(jobs, "freq", new Set<JobStatus>(["failed"])).map((x) => x.id),
    ).toEqual(["b"]);
  });

  it("returns nothing when the title matches but the status excludes it (the AND)", () => {
    // "water" matches job a (completed), but the status filter only allows 'failed'.
    expect(filterJobsBySearch(jobs, "water", new Set<JobStatus>(["failed"]))).toEqual([]);
  });
});

describe("composition — search narrows WITHIN the group subtree (the main risk)", () => {
  // Group R with child S; job a1 in R, a2 in S, and b in an unrelated group O.
  const groups = [g("R", "root", null), g("S", "sub", "R"), g("O", "other", null)];
  const jobs = [
    j("a1", "alpha one", "! HF", "completed", "R"),
    j("a2", "alpha two", "! HF", "completed", "S"),
    j("b", "alpha three", "! HF", "completed", "O"), // matches query but is OUTSIDE R
  ];

  it("a query that also matches a job outside the group never re-widens the result", () => {
    const withinGroup = filterJobsByGroup(jobs, { kind: "group", id: "R" }, groups);
    const composed = filterJobsBySearch(withinGroup, "alpha", new Set<JobStatus>());
    // "alpha" matches all three titles, but only R + descendant S survive the group filter.
    expect(composed.map((x) => x.id).sort()).toEqual(["a1", "a2"]);
    expect(composed.some((x) => x.id === "b")).toBe(false);
  });
});
