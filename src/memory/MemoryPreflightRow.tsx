import { useEffect, useId, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { MemoryEstimateReply } from "../types";
import { isLatestReply, memorySummary, verdictLines } from "./preflight";

/** The two Rust calls the preflight makes; a seam for the tests. */
export interface MemoryBridge {
  estimate(seq: number, input: string, target: string | null): Promise<MemoryEstimateReply>;
  applyMaxcore(input: string, maxcoreMib: number): Promise<string>;
}

export const tauriMemoryBridge: MemoryBridge = {
  estimate: (seq, input, target) => invoke<MemoryEstimateReply>("memory_estimate", { seq, input, target }),
  applyMaxcore: (input, maxcoreMib) => invoke<string>("apply_maxcore", { input, maxcoreMib }),
};

/** The debounce of a recomputation after an edit or a Run On change. */
export const ESTIMATE_DEBOUNCE_MS = 400;

/**
 * `memory_estimate` for the buffer and the selected target, debounced; every request carries the
 * next sequence number and only the reply to the latest one is applied (o26.13.6), so a slow older
 * answer never overwrites a newer one. An empty buffer asks nothing.
 */
export function useMemoryEstimate(
  input: string,
  target: string | null,
  bridge: MemoryBridge = tauriMemoryBridge,
  debounceMs: number = ESTIMATE_DEBOUNCE_MS,
): { reply: MemoryEstimateReply | null; error: string | null } {
  const [reply, setReply] = useState<MemoryEstimateReply | null>(null);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef(0);
  useEffect(() => {
    const seq = ++latest.current;
    if (input.trim().length === 0) {
      setReply(null);
      setError(null);
      return;
    }
    const timer = setTimeout(() => {
      bridge.estimate(seq, input, target).then(
        (r) => {
          if (!isLatestReply(latest.current, r)) return;
          setReply(r);
          setError(null);
        },
        (e) => {
          if (seq === latest.current) setError(String(e));
        },
      );
    }, debounceMs);
    return () => clearTimeout(timer);
  }, [input, target, bridge, debounceMs]);
  return { reply, error };
}

interface MemoryPreflightRowProps {
  /** The latest applied reply of {@link useMemoryEstimate} (null: none yet, or an empty buffer). */
  reply: MemoryEstimateReply | null;
  /** The last request's failure, if it failed. */
  error: string | null;
  /** The editor buffer, the button's input. */
  input: string;
  /** The selected server's display name (the line's label). */
  targetName: string | null;
  /** Replace the editor buffer (the button's result). */
  onInputChange: (text: string) => void;
  bridge?: MemoryBridge;
}

/**
 * The New Job screen's memory and disk warning (ADR-024 o26.7): a non-blocking `role="status"` row
 * like the parity warning — it never disables Create or Create & Run. **Collapsed by default**: one
 * summary line (target, U and threshold per warning; "uni: memory unknown" for an unread RAM) that
 * is a `<button aria-expanded>`; its click (a native button: Enter and Space too) opens the lines
 * below, per visit and unpersisted. One line for this machine; with
 * a server selected, two (o26.13.1): "Create (runs locally)" and "Create & Run on <server>". Each line
 * gives U (none when the RAM is unknown, o26.14.3), the RAM, for a server when it was measured, the
 * free disk, and — when the RAM warns — the button "Set %maxcore to P", which replaces the buffer
 * with Rust's `apply_maxcore` text. Hidden when nothing warns and every fact is known.
 */
export function MemoryPreflightRow({
  reply,
  error,
  input,
  targetName,
  onInputChange,
  bridge = tauriMemoryBridge,
}: MemoryPreflightRowProps) {
  const [applyError, setApplyError] = useState<string | null>(null);
  // Closed by default, per New Job visit (no persistence): the long lines are one click away.
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const lines = verdictLines(reply, targetName);
  const summary = memorySummary(reply, targetName);
  const warns = reply != null && (reply.local.warns || (reply.server?.warns ?? false));

  const apply = async (p: number) => {
    setApplyError(null);
    try {
      onInputChange(await bridge.applyMaxcore(input, p));
    } catch (e) {
      setApplyError(String(e));
    }
  };

  if (lines.length === 0 && !error && !applyError) return null;
  return (
    <div className="memory-preflight" role="status" data-testid="memory-preflight">
      {summary ? (
        <button
          type="button"
          className={`memory-summary${warns ? " memory-warns" : ""}`}
          aria-expanded={open}
          aria-controls={detailsId}
          data-testid="memory-summary"
          onClick={() => setOpen((o) => !o)}
        >
          <span className="memory-summary-text">{summary}</span>
          <span className="memory-summary-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>
        </button>
      ) : null}
      {open && lines.length > 0 ? (
        <div id={detailsId} className="memory-details" data-testid="memory-details">
          {lines.map((l) => (
            <div key={l.key} className={`memory-line memory-${l.verdict}`} data-testid={`memory-line-${l.key}`}>
              <strong>
                {l.verdict === "warns" ? "⚠ " : ""}
                {l.label}:
              </strong>{" "}
              {l.estimate ? <span data-testid="memory-estimate">{l.estimate}; </span> : null}
              {l.reason ? <span data-testid="memory-reason">{l.reason}; </span> : null}
              <span>{l.ram}</span>
              {l.measured ? <span> ({l.measured})</span> : null}
              <span>; {l.disk}</span>
              {l.tooSmall ? <span>; {l.tooSmall}</span> : null}
              {l.proposal != null ? (
                <button className="btn btn-small memory-apply" onClick={() => void apply(l.proposal as number)}>
                  Set %maxcore to {l.proposal}
                </button>
              ) : null}
            </div>
          ))}
        </div>
      ) : null}
      {error ? <div className="muted">Memory check unavailable: {error}</div> : null}
      {applyError ? <div className="muted">Could not set %maxcore: {applyError}</div> : null}
    </div>
  );
}
