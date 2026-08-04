import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { PageView } from "../manual/PageView";
import {
  SNIP_OPEN,
  SNIP_CLOSE,
  type IngestReport,
  type ManualHit,
  type ManualPage,
  type ManualStatus,
} from "../manual/types";

/** Split a snippet on the PUA match markers → alternating plain / highlighted spans.
 *  The markers occur 0× in the corpus (measured), so this never highlights real text. */
function Snippet({ text }: { text: string }) {
  const parts = text.split(new RegExp(`[${SNIP_OPEN}${SNIP_CLOSE}]`));
  return (
    <>
      {parts.map((p, i) =>
        i % 2 === 1 ? <mark key={i}>{p}</mark> : <Fragment key={i}>{p}</Fragment>,
      )}
    </>
  );
}

export function ManualScreen() {
  const [status, setStatus] = useState<ManualStatus | null>(null);
  const [checked, setChecked] = useState(false); // status fetch resolved once
  const [building, setBuilding] = useState(false);
  const [report, setReport] = useState<IngestReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<ManualHit[]>([]);
  // A search result opens the FULL PAGE; `targetId` is the hit's section, scrolled to
  // and highlighted inside it — so the reader sees where the match came from, in context.
  const [page, setPage] = useState<ManualPage | null>(null);
  const [targetId, setTargetId] = useState<number | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await invoke<ManualStatus | null>("manual_index_status"));
    } catch (e) {
      setError(String(e));
    } finally {
      setChecked(true);
    }
  }, []);

  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  const build = useCallback(async () => {
    setBuilding(true);
    setError(null);
    try {
      const r = await invoke<IngestReport>("build_manual_index", {});
      setReport(r);
      await refreshStatus();
    } catch (e) {
      setError(String(e));
    } finally {
      setBuilding(false);
    }
  }, [refreshStatus]);

  // Debounced search. Empty query → empty list (the command's contract), not an error.
  const timer = useRef<number | null>(null);
  useEffect(() => {
    if (!status) return; // no index → the Build state owns the screen
    if (timer.current) window.clearTimeout(timer.current);
    if (query.trim() === "") {
      setHits([]);
      return;
    }
    timer.current = window.setTimeout(async () => {
      try {
        setHits(await invoke<ManualHit[]>("search_manual", { query }));
      } catch (e) {
        setError(String(e));
      }
    }, 250);
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
    };
  }, [query, status]);

  const openHit = useCallback(async (hit: ManualHit) => {
    try {
      setPage(await invoke<ManualPage>("get_manual_page", { file: hit.file }));
      setTargetId(hit.id);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // A cross-page cross-reference click (same-page scroll is internal to PageView): load
  // the target file and scroll to the referenced section.
  const navigate = useCallback(async (file: string, sectionId: number) => {
    try {
      setPage(await invoke<ManualPage>("get_manual_page", { file }));
      setTargetId(sectionId);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // ── Not-built state: a clear call to action, not an empty result list. ──
  if (checked && !status) {
    return (
      <div className="screen">
        <h2 className="section-title" style={{ marginTop: 0 }}>
          Manual
        </h2>
        {error ? <div className="banner err">{error}</div> : null}
        <div className="card" style={{ maxWidth: 640 }}>
          <p>
            The ORCA manual index has not been built yet. Indexing reads the local corpus
            under <span className="mono">resources/manual/</span> and is a one-off per ORCA
            version.
          </p>
          <button className="btn btn-primary" onClick={build} disabled={building}>
            {building ? "Building index…" : "Build index"}
          </button>
          {report ? (
            <p className="mono" style={{ marginBottom: 0 }}>
              Indexed {report.section_count} sections · {report.anchors_verified} anchors
              verified · {report.null_anchors} undetermined.
            </p>
          ) : null}
        </div>
      </div>
    );
  }

  return (
    <div className="screen manual-screen">
      <div className="manual-search-col">
        <h2 className="section-title" style={{ marginTop: 0 }}>
          Manual{" "}
          {status ? (
            <span className="muted mono" style={{ fontSize: "0.8em" }}>
              {status.section_count} sections
            </span>
          ) : null}
        </h2>
        {error ? <div className="banner err">{error}</div> : null}
        <input
          className="input"
          type="text"
          placeholder="Search the ORCA manual…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          autoFocus
        />
        <div className="manual-results">
          {query.trim() === "" ? (
            <div className="empty">Type to search.</div>
          ) : hits.length === 0 ? (
            <div className="empty">No matches.</div>
          ) : (
            hits.map((h) => (
              <button
                key={h.id}
                className={"manual-result" + (targetId === h.id ? " active" : "")}
                onClick={() => openHit(h)}
              >
                <div className="manual-result-crumb mono">
                  {h.breadcrumb.length ? h.breadcrumb.join(" › ") + " › " : ""}
                  <strong>{h.title}</strong>
                </div>
                <div className="manual-result-snippet">
                  <Snippet text={h.snippet} />
                </div>
              </button>
            ))
          )}
        </div>
      </div>

      <div className="manual-view-col">
        {page ? (
          <PageView page={page} targetSectionId={targetId} onNavigate={navigate} />
        ) : (
          <div className="empty">Select a result to read the full page.</div>
        )}
      </div>
    </div>
  );
}
