/**
 * Settings → MCP, the grant card and its editor (ADR-027 M4 amendment item 5). The card shows the
 * grant as Rust last returned it and says when it read it; it never shows a stale grant as current:
 *
 * - it re-reads `mcp_get_grant` on every tick of the section's 5 s poll (`pollTick`) and on every
 *   `job:status` (a job charged, started or finished moves `in_flight`);
 * - a failed read drops the grant it showed and shows the error instead;
 * - a read that is overtaken by a later one is discarded (`readSeq`), so a slow answer cannot put
 *   an older grant back.
 *
 * "in flight (all grants)": decision 5 counts every grant's non-terminal jobs, not just this one's.
 *
 * **Revoke** reads the grant afresh at the click and confirms it by id; it never revokes what the card
 * showed if that is no longer the active grant.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";

import type { GrantView, Job, ServerProfile } from "../types";
import {
  chargedLabel,
  grantStateLabel,
  grantTimeLabel,
  inFlightLabel,
  revokeConfirmText,
  targetsLabel,
} from "./grant";
import { McpGrantEditor } from "./McpGrantEditor";

const two = (n: number): string => String(n).padStart(2, "0");

/** The wall-clock time of a read, `HH:MM:SS` local. */
const clock = (d: Date): string => `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;

const STATE_TONE = { active: "tone-ok", expired: "tone-warn", revoked: "tone-muted" } as const;

export function McpGrantPanel({ pollTick }: { pollTick: number }) {
  // `undefined`: not read yet, or the last read failed. `null`: read, and no grant was ever made.
  const [grant, setGrant] = useState<GrantView | null | undefined>(undefined);
  const [readAt, setReadAt] = useState<Date | null>(null);
  const [grantError, setGrantError] = useState<string | null>(null);
  const [profiles, setProfiles] = useState<ServerProfile[]>([]);
  const [profilesError, setProfilesError] = useState<string | null>(null);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [jobsError, setJobsError] = useState<string | null>(null);
  const [revoking, setRevoking] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const readSeq = useRef(0);

  const applyGrant = useCallback((g: GrantView | null) => {
    setGrant(g);
    setReadAt(new Date());
    setGrantError(null);
  }, []);

  const loadGrant = useCallback(async () => {
    const seq = ++readSeq.current;
    try {
      const g = await invoke<GrantView | null>("mcp_get_grant");
      if (seq === readSeq.current) applyGrant(g);
    } catch (e) {
      if (seq !== readSeq.current) return;
      setGrant(undefined);
      setGrantError(String(e));
    }
  }, [applyGrant]);

  const loadProfiles = useCallback(async () => {
    try {
      const list = await invoke<ServerProfile[]>("list_server_profiles");
      if (!Array.isArray(list)) throw new Error("list_server_profiles did not return a list");
      setProfiles(list);
      setProfilesError(null);
    } catch (e) {
      setProfiles([]);
      setProfilesError(String(e));
    }
  }, []);

  const loadJobs = useCallback(async () => {
    try {
      const list = await invoke<Job[]>("list_jobs");
      if (!Array.isArray(list)) throw new Error("list_jobs did not return a list");
      setJobs(list);
      setJobsError(null);
    } catch (e) {
      // The picker is an aid: without jobs it offers none (and says why); pasting a level still works.
      setJobs([]);
      setJobsError(String(e));
    }
  }, []);

  // The 5 s poll: the section bumps `pollTick`; the first run is the mount.
  useEffect(() => {
    void loadGrant();
    void loadProfiles();
  }, [pollTick, loadGrant, loadProfiles]);

  // A job's status changed: the in-flight count (and the finished-job picker) moved.
  useEffect(() => {
    void loadJobs();
    const unlisten = listen("job:status", () => {
      void loadGrant();
      void loadJobs();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [loadGrant, loadJobs]);

  // A grant the editor learned of is newer than any read still in flight: those are discarded.
  const onEditorGrant = (g: GrantView | null) => {
    readSeq.current += 1;
    applyGrant(g);
  };

  const profileName = (id: string) => profiles.find((p) => p.id === id)?.name;

  const revoke = async () => {
    setRevoking(true);
    setActionError(null);
    setNotice(null);
    try {
      const fresh = await invoke<GrantView | null>("mcp_get_grant");
      readSeq.current += 1; // newer than any poll read still in flight, as `onEditorGrant`
      applyGrant(fresh);
      if (fresh?.state !== "active") {
        setNotice("There is no active grant to revoke.");
        return;
      }
      const ok = await confirm(revokeConfirmText(fresh), { title: "Revoke grant", kind: "warning" });
      if (!ok) return;
      applyGrant(await invoke<GrantView>("mcp_revoke_grant", { id: fresh.id }));
      setNotice("Grant revoked. Running jobs were not cancelled.");
    } catch (e) {
      setActionError(String(e));
    } finally {
      setRevoking(false);
    }
    void loadGrant();
  };

  return (
    <div data-testid="grant-panel" style={{ marginTop: 14, borderTop: "1px solid var(--border)", paddingTop: 10 }}>
      <label className="label">Grant</label>
      <div data-testid="grant-card" style={{ marginTop: 6, fontSize: 13 }}>
        {grantError ? (
          <span style={{ color: "var(--err)" }}>Could not read the grant: {grantError}</span>
        ) : grant === undefined ? (
          <span style={{ color: "var(--muted)" }}>Reading the grant…</span>
        ) : grant === null ? (
          <span style={{ color: "var(--muted)" }}>No grant has been made. Claude can start no job.</span>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
            <div className="row">
              <span data-testid="grant-state" className={`badge ${STATE_TONE[grant.state]}`}>
                {grantStateLabel(grant)}
              </span>
              <span data-testid="grant-time">{grantTimeLabel(grant)}</span>
              {grant.state === "active" ? (
                <button className="btn btn-sm btn-danger" onClick={() => void revoke()} disabled={revoking}>
                  {revoking ? "Revoking…" : "Revoke"}
                </button>
              ) : null}
            </div>
            <div data-testid="grant-charged">{chargedLabel(grant)}</div>
            <div data-testid="grant-in-flight">{inFlightLabel(grant)}</div>
            <div data-testid="grant-targets">Targets: {targetsLabel(grant, profileName)}</div>
            <div>
              Levels:
              {grant.levels.map((l, i) => (
                <pre key={i} data-testid="grant-card-level" className="mono" style={{ margin: "2px 0 0 12px", fontSize: 12, whiteSpace: "pre-wrap" }}>
                  {l}
                </pre>
              ))}
            </div>
          </div>
        )}
        {readAt && !grantError ? (
          <div data-testid="grant-read-at" style={{ fontSize: 11, color: "var(--muted)", marginTop: 3 }}>
            read at {clock(readAt)}
          </div>
        ) : null}
      </div>
      {actionError ? (
        <div data-testid="grant-action-error" className="banner err" style={{ marginTop: 6 }}>
          {actionError}
        </div>
      ) : null}
      {notice ? (
        <div data-testid="grant-notice" style={{ fontSize: 12, color: "var(--muted)", marginTop: 6 }}>
          {notice}
        </div>
      ) : null}
      {profilesError ? (
        <div style={{ fontSize: 12, color: "var(--err)", marginTop: 6 }}>
          Could not read the server profiles, so only "local" is offered: {profilesError}
        </div>
      ) : null}
      <McpGrantEditor profiles={profiles} jobs={jobs} jobsError={jobsError} shown={grant ?? null} onGrant={onEditorGrant} />
    </div>
  );
}
