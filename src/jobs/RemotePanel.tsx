import type { Job, RemoteState } from "../types";
import { remoteView, type RemoteAction, type RemoteBadge, type RemoteView } from "./remote";

const ACTION_LABEL: Record<RemoteAction, string> = {
  retry_submit: "Retry submit",
  withdraw: "Withdraw",
  retry_fetch: "Retry fetch",
};

/** One remote badge, coloured by its tone (`.badge.tone-*`). */
export function RemoteBadgeChip({ badge }: { badge: RemoteBadge }) {
  return <span className={`badge tone-${badge.tone}`}>{badge.label}</span>;
}

/**
 * A job's remote marks for the jobs table: its host, and while it is live (or its coordinates are
 * corrupt) the badges `remoteView` derives from its record. Nothing for a local job.
 */
export function RemoteJobBadges({ job, record }: { job: Job; record: RemoteState | null | undefined }) {
  const view = remoteView(job, record);
  if (view.kind === "local") return null;
  return (
    <>
      {job.remote_host ? <span className="muted mono"> @{job.remote_host}</span> : null}
      {view.kind === "terminal"
        ? null
        : view.badges.map((b) => (
            <span key={b.kind}>
              {" "}
              <RemoteBadgeChip badge={b} />
            </span>
          ))}
    </>
  );
}

interface RemotePanelProps {
  /** `remoteView`'s answer; the panel renders nothing for a local or finished job. */
  view: RemoteView;
  /** The action in flight, if any: every button is disabled meanwhile. */
  busy: RemoteAction | null;
  onAction: (action: RemoteAction) => void;
}

/**
 * The remote part of a job's detail view (ADR-024 o19.2, o19.7): the badges and the offered
 * actions decided by `remoteView`, and the row's `error_message` as the text — labelled "from the
 * last check" while the record holds no decided state. Arranges `remoteView`'s values only.
 */
export function RemotePanel({ view, busy, onAction }: RemotePanelProps) {
  if (view.kind === "local" || view.kind === "terminal") return null;
  const text = view.kind === "live" ? view.text : null;
  return (
    <div className="remote-panel" data-testid="remote-panel">
      <div className="row" style={{ gap: 6, flexWrap: "wrap" }}>
        {view.badges.map((b) => (
          <RemoteBadgeChip key={b.kind} badge={b} />
        ))}
      </div>
      {text ? (
        <div className="remote-text">
          {text.fromLastCheck ? <span className="muted">From the last check: </span> : null}
          {text.body}
        </div>
      ) : null}
      {view.actions.length ? (
        <div className="row" style={{ gap: 8, marginTop: 8 }}>
          {view.actions.map((a) => (
            <button key={a} className="btn btn-sm" disabled={busy !== null} onClick={() => onAction(a)}>
              {busy === a ? `${ACTION_LABEL[a]}…` : ACTION_LABEL[a]}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
