//! Pure tree + filter logic for job groups (Phase 4.7.3, ADR-019).
//!
//! No React, no `invoke` — just data → data, so it is unit-testable in isolation.
//! The group tree is an **adjacency list** (`Group.parent_id`); this module turns it
//! into a nested render tree and derives the two subtree-dependent behaviours that MUST
//! agree: the **deep jobs filter** (a selected group shows jobs in it OR any descendant)
//! and the **Move-to exclusion** (a group may never be moved under itself or a
//! descendant). Both derive from the same `descendantGroupIds` — one subtree walk, reused.

import type { Group, Job } from "../types";

/** One node of the nested render tree. */
export interface GroupNode {
  group: Group;
  children: GroupNode[];
}

/** The active selection driving the Jobs view. A single source of truth (lifted to
 * `App`) feeding the deep filter, assign-on-create, and the Move-to target list. */
export type GroupSelection =
  | { kind: "all" }
  | { kind: "ungrouped" }
  | { kind: "group"; id: string };

/** Deterministic sort: by name (locale-aware), then created_at, then id (a total
 * order so the render is stable regardless of input order). */
function compareGroups(a: Group, b: Group): number {
  return (
    a.name.localeCompare(b.name) ||
    a.created_at.localeCompare(b.created_at) ||
    a.id.localeCompare(b.id)
  );
}

/**
 * Flat `Group[]` (adjacency list) → nested `GroupNode[]` (roots first, each with its
 * children). **Orphan defense:** a group whose `parent_id` points at a non-existent
 * group is treated as a ROOT, so nothing vanishes from the tree on bad data. Children
 * at every level are sorted by {@link compareGroups}.
 */
export function buildGroupTree(groups: Group[]): GroupNode[] {
  const byId = new Map<string, Group>(groups.map((g) => [g.id, g]));
  const nodes = new Map<string, GroupNode>(
    groups.map((g) => [g.id, { group: g, children: [] }]),
  );
  const roots: GroupNode[] = [];

  for (const g of groups) {
    const node = nodes.get(g.id)!;
    // A group is a root if it has no parent OR its parent_id dangles (orphan defense).
    if (g.parent_id !== null && byId.has(g.parent_id)) {
      nodes.get(g.parent_id)!.children.push(node);
    } else {
      roots.push(node);
    }
  }

  const sortRec = (list: GroupNode[]): GroupNode[] => {
    list.sort((a, b) => compareGroups(a.group, b.group));
    for (const n of list) sortRec(n.children);
    return list;
  };
  return sortRec(roots);
}

/**
 * `rootId` plus every group beneath it (transitively). **Cycle-safe**: a `visited` set
 * guarantees termination even if the data contains a cycle (the backend prevents them,
 * but the UI must never hang). The returned set always contains `rootId` itself.
 */
export function descendantGroupIds(groups: Group[], rootId: string): Set<string> {
  // Index children by parent for an O(n) walk.
  const childrenOf = new Map<string, string[]>();
  for (const g of groups) {
    if (g.parent_id !== null) {
      const list = childrenOf.get(g.parent_id) ?? [];
      list.push(g.id);
      childrenOf.set(g.parent_id, list);
    }
  }
  const out = new Set<string>();
  const stack = [rootId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (out.has(id)) continue; // cycle-safe
    out.add(id);
    for (const child of childrenOf.get(id) ?? []) stack.push(child);
  }
  return out;
}

/**
 * The jobs a selection shows. `"all"` → every job; `"ungrouped"` → `group_id == null`;
 * `"group"` → jobs whose `group_id` is in the **deep** subtree set of the selected group
 * (the group OR any descendant). Job order is preserved (the caller sorts once, upstream).
 */
export function filterJobsByGroup(
  jobs: Job[],
  sel: GroupSelection,
  groups: Group[],
): Job[] {
  switch (sel.kind) {
    case "all":
      return jobs;
    case "ungrouped":
      return jobs.filter((j) => j.group_id === null);
    case "group": {
      const ids = descendantGroupIds(groups, sel.id);
      return jobs.filter((j) => j.group_id !== null && ids.has(j.group_id));
    }
  }
}

/**
 * Valid targets for **moving a group**: every group EXCEPT `movingGroupId` and its
 * descendants — so the Move-to picker can never offer a target that would create a
 * cycle (the backend `move_group` would reject it; the UI just doesn't present it).
 * Sorted by {@link compareGroups}. (Moving to root — `null` — is offered by the UI
 * separately, not in this list.)
 */
export function moveTargetsFor(groups: Group[], movingGroupId: string): Group[] {
  const excluded = descendantGroupIds(groups, movingGroupId);
  return groups.filter((g) => !excluded.has(g.id)).sort(compareGroups);
}
