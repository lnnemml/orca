import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { Job, JobStatus } from "../types";
import { formatEnergy, formatWallTime } from "../format";

interface LogPayload {
  job_id: string;
  lines: string[];
}

interface StatusPayload {
  job_id: string;
  status: JobStatus;
}

/** Cap retained console lines so a long run doesn't grow the DOM unbounded. */
const MAX_LINES = 5000;

interface JobDetailScreenProps {
  jobId: string;
  /** Submit the job once listeners are attached (from a Run action). */
  autoRun: boolean;
  onBack: () => void;
}

export function JobDetailScreen({ jobId, autoRun, onBack }: JobDetailScreenProps) {
  const [job, setJob] = useState<Job | null>(null);
  const [lines, setLines] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const preRef = useRef<HTMLPreElement>(null);
  const didSubmit = useRef(false);

  const loadJob = useCallback(async () => {
    try {
      setJob(await invoke<Job>("get_job", { id: jobId }));
    } catch (e) {
      setError(String(e));
    }
  }, [jobId]);

  useEffect(() => {
    let unlistenLog: UnlistenFn | undefined;
    let unlistenStatus: UnlistenFn | undefined;
    let cancelled = false;

    (async () => {
      let current: Job | null = null;
      try {
        current = await invoke<Job>("get_job", { id: jobId });
        if (cancelled) return;
        setJob(current);
      } catch (e) {
        setError(String(e));
      }

      unlistenLog = await listen<LogPayload>("job:log", (e) => {
        if (e.payload.job_id !== jobId) return;
        setLines((prev) => {
          const next = prev.concat(e.payload.lines);
          return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
        });
      });

      unlistenStatus = await listen<StatusPayload>("job:status", (e) => {
        if (e.payload.job_id !== jobId) return;
        setJob((prev) => (prev ? { ...prev, status: e.payload.status } : prev));
        // Reload the full record so error_message / completed_at / results appear.
        if (e.payload.status === "completed" || e.payload.status === "failed") {
          loadJob();
        }
      });

      // Backfill existing output from output.out (after listeners are attached).
      // For a completed/failed job this loads the whole log; for a running job it
      // loads what exists so far and live events append from here (a small
      // duplicate window is possible for a running job — acceptable for Phase 1).
      if (current && current.status !== "draft") {
        try {
          const existing = await invoke<string[]>("read_job_output", { id: jobId });
          if (!cancelled && existing.length) {
            setLines((prev) => (prev.length ? prev : existing));
          }
        } catch (e) {
          setError(String(e));
        }
      }

      // Kick off the run only AFTER listeners are attached, so no early output
      // lines are lost. The ref guards against React StrictMode's double-mount.
      if (autoRun && !didSubmit.current && !cancelled) {
        didSubmit.current = true;
        try {
          await invoke("submit_job", { id: jobId });
        } catch (e) {
          setError(String(e));
        }
      }
    })();

    return () => {
      cancelled = true;
      unlistenLog?.();
      unlistenStatus?.();
    };
  }, [jobId, autoRun, loadJob]);

  // Auto-scroll the console to the bottom as new lines arrive.
  useEffect(() => {
    const el = preRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines]);

  const openFolder = async () => {
    try {
      await invoke("open_job_folder", { id: jobId });
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="screen detail">
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 10 }}>
        <div className="row">
          <button className="btn btn-sm" onClick={onBack}>
            ← Jobs
          </button>
          {job?.job_dir ? (
            <button className="btn btn-sm" onClick={openFolder}>
              Open Folder
            </button>
          ) : null}
        </div>
        {job ? <span className={`badge ${job.status}`}>{job.status}</span> : null}
      </div>

      {job ? (
        <div style={{ marginBottom: 10 }}>
          <h2 className="section-title" style={{ marginBottom: 4 }}>
            {job.title}
          </h2>
          <div className="mono" style={{ color: "var(--muted)", fontSize: 12 }}>
            created {job.created_at}
            {job.started_at ? ` · started ${job.started_at}` : ""}
            {job.completed_at ? ` · finished ${job.completed_at}` : ""}
          </div>
          {job.energy != null || job.wall_time != null ? (
            <div className="mono" style={{ color: "var(--text)", fontSize: 12, marginTop: 4 }}>
              energy {formatEnergy(job.energy)} Eh · time {formatWallTime(job.wall_time)}
            </div>
          ) : null}
        </div>
      ) : null}

      {error ? (
        <div className="banner err" style={{ marginBottom: 10 }}>
          {error}
        </div>
      ) : null}
      {job?.error_message ? (
        <div className="banner err" style={{ marginBottom: 10, whiteSpace: "pre-wrap" }}>
          {job.error_message}
        </div>
      ) : null}

      <pre className="log-console" ref={preRef}>
        {lines.length ? lines.join("\n") : "Waiting for ORCA output…"}
      </pre>
    </div>
  );
}
