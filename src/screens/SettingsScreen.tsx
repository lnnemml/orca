import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function SettingsScreen() {
  const [orcaPath, setOrcaPath] = useState("");
  const [savedPath, setSavedPath] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
  }, [loadSettings]);

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
    <div className="screen">
      <h2 className="section-title">Settings</h2>
      <div className="card">
        <div className="field">
          <label className="label" htmlFor="orca-path">
            ORCA executable path
          </label>
          <div className="row">
            <input
              id="orca-path"
              className="input mono"
              style={{ flex: 1 }}
              value={orcaPath}
              onChange={(e) => setOrcaPath(e.currentTarget.value)}
              spellCheck={false}
            />
            <button
              className="btn btn-primary"
              onClick={saveOrcaPath}
              disabled={!dirty || saving}
            >
              {saving ? "Saving…" : "Save"}
            </button>
          </div>
          <div style={{ fontSize: 12, marginTop: 2 }}>
            {dirty ? (
              <span style={{ color: "var(--warn)" }}>Unsaved change</span>
            ) : (
              <span style={{ color: "var(--ok)" }}>Configured</span>
            )}
          </div>
        </div>
        {error ? (
          <div className="banner err" style={{ marginTop: 12 }}>
            {error}
          </div>
        ) : null}
      </div>
    </div>
  );
}
