/**
 * Settings → Servers (Phase 5 unit 5.1, ADR-023, ADR-024 n): list, add, edit and delete server
 * profiles, and run the connection test. The connection test is the only thing that verifies a
 * profile, and it runs entirely in Rust (`test_server_profile`): this component never stamps.
 * Validation errors come from Rust's save-time checks and are shown in the form as returned.
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { ConnTestReport, ServerProfile } from "../types";
import { checkLabel, outcomeSummary, profileStatus, warningText, type Tone } from "./status";

interface FormState {
  /** `null` = a new profile. */
  id: string | null;
  name: string;
  host: string;
  remoteOrcaPath: string;
  remoteScratchDir: string;
  coreMask: string;
  availabilityWindow: string;
}

const EMPTY_FORM: FormState = {
  id: null,
  name: "",
  host: "",
  remoteOrcaPath: "/opt/orca/orca",
  remoteScratchDir: "",
  coreMask: "",
  availabilityWindow: "",
};

function formOf(p: ServerProfile): FormState {
  return {
    id: p.id,
    name: p.name,
    host: p.host,
    remoteOrcaPath: p.remote_orca_path,
    remoteScratchDir: p.remote_scratch_dir,
    coreMask: p.core_mask ?? "",
    availabilityWindow: p.availability_window ?? "",
  };
}

/** An empty optional field is "not set" (NULL), never an empty string. */
function optional(value: string): string | null {
  const v = value.trim();
  return v === "" ? null : v;
}

const TONE_COLOR: Record<Tone, string> = {
  ok: "var(--ok)",
  warn: "var(--warn)",
  err: "var(--err)",
  muted: "var(--muted)",
};

export function ServersSection() {
  const [profiles, setProfiles] = useState<ServerProfile[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [form, setForm] = useState<FormState | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [testing, setTesting] = useState<Record<string, boolean>>({});
  const [reports, setReports] = useState<Record<string, ConnTestReport>>({});
  const [testErrors, setTestErrors] = useState<Record<string, string>>({});

  const load = useCallback(async () => {
    try {
      setProfiles(await invoke<ServerProfile[]>("list_server_profiles"));
      setLoadError(null);
    } catch (e) {
      setLoadError(String(e));
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const forget = (id: string) => {
    setReports(({ [id]: _, ...rest }) => rest);
    setTestErrors(({ [id]: _, ...rest }) => rest);
  };

  const save = async () => {
    if (!form) return;
    setSaving(true);
    setFormError(null);
    const fields = {
      name: form.name,
      host: form.host,
      remoteOrcaPath: form.remoteOrcaPath,
      remoteScratchDir: form.remoteScratchDir,
      coreMask: optional(form.coreMask),
      availabilityWindow: optional(form.availabilityWindow),
    };
    try {
      if (form.id === null) {
        await invoke("create_server_profile", fields);
      } else {
        await invoke("update_server_profile", { id: form.id, ...fields });
        // An old report describes the profile before this edit.
        forget(form.id);
      }
      setForm(null);
      await load();
    } catch (e) {
      setFormError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (id: string) => {
    setConfirmDelete(null);
    try {
      await invoke("delete_server_profile", { id });
      forget(id);
      if (form?.id === id) setForm(null);
      await load();
    } catch (e) {
      setLoadError(String(e));
    }
  };

  const test = async (id: string) => {
    setTesting((t) => ({ ...t, [id]: true }));
    forget(id);
    try {
      const report = await invoke<ConnTestReport>("test_server_profile", { id });
      setReports((r) => ({ ...r, [id]: report }));
      setProfiles((ps) => ps.map((p) => (p.id === id ? report.profile : p)));
    } catch (e) {
      setTestErrors((t) => ({ ...t, [id]: String(e) }));
      // The stamp may have changed even when the call errored; show the stored state.
      await load();
    } finally {
      setTesting((t) => ({ ...t, [id]: false }));
    }
  };

  const field = (
    key: keyof Omit<FormState, "id">,
    label: string,
    placeholder: string,
    hint?: string,
  ) =>
    form ? (
      <div className="field" style={{ marginBottom: 8 }}>
        <label className="label" htmlFor={`server-${key}`}>
          {label}
        </label>
        <input
          id={`server-${key}`}
          className="input mono"
          value={form[key]}
          placeholder={placeholder}
          spellCheck={false}
          onChange={(e) => {
            const value = e.currentTarget.value;
            setForm((f) => (f ? { ...f, [key]: value } : f));
          }}
        />
        {hint ? (
          <div style={{ fontSize: 12, color: "var(--muted)", marginTop: 2 }}>{hint}</div>
        ) : null}
      </div>
    ) : null;

  return (
    <div className="card" style={{ marginTop: 12 }}>
      <div className="field">
        <label className="label">Servers</label>
        <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 8 }}>
          Remote ORCA hosts, reached with your own <span className="mono">~/.ssh/config</span>{" "}
          alias (no credentials are stored here). A profile becomes a run target only after a
          passing connection test, and only with a core mask inside the measured cores.
        </div>

        {profiles.length === 0 && !loadError ? (
          <div style={{ fontSize: 12, color: "var(--muted)" }}>No servers yet.</div>
        ) : null}

        {profiles.map((p) => {
          const status = profileStatus(p);
          const report = reports[p.id];
          const testError = testErrors[p.id];
          const busy = testing[p.id] === true;
          return (
            <div
              key={p.id}
              data-testid={`server-${p.id}`}
              style={{ borderTop: "1px solid var(--border)", padding: "8px 0" }}
            >
              <div className="row" style={{ justifyContent: "space-between" }}>
                <span>
                  <span style={{ fontWeight: 500 }}>{p.name}</span>{" "}
                  <span className="mono" style={{ color: "var(--muted)", fontSize: 12 }}>
                    {p.host}
                  </span>
                </span>
                <span className="row">
                  <button className="btn" onClick={() => test(p.id)} disabled={busy}>
                    {busy ? (
                      <>
                        <span className="spinner" aria-hidden="true" /> Testing…
                      </>
                    ) : (
                      "Test connection"
                    )}
                  </button>
                  <button
                    className="btn"
                    onClick={() => {
                      setFormError(null);
                      setForm(formOf(p));
                    }}
                    disabled={busy}
                  >
                    Edit
                  </button>
                  {confirmDelete === p.id ? (
                    <>
                      <button className="btn btn-danger" onClick={() => remove(p.id)}>
                        Confirm delete
                      </button>
                      <button className="btn" onClick={() => setConfirmDelete(null)}>
                        Cancel
                      </button>
                    </>
                  ) : (
                    <button
                      className="btn btn-danger"
                      onClick={() => setConfirmDelete(p.id)}
                      disabled={busy}
                    >
                      Delete
                    </button>
                  )}
                </span>
              </div>
              <div className="mono" style={{ fontSize: 12, color: "var(--muted)" }}>
                {p.remote_orca_path} · root {p.remote_scratch_dir} · mask{" "}
                {p.core_mask ?? "(none)"} · slots {p.slot_count}
                {p.availability_window ? ` · window ${p.availability_window} (local time)` : ""}
              </div>
              <div
                data-testid="run-target"
                style={{ fontSize: 12, color: TONE_COLOR[status.runTarget.tone] }}
              >
                {status.runTarget.label}
              </div>
              <div
                data-testid="verification"
                style={{ fontSize: 12, color: TONE_COLOR[status.verification.tone] }}
              >
                {status.verification.label}
                {p.verified_at
                  ? ` · ORCA ${p.orca_version ?? "?"} · OpenMPI ${p.openmpi_version ?? "not reported"} · ${p.core_count ?? "?"} CPUs`
                  : ""}
              </div>
              {testError ? (
                <div data-testid="test-error" className="banner err" style={{ marginTop: 6 }}>
                  {testError}
                </div>
              ) : null}
              {report ? <ReportView report={report} /> : null}
            </div>
          );
        })}

        {form ? (
          <div
            data-testid="server-form"
            style={{ borderTop: "1px solid var(--border)", paddingTop: 8 }}
          >
            {field("name", "Name", "university cluster")}
            {field(
              "host",
              "SSH host alias",
              "uni",
              "A Host entry of your ~/.ssh/config: letters, digits and . _ @ - only.",
            )}
            {field("remoteOrcaPath", "Remote ORCA path (absolute)", "/opt/orca/orca")}
            {field(
              "remoteScratchDir",
              "Remote root",
              "/home/<user>/.orcastudio",
              "Job directories, the tsp sockets and the scripts live under it. The test creates it.",
            )}
            {field("coreMask", "Core mask (taskset list)", "0-23", "Leave empty until measured.")}
            {field(
              "availabilityWindow",
              "Availability window (local time, optional)",
              "08:00-22:00",
            )}
            <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 8 }}>
              Slots: 1 (fixed until per-slot masks are measured).
              {form.id !== null
                ? " Changing the host, ORCA path, root or core mask clears the verification."
                : ""}
            </div>
            {formError ? (
              <div data-testid="form-error" className="banner err" style={{ marginBottom: 8 }}>
                {formError}
              </div>
            ) : null}
            <div className="row">
              <button className="btn btn-primary" onClick={save} disabled={saving}>
                {saving ? "Saving…" : form.id === null ? "Add server" : "Save"}
              </button>
              <button
                className="btn"
                onClick={() => {
                  setForm(null);
                  setFormError(null);
                }}
                disabled={saving}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <div className="row" style={{ marginTop: 8 }}>
            <button
              className="btn"
              onClick={() => {
                setFormError(null);
                setForm(EMPTY_FORM);
              }}
            >
              Add server
            </button>
          </div>
        )}

        {loadError ? (
          <div className="banner err" style={{ marginTop: 8 }}>
            {loadError}
          </div>
        ) : null}
      </div>
    </div>
  );
}

function ReportView({ report }: { report: ConnTestReport }) {
  const summary = outcomeSummary(report);
  const checks = report.outcome === "failed" ? [] : report.checks;
  const facts =
    report.outcome === "verified" || report.outcome === "conflict" ? report.facts : null;
  const warnings = report.outcome === "failed" ? [] : report.warnings;
  return (
    <div data-testid="test-report" style={{ marginTop: 6, fontSize: 12 }}>
      <div data-testid="test-summary" style={{ color: TONE_COLOR[summary.tone] }}>
        {summary.label}{" "}
        <span style={{ color: "var(--muted)" }}>({(report.elapsed_ms / 1000).toFixed(1)} s)</span>
      </div>
      {checks.length ? (
        <ul style={{ margin: "4px 0", paddingLeft: 18 }}>
          {checks.map((c) => (
            <li key={c.check} data-testid={`check-${c.check}`}>
              <span style={{ color: c.passed ? "var(--ok)" : "var(--err)" }}>
                {c.passed ? "pass" : "FAIL"}
              </span>{" "}
              {checkLabel(c.check)}
              {c.reason ? <span style={{ color: "var(--muted)" }}> — {c.reason}</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
      {facts ? (
        <div data-testid="test-facts" className="mono" style={{ color: "var(--muted)" }}>
          Measured: ORCA {facts.orca_version} · OpenMPI {facts.openmpi_version ?? "not reported"} ·{" "}
          {facts.core_count} CPUs
        </div>
      ) : null}
      {warnings.map((w, i) => (
        <div key={i} data-testid="test-warning" style={{ color: "var(--warn)" }}>
          Warning: {warningText(w)}
        </div>
      ))}
    </div>
  );
}
