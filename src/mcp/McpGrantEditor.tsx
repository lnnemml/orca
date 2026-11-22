/**
 * Settings → MCP, the grant editor (ADR-027 M4 amendment item 5). The form collects; Rust decides: a
 * bad level, a count below 1, an empty target or level list, an expiry not after now are refused by
 * `mcp_create_grant` and the refusal is shown as returned. TypeScript does not re-implement the level
 * grammar. What it does check is what it must to build the call: the counts are integers, and the
 * expiry reads back as the local time it shows (`expiry.ts`: a time skipped by a DST change is
 * refused, and Create stays disabled).
 *
 * **Create goes through `createGrantFlow`** (`grant.ts`): it reads the grant afresh at the click and
 * asks the replace confirm only if that read shows an active grant. This component never decides
 * that itself.
 */
import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";

import { formatLaptopUtc } from "../format";
import type { GrantView, Job, ServerProfile } from "../types";
import {
  DEFAULT_EXPIRY_PRESET,
  EXPIRY_PRESETS,
  customExpiry,
  expiryShownText,
  presetHint,
  resolveExpiry,
  type ExpiryChoice,
  type ExpiryPreset,
} from "./expiry";
import { DEFAULT_MAX_IN_FLIGHT, DEFAULT_MAX_JOBS, createGrantFlow } from "./grant";

const two = (n: number): string => String(n).padStart(2, "0");
/** The custom time's hour select (00–23) and minute select (00–55, steps of 5). */
const HOURS = Array.from({ length: 24 }, (_, i) => two(i));
const MINUTES = Array.from({ length: 12 }, (_, i) => two(i * 5));
const DEFAULT_CUSTOM_HOUR = "12";
const DEFAULT_CUSTOM_MINUTE = "00";

/**
 * The expiry the three custom inputs and the preset make. A blank date is no custom time (the preset
 * rules); otherwise the date text, a space and the selects' `HH:MM` go to `customExpiry` unchanged, so
 * a malformed date is refused there and never by a second parser here.
 */
export function composeExpiryChoice(preset: ExpiryPreset, date: string, hour: string, minute: string): ExpiryChoice {
  const d = date.trim();
  return d === "" ? { kind: "preset", preset } : { kind: "custom", value: `${d} ${hour}:${minute}` };
}

/** A button label stays on one line; the row it sits in wraps instead. */
const NOWRAP = { whiteSpace: "nowrap" } as const;

/** How many finished jobs the "this job's level" picker offers (newest first). */
const PICKER_LIMIT = 30;

/** Jobs whose input can give a level: finished ones, newest first. */
export function finishedJobs(jobs: readonly Job[]): Job[] {
  return jobs
    .filter((j) => j.status === "completed" || j.status === "parsed")
    .sort((a, b) => (a.created_at < b.created_at ? 1 : a.created_at > b.created_at ? -1 : 0))
    .slice(0, PICKER_LIMIT);
}

/** A count field's value as an integer ≥ 1, or `null`. */
function wholeCount(text: string): number | null {
  if (!/^\d+$/.test(text.trim())) return null;
  const n = Number(text.trim());
  return Number.isSafeInteger(n) && n >= 1 ? n : null;
}

interface Props {
  profiles: ServerProfile[];
  jobs: Job[];
  /** Why the finished-job picker has no jobs to offer, when `list_jobs` could not be read. */
  jobsError: string | null;
  /** What the grant card shows now; it decides nothing, and only annotates the replace confirm. */
  shown: GrantView | null;
  /** A grant the editor learned of (created, or the fresh read behind a declined replace). */
  onGrant: (grant: GrantView | null) => void;
}

export function McpGrantEditor({ profiles, jobs, jobsError, shown, onGrant }: Props) {
  const [targets, setTargets] = useState<string[]>([]);
  const [levels, setLevels] = useState<string[]>([]);
  const [levelDraft, setLevelDraft] = useState("");
  const [pickedJob, setPickedJob] = useState("");
  const [levelError, setLevelError] = useState<string | null>(null);
  /** `mcp_level_from_job` is pending: the button is disabled, so a double-click adds one level. */
  const [adding, setAdding] = useState(false);
  const [maxJobs, setMaxJobs] = useState(String(DEFAULT_MAX_JOBS));
  const [maxInFlight, setMaxInFlight] = useState(String(DEFAULT_MAX_IN_FLIGHT));
  // The expiry's state is the editor's own, so the section's poll cannot reset what is being typed.
  const [preset, setPreset] = useState<ExpiryPreset>(DEFAULT_EXPIRY_PRESET);
  const [customDate, setCustomDate] = useState("");
  const [customHour, setCustomHour] = useState(DEFAULT_CUSTOM_HOUR);
  const [customMinute, setCustomMinute] = useState(DEFAULT_CUSTOM_MINUTE);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [info, setInfo] = useState<string | null>(null);

  const pickable = useMemo(() => finishedJobs(jobs), [jobs]);

  // A profile deleted since it was checked is no longer offered, so it is no longer sent.
  const known = new Set(["local", ...profiles.map((p) => p.id)]);
  const chosenTargets = targets.filter((t) => known.has(t));
  const toggleTarget = (id: string, on: boolean) =>
    setTargets((prev) => (on ? [...prev.filter((t) => t !== id), id] : prev.filter((t) => t !== id)));

  const jobsCount = wholeCount(maxJobs);
  const inFlightCount = wholeCount(maxInFlight);

  const now = new Date();
  // An empty date is no custom time: the preset rules. Anything typed in it is a custom time, read by
  // the strict helper as `YYYY-MM-DD HH:MM` (the hour and minute come from the selects).
  const choice = composeExpiryChoice(preset, customDate, customHour, customMinute);
  const custom = choice.kind === "custom" ? customExpiry(choice.value) : null;
  const expiryLine =
    choice.kind === "preset"
      ? { ok: true as const, text: presetHint(choice.preset, now) }
      : custom && custom.ok
        ? { ok: true as const, text: expiryShownText(custom) }
        : { ok: false as const, text: custom && !custom.ok ? custom.error : "" };

  const canCreate = !creating && jobsCount !== null && inFlightCount !== null && expiryLine.ok;

  const addPasted = () => {
    const text = levelDraft.trim();
    if (text === "") return;
    setLevels((prev) => [...prev, text]);
    setLevelDraft("");
    setLevelError(null);
  };

  const addFromJob = async () => {
    if (pickedJob === "") return;
    setLevelError(null);
    setAdding(true);
    try {
      const text = await invoke<string>("mcp_level_from_job", { jobId: pickedJob });
      setLevels((prev) => [...prev, text]);
    } catch (e) {
      setLevelError(String(e));
    } finally {
      setAdding(false);
    }
  };

  const create = async () => {
    if (jobsCount === null || inFlightCount === null) return;
    // The preset is counted from this moment; a custom time is read back again here (rule #9).
    const expiry = resolveExpiry(choice, new Date());
    if (!expiry.ok) {
      setCreateError(expiry.error);
      return;
    }
    setCreating(true);
    setCreateError(null);
    setInfo(null);
    try {
      const outcome = await createGrantFlow(
        {
          readGrant: () => invoke<GrantView | null>("mcp_get_grant"),
          confirm: (text) => confirm(text, { title: "Replace the active grant", kind: "warning" }),
          create: (args) => invoke<GrantView>("mcp_create_grant", { ...args }),
        },
        { targets: chosenTargets, levels, maxJobs: jobsCount, maxInFlight: inFlightCount, expiresAt: expiry.utc },
        shown,
      );
      if (outcome.kind === "created") {
        onGrant(outcome.grant);
        setInfo(`Grant created, expires ${formatLaptopUtc(outcome.grant.expires_at)}.`);
      } else {
        onGrant(outcome.fresh);
        setInfo("The active grant was not replaced.");
      }
    } catch (e) {
      setCreateError(String(e));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div data-testid="grant-editor" style={{ borderTop: "1px solid var(--border)", marginTop: 10, paddingTop: 8 }}>
      <label className="label">New grant</label>

      <div className="field" style={{ marginTop: 6 }}>
        <span style={{ fontSize: 12, color: "var(--muted)" }}>Targets Claude may run on</span>
        <label className="radio-row" style={{ display: "flex", gap: 8 }}>
          <input
            type="checkbox"
            checked={chosenTargets.includes("local")}
            onChange={(e) => toggleTarget("local", e.currentTarget.checked)}
          />
          <span>local</span>
        </label>
        {profiles.map((p) => (
          <label key={p.id} className="radio-row" style={{ display: "flex", gap: 8 }}>
            <input
              type="checkbox"
              checked={chosenTargets.includes(p.id)}
              onChange={(e) => toggleTarget(p.id, e.currentTarget.checked)}
            />
            <span>
              {p.name} <span className="mono" style={{ color: "var(--muted)", fontSize: 12 }}>{p.host}</span>
            </span>
          </label>
        ))}
      </div>

      <div className="field" style={{ marginTop: 10 }}>
        <span style={{ fontSize: 12, color: "var(--muted)" }}>
          Levels Claude may use: a <span className="mono">!</span> line, then an optional solvation block
        </span>
        {levels.map((text, i) => (
          <div key={i} className="row" style={{ justifyContent: "space-between", alignItems: "flex-start" }}>
            <pre data-testid="grant-level" className="mono" style={{ margin: 0, fontSize: 12, whiteSpace: "pre-wrap" }}>
              {text}
            </pre>
            <button
              className="btn btn-sm"
              onClick={() => setLevels((prev) => prev.filter((_, j) => j !== i))}
              aria-label={`Remove level ${i + 1}`}
            >
              Remove
            </button>
          </div>
        ))}
        <textarea
          className="input mono"
          rows={3}
          value={levelDraft}
          placeholder={"! B3LYP D4 def2-SVP\n%cpcm\n  smd true\n  SMDsolvent \"water\"\nend"}
          aria-label="Level to add"
          onChange={(e) => setLevelDraft(e.currentTarget.value)}
        />
        {/* Wraps inside the card: the select flexes (and shrinks), both buttons keep one line. */}
        <div className="row" style={{ flexWrap: "wrap" }}>
          <button className="btn btn-sm" style={NOWRAP} onClick={addPasted} disabled={levelDraft.trim() === ""}>
            Add level
          </button>
          <span style={{ color: "var(--muted)", fontSize: 12 }}>or</span>
          <select
            className="input"
            style={{ flex: "1 1 180px", minWidth: 0, maxWidth: "100%" }}
            value={pickedJob}
            aria-label="Finished job"
            onChange={(e) => setPickedJob(e.currentTarget.value)}
          >
            <option value="">this job's level…</option>
            {pickable.map((j) => (
              <option key={j.id} value={j.id}>
                {(j.title || j.id) + " · " + formatLaptopUtc(j.created_at)}
              </option>
            ))}
          </select>
          <button
            className="btn btn-sm"
            style={NOWRAP}
            onClick={() => void addFromJob()}
            disabled={pickedJob === "" || adding}
          >
            Add this job's level
          </button>
        </div>
        {jobsError ? (
          <div data-testid="jobs-error" style={{ fontSize: 12, color: "var(--err)" }}>
            Could not read the jobs, so the picker offers none: {jobsError}
          </div>
        ) : null}
        {levelError ? (
          <div data-testid="level-error" style={{ fontSize: 12, color: "var(--err)" }}>
            {levelError}
          </div>
        ) : null}
      </div>

      <div className="row" style={{ marginTop: 10, alignItems: "flex-start" }}>
        <div className="field">
          <label className="label" htmlFor="grant-max-jobs">
            max_jobs
          </label>
          <input
            id="grant-max-jobs"
            className="input"
            type="number"
            min={1}
            step={1}
            value={maxJobs}
            onChange={(e) => setMaxJobs(e.currentTarget.value)}
            style={{ width: 90 }}
          />
          {jobsCount === null ? <span style={{ fontSize: 12, color: "var(--err)" }}>a whole number, at least 1</span> : null}
        </div>
        <div className="field">
          <label className="label" htmlFor="grant-max-in-flight">
            max_in_flight
          </label>
          <input
            id="grant-max-in-flight"
            className="input"
            type="number"
            min={1}
            step={1}
            value={maxInFlight}
            onChange={(e) => setMaxInFlight(e.currentTarget.value)}
            style={{ width: 90 }}
          />
          {inFlightCount === null ? (
            <span style={{ fontSize: 12, color: "var(--err)" }}>a whole number, at least 1</span>
          ) : null}
        </div>
      </div>

      <div className="field" style={{ marginTop: 10 }}>
        <span style={{ fontSize: 12, color: "var(--muted)" }}>Expires</span>
        <div className="row" style={{ flexWrap: "wrap" }}>
          {EXPIRY_PRESETS.map((p) => (
            <button
              key={p.id}
              className={choice.kind === "preset" && choice.preset === p.id ? "btn btn-sm btn-primary" : "btn btn-sm"}
              aria-pressed={choice.kind === "preset" && choice.preset === p.id}
              onClick={() => {
                setPreset(p.id);
                setCustomDate("");
              }}
            >
              {p.label}
            </button>
          ))}
          <span style={{ color: "var(--muted)", fontSize: 12 }}>or</span>
          {/* Not `datetime-local`: WebKitGTK's native picker hangs (M4a live gate, 2026-11-22). */}
          <label htmlFor="grant-expiry-date" style={{ fontSize: 12, color: "var(--muted)" }}>
            date (local)
          </label>
          <input
            id="grant-expiry-date"
            className="input"
            type="text"
            placeholder="YYYY-MM-DD"
            value={customDate}
            onChange={(e) => setCustomDate(e.currentTarget.value)}
            style={{ width: 120 }}
          />
          <select
            className="input"
            aria-label="Expiry hour"
            value={customHour}
            onChange={(e) => setCustomHour(e.currentTarget.value)}
          >
            {HOURS.map((h) => (
              <option key={h} value={h}>
                {h}
              </option>
            ))}
          </select>
          <span style={{ color: "var(--muted)" }}>:</span>
          <select
            className="input"
            aria-label="Expiry minute"
            value={customMinute}
            onChange={(e) => setCustomMinute(e.currentTarget.value)}
          >
            {MINUTES.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </div>
        <div
          data-testid="expiry-line"
          style={{ fontSize: 12, color: expiryLine.ok ? "var(--muted)" : "var(--err)" }}
        >
          {expiryLine.text}
        </div>
      </div>

      {createError ? (
        <div data-testid="create-error" className="banner err" style={{ marginTop: 8 }}>
          {createError}
        </div>
      ) : null}
      {info ? (
        <div data-testid="create-info" style={{ fontSize: 12, color: "var(--ok)", marginTop: 8 }}>
          {info}
        </div>
      ) : null}
      <div className="row" style={{ marginTop: 8 }}>
        <button className="btn btn-primary" onClick={() => void create()} disabled={!canCreate}>
          {creating ? "Creating…" : "Create grant"}
        </button>
        <span style={{ fontSize: 12, color: "var(--muted)" }}>Creating a grant revokes the active one.</span>
      </div>
    </div>
  );
}
