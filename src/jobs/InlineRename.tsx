import { useRef, useState } from "react";

import { sanitizeRenameInput } from "./rename";

interface InlineRenameProps {
  /** The current title shown when not editing, and the seed when editing starts. */
  value: string;
  /** Called with the SANITIZED (trimmed, non-empty) new title. Skipped when the input
   *  sanitizes to null (empty) or is unchanged — the Rust `rename_job` stays authoritative. */
  onCommit: (title: string) => void;
  /** Optional: called on an explicit cancel (Esc). */
  onCancel?: () => void;
  /** Class for the display wrapper (e.g. reuse the header's `section-title`). */
  className?: string;
}

/**
 * Inline title editor used at the Jobs-list row and the job-detail header. Display = the title
 * + a pencil; click the pencil (safe in a clickable row — it `stopPropagation`s so the edit
 * never triggers row-navigation) or double-click the title (for the non-clickable header) to
 * edit; the display renders an `<input>`.
 *
 * **Esc / blur are disambiguated explicitly** (the classic inline-edit trap: Esc also unmounts
 * the focused input, which fires `blur`, which would otherwise commit the cancel). A `handledRef`
 * flag records that Enter/Esc already resolved this edit, so the blur that follows the unmount is
 * a no-op:
 *   - **Enter** → commit;  **Esc** → cancel (never commits);  **blur** (click away) → commit.
 *
 * Commit is `sanitizeRenameInput(draft)`: skipped when it is null (empty — same refusal Rust
 * would give) or unchanged (a no-op rename, no needless invoke/reload — a plain equality on the
 * display value, not a guard on an invariant).
 */
export function InlineRename({ value, onCommit, onCancel, className }: InlineRenameProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  // True once Enter or Esc has resolved this edit, so the blur fired by the input's unmount is
  // ignored (prevents Enter/Esc from double-firing through the follow-on blur).
  const handledRef = useRef(false);

  const startEdit = () => {
    setDraft(value);
    handledRef.current = false;
    setEditing(true);
  };

  const finish = (doCommit: boolean) => {
    handledRef.current = true;
    setEditing(false);
    if (doCommit) {
      const sanitized = sanitizeRenameInput(draft);
      if (sanitized !== null && sanitized !== value) onCommit(sanitized);
    } else {
      onCancel?.();
    }
  };

  if (editing) {
    return (
      <input
        className="input"
        autoFocus
        value={draft}
        aria-label="Rename job title"
        onChange={(e) => setDraft(e.currentTarget.value)}
        // Never let a click inside the editor bubble to a clickable row (no row-nav on edit).
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            finish(true);
          } else if (e.key === "Escape") {
            e.preventDefault();
            finish(false);
          }
        }}
        onBlur={() => {
          if (handledRef.current) {
            handledRef.current = false;
            return; // Enter/Esc already resolved this edit — the unmount-blur is a no-op.
          }
          finish(true); // a genuine click-away commits.
        }}
      />
    );
  }

  return (
    <span className={className}>
      <span onDoubleClick={startEdit}>{value}</span>
      <button
        type="button"
        className="icon-btn"
        title="Rename"
        aria-label="Rename job title"
        style={{ marginLeft: 6 }}
        // stopPropagation so the pencil never triggers the row's open-detail click.
        onClick={(e) => {
          e.stopPropagation();
          startEdit();
        }}
      >
        ✎
      </button>
    </span>
  );
}
