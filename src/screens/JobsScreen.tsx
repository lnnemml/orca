import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job } from "../types";
import { formatEnergy, formatTimestamp, formatWallTime } from "../format";

interface JobsScreenProps {
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
}

export function JobsScreen({ onOpenDetail }: JobsScreenProps) {
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

  useEffect(() => {
    load();
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
                  <span className={`badge ${job.status}`}>{job.status}</span>
                </td>
                <td className="mono" style={{ textAlign: "right" }}>
                  {formatEnergy(job.energy)}
                </td>
                <td className="mono" style={{ textAlign: "right" }}>
                  {formatWallTime(job.wall_time)}
                </td>
                <td className="mono">{formatTimestamp(job.created_at)}</td>
                <td style={{ textAlign: "right" }}>
                  {job.status === "draft" ? (
                    <button
                      className="btn btn-primary btn-sm"
                      onClick={(e) => {
                        e.stopPropagation();
                        onOpenDetail(job.id, true);
                      }}
                    >
                      Run
                    </button>
                  ) : job.status === "running" ? (
                    <span className="muted">Running…</span>
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
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
