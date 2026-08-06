import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { Job, JobStatus } from "../types";
import { formatEnergy, formatWallTime } from "../format";
import { ConvergenceDashboard } from "../convergence/ConvergenceDashboard";
import type { ConvergenceEvent, ConvergencePayload } from "../convergence/types";
import { OutputSearchPanel } from "./OutputSearchPanel";
import { ResultsCard } from "./ResultsCard";
import { OutputViewer, type OutputViewerHandle } from "./OutputViewer";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { useSceneStore } from "../scene/store";
import { stampFreshIds } from "../scene/ids";
import { deserializeScene } from "../scene/scene";
import {
  deltaEKcal,
  isGoatInput,
  parseEnsemble,
  planConformerApply,
  type Conformer,
} from "../scene/ensemble";
import type { Scene } from "../scene/types";

/** Mirrors `commands::jobs::OutputContent`. */
interface OutputContent {
  content: string;
  first_line_no: number;
  total_lines: number;
  truncated: boolean;
}

interface LogPayload {
  job_id: string;
  lines: string[];
}

interface StatusPayload {
  job_id: string;
  status: JobStatus;
}

/** Cap retained console lines so a long run doesn't grow the DOM unbounded. */
const MAX_LINES = 5000;

interface JobDetailScreenProps {
  jobId: string;
  /** Submit the job once listeners are attached (from a Run action). */
  autoRun: boolean;
  onBack: () => void;
  /** Start a new iteration seeded from this job (input + fragment snapshot). */
  onIterate: (job: Job) => void;
  /** A conformer was applied to the scene store — go to New Job, keeping it. */
  onUseConformer: () => void;
}

export function JobDetailScreen({
  jobId,
  autoRun,
  onBack,
  onIterate,
  onUseConformer,
}: JobDetailScreenProps) {
  const [job, setJob] = useState<Job | null>(null);
  const [lines, setLines] = useState<string[]>([]);
  const [events, setEvents] = useState<ConvergenceEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Dashboard accordion: user override (null = follow the default, which is
  // expanded while the job is active, collapsed once it's finished).
  const [dashOpen, setDashOpen] = useState<boolean | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  // Output area: "live" streams the <pre> console; "browse" shows the Monaco
  // file viewer (loaded lazily on first entry).
  const [mode, setMode] = useState<"live" | "browse">("live");
  const [viewerContent, setViewerContent] = useState<OutputContent | null>(null);
  const [loadingViewer, setLoadingViewer] = useState(false);
  const [viewerApi, setViewerApi] = useState<OutputViewerHandle | null>(null);
  const [snapshotAt, setSnapshotAt] = useState<string | null>(null);
  const [resetToken, setResetToken] = useState(0);
  const navRef = useRef<{ next: () => void; prev: () => void }>({
    next: () => {},
    prev: () => {},
  });
  const preRef = useRef<HTMLPreElement>(null);
  const didSubmit = useRef(false);

  // GOAT conformer ensemble (2.5.1b): read lazily once the job is completed, and
  // only if `input.finalensemble.xyz` exists + parses (so non-GOAT jobs show
  // nothing). `null` = not a GOAT ensemble; a `[]` never happens (parse → null).
  const [ensemble, setEnsemble] = useState<Conformer[] | null>(null);
  const [selectedConf, setSelectedConf] = useState(0);
  const ensembleTried = useRef(false);

  useEffect(() => {
    if (job?.status !== "completed" || ensembleTried.current) return;
    ensembleTried.current = true;
    invoke<string>("read_job_ensemble", { id: jobId })
      .then((text) => {
        const parsed = parseEnsemble(text);
        if (parsed) {
          setEnsemble(parsed);
          setSelectedConf(0);
        }
      })
      .catch(() => {
        /* no ensemble / not a GOAT job — leave the panel hidden */
      });
  }, [job?.status, jobId]);

  // The fragment this GOAT job ran on (its single-fragment snapshot) and the
  // per-conformer ΔE (kcal/mol) — both derived, memoised so the viewer below
  // keeps a stable scene reference and doesn't redraw every render.
  const snapshotFragment = useMemo(
    () => (job?.scene_json ? (deserializeScene(job.scene_json)?.fragments[0] ?? null) : null),
    [job?.scene_json],
  );
  const deltas = useMemo(() => (ensemble ? deltaEKcal(ensemble) : []), [ensemble]);
  const conformerScene = useMemo<Scene | null>(() => {
    const conf = ensemble?.[selectedConf];
    if (!conf) return null;
    // Preview-only scene: the conformer's raw atoms get fresh 0-based ids.
    const { atoms, nextAtomId } = stampFreshIds(conf.atoms, 0);
    return {
      fragments: [
        {
          id: "conformer-preview",
          name: snapshotFragment?.name ?? "conformer",
          atoms,
          charge: snapshotFragment?.charge ?? 0,
          source: "editor",
        },
      ],
      multiplicity: 1,
      nextAtomId,
    };
  }, [ensemble, selectedConf, snapshotFragment]);

  // "Use this conformer": replace the live fragment in place if it's still in the
  // store scene, else start a fresh single-fragment scene — decided purely by
  // `planConformerApply`, which refuses (no throw) if the composition changed.
  const useConformer = () => {
    const conf = ensemble?.[selectedConf];
    if (!conf || !snapshotFragment || !job?.scene_json) {
      setError("This job has no fragment snapshot to apply a conformer to.");
      return;
    }
    const plan = planConformerApply(
      useSceneStore.getState().scene,
      snapshotFragment,
      conf,
    );
    if (plan.action === "refuse") {
      setError(plan.reason);
      return;
    }
    if (plan.action === "replace") {
      // A logged geometry op — its provenance names the conformer (unit 2b).
      useSceneStore
        .getState()
        .replaceFragmentAtoms(plan.fragmentId, plan.atoms, {
          via: "conformer",
          conformerIndex: conf.index,
          deltaEKcal: null,
        });
    } else {
      const mult = deserializeScene(job.scene_json)?.multiplicity ?? 1;
      // The store scene was cleared → seed a fresh lineage from the conformer.
      useSceneStore.getState().seedScene(
        {
          fragments: [plan.fragment],
          multiplicity: mult,
          nextAtomId: plan.nextAtomId,
        },
        "new-iteration",
      );
    }
    onUseConformer();
  };

  const loadJob = useCallback(async () => {
    try {
      setJob(await invoke<Job>("get_job", { id: jobId }));
    } catch (e) {
      setError(String(e));
    }
  }, [jobId]);

  useEffect(() => {
    let unlistenLog: UnlistenFn | undefined;
    let unlistenStatus: UnlistenFn | undefined;
    let unlistenConv: UnlistenFn | undefined;
    let cancelled = false;

    (async () => {
      let current: Job | null = null;
      try {
        current = await invoke<Job>("get_job", { id: jobId });
        if (cancelled) return;
        setJob(current);
      } catch (e) {
        setError(String(e));
      }

      unlistenLog = await listen<LogPayload>("job:log", (e) => {
        if (e.payload.job_id !== jobId) return;
        setLines((prev) => {
          const next = prev.concat(e.payload.lines);
          return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
        });
      });

      // Live convergence datapoints (SCF iterations + optimization steps),
      // attached before submit so no early points are missed (listeners-first).
      unlistenConv = await listen<ConvergencePayload>("job:convergence", (e) => {
        if (e.payload.job_id !== jobId) return;
        setEvents((prev) => prev.concat(e.payload.events));
      });

      unlistenStatus = await listen<StatusPayload>("job:status", (e) => {
        if (e.payload.job_id !== jobId) return;
        setJob((prev) => (prev ? { ...prev, status: e.payload.status } : prev));
        // Reload the full record so error_message / completed_at / results appear.
        // `parsed` is the terminal success state (results stored); a `completed`
        // with a parse error also lands here so its message shows.
        if (
          e.payload.status === "completed" ||
          e.payload.status === "parsed" ||
          e.payload.status === "failed" ||
          e.payload.status === "cancelled"
        ) {
          loadJob();
        }
      });

      // Backfill existing output from output.out (after listeners are attached).
      // For a completed/failed job this loads the whole log; for a running job it
      // loads what exists so far and live events append from here (a small
      // duplicate window is possible for a running job — acceptable for Phase 1).
      if (current && current.status !== "draft") {
        try {
          const existing = await invoke<string[]>("read_job_output", { id: jobId });
          if (!cancelled && existing.length) {
            setLines((prev) => (prev.length ? prev : existing));
          }
        } catch (e) {
          setError(String(e));
        }
        // Backfill convergence datapoints the same way (and in the same order)
        // as the log: seed only if live events haven't already populated.
        try {
          const past = await invoke<ConvergenceEvent[]>("read_job_convergence", {
            id: jobId,
          });
          if (!cancelled && past.length) {
            setEvents((prev) => (prev.length ? prev : past));
          }
        } catch (e) {
          setError(String(e));
        }
      }

      // Kick off the run only AFTER listeners are attached, so no early output
      // lines are lost. The ref guards against React StrictMode's double-mount.
      if (autoRun && !didSubmit.current && !cancelled) {
        didSubmit.current = true;
        try {
          await invoke("submit_job", { id: jobId });
        } catch (e) {
          setError(String(e));
        }
      }
    })();

    return () => {
      cancelled = true;
      unlistenLog?.();
      unlistenStatus?.();
      unlistenConv?.();
    };
  }, [jobId, autoRun, loadJob]);

  // Auto-scroll the live console to the bottom as new lines arrive (and when
  // switching back to Live).
  useEffect(() => {
    if (mode !== "live") return;
    const el = preRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines, mode]);

  // Load the viewer's content (last MAX_VIEWER_LINES) and stamp the snapshot.
  const loadViewer = useCallback(async () => {
    const c = await invoke<OutputContent>("read_job_output_for_viewer", {
      id: jobId,
    });
    setViewerContent(c);
    setSnapshotAt(new Date().toLocaleTimeString());
  }, [jobId]);

  // Enter Browse — loading the file lazily on the first entry only.
  const enterBrowse = useCallback(async () => {
    try {
      if (!viewerContent) {
        setLoadingViewer(true);
        await loadViewer();
      }
      setMode("browse");
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingViewer(false);
    }
  }, [viewerContent, loadViewer]);

  // Re-read the file (it grew while browsing a running job) and clear stale hits.
  const reloadViewer = useCallback(async () => {
    setLoadingViewer(true);
    try {
      await loadViewer();
      setResetToken((t) => t + 1);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingViewer(false);
    }
  }, [loadViewer]);

  // Stable callbacks so the viewer's F3 keybindings reach the search panel.
  const onFindNext = useCallback(() => navRef.current.next(), []);
  const onFindPrev = useCallback(() => navRef.current.prev(), []);

  const openFolder = async () => {
    try {
      await invoke("open_job_folder", { id: jobId });
    } catch (e) {
      setError(String(e));
    }
  };

  const cancel = async () => {
    // The backend kills the ORCA tree off-thread, so this invoke returns fast;
    // keep the button in a disabled "Cancelling…" state until the terminal
    // `job:status` event arrives (which flips `cancellable` false and unmounts
    // the button).
    setCancelling(true);
    try {
      await invoke("cancel_job", { id: jobId });
    } catch (e) {
      setError(String(e));
      setCancelling(false); // let the user retry
    }
  };

  const cancellable = job?.status === "running" || job?.status === "queued";
  const isActive = job?.status === "running" || job?.status === "queued";
  // Expanded by default while the job is active (or auto-running), collapsed for
  // a finished job — the live dashboard is the main thing to watch mid-run.
  const dashboardOpen = dashOpen ?? (isActive || autoRun);
  const showDashboard = events.length > 0 || isActive;

  return (
    <div className="screen detail">
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 10 }}>
        <div className="row">
          <button className="btn btn-sm" onClick={onBack}>
            ← Jobs
          </button>
          {job?.job_dir ? (
            <button className="btn btn-sm" onClick={openFolder}>
              Open Folder
            </button>
          ) : null}
          {job ? (
            <button
              className="btn btn-sm"
              onClick={() => onIterate(job)}
              title="Start a new job from this one's input and fragment layout"
            >
              New iteration
            </button>
          ) : null}
          {cancellable ? (
            <button className="btn btn-sm" onClick={cancel} disabled={cancelling}>
              {cancelling ? "Cancelling…" : "Cancel"}
            </button>
          ) : null}
        </div>
        {job ? <span className={`badge ${job.status}`}>{job.status}</span> : null}
      </div>

      {job ? (
        <div style={{ marginBottom: 10 }}>
          <h2 className="section-title" style={{ marginBottom: 4 }}>
            {job.title}
          </h2>
          <div className="mono" style={{ color: "var(--muted)", fontSize: 12 }}>
            created {job.created_at}
            {job.started_at ? ` · started ${job.started_at}` : ""}
            {job.completed_at ? ` · finished ${job.completed_at}` : ""}
          </div>
          {job.energy != null || job.wall_time != null ? (
            <div className="mono" style={{ color: "var(--text)", fontSize: 12, marginTop: 4 }}>
              energy {formatEnergy(job.energy)} Eh · time {formatWallTime(job.wall_time)}
            </div>
          ) : null}
        </div>
      ) : null}

      {error ? (
        <div className="banner err" style={{ marginBottom: 10 }}>
          {error}
        </div>
      ) : null}
      {job?.error_message ? (
        <div className="banner err" style={{ marginBottom: 10, whiteSpace: "pre-wrap" }}>
          {job.error_message}
        </div>
      ) : null}

      {job ? <ResultsCard jobId={jobId} jobTitle={job.title} status={job.status} /> : null}

      {ensemble ? (
        <div className="ensemble-panel">
          <div className="ensemble-head">
            <span className="section-title">
              Conformers ({ensemble.length})
            </span>
            <button className="btn btn-primary btn-sm" onClick={useConformer}>
              Use this conformer
            </button>
          </div>
          <div className="ensemble-body">
            <div className="ensemble-list">
              {ensemble.map((c, i) => (
                <button
                  key={i}
                  className={
                    "ensemble-row" + (i === selectedConf ? " selected" : "")
                  }
                  onClick={() => setSelectedConf(i)}
                >
                  <span className="mono">#{i + 1}</span>
                  <span className="ensemble-de">
                    {Number.isNaN(deltas[i])
                      ? "—"
                      : `+${deltas[i].toFixed(2)} kcal/mol`}
                  </span>
                  <span className="ensemble-abs muted mono">
                    {Number.isNaN(c.energy) ? "—" : `${c.energy.toFixed(6)} Eh`}
                  </span>
                </button>
              ))}
            </div>
            <div className="ensemble-viewer viewer-panel">
              {conformerScene ? <MoleculeViewer scene={conformerScene} /> : null}
            </div>
          </div>
          <div className="muted" style={{ fontSize: 12, marginTop: 6 }}>
            ΔE relative to the lowest conformer (kcal/mol). Boltzmann weighting +
            DFT re-optimisation of the lowest few is Phase 4.5.
          </div>
        </div>
      ) : null}

      {showDashboard ? (
        <div className="input-builder" style={{ marginBottom: 10 }}>
          <button
            className="builder-toggle"
            onClick={() => setDashOpen(!dashboardOpen)}
            aria-expanded={dashboardOpen}
          >
            <span className="builder-caret">{dashboardOpen ? "▾" : "▸"}</span>
            Convergence
            <span className="muted" style={{ marginLeft: 8 }}>
              energy &amp; convergence criteria per cycle
            </span>
          </button>
          {dashboardOpen ? (
            <div className="builder-body">
              {events.length ? (
                <ConvergenceDashboard
                  events={events}
                  status={job?.status ?? "running"}
                  variant={
                    job && isGoatInput(job.input_content) ? "goat" : "standard"
                  }
                />
              ) : (
                <div className="muted" style={{ fontSize: 12 }}>
                  Waiting for convergence data…
                </div>
              )}
            </div>
          ) : null}
        </div>
      ) : null}

      {job && job.status !== "draft" ? (
        <div className="input-builder" style={{ marginBottom: 10 }}>
          <button
            className="builder-toggle"
            onClick={() => setSearchOpen((o) => !o)}
            aria-expanded={searchOpen}
          >
            <span className="builder-caret">{searchOpen ? "▾" : "▸"}</span>
            Search output
            <span className="muted" style={{ marginLeft: 8 }}>
              find warnings, energies, errors… and jump to them in the file
            </span>
          </button>
          {searchOpen ? (
            <div className="builder-body">
              <OutputSearchPanel
                jobId={jobId}
                viewer={viewerApi}
                onSearchHit={enterBrowse}
                navRef={navRef}
                resetToken={resetToken}
              />
            </div>
          ) : null}
        </div>
      ) : null}

      {job && job.status !== "draft" ? (
        <div className="output-modebar">
          <div className="mode-toggle">
            <button
              className={"mode-btn" + (mode === "live" ? " active" : "")}
              onClick={() => setMode("live")}
            >
              Live log
            </button>
            <button
              className={"mode-btn" + (mode === "browse" ? " active" : "")}
              onClick={enterBrowse}
            >
              {loadingViewer ? "Loading…" : "Browse file"}
            </button>
          </div>
          {mode === "browse" && viewerContent ? (
            <div className="row" style={{ gap: 10 }}>
              {isActive ? (
                <button
                  className="btn btn-sm"
                  onClick={reloadViewer}
                  disabled={loadingViewer}
                >
                  Reload
                </button>
              ) : null}
              {snapshotAt ? (
                <span className="muted">snapshot at {snapshotAt}</span>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}

      <pre
        className="log-console"
        ref={preRef}
        style={{ display: mode === "live" ? "block" : "none" }}
      >
        {lines.length ? lines.join("\n") : "Waiting for ORCA output…"}
      </pre>

      {viewerContent ? (
        <div
          className="output-viewer-wrap"
          style={{ display: mode === "browse" ? "flex" : "none" }}
        >
          <OutputViewer
            ref={setViewerApi}
            content={viewerContent.content}
            firstLineNo={viewerContent.first_line_no}
            truncated={viewerContent.truncated}
            totalLines={viewerContent.total_lines}
            onFindNext={onFindNext}
            onFindPrev={onFindPrev}
          />
        </div>
      ) : null}
    </div>
  );
}
