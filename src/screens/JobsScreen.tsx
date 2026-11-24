import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, message } from "@tauri-apps/plugin-dialog";

import type { Group, Job, JobStatus, ServerProfile } from "../types";
import { formatEnergy, formatLaptopUtc, formatWallTime } from "../format";
import { GroupSidebar } from "../groups/GroupSidebar";
import { filterJobsByGroup, type GroupSelection } from "../groups/tree";
import { filterJobsBySearch } from "../groups/search";
import { ROOT_OPTION } from "../groups/GroupSelect";
import { InlineRename } from "../jobs/InlineRename";
import { CancelButtons } from "../jobs/CancelButtons";
import { ClaudeBadge, DeleteHeldNote, DraftRunSlot, RowReason, SubmittingButton } from "../jobs/ClaudeMarks";
import { subscribeJobsReload } from "../jobs/jobs-reload";
import { clickCancelFamily, tauriBridge, trackRemoteStates, type RemoteStateTracker } from "../jobs/live-wiring";
import { bulkDeleteFlow, planBulkDelete } from "../jobs/bulk-delete";
import { bannerAfterClick, remoteView, rowActions, runSlot, type CancelBusy, type CancelFamilyAction, type HeldRemoteState } from "../jobs/remote";
import { RemoteJobBadges } from "../jobs/RemotePanel";
import { STATUS_CHIPS } from "../jobs/status";
import { exportSelection, type CopyMode } from "../export/save";

interface JobsScreenProps {
  /** `runTarget` is where an `autoRun` submits: null = this machine, else a server profile id
   *  (`submit_job`'s `target`). A Claude draft's Run passes its stored target (ADR-027 M4c). */
  onOpenDetail: (jobId: string, autoRun: boolean, runTarget?: string | null) => void;
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
  // What the last Cancel / Retry cancel / Undo cancel answered, in words (ADR-024 o22.14.7).
  const [notice, setNotice] = useState<string | null>(null);
  // The rows whose cancel-family click is in flight (their buttons are disabled meanwhile).
  const [cancelling, setCancelling] = useState<ReadonlyMap<string, "cancel" | "undo">>(new Map());
  const [movingJobId, setMovingJobId] = useState<string | null>(null);
  // Search/status filter — LOCAL to this screen (not lifted, not persisted). Composed
  // AFTER the group filter: the rendered rows are always filterJobsBySearch(filter
  // JobsByGroup(...)), so search narrows within the selected group's subtree.
  const [query, setQuery] = useState("");
  const [statuses, setStatuses] = useState<Set<JobStatus>>(new Set());
  // Multi-select for the selection export (the third projection, ADR-021). LOCAL to this
  // screen — like the search/status filter, it is NOT lifted and NOT persisted across restarts.
  // The Set is authoritative: a selection survives search/status/group-filter changes, so its
  // count reflects the FULL set even when some picked jobs are currently filtered out of view.
  const [selected, setSelected] = useState<Set<string>>(new Set());
  // Whether the Curated/Full chooser (mirroring GroupSidebar.runExport) is open.
  const [exportChoosing, setExportChoosing] = useState(false);
  // A "Delete selected" run in flight. The ref is the synchronous guard (a second click in the same
  // tick must do nothing); the state only draws the disabled "Deleting…" button.
  const bulkDeleting = useRef(false);
  const [bulkDeletingUi, setBulkDeletingUi] = useState(false);

  const toggleSelected = useCallback((id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const clearSelection = useCallback(() => {
    setSelected(new Set());
    setExportChoosing(false);
  }, []);

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

  // The remote records of the listed live remote jobs (ADR-024 o19.6): one `job:remote-state`
  // listener for the screen's life, attached before any `remote_job_state` read (the tracker waits
  // for it), every value merged by `seq`. Each reload of the list re-reads the live remote jobs.
  const [remoteHeld, setRemoteHeld] = useState<ReadonlyMap<string, HeldRemoteState>>(new Map());
  const remoteTracker = useRef<RemoteStateTracker | null>(null);
  useEffect(() => {
    const tracker = trackRemoteStates(tauriBridge, setRemoteHeld, setError);
    remoteTracker.current = tracker;
    return () => {
      tracker.dispose();
      remoteTracker.current = null;
    };
  }, []);

  // The server profiles, read with every reload of the list: a Claude draft's Run names its stored
  // target's profile and judges whether it is a run target (`runSlot`, ADR-027 M4c item 3). `null`
  // = not read (loading, the read failed, or an answer that is not a list): such a draft then shows
  // a disabled Run, never Run and never a claim that its profile is gone. The read has its own
  // catch, so a profile failure never fails the job list.
  const [profiles, setProfiles] = useState<readonly ServerProfile[] | null>(null);

  const load = useCallback(async () => {
    try {
      const [jobList, groupList, profileList] = await Promise.all([
        invoke<Job[]>("list_jobs"),
        invoke<Group[]>("list_groups"),
        invoke<ServerProfile[]>("list_server_profiles").catch(() => null),
      ]);
      setJobs(jobList);
      setGroups(groupList);
      setProfiles(Array.isArray(profileList) ? profileList : null);
      const liveRemote = jobList.filter((j) => remoteView(j, null).kind === "live").map((j) => j.id);
      void remoteTracker.current?.read(liveRemote);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // Every cancel-family click goes through the one shared helper (o22.14.10, o22.14.14.1): it is
  // given the job id and the in-flight callback and nothing else. It reads the row and the record
  // from the backend itself, derives the offer, asks with the plugin's own dialog when that offer is
  // a remote job's first Cancel (re-checked after the dialog), and words the answer. Then the list
  // reloads (it re-reads the live remote records).
  const cancelFamily = useCallback(
    async (job: Job, action: CancelFamilyAction) => {
      // The banner is not cleared here: the next *answer* replaces it (`bannerAfterClick`); a declined
      // dialog or a click that offered nothing leaves it (o22.14.14.3).
      try {
        const outcome = await clickCancelFamily(action, {
          id: job.id,
          sending: (busy: CancelBusy) =>
            setCancelling((prev) => {
              const next = new Map(prev);
              if (busy) next.set(job.id, busy);
              else next.delete(job.id);
              return next;
            }),
        });
        setNotice((prev) => bannerAfterClick(prev, outcome));
        // The terminal status of a local job arrives via job:status; reload to be safe.
        if (outcome) await load();
      } catch (e) {
        // An error is an answer too: the older notice does not stand beside it.
        setNotice((prev) => bannerAfterClick(prev, "error"));
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

  // The list is read after the listeners are attached ("listen first, then ask", o16.7) and again on
  // every `job:status` (the queue advances jobs; the event carries no `mcp_state`, so "Claude is
  // submitting" clears only through this reload) and every `groups:changed` (Claude made a group,
  // ADR-027 M4). A registration that fails is shown, not thrown; unmount detaches both.
  useEffect(() => {
    let disposed = false;
    const sub = subscribeJobsReload(tauriBridge, () => void load(), setError);
    void sub.ready.then(() => {
      if (!disposed) void load();
    });
    return () => {
      disposed = true;
      sub.dispose();
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

  // Select-all-visible: adds/removes EXACTLY the currently visible jobs (leaving any selected-
  // but-filtered-out jobs untouched). The header box is checked only when every visible job is
  // already selected.
  const allVisibleSelected =
    visibleJobs.length > 0 && visibleJobs.every((j) => selected.has(j.id));
  const toggleSelectAllVisible = () => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (allVisibleSelected) {
        for (const j of visibleJobs) next.delete(j.id);
      } else {
        for (const j of visibleJobs) next.add(j.id);
      }
      return next;
    });
  };

  // Run the selection export in the chosen copy mode (native folder picker → path toast),
  // mirroring GroupSidebar.runExport. The selection is left intact after a successful export.
  const runSelectionExport = async (mode: CopyMode) => {
    setExportChoosing(false);
    const ids = [...selected];
    try {
      const path = await exportSelection(ids, mode);
      if (path) {
        await message(`Exported ${ids.length} selected job(s) to:\n${path}`, {
          title: "Export complete",
        });
      }
    } catch (e) {
      setError(String(e));
    }
  };

  // "Delete selected": the WHOLE selection (hidden jobs included), planned with the same rowActions
  // and the same remote records the rows' own buttons use; one confirm, then `delete_job` once per
  // deletable job in sequence, then one summary (`bulkDeleteFlow`). Rust still refuses a row that
  // turned live meanwhile — that comes back as a per-job failure in the summary.
  const deleteSelected = async () => {
    if (bulkDeleting.current) return;
    bulkDeleting.current = true;
    setBulkDeletingUi(true);
    setExportChoosing(false);
    try {
      const plan = planBulkDelete(selected, jobs, new Set(visibleJobs.map((j) => j.id)), (id) => remoteHeld.get(id)?.state);
      await bulkDeleteFlow(plan, {
        ask: (text) => confirm(text, { title: "Delete selected jobs", kind: "warning" }),
        notify: async (text, kind) => {
          await message(text, { title: "Delete selected", kind });
        },
        del: async (id) => {
          await invoke("delete_job", { id });
        },
        reload: load,
        setSelection: (update) => setSelected((prev) => update(prev)),
      });
    } catch (e) {
      setError(String(e));
    } finally {
      bulkDeleting.current = false;
      setBulkDeletingUi(false);
    }
  };

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
            <div className="row" style={{ gap: 8 }}>
              {/* Selection-export toolbar — only when at least one job is selected. */}
              {selected.size > 0 ? (
                exportChoosing ? (
                  <div className="row" style={{ gap: 8 }}>
                    <span className="muted">Export {selected.size}:</span>
                    <button className="btn btn-sm" onClick={() => runSelectionExport("curated")}>
                      Curated…
                    </button>
                    <button className="btn btn-sm" onClick={() => runSelectionExport("full")}>
                      Full…
                    </button>
                    <button
                      className="icon-btn"
                      title="Cancel"
                      onClick={() => setExportChoosing(false)}
                    >
                      ✕
                    </button>
                  </div>
                ) : (
                  <>
                    <button className="btn btn-sm" onClick={() => setExportChoosing(true)}>
                      Export selected ({selected.size})…
                    </button>
                    <button
                      className="btn btn-sm btn-danger"
                      disabled={bulkDeletingUi}
                      onClick={deleteSelected}
                    >
                      {bulkDeletingUi ? "Deleting…" : `Delete selected (${selected.size})…`}
                    </button>
                    <button className="btn btn-sm" onClick={clearSelection}>
                      Clear selection
                    </button>
                  </>
                )
              ) : null}
              <button className="btn" onClick={load}>
                Refresh
              </button>
            </div>
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
          {notice ? (
            <div className="banner warn row" style={{ justifyContent: "space-between" }}>
              <span>{notice}</span>
              <button className="icon-btn" title="Dismiss" onClick={() => setNotice(null)}>
                ✕
              </button>
            </div>
          ) : null}

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
                  <th style={{ width: 28 }}>
                    <input
                      type="checkbox"
                      checked={allVisibleSelected}
                      onChange={toggleSelectAllVisible}
                      title="Select all visible"
                    />
                  </th>
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
                    <td onClick={(e) => e.stopPropagation()}>
                      <input
                        type="checkbox"
                        checked={selected.has(job.id)}
                        onChange={() => toggleSelected(job.id)}
                        onClick={(e) => e.stopPropagation()}
                        title="Select for export"
                      />
                    </td>
                    <td>
                      <InlineRename
                        value={job.title}
                        onCommit={async (title) => {
                          try {
                            await invoke<Job>("rename_job", { id: job.id, title });
                            await load();
                          } catch (e) {
                            setError(String(e));
                          }
                        }}
                      />
                    </td>
                    <td>
                      {job.status === "queued" && queuePaused ? (
                        <span className="badge queued" title="Queue is paused">
                          queued (paused)
                        </span>
                      ) : (
                        <span className={`badge ${job.status}`}>{job.status}</span>
                      )}
                      <RemoteJobBadges job={job} record={remoteHeld.get(job.id)?.state} />
                      <ClaudeBadge job={job} />
                      <RowReason job={job} />
                    </td>
                    <td className="mono" style={{ textAlign: "right" }}>
                      {formatEnergy(job.energy)}
                    </td>
                    <td className="mono" style={{ textAlign: "right" }}>
                      {formatWallTime(job.wall_time)}
                    </td>
                    <td className="mono">{formatLaptopUtc(job.created_at)}</td>
                    <td style={{ textAlign: "right" }}>
                      <div
                        className="row"
                        style={{ gap: 8, justifyContent: "flex-end" }}
                      >
                        {(() => {
                          // The row's actions (o22.14.8): a live job offers its cancel family only
                          // (never Delete — Rust refuses it): Cancel, Retry cancel or Undo cancel,
                          // each labelled by its offer; a terminal row opens or is deleted.
                          const actions = rowActions(job, remoteHeld.get(job.id)?.state);
                          const terminal = actions.some((a) => a.action === "delete");
                          // A draft Claude is still submitting: a disabled "Claude is submitting" in
                          // Run's place and no Delete (ADR-027 M4 item 6); `rowActions` decides.
                          const claudeSubmitting = actions.some((a) => a.action === "submitting");
                          // A draft's Run slot (`runSlot`): a user draft runs here; a Claude draft
                          // runs only on the target it was assembled for, or offers a note / a
                          // disabled Run (ADR-027 M4c item 3). Drawn only where `rowActions` offers
                          // a Run, so "submitting" has one decider.
                          const slot = actions.some((a) => a.action === "run") ? runSlot(job, profiles) : null;
                          // A terminal row a crash left flagged: Open stays, Delete is a muted note.
                          const deleteHeld = actions.some((a) => a.action === "delete_held");
                          return (
                            <>
                              <CancelButtons
                                actions={actions}
                                busy={cancelling.get(job.id) ?? null}
                                onAction={(action) => cancelFamily(job, action)}
                                stopPropagation
                              />
                              {terminal || deleteHeld ? (
                                // Terminal states: the primary action + Delete (or its held-back note).
                                <>
                                  {slot !== null ? (
                                    <DraftRunSlot
                                      slot={slot}
                                      onRun={(target) => onOpenDetail(job.id, true, target)}
                                    />
                                  ) : (
                                    <button
                                      className="btn btn-sm"
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        onOpenDetail(job.id, false);
                                      }}
                                    >
                                      Open
                                    </button>
                                  )}
                                  {terminal ? (
                                    <button
                                      className="btn btn-sm btn-danger"
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        remove(job);
                                      }}
                                    >
                                      Delete
                                    </button>
                                  ) : (
                                    <DeleteHeldNote />
                                  )}
                                </>
                              ) : null}
                              {claudeSubmitting ? (
                                <span onClick={(e) => e.stopPropagation()}>
                                  <SubmittingButton />
                                </span>
                              ) : null}
                            </>
                          );
                        })()}

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
