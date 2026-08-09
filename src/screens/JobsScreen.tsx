import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";

import type { Job } from "../types";
import { formatEnergy, formatTimestamp, formatWallTime } from "../format";

interface JobsScreenProps {
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
  /** Whether the queue is paused — surfaced on `queued` badges so a job that
   *  isn't starting shows why. */
  queuePaused: boolean;
}

export function JobsScreen({ onOpenDetail, queuePaused }: JobsScreenProps) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const list = await invoke<Job[]>("list_jobs");
      setJobs(list);
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

  useEffect(() => {
    load();
    // Statuses change as the queue advances jobs — refresh on every transition.
    const unlisten = listen("job:status", () => load());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [load]);

  return (
    <div className="screen">
      <div
        className="row"
        style={{ justifyContent: "space-between", marginBottom: 14 }}
      >
        <h2 className="section-title" style={{ margin: 0 }}>
          Jobs
        </h2>
        <button className="btn" onClick={load}>
          Refresh
        </button>
      </div>

      {error ? <div className="banner err">{error}</div> : null}

      {loading ? (
        <div className="empty">Loading…</div>
      ) : jobs.length === 0 ? (
        <div className="empty">No jobs yet — create one from “New Job”.</div>
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
            {jobs.map((job) => (
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
                    // Terminal states (draft/completed/parsed/failed/cancelled):
                    // the primary action + Delete (removes the job and its files).
                    <div
                      className="row"
                      style={{ gap: 8, justifyContent: "flex-end" }}
                    >
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
                    </div>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
