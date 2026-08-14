import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Group, Job } from "../types";
import { resolveGroupAssignment } from "./GroupSelect";

/**
 * Fetch a source job's current `group_id` — the **seed** for a derived-child destination
 * picker (Phase 4.7 unit 2b). `null` while loading, on error, or for a `null` jobId; the
 * picker then shows "(ungrouped)" until the real value arrives (the follow-until-touched sync
 * in {@link useGroupPicker} re-seeds it). Used by the three sites that hold only the source
 * job's id (Scan / NEB / Connectivity); JobDetailScreen already holds `job.group_id` and
 * passes it straight to {@link useGroupPicker}.
 */
export function useJobGroupId(jobId: string | null): string | null {
  const [groupId, setGroupId] = useState<string | null>(null);
  useEffect(() => {
    if (jobId == null) {
      setGroupId(null);
      return;
    }
    let live = true;
    invoke<Job>("get_job", { id: jobId })
      .then((j) => {
        if (live) setGroupId(j.group_id);
      })
      .catch(() => {
        if (live) setGroupId(null);
      });
    return () => {
      live = false;
    };
  }, [jobId]);
  return groupId;
}

export interface GroupPicker {
  groups: Group[];
  pickedGroupId: string | null;
  /** Wire straight into `<GroupSelect onChange>`. Marks the picker touched. */
  onChange: (groupId: string | null) => void;
  /** After a child is created, move it into the picked group per `resolveGroupAssignment`:
   *  a no-op when untouched + the seed is ungrouped (leaves Unit-1's Rust source-group
   *  default); the picked value otherwise, including an explicit "(ungrouped)" (null). Safe to
   *  call for EACH child of a multi-child action (Connectivity's forward+backward, the reopt
   *  fan-out) — one picker state, applied to all. */
  assignPicked: (jobId: string) => Promise<void>;
}

/**
 * The derived-child destination-group picker (Phase 4.7 unit 2b), mirroring the New Job
 * picker (2a) but **seeded from THIS panel's SOURCE job's group** — NOT the active sidebar
 * group. Composes the *proven* pure {@link resolveGroupAssignment} + `move_job`; the hook
 * itself is thin glue (like `useContainerWidth`) — the verifiable invariant it reuses is the
 * one already bite-tested in 2a.
 *
 * `seedGroupId` = the source's `group_id` (the default). The picker **FOLLOWS the seed until
 * the user touches it** (so a seed that arrives after an async {@link useJobGroupId} fetch
 * still lands), then an explicit pick **WINS** — including "(ungrouped)".
 */
export function useGroupPicker(seedGroupId: string | null): GroupPicker {
  const [groups, setGroups] = useState<Group[]>([]);
  const [pickedGroupId, setPickedGroupId] = useState<string | null>(seedGroupId);
  const [groupTouched, setGroupTouched] = useState(false);
  useEffect(() => {
    invoke<Group[]>("list_groups")
      .then(setGroups)
      .catch(() => setGroups([]));
  }, []);
  useEffect(() => {
    if (!groupTouched) setPickedGroupId(seedGroupId);
  }, [seedGroupId, groupTouched]);
  const onChange = useCallback((groupId: string | null) => {
    setGroupTouched(true);
    setPickedGroupId(groupId);
  }, []);
  const assignPicked = useCallback(
    async (jobId: string) => {
      const decision = resolveGroupAssignment(groupTouched, pickedGroupId);
      if (decision.assign) {
        await invoke("move_job", { jobId, groupId: decision.groupId });
      }
    },
    [groupTouched, pickedGroupId],
  );
  return { groups, pickedGroupId, onChange, assignPicked };
}
