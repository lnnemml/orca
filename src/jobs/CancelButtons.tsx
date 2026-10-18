import { cancelButtons, type CancelBusy, type CancelFamilyAction } from "./remote";

interface CancelButtonsProps {
  /** The offered actions — a `rowActions` list or a `remoteView`'s offers; only the cancel family
   *  is drawn from them. */
  actions: readonly { action: string }[];
  /** Which click of the family is in flight, if any: every button is disabled meanwhile. */
  busy: CancelBusy;
  /** The clicked button's action; the screen hands it, with the job id, to `clickCancelFamily` —
   *  which reads the row and record from the backend and derives the offer itself (o22.14.14.1). */
  onAction: (action: CancelFamilyAction) => void;
  /** Another action of the view is in flight (a Retry): disable the family too. */
  locked?: boolean;
  /** In a clickable table row the click must not open the row. */
  stopPropagation?: boolean;
}

/**
 * The cancel family's buttons (ADR-024 o22.14.1–2): Cancel, Retry cancel, Undo cancel — each
 * labelled by its own offer (`cancelButtons`). Nothing when the row offers none of them.
 */
export function CancelButtons({ actions, busy, onAction, locked, stopPropagation }: CancelButtonsProps) {
  return (
    <>
      {cancelButtons(actions, busy, locked).map((b) => (
        <button
          key={b.action}
          className="btn btn-sm"
          disabled={b.disabled}
          onClick={(e) => {
            if (stopPropagation) e.stopPropagation();
            onAction(b.action);
          }}
        >
          {b.label}
        </button>
      ))}
    </>
  );
}
