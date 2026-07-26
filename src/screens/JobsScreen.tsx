import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job } from "../types";

/** Render an ISO-ish SQLite timestamp (`YYYY-MM-DD HH:MM:SS` UTC) compactly. */
function formatTimestamp(ts: string): string {
  // SQLite `datetime('now')` yields `2026-07-26 12:34:56` (UTC, space-separated).
  return ts.replace("T", " ").replace(/\.\d+Z?$/, "");
}

export function JobsScreen() {
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
              <th>Created</th>
            </tr>
          </thead>
          <tbody>
            {jobs.map((job) => (
              <tr key={job.id}>
                <td>{job.title}</td>
                <td>
                  <span className={`badge ${job.status}`}>{job.status}</span>
                </td>
                <td className="mono">{formatTimestamp(job.created_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
