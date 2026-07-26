import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { NewJobScreen } from "./screens/NewJobScreen";
import { JobsScreen } from "./screens/JobsScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import type { SidecarState, SidecarStatus } from "./types";
import "./styles/app.css";

type Screen = "new-job" | "jobs" | "settings";

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

const TABS: { id: Screen; label: string }[] = [
  { id: "new-job", label: "New Job" },
  { id: "jobs", label: "Jobs" },
  { id: "settings", label: "Settings" },
];

function App() {
  const [screen, setScreen] = useState<Screen>("new-job");
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const [orcaPath, setOrcaPath] = useState("");

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

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">OrcaStudio</span>
        <nav className="tabs">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={"tab" + (screen === t.id ? " active" : "")}
              onClick={() => setScreen(t.id)}
            >
              {t.label}
            </button>
          ))}
        </nav>
      </header>

      {screen === "new-job" ? (
        <NewJobScreen onCreated={() => setScreen("jobs")} />
      ) : screen === "jobs" ? (
        <JobsScreen />
      ) : (
        <SettingsScreen />
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
