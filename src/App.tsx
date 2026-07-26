import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { NewJobScreen } from "./screens/NewJobScreen";
import { JobsScreen } from "./screens/JobsScreen";
import { JobDetailScreen } from "./screens/JobDetailScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import type { SidecarState, SidecarStatus } from "./types";
import "./styles/app.css";

type Screen =
  | { kind: "new-job" }
  | { kind: "jobs" }
  | { kind: "settings" }
  | { kind: "job-detail"; jobId: string; autoRun: boolean };

const DOT_COLOR: Record<SidecarState, string> = {
  healthy: "var(--ok)",
  starting: "var(--warn)",
  down: "var(--err)",
};

const STATUS_LABEL: Record<SidecarState, string> = {
  healthy: "Sidecar healthy",
  starting: "Sidecar starting…",
  down: "Sidecar down",
};

const TABS: { id: "new-job" | "jobs" | "settings"; label: string }[] = [
  { id: "new-job", label: "New Job" },
  { id: "jobs", label: "Jobs" },
  { id: "settings", label: "Settings" },
];

function App() {
  const [screen, setScreen] = useState<Screen>({ kind: "new-job" });
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const [orcaPath, setOrcaPath] = useState("");

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
    const id = setInterval(refreshSidecar, 5000);
    return () => clearInterval(id);
  }, [loadOrcaPath, refreshSidecar]);

  // The Jobs tab stays highlighted while drilled into a job's detail.
  const activeTab = screen.kind === "job-detail" ? "jobs" : screen.kind;

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
        />
      ) : screen.kind === "jobs" ? (
        <JobsScreen onOpenDetail={openDetail} />
      ) : screen.kind === "settings" ? (
        <SettingsScreen />
      ) : (
        <JobDetailScreen
          jobId={screen.jobId}
          autoRun={screen.autoRun}
          onBack={() => setScreen({ kind: "jobs" })}
        />
      )}

      <footer className="statusbar">
        <span>
          <span
            className="status-dot"
            style={{ background: sidecar ? DOT_COLOR[sidecar.status] : "#4b5563" }}
          />
          {sidecar ? STATUS_LABEL[sidecar.status] : "Checking…"}
          {sidecar?.port ? ` :${sidecar.port}` : ""}
        </span>
        <span>
          ORCA: <span className="mono">{orcaPath || "not configured"}</span>
        </span>
      </footer>
    </div>
  );
}

export default App;
