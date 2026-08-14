import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";

import type { Group, Job, JobStatus } from "../types";
import { formatEnergy, formatTimestamp, formatWallTime } from "../format";
import { GroupSidebar } from "../groups/GroupSidebar";
import { filterJobsByGroup, type GroupSelection } from "../groups/tree";
import { filterJobsBySearch } from "../groups/search";
import { ROOT_OPTION } from "../groups/GroupSelect";

/** The status chips, in state-machine order. */
const STATUS_CHIPS: JobStatus[] = [
  "draft",
  "queued",
  "running",
  "completed",
  "parsed",
  "failed",
  "cancelled",
];

interface JobsScreenProps {
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
  /** Whether the queue is paused — surfaced on `queued` badges so a job that
   *  isn't starting shows why. */
  queuePaused: boolean;
  /** The active group selection (lifted to App — the single source of truth that
   *  also drives assign-on-create). */
  selection: GroupSelection;
  onSelectionChange: (sel: GroupSelection) => void;
}

export function JobsScreen({
  onOpenDetail,
  queuePaused,
  selection,
  onSelectionChange,
}: JobsScreenProps) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [groups, setGroups] = useState<Group[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [movingJobId, setMovingJobId] = useState<string | null>(null);
  // Search/status filter — LOCAL to this screen (not lifted, not persisted). Composed
  // AFTER the group filter: the rendered rows are always filterJobsBySearch(filter
  // JobsByGroup(...)), so search narrows within the selected group's subtree.
  const [query, setQuery] = useState("");
  const [statuses, setStatuses] = useState<Set<JobStatus>>(new Set());

  const toggleStatus = useCallback((s: JobStatus) => {
    setStatuses((prev) => {
      const next = new Set(prev);
      if (next.has(s)) next.delete(s);
      else next.add(s);
      return next;
    });
  }, []);

  const clearFilter = useCallback(() => {
    setQuery("");
    setStatuses(new Set());
  }, []);

  const load = useCallback(async () => {
    try {
      const [jobList, groupList] = await Promise.all([
        invoke<Job[]>("list_jobs"),
        invoke<Group[]>("list_groups"),
      ]);
      setJobs(jobList);
      setGroups(groupList);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const cancel = useCallback(
    async (jobId: string) => {
      try {
        await invoke("cancel_job", { id: jobId });
        // The terminal status arrives via job:status; reload to be safe.
        load();
      } catch (e) {
        setError(String(e));
      }
    },
    [load],
  );

  const remove = useCallback(
    async (job: Job) => {
      const ok = await confirm(
        `Permanently delete "${job.title}"? This removes the job and its files ` +
          "from disk — it cannot be undone.",
        { title: "Delete job", kind: "warning" },
      );
      if (!ok) return;
      try {
        await invoke("delete_job", { id: job.id });
        await load();
      } catch (e) {
        setError(String(e));
      }
    },
    [load],
  );

  const moveJob = useCallback(
    async (jobId: string, target: string) => {
      try {
        await invoke("move_job", {
          jobId,
          groupId: target === ROOT_OPTION ? null : target,
        });
        setMovingJobId(null);
        await load();
      } catch (e) {
        setError(String(e));
      }
    },
    [load],
  );

  useEffect(() => {
    load();
    // Statuses change as the queue advances jobs — refresh on every transition.
    const unlisten = listen("job:status", () => load());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [load]);

  // If the selected group vanished (deleted elsewhere), fall back to "All jobs" so
  // the filter never points at a non-existent group.
  useEffect(() => {
    if (
      selection.kind === "group" &&
      groups.length > 0 &&
      !groups.some((g) => g.id === selection.id)
    ) {
      onSelectionChange({ kind: "all" });
    }
  }, [selection, groups, onSelectionChange]);

  // Compose: group filter FIRST (4.7.3), then the search/status filter (4.7.4).
  const groupJobs = filterJobsByGroup(jobs, selection, groups);
  const visibleJobs = filterJobsBySearch(groupJobs, query, statuses);
  const filterActive = query.trim() !== "" || statuses.size > 0;

  const selectionLabel =
    selection.kind === "all"
      ? "All jobs"
      : selection.kind === "ungrouped"
        ? "Ungrouped"
        : (groups.find((g) => g.id === selection.id)?.name ?? "Group");

  return (
    <div className="screen jobs-screen">
      <div className="jobs-layout">
        <GroupSidebar
          groups={groups}
          selection={selection}
          onSelect={onSelectionChange}
          onChanged={load}
          onError={setError}
        />

        <div className="jobs-main">
          <div
            className="row"
            style={{ justifyContent: "space-between", marginBottom: 14 }}
          >
            <h2 className="section-title" style={{ margin: 0 }}>
              Jobs — <span className="muted">{selectionLabel}</span>
            </h2>
            <button className="btn" onClick={load}>
              Refresh
            </button>
          </div>

          {/* Search + status filter (4.7.4) — composed over the group filter. */}
          <div className="jobs-filter">
            <input
              className="input jobs-search"
              type="text"
              placeholder="Search title or method…"
              value={query}
              onChange={(e) => setQuery(e.currentTarget.value)}
            />
            <div className="status-chips">
              {STATUS_CHIPS.map((s) => (
                <button
                  key={s}
                  className={"chip" + (statuses.has(s) ? " active" : "")}
                  onClick={() => toggleStatus(s)}
                  title={`Filter by ${s}`}
                >
                  {s}
                </button>
              ))}
            </div>
            {filterActive ? (
              <button className="btn btn-sm" onClick={clearFilter} title="Clear the filter">
                Clear
              </button>
            ) : null}
          </div>

          {error ? <div className="banner err">{error}</div> : null}

          {loading ? (
            <div className="empty">Loading…</div>
          ) : visibleJobs.length === 0 ? (
            <div className="empty">
              {jobs.length === 0
                ? "No jobs yet — create one from “New Job”."
                : groupJobs.length === 0
                  ? "No jobs in this group."
                  : "No jobs match this filter."}
            </div>
          ) : (
            <table className="jobs-table">
              <thead>
                <tr>
                  <th>Title</th>
                  <th>Status</th>
                  <th style={{ textAlign: "right" }}>Energy (Eh)</th>
                  <th style={{ textAlign: "right" }}>Time</th>
                  <th>Created</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {visibleJobs.map((job) => (
                  <tr
                    key={job.id}
                    className="clickable"
                    onClick={() => onOpenDetail(job.id, false)}
                  >
                    <td>{job.title}</td>
                    <td>
                      {job.status === "queued" && queuePaused ? (
                        <span className="badge queued" title="Queue is paused">
                          queued (paused)
                        </span>
                      ) : (
                        <span className={`badge ${job.status}`}>{job.status}</span>
                      )}
                    </td>
                    <td className="mono" style={{ textAlign: "right" }}>
                      {formatEnergy(job.energy)}
                    </td>
                    <td className="mono" style={{ textAlign: "right" }}>
                      {formatWallTime(job.wall_time)}
                    </td>
                    <td className="mono">{formatTimestamp(job.created_at)}</td>
                    <td style={{ textAlign: "right" }}>
                      <div
                        className="row"
                        style={{ gap: 8, justifyContent: "flex-end" }}
                      >
                        {job.status === "running" || job.status === "queued" ? (
                          // Live job: cancel only. Deleting a running/queued job is
                          // refused by the backend — cancel it first.
                          <button
                            className="btn btn-sm"
                            onClick={(e) => {
                              e.stopPropagation();
                              cancel(job.id);
                            }}
                          >
                            Cancel
                          </button>
                        ) : (
                          // Terminal states: the primary action + Delete.
                          <>
                            <button
                              className={
                                job.status === "draft"
                                  ? "btn btn-primary btn-sm"
                                  : "btn btn-sm"
                              }
                              onClick={(e) => {
                                e.stopPropagation();
                                onOpenDetail(job.id, job.status === "draft");
                              }}
                            >
                              {job.status === "draft" ? "Run" : "Open"}
                            </button>
                            <button
                              className="btn btn-sm btn-danger"
                              onClick={(e) => {
                                e.stopPropagation();
                                remove(job);
                              }}
                            >
                              Delete
                            </button>
                          </>
                        )}

                        {/* Move to… — a job can be (re)grouped in any run state; it
                            is pure metadata (no cycle concern for a job). */}
                        {movingJobId === job.id ? (
                          <select
                            className="select job-move-select"
                            defaultValue=""
                            onClick={(e) => e.stopPropagation()}
                            onChange={(e) => {
                              e.stopPropagation();
                              if (e.currentTarget.value) moveJob(job.id, e.currentTarget.value);
                            }}
                          >
                            <option value="" disabled>
                              move to…
                            </option>
                            <option value={ROOT_OPTION}>(ungrouped)</option>
                            {groups.map((g) => (
                              <option key={g.id} value={g.id}>
                                {g.name}
                              </option>
                            ))}
                          </select>
                        ) : (
                          <button
                            className="btn btn-sm"
                            title="Move to a group"
                            onClick={(e) => {
                              e.stopPropagation();
                              setMovingJobId(job.id);
                            }}
                          >
                            Move…
                          </button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  );
}
