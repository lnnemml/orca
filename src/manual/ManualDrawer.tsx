//! A side drawer that shows a manual page opened from an editor hover — WITHOUT pulling
//! the author out of the editor (task 4.4). It renders the SAME `PageView` as
//! `ManualScreen` (one display component, no second copy). The hover's descriptor is
//! resolved to a DB row through `resolve_manual_section` (the keywords.json→DB bridge,
//! with a version check); the drawer then loads that row's FULL PAGE and scrolls to /
//! highlights the resolved section.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { PageView } from "./PageView";
import { orcaMapVersion, type SectionDescriptor } from "./keyword-lookup";
import { setManualOpenHandler } from "../editor/manual-open";
import { setExplainHandler, type ExplainRequest } from "../editor/explain-open";
import { clampDrawerWidth, readDrawerWidth, storeDrawerWidth } from "./drawer-width";
import type { ManualPage, ManualSection } from "./types";

export function ManualDrawer() {
  const [page, setPage] = useState<ManualPage | null>(null);
  const [targetId, setTargetId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The Claude explanation for the current selection (ADR-014 T1). Kept SEPARATE from `page`
  // so the reader always sees the border between the ORCA source and the model's interpretation.
  const [explaining, setExplaining] = useState(false);
  const [explanation, setExplanation] = useState<string | null>(null);
  const [explainError, setExplainError] = useState<string | null>(null);
  const clearExplanation = () => {
    setExplaining(false);
    setExplanation(null);
    setExplainError(null);
  };

  // Resizable left edge. The COMMITTED width is React state (drives the inline style); a
  // DRAG moves ONLY the width via direct DOM (`asideRef.style.width`) so PageView — a page
  // is up to 209 KB — is NOT re-rendered on every pointer move. Commit + persist on release.
  const asideRef = useRef<HTMLElement>(null);
  const dragging = useRef(false);
  const [width, setWidth] = useState<number>(() =>
    readDrawerWidth(window.localStorage, window.innerWidth),
  );

  const onResizeDown = (e: React.PointerEvent) => {
    e.preventDefault();
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    dragging.current = true;
  };
  const onResizeMove = (e: React.PointerEvent) => {
    if (!dragging.current || !asideRef.current) return;
    // The drawer is anchored to the right, so width = distance from the pointer to the edge.
    const w = clampDrawerWidth(window.innerWidth - e.clientX, window.innerWidth);
    asideRef.current.style.width = `${w}px`; // direct — no setState, no PageView re-render
  };
  const onResizeUp = (e: React.PointerEvent) => {
    if (!dragging.current) return;
    dragging.current = false;
    (e.target as HTMLElement).releasePointerCapture(e.pointerId);
    const w = asideRef.current
      ? Math.round(asideRef.current.getBoundingClientRect().width)
      : width;
    setWidth(w); // one re-render, matching the DOM
    storeDrawerWidth(window.localStorage, w);
  };

  // Resolve a keywords.json descriptor → its section (version-checked) and open its page.
  const openSection = async (d: SectionDescriptor): Promise<ManualSection> => {
    const section = await invoke<ManualSection>("resolve_manual_section", {
      file: d.file,
      breadcrumb: d.breadcrumb,
      title: d.title,
      nth: d.nth,
      mapVersion: orcaMapVersion, // → map_version; a stale map is reported, not resolved
    });
    setPage(await invoke<ManualPage>("get_manual_page", { file: section.file }));
    setTargetId(section.id);
    return section;
  };

  useEffect(() => {
    setManualOpenHandler(async (d: SectionDescriptor) => {
      setError(null);
      clearExplanation(); // opening a section is a fresh context — drop any prior explanation
      try {
        await openSection(d);
      } catch (e) {
        setError(String(e));
        setPage(null);
        setTargetId(null);
      }
    });

    // T1 Explain: open the section AND ask Claude, grounded ONLY in that section's body. The
    // three fields (word + line + section body) are exactly what `explain_selection` accepts.
    setExplainHandler(async (req: ExplainRequest) => {
      setError(null);
      setExplanation(null);
      setExplainError(null);
      setExplaining(true);
      try {
        const section = await openSection(req.descriptor);
        const answer = await invoke<string>("explain_selection", {
          word: req.word,
          line: req.line,
          section: section.body_md,
        });
        setExplanation(answer);
      } catch (e) {
        setExplainError(String(e)); // distinct causes come pre-worded from Rust (TASK 5)
      } finally {
        setExplaining(false);
      }
    });

    return () => {
      setManualOpenHandler(null);
      setExplainHandler(null);
    };
  }, []);

  // A cross-page cross-reference click inside the drawer → load the target file in place.
  const navigate = async (file: string, sectionId: number) => {
    setError(null);
    clearExplanation(); // navigating away leaves the explained selection's context
    try {
      setPage(await invoke<ManualPage>("get_manual_page", { file }));
      setTargetId(sectionId);
    } catch (e) {
      setError(String(e));
    }
  };

  const showExplain = explaining || explanation !== null || explainError !== null;
  if (!page && !error && !showExplain) return null;
  return (
    <aside ref={asideRef} className="manual-drawer" style={{ width }}>
      {/* Draggable left edge — resize only (col-resize); commits + persists on release. */}
      <div
        className="manual-drawer-resize"
        onPointerDown={onResizeDown}
        onPointerMove={onResizeMove}
        onPointerUp={onResizeUp}
        title="Drag to resize"
      />
      <div className="manual-drawer-head">
        <span className="muted mono">Manual</span>
        <button
          className="btn btn-sm"
          onClick={() => {
            setPage(null);
            setTargetId(null);
            setError(null);
            clearExplanation();
          }}
        >
          Close
        </button>
      </div>
      <div className="manual-drawer-body">
        {/* Claude's explanation — visually SEPARATED and LABELLED as interpretation, so the
            reader always sees the border between the ORCA source (below) and the model. */}
        {showExplain ? (
          <div className="manual-explanation">
            <div className="manual-explanation-head">
              Explained by Claude — not ORCA manual text
            </div>
            {explaining ? (
              <div className="manual-explanation-body muted">Explaining…</div>
            ) : explainError ? (
              <div className="banner err">{explainError}</div>
            ) : (
              <div className="manual-explanation-body">{explanation}</div>
            )}
          </div>
        ) : null}
        {error ? (
          <div className="banner err">{error}</div>
        ) : page ? (
          <PageView page={page} targetSectionId={targetId} onNavigate={navigate} />
        ) : null}
      </div>
    </aside>
  );
}
