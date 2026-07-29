import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { CpuPresetInfo } from "../types";

export function SettingsScreen() {
  const [orcaPath, setOrcaPath] = useState("");
  const [savedPath, setSavedPath] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // --- xtb (standalone pre-optimizer, 2.5.5) ---
  const [xtbPath, setXtbPath] = useState("");
  const [savedXtbPath, setSavedXtbPath] = useState("");
  const [savingXtb, setSavingXtb] = useState(false);
  const [xtbVersion, setXtbVersion] = useState<string | null>(null);
  const [checkingXtb, setCheckingXtb] = useState(false);

  // --- CPU pinning (domain rule #8) ---
  const [presets, setPresets] = useState<CpuPresetInfo[]>([]);
  const [cpuPreset, setCpuPreset] = useState("interactive");
  const [cpuMask, setCpuMask] = useState("8-15");
  const [cpuNprocs, setCpuNprocs] = useState("8");
  const [cpuSaved, setCpuSaved] = useState(false);

  const loadSettings = useCallback(async () => {
    try {
      const [settings, presetList] = await Promise.all([
        invoke<Record<string, string>>("get_settings"),
        invoke<CpuPresetInfo[]>("get_cpu_presets"),
      ]);
      const path = settings.orca_path ?? "";
      setOrcaPath(path);
      setSavedPath(path);
      const xtb = settings.xtb_path ?? "";
      setXtbPath(xtb);
      setSavedXtbPath(xtb);
      setPresets(presetList);
      setCpuPreset(settings.cpu_preset ?? "interactive");
      setCpuMask(settings.cpu_mask ?? "8-15");
      setCpuNprocs(settings.cpu_nprocs ?? "8");
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

  const saveCpu = async () => {
    setError(null);
    setCpuSaved(false);
    try {
      await invoke("set_setting", { key: "cpu_preset", value: cpuPreset });
      if (cpuPreset === "custom") {
        await invoke("set_setting", { key: "cpu_mask", value: cpuMask });
        await invoke("set_setting", { key: "cpu_nprocs", value: cpuNprocs });
      }
      setCpuSaved(true);
    } catch (e) {
      setError(String(e));
    }
  };

  const saveXtbPath = async () => {
    setSavingXtb(true);
    setError(null);
    try {
      await invoke("set_setting", { key: "xtb_path", value: xtbPath });
      setSavedXtbPath(xtbPath);
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingXtb(false);
    }
  };

  const checkXtb = async () => {
    setCheckingXtb(true);
    setXtbVersion(null);
    setError(null);
    try {
      // Verifies the saved path (persist first so the command reads what's shown).
      if (xtbPath !== savedXtbPath) await saveXtbPath();
      setXtbVersion(await invoke<string>("xtb_version"));
    } catch (e) {
      setError(String(e));
    } finally {
      setCheckingXtb(false);
    }
  };

  const dirty = orcaPath !== savedPath;
  const xtbDirty = xtbPath !== savedXtbPath;
  const isCustom = cpuPreset === "custom";

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
      </div>

      <div className="card" style={{ marginTop: 12 }}>
        <div className="field">
          <label className="label" htmlFor="xtb-path">
            xtb executable path
          </label>
          <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 6 }}>
            Standalone <span className="mono">xtb</span> for pre-optimization (not
            xtb-via-ORCA). Point this at the binary — an absolute path is best.
          </div>
          <div className="row">
            <input
              id="xtb-path"
              className="input mono"
              style={{ flex: 1 }}
              value={xtbPath}
              onChange={(e) => setXtbPath(e.currentTarget.value)}
              spellCheck={false}
            />
            <button
              className="btn btn-primary"
              onClick={saveXtbPath}
              disabled={!xtbDirty || savingXtb}
            >
              {savingXtb ? "Saving…" : "Save"}
            </button>
            <button className="btn" onClick={checkXtb} disabled={checkingXtb}>
              {checkingXtb ? "Checking…" : "Check"}
            </button>
          </div>
          <div style={{ fontSize: 12, marginTop: 2 }}>
            {xtbVersion ? (
              <span style={{ color: "var(--ok)" }}>{xtbVersion}</span>
            ) : xtbDirty ? (
              <span style={{ color: "var(--warn)" }}>Unsaved change</span>
            ) : (
              <span style={{ color: "var(--muted)" }}>
                Configured — click Check to verify.
              </span>
            )}
          </div>
        </div>
      </div>

      <div className="card" style={{ marginTop: 12 }}>
        <div className="field">
          <label className="label">CPU cores for ORCA</label>
          <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 8 }}>
            ORCA is pinned to these cores with <span className="mono">taskset</span>;{" "}
            <span className="mono">%pal nprocs</span> is aligned to the rank count
            automatically. Measured on this machine — see the performance notes.
          </div>

          {presets.map((p) => (
            <label
              key={p.id}
              className="radio-row"
              style={{ display: "flex", gap: 8, alignItems: "flex-start", marginBottom: 8 }}
            >
              <input
                type="radio"
                name="cpu-preset"
                checked={cpuPreset === p.id}
                onChange={() => setCpuPreset(p.id)}
              />
              <span>
                <span style={{ fontWeight: 500 }}>{p.label}</span>{" "}
                <span className="mono" style={{ color: "var(--muted)", fontSize: 12 }}>
                  ({p.mask}, {p.nprocs} ranks)
                </span>
                <div style={{ fontSize: 12, color: "var(--muted)" }}>{p.description}</div>
              </span>
            </label>
          ))}

          <label
            className="radio-row"
            style={{ display: "flex", gap: 8, alignItems: "flex-start", marginBottom: 8 }}
          >
            <input
              type="radio"
              name="cpu-preset"
              checked={isCustom}
              onChange={() => setCpuPreset("custom")}
            />
            <span style={{ flex: 1 }}>
              <span style={{ fontWeight: 500 }}>Custom</span>
              <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 6 }}>
                For a different machine. Masks are machine-specific — check{" "}
                <span className="mono">lscpu -e</span> for your topology.
              </div>
              <div className="row">
                <div className="field" style={{ margin: 0 }}>
                  <label className="label" htmlFor="cpu-mask">
                    taskset mask
                  </label>
                  <input
                    id="cpu-mask"
                    className="input mono"
                    style={{ width: 160 }}
                    value={cpuMask}
                    disabled={!isCustom}
                    onChange={(e) => setCpuMask(e.currentTarget.value)}
                    spellCheck={false}
                  />
                </div>
                <div className="field" style={{ margin: 0 }}>
                  <label className="label" htmlFor="cpu-nprocs">
                    nprocs
                  </label>
                  <input
                    id="cpu-nprocs"
                    className="input mono"
                    type="number"
                    min={1}
                    style={{ width: 90 }}
                    value={cpuNprocs}
                    disabled={!isCustom}
                    onChange={(e) => setCpuNprocs(e.currentTarget.value)}
                  />
                </div>
              </div>
            </span>
          </label>

          <div className="row" style={{ marginTop: 4 }}>
            <button className="btn btn-primary" onClick={saveCpu}>
              Save CPU settings
            </button>
            {cpuSaved ? (
              <span style={{ color: "var(--ok)", fontSize: 12 }}>Saved</span>
            ) : null}
          </div>
        </div>
      </div>

      {error ? (
        <div className="banner err" style={{ marginTop: 12 }}>
          {error}
        </div>
      ) : null}
    </div>
  );
}
