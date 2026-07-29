import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { NewJobScreen } from "./screens/NewJobScreen";
import { JobsScreen } from "./screens/JobsScreen";
import { JobDetailScreen } from "./screens/JobDetailScreen";
import { MoleculesScreen } from "./screens/MoleculesScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import type { Job, Molecule, SidecarState, SidecarStatus } from "./types";
import "./styles/app.css";

interface QueueInfo {
  running: number;
  queued: number;
  paused: boolean;
}

type Screen =
  | { kind: "new-job"; initialMolecule?: Molecule; initialJob?: Job; keepScene?: boolean }
  | { kind: "jobs" }
  | { kind: "molecules" }
  | { kind: "settings" }
  | { kind: "job-detail"; jobId: string; autoRun: boolean };

const DOT_COLOR: Record<SidecarState, string> = {
  healthy: "var(--ok)",
  starting: "var(--warn)",
  stale: "var(--warn)",
  down: "var(--err)",
};

const STATUS_LABEL: Record<SidecarState, string> = {
  healthy: "Sidecar healthy",
  starting: "Sidecar starting…",
  stale: "Sidecar STALE — restart the app",
  down: "Sidecar down",
};

const TABS: { id: "new-job" | "jobs" | "molecules" | "settings"; label: string }[] = [
  { id: "new-job", label: "New Job" },
  { id: "jobs", label: "Jobs" },
  { id: "molecules", label: "Molecules" },
  { id: "settings", label: "Settings" },
];

function App() {
  const [screen, setScreen] = useState<Screen>({ kind: "new-job" });
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const [orcaPath, setOrcaPath] = useState("");
  const [queue, setQueue] = useState<QueueInfo>({ running: 0, queued: 0, paused: false });

  const openDetail = useCallback((jobId: string, autoRun: boolean) => {
    setScreen({ kind: "job-detail", jobId, autoRun });
  }, []);

  const refreshSidecar = useCallback(async () => {
    try {
      setSidecar(await invoke<SidecarStatus>("get_sidecar_status"));
    } catch {
      setSidecar({ status: "down", port: null });
    }
  }, []);

  const refreshQueue = useCallback(async () => {
    try {
      const [jobs, paused] = await Promise.all([
        invoke<Job[]>("list_jobs"),
        invoke<boolean>("is_queue_paused"),
      ]);
      setQueue({
        running: jobs.filter((j) => j.status === "running").length,
        queued: jobs.filter((j) => j.status === "queued").length,
        paused,
      });
    } catch {
      /* backend not ready / plain browser — leave the last known counts */
    }
  }, []);

  const togglePause = useCallback(async () => {
    const next = !queue.paused;
    try {
      await invoke(next ? "pause_queue" : "resume_queue");
      // Flip locally right away so the label updates without waiting for the
      // refreshQueue round-trip.
      setQueue((q) => ({ ...q, paused: next }));
    } catch {
      /* surfaced elsewhere */
    } finally {
      refreshQueue();
    }
  }, [queue.paused, refreshQueue]);

  const loadOrcaPath = useCallback(async () => {
    try {
      const settings = await invoke<Record<string, string>>("get_settings");
      setOrcaPath(settings.orca_path ?? "");
    } catch {
      /* surfaced on the Settings screen */
    }
  }, []);

  useEffect(() => {
    loadOrcaPath();
    refreshSidecar();
    refreshQueue();
    const id = setInterval(() => {
      refreshSidecar();
      refreshQueue();
    }, 5000);
    // Update queue counts promptly as jobs transition.
    const unlisten = listen("job:status", () => refreshQueue());
    return () => {
      clearInterval(id);
      unlisten.then((fn) => fn());
    };
  }, [loadOrcaPath, refreshSidecar, refreshQueue]);

  // The Jobs tab stays highlighted while drilled into a job's detail.
  const activeTab = screen.kind === "job-detail" ? "jobs" : screen.kind;

  // Adaptive queue label: activity → "1 running, 2 waiting"; empty → "idle" or
  // "paused". Paused is always shown (even with an empty queue) so it can't be
  // missed — a silently-not-starting queue is the failure this guards against.
  const queueParts: string[] = [];
  if (queue.running > 0) queueParts.push(`${queue.running} running`);
  if (queue.queued > 0) queueParts.push(`${queue.queued} waiting`);
  const queueLabel = queueParts.length
    ? queueParts.join(", ") + (queue.paused ? " (paused)" : "")
    : queue.paused
      ? "paused"
      : "idle";

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">OrcaStudio</span>
        <nav className="tabs">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={"tab" + (activeTab === t.id ? " active" : "")}
              onClick={() => setScreen({ kind: t.id })}
            >
              {t.label}
            </button>
          ))}
        </nav>
      </header>

      {screen.kind === "new-job" ? (
        <NewJobScreen
          onCreatedDraft={() => setScreen({ kind: "jobs" })}
          onOpenDetail={openDetail}
          initialMolecule={screen.initialMolecule}
          initialJob={screen.initialJob}
          keepScene={screen.keepScene}
        />
      ) : screen.kind === "jobs" ? (
        <JobsScreen onOpenDetail={openDetail} queuePaused={queue.paused} />
      ) : screen.kind === "molecules" ? (
        <MoleculesScreen
          onUseMolecule={(mol) =>
            setScreen({ kind: "new-job", initialMolecule: mol })
          }
        />
      ) : screen.kind === "settings" ? (
        <SettingsScreen />
      ) : (
        <JobDetailScreen
          jobId={screen.jobId}
          autoRun={screen.autoRun}
          onBack={() => setScreen({ kind: "jobs" })}
          onIterate={(job) => setScreen({ kind: "new-job", initialJob: job })}
          onUseConformer={() => setScreen({ kind: "new-job", keepScene: true })}
        />
      )}

      <footer className="statusbar">
        <span
          className={
            "status-sidecar" + (sidecar?.status === "stale" ? " status-stale" : "")
          }
        >
          <span
            className="status-dot"
            style={{ background: sidecar ? DOT_COLOR[sidecar.status] : "#4b5563" }}
          />
          {sidecar ? STATUS_LABEL[sidecar.status] : "Checking…"}
          {sidecar?.status === "stale" && sidecar.version
            ? ` (running ${sidecar.version}, need ${sidecar.expected_version})`
            : sidecar?.port
              ? ` :${sidecar.port}`
              : ""}
        </span>
        <span>
          ORCA: <span className="mono">{orcaPath || "not configured"}</span>
        </span>
        <span className="row" style={{ gap: 8 }}>
          <span className={queue.paused ? "queue-paused" : undefined}>
            Queue: {queueLabel}
          </span>
          <button
            className="btn btn-sm"
            onClick={togglePause}
            title={
              queue.paused
                ? "Resume the queue"
                : "Pause the queue — the running job finishes, but no queued job starts"
            }
          >
            {queue.paused ? "Resume" : "Pause"}
          </button>
        </span>
      </footer>
    </div>
  );
}

export default App;
