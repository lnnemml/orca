import type { Job, RemoteState } from "../types";
import { remoteView, type RemoteBadge, type RemoteOffer, type RemoteView, type RetryAction } from "./remote";

const ACTION_LABEL: Record<RetryAction, string> = {
  retry_submit: "Retry submit",
  retry_fetch: "Retry fetch",
};

/** The panel's buttons: the retries. The cancel family (Cancel, Retry cancel, Undo cancel) is the
 *  screen's own Cancel button (`sendCancel`); its buttons in the panel come with 5.4a Part B2. */
function retryOffers(actions: readonly RemoteOffer[]): (RemoteOffer & { action: RetryAction })[] {
  return actions.filter((o): o is RemoteOffer & { action: RetryAction } => o.action === "retry_submit" || o.action === "retry_fetch");
}

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
  busy: RetryAction | null;
  onAction: (action: RetryAction) => void;
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
      {retryOffers(view.actions).length ? (
        <div className="row" style={{ gap: 8, marginTop: 8 }}>
          {retryOffers(view.actions).map((o) => (
            <button
              key={o.action}
              className="btn btn-sm"
              disabled={busy !== null || o.disabled !== null}
              title={o.disabled ?? undefined}
              onClick={() => onAction(o.action)}
            >
              {busy === o.action ? `${ACTION_LABEL[o.action]}…` : ACTION_LABEL[o.action]}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
