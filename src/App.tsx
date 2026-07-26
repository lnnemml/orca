import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type SidecarStatus = {
  status: "healthy" | "starting" | "down";
  port: number | null;
};

const DOT_COLOR: Record<SidecarStatus["status"], string> = {
  healthy: "#22c55e",
  starting: "#eab308",
  down: "#ef4444",
};

const STATUS_LABEL: Record<SidecarStatus["status"], string> = {
  healthy: "Healthy",
  starting: "Starting…",
  down: "Down",
};

function App() {
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const [orcaPath, setOrcaPath] = useState("");
  const [savedPath, setSavedPath] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshSidecar = useCallback(async () => {
    try {
      const status = await invoke<SidecarStatus>("get_sidecar_status");
      setSidecar(status);
    } catch (e) {
      setSidecar({ status: "down", port: null });
      setError(String(e));
    }
  }, []);

  const loadSettings = useCallback(async () => {
    try {
      const settings = await invoke<Record<string, string>>("get_settings");
      const path = settings.orca_path ?? "";
      setOrcaPath(path);
      setSavedPath(path);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    loadSettings();
    refreshSidecar();
    // No Tauri events yet — poll the sidecar status until it settles.
    const id = setInterval(refreshSidecar, 5000);
    return () => clearInterval(id);
  }, [loadSettings, refreshSidecar]);

  const saveOrcaPath = async () => {
    setSaving(true);
    setError(null);
    try {
      await invoke("set_setting", { key: "orca_path", value: orcaPath });
      setSavedPath(orcaPath);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const dirty = orcaPath !== savedPath;

  return (
    <main
      style={{
        fontFamily: "system-ui, sans-serif",
        color: "#1e293b",
        padding: 32,
        maxWidth: 640,
        margin: "0 auto",
      }}
    >
      <h1 style={{ fontSize: 28, marginBottom: 4 }}>OrcaStudio</h1>
      <p style={{ color: "#64748b", marginTop: 0 }}>
        A desktop workbench for the ORCA quantum chemistry package.
      </p>

      <section
        style={{
          border: "1px solid #e2e8f0",
          borderRadius: 12,
          padding: 20,
          marginTop: 24,
        }}
      >
        <h2 style={{ fontSize: 18, marginTop: 0 }}>System Status</h2>

        {/* Sidecar */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "10px 0",
            borderBottom: "1px solid #f1f5f9",
          }}
        >
          <span style={{ width: 120, color: "#475569" }}>Sidecar</span>
          <span
            aria-hidden
            style={{
              width: 12,
              height: 12,
              borderRadius: "50%",
              background: sidecar ? DOT_COLOR[sidecar.status] : "#cbd5e1",
              display: "inline-block",
            }}
          />
          <span>{sidecar ? STATUS_LABEL[sidecar.status] : "Checking…"}</span>
          {sidecar?.port ? (
            <span style={{ color: "#94a3b8", fontSize: 13 }}>
              :{sidecar.port}
            </span>
          ) : null}
        </div>

        {/* ORCA path */}
        <div style={{ padding: "14px 0" }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ width: 120, color: "#475569" }}>ORCA path</span>
            <input
              value={orcaPath}
              onChange={(e) => setOrcaPath(e.currentTarget.value)}
              spellCheck={false}
              style={{
                flex: 1,
                padding: "6px 8px",
                border: "1px solid #cbd5e1",
                borderRadius: 6,
                fontFamily: "ui-monospace, monospace",
                fontSize: 13,
              }}
            />
            <button
              onClick={saveOrcaPath}
              disabled={!dirty || saving}
              style={{
                padding: "6px 14px",
                borderRadius: 6,
                border: "none",
                background: !dirty || saving ? "#cbd5e1" : "#2563eb",
                color: "white",
                cursor: !dirty || saving ? "default" : "pointer",
              }}
            >
              {saving ? "Saving…" : "Save"}
            </button>
          </div>
          <div style={{ marginLeft: 130, marginTop: 6, fontSize: 13 }}>
            {dirty ? (
              <span style={{ color: "#b45309" }}>Unsaved change</span>
            ) : (
              <span style={{ color: "#16a34a" }}>Configured</span>
            )}
          </div>
        </div>
      </section>

      {error ? (
        <p style={{ color: "#dc2626", marginTop: 16, fontSize: 13 }}>{error}</p>
      ) : null}
    </main>
  );
}

export default App;
