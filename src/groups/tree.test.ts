import { describe, it, expect } from "vitest";
import type { Group, Job } from "../types";
import {
  buildGroupTree,
  descendantGroupIds,
  filterJobsByGroup,
  moveTargetsFor,
} from "./tree";

/** Minimal Group with sane defaults; created_at kept distinct so ordering is testable. */
function g(id: string, name: string, parent_id: string | null, created_at = "2026-01-01"): Group {
  return { id, name, parent_id, created_at };
}

/** Minimal Job carrying only what the filter reads. */
function j(id: string, group_id: string | null): Job {
  return {
    id,
    title: `job ${id}`,
    input_content: "",
    status: "draft",
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

describe("buildGroupTree — flat adjacency list → nested tree", () => {
  it("nests children under parents", () => {
    const tree = buildGroupTree([g("r", "root", null), g("s", "sub", "r"), g("t", "leaf", "s")]);
    expect(tree.length).toBe(1);
    expect(tree[0].group.id).toBe("r");
    expect(tree[0].children.length).toBe(1);
    expect(tree[0].children[0].group.id).toBe("s");
    expect(tree[0].children[0].children[0].group.id).toBe("t");
  });

  it("sorts siblings by name (deterministic), regardless of input order", () => {
    const tree = buildGroupTree([g("b", "Bravo", null), g("a", "Alpha", null), g("c", "Charlie", null)]);
    expect(tree.map((n) => n.group.name)).toEqual(["Alpha", "Bravo", "Charlie"]);
  });

  it("treats an orphan (parent_id points at a missing group) as a ROOT — nothing vanishes", () => {
    const tree = buildGroupTree([g("x", "orphan", "ghost"), g("r", "root", null)]);
    const ids = tree.map((n) => n.group.id).sort();
    expect(ids).toEqual(["r", "x"]);
    // every input group is represented somewhere
    expect(tree.length).toBe(2);
  });
});

describe("descendantGroupIds — deep subtree set", () => {
  const groups = [g("r", "root", null), g("s", "sub", "r"), g("t", "leaf", "s"), g("o", "other", null)];

  it("includes the root itself and all transitive descendants", () => {
    expect([...descendantGroupIds(groups, "r")].sort()).toEqual(["r", "s", "t"]);
    expect([...descendantGroupIds(groups, "s")].sort()).toEqual(["s", "t"]);
    expect([...descendantGroupIds(groups, "t")].sort()).toEqual(["t"]);
  });

  it("excludes unrelated groups", () => {
    expect(descendantGroupIds(groups, "r").has("o")).toBe(false);
  });

  it("is cycle-safe (terminates on corrupt cyclic data)", () => {
    // a↔b cycle plus a self-loop — must not hang, and returns exactly the reachable set.
    const cyclic = [g("a", "a", "b"), g("b", "b", "a"), g("c", "c", "c")];
    expect([...descendantGroupIds(cyclic, "a")].sort()).toEqual(["a", "b"]);
    expect([...descendantGroupIds(cyclic, "c")].sort()).toEqual(["c"]);
  });
});

describe("filterJobsByGroup — the three modes", () => {
  const groups = [g("r", "root", null), g("s", "sub", "r"), g("o", "other", null)];
  const jobs = [j("j1", "r"), j("j2", "s"), j("j3", "o"), j("j4", null)];

  it("'all' returns every job", () => {
    expect(filterJobsByGroup(jobs, { kind: "all" }, groups).map((x) => x.id)).toEqual([
      "j1",
      "j2",
      "j3",
      "j4",
    ]);
  });

  it("'ungrouped' returns only group_id === null", () => {
    expect(filterJobsByGroup(jobs, { kind: "ungrouped" }, groups).map((x) => x.id)).toEqual(["j4"]);
  });

  it("'group' is DEEP — jobs in the group OR any descendant", () => {
    // selecting r shows j1 (in r) AND j2 (in the descendant s), but NOT j3/j4.
    expect(filterJobsByGroup(jobs, { kind: "group", id: "r" }, groups).map((x) => x.id)).toEqual([
      "j1",
      "j2",
    ]);
    // selecting the leaf s shows only j2.
    expect(filterJobsByGroup(jobs, { kind: "group", id: "s" }, groups).map((x) => x.id)).toEqual([
      "j2",
    ]);
  });
});

describe("moveTargetsFor — excludes self + descendants (no cycle offered)", () => {
  const groups = [g("r", "root", null), g("s", "sub", "r"), g("t", "leaf", "s"), g("o", "other", null)];

  it("moving 'r' offers only 'o' (not r, s, or t)", () => {
    expect(moveTargetsFor(groups, "r").map((x) => x.id)).toEqual(["o"]);
  });

  it("moving 's' offers r and o (not s or its descendant t)", () => {
    expect(moveTargetsFor(groups, "s").map((x) => x.id).sort()).toEqual(["o", "r"]);
  });

  it("moving a leaf offers every other group", () => {
    expect(moveTargetsFor(groups, "t").map((x) => x.id).sort()).toEqual(["o", "r", "s"]);
  });
});
