import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { NewJobScreen } from "./screens/NewJobScreen";
import { JobsScreen } from "./screens/JobsScreen";
import { JobDetailScreen } from "./screens/JobDetailScreen";
import { MoleculesScreen } from "./screens/MoleculesScreen";
import { ReactionsScreen } from "./screens/ReactionsScreen";
import { ManualScreen } from "./screens/ManualScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import type { Job, Molecule, SidecarState, SidecarStatus } from "./types";
import type { GroupSelection } from "./groups/tree";
import { queueCounts } from "./jobs/status";
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
  | { kind: "reactions" }
  | { kind: "manual" }
  | { kind: "settings" }
  | { kind: "job-detail"; jobId: string; autoRun: boolean; runTarget: string | null };

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

const TABS: {
  id: "new-job" | "jobs" | "molecules" | "reactions" | "manual" | "settings";
  label: string;
}[] = [
  { id: "new-job", label: "New Job" },
  { id: "jobs", label: "Jobs" },
  { id: "molecules", label: "Molecules" },
  { id: "reactions", label: "Reactions" },
  { id: "manual", label: "Manual" },
  { id: "settings", label: "Settings" },
];

function App() {
  const [screen, setScreen] = useState<Screen>({ kind: "new-job" });
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const [orcaPath, setOrcaPath] = useState("");
  const [queue, setQueue] = useState<QueueInfo>({ running: 0, queued: 0, paused: false });
  // The active job-group selection (Phase 4.7.3). Lifted here — the SINGLE source of
  // truth — because it must survive a tab switch to "New Job" (assign-on-create reads
  // it) as well as drive the Jobs sidebar filter. React-only, not persisted across
  // restarts. `activeGroupId` (a real group, else null) is the value new jobs inherit.
  const [groupSelection, setGroupSelection] = useState<GroupSelection>({ kind: "all" });
  const activeGroupId = groupSelection.kind === "group" ? groupSelection.id : null;

  // `runTarget` is where an `autoRun` submits (New Job's "Run on" choice); null = this machine.
  const openDetail = useCallback((jobId: string, autoRun: boolean, runTarget: string | null = null) => {
    setScreen({ kind: "job-detail", jobId, autoRun, runTarget });
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
      // `cancelling` counts as running: the server is still busy with it (ADR-024 o22.14.8).
      setQueue({ ...queueCounts(jobs), paused });
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
          activeGroupId={activeGroupId}
        />
      ) : screen.kind === "jobs" ? (
        <JobsScreen
          onOpenDetail={openDetail}
          queuePaused={queue.paused}
          selection={groupSelection}
          onSelectionChange={setGroupSelection}
        />
      ) : screen.kind === "molecules" ? (
        <MoleculesScreen
          onUseMolecule={(mol) =>
            setScreen({ kind: "new-job", initialMolecule: mol })
          }
        />
      ) : screen.kind === "reactions" ? (
        <ReactionsScreen onOpenJob={(jobId) => openDetail(jobId, false)} />
      ) : screen.kind === "manual" ? (
        <ManualScreen />
      ) : screen.kind === "settings" ? (
        <SettingsScreen />
      ) : (
        // `key`: opening another job from inside a job view (a refine child, a fan-out) remounts
        // the view — no log, convergence points, row, notice or remote record carries over, and
        // the new job's listeners, watch and backfill start clean (verifier M1).
        <JobDetailScreen
          key={screen.jobId}
          jobId={screen.jobId}
          autoRun={screen.autoRun}
          runTarget={screen.runTarget}
          onBack={() => setScreen({ kind: "jobs" })}
          onIterate={(job) => setScreen({ kind: "new-job", initialJob: job })}
          onUseConformer={() => setScreen({ kind: "new-job", keepScene: true })}
          onOpenJob={(id) => openDetail(id, false)}
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
