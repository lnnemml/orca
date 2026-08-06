import { useEffect } from "react";

import { useSceneStore } from "./store";
import { canRedo, canUndo, describeInScene } from "./oplog";

/**
 * The operation-log history panel (Phase 4.2 Stage 2 unit 2b) — the provenance
 * payoff. A **read-only list of `describe()` lines**, the current step highlighted,
 * a click **jumps the pointer** (the same undo/redo mechanism, not a new one), plus
 * Undo/Redo buttons and Cmd/Ctrl+Z / Cmd/Ctrl+Shift+Z hotkeys.
 *
 * It is deliberately *only* a list + a click + the two buttons (ADR-017 / the 2b
 * brief): it reads the store's log and calls `undo`/`redo`/`jumpTo` — no local
 * state of its own, no second notion of "the current scene".
 */
export function HistoryPanel() {
  const log = useSceneStore((s) => s.log);
  const undo = useSceneStore((s) => s.undo);
  const redo = useSceneStore((s) => s.redo);
  const jumpTo = useSceneStore((s) => s.jumpTo);

  // Cmd/Ctrl+Z = undo, +Shift = redo. Skipped when focus is in a text field or
  // Monaco (a <textarea>) so the editor keeps its own undo stack — the log's undo
  // is for geometry edits made through the rail, not for typing in the input.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "z") return;
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)
      ) {
        return;
      }
      e.preventDefault();
      if (e.shiftKey) redo();
      else undo();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undo, redo]);

  if (log.entries.length === 0) return null;

  return (
    <div className="history-panel" style={{ marginTop: 8 }}>
      <div
        className="history-head"
        style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 4 }}
      >
        <span style={{ fontWeight: 600, fontSize: 12, opacity: 0.8, flex: 1 }}>
          History
        </span>
        <button
          className="btn btn-sm"
          disabled={!canUndo(log)}
          onClick={() => undo()}
          title="Undo (Ctrl/Cmd+Z)"
        >
          Undo
        </button>
        <button
          className="btn btn-sm"
          disabled={!canRedo(log)}
          onClick={() => redo()}
          title="Redo (Ctrl/Cmd+Shift+Z)"
        >
          Redo
        </button>
      </div>
      <ol
        className="history-list"
        style={{ listStyle: "none", margin: 0, padding: 0, fontSize: 12 }}
      >
        {log.entries.map((entry, i) => {
          const current = i === log.pointer;
          const undone = i > log.pointer;
          return (
            <li
              key={i}
              onClick={() => jumpTo(i)}
              title="Jump to this step"
              style={{
                cursor: "pointer",
                padding: "2px 6px",
                borderLeft: current ? "3px solid var(--accent, #4a9)" : "3px solid transparent",
                opacity: undone ? 0.45 : 1,
                fontWeight: current ? 600 : 400,
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {describeInScene(entry.op, entry.scene)}
            </li>
          );
        })}
      </ol>
    </div>
  );
}
