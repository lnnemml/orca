import type { ReactNode } from "react";

/**
 * The editor workspace's right dock (Phase 4.2 unit 2b-ux). A **viewer-first**
 * layout: the 3Dmol canvas is the primary surface and this dock is a thin,
 * always-visible **icon rail** on its right edge; clicking a section's icon
 * **toggles** that section's body open/closed (independently of the others). When
 * no section is open the dock is just the rail, so the canvas gets the width.
 *
 * This component is **pure layout** — it holds no editor state and no model
 * logic (ADR-017 / the 2b-ux brief: nothing under `scene.ts`/`oplog.ts`/`store.ts`
 * moves). The section bodies are handed in as nodes by `NewJobScreen`, which owns
 * every panel's props and the open/closed state (session-only). The **same dock
 * is used in fullscreen** (the workspace mode), so every section — including Add
 * Fragment — is reachable without leaving fullscreen.
 *
 * Resize: opening/closing a section changes the dock's width, which changes the
 * viewer container's flex box, which fires `MoleculeViewer`'s `ResizeObserver`
 * (the one shared mechanism the split-panel resize and fullscreen already use —
 * no per-toggle `viewer.resize()` call here).
 */

export interface DockSection {
  id: string;
  /** Full name — the rail tooltip and the open-section header. */
  label: string;
  /** Short name shown under the rail glyph. */
  short: string;
  /** A single BMP glyph for the rail (kept out of an icon-font dependency). */
  glyph: string;
  /** The panel to show when the section is open. May be `null` (empty state). */
  body: ReactNode;
}

export function EditorDock({
  sections,
  open,
  onToggle,
}: {
  sections: DockSection[];
  open: Record<string, boolean>;
  onToggle: (id: string) => void;
}) {
  const openSections = sections.filter((s) => open[s.id]);

  return (
    <div className={"editor-dock" + (openSections.length ? " editor-dock-open" : "")}>
      {openSections.length ? (
        <div className="dock-body">
          {openSections.map((s) => (
            <section className="dock-section" key={s.id} aria-label={s.label}>
              <header className="dock-section-head">
                <span className="dock-section-title">{s.label}</span>
                <button
                  className="dock-section-close"
                  title={`Close ${s.label}`}
                  aria-label={`Close ${s.label}`}
                  onClick={() => onToggle(s.id)}
                >
                  ×
                </button>
              </header>
              <div className="dock-section-body">{s.body}</div>
            </section>
          ))}
        </div>
      ) : null}
      <nav className="dock-rail" aria-label="Editor sections">
        {sections.map((s) => (
          <button
            key={s.id}
            className={"dock-rail-btn" + (open[s.id] ? " active" : "")}
            title={s.label}
            aria-pressed={open[s.id] ?? false}
            onClick={() => onToggle(s.id)}
          >
            <span className="dock-glyph" aria-hidden="true">
              {s.glyph}
            </span>
            <span className="dock-rail-label">{s.short}</span>
          </button>
        ))}
      </nav>
    </div>
  );
}
