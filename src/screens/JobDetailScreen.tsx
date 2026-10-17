import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, message } from "@tauri-apps/plugin-dialog";

import type { Job, ParsedResults, StatusStep, SubmitResponse, WithdrawReport } from "../types";
import type { FragmentGeometryVia } from "../scene/oplog";
import { formatEnergy, formatWallTime } from "../format";
import { ConvergenceDashboard } from "../convergence/ConvergenceDashboard";
import type { ConvergenceEvent } from "../convergence/types";
import { OutputSearchPanel } from "./OutputSearchPanel";
import { ResultsCard } from "./ResultsCard";
import { OutputViewer, type OutputViewerHandle } from "./OutputViewer";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { useSceneStore } from "../scene/store";
import { stampFreshIds } from "../scene/ids";
import { deserializeScene } from "../scene/scene";
import { buildReoptInput, DEFAULT_REOPT_METHOD } from "../scene/reopt";
import { GroupSelect } from "../groups/GroupSelect";
import { useGroupPicker } from "../groups/useGroupPicker";
import { InlineRename } from "../jobs/InlineRename";
import { confirmThenWithdraw, openJobView, tauriBridge, trackRemoteStates } from "../jobs/live-wiring";
import { NOTHING_HELD, remoteView, type HeldRemoteState, type RemoteAction } from "../jobs/remote";
import { RemotePanel } from "../jobs/RemotePanel";
import { parseScanCoordinates } from "../scene/scan";
import { ScanSurface2dPanel } from "../scan/ScanSurface2dPanel";
import {
  aggregateReopt,
  CO_MINIMUM_TIE_KCAL,
  type ReoptAggregateRaw,
  type ReoptComparison,
} from "../scene/reopt-aggregate";
import {
  boltzmannWeights,
  deltaEKcal,
  isGoatInput,
  isTerminalSuccessStatus,
  parseEnsemble,
  planConformerApply,
  showsSingleStructureResults,
  type Conformer,
} from "../scene/ensemble";
import { exportJob, type CopyMode } from "../export/save";
import type { Scene } from "../scene/types";

/** Mirrors `commands::jobs::OutputContent`. */
interface OutputContent {
  content: string;
  first_line_no: number;
  total_lines: number;
  truncated: boolean;
}

/** Cap retained console lines so a long run doesn't grow the DOM unbounded. */
const MAX_LINES = 5000;

interface JobDetailScreenProps {
  jobId: string;
  /** Submit the job once listeners are attached (from a Run action). */
  autoRun: boolean;
  /** Where `autoRun` submits: null = this machine, else a server profile id (ADR-024 o5). */
  runTarget: string | null;
  onBack: () => void;
  /** Start a new iteration seeded from this job (input + fragment snapshot). */
  onIterate: (job: Job) => void;
  /** A conformer was applied to the scene store — go to New Job, keeping it. */
  onUseConformer: () => void;
  /** Open another job's detail screen (e.g. the scan panel's OptTS-refine child). */
  onOpenJob: (jobId: string) => void;
}

export function JobDetailScreen({
  jobId,
  autoRun,
  runTarget,
  onBack,
  onIterate,
  onUseConformer,
  onOpenJob,
}: JobDetailScreenProps) {
  const [job, setJob] = useState<Job | null>(null);
  const [lines, setLines] = useState<string[]>([]);
  const [events, setEvents] = useState<ConvergenceEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  // A remote submit's `%pal` notice (Rust's words), shown once beside the run.
  const [notice, setNotice] = useState<string | null>(null);
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

  // GOAT conformer ensemble (2.5.1b): read lazily once the job reaches a TERMINAL
  // SUCCESS status, and only if `input.finalensemble.xyz` exists + parses (so non-GOAT
  // jobs show nothing). `null` = not a GOAT ensemble; a `[]` never happens (parse →
  // null). The status guard must accept BOTH `completed` and `parsed`: a GOAT job whose
  // single-structure `.property.txt` parsed reaches `parsed`, and the old `=== "completed"`
  // guard hid its ensemble (regression, debugging/017).
  const [ensemble, setEnsemble] = useState<Conformer[] | null>(null);
  const [selectedConf, setSelectedConf] = useState(0);
  const [ensembleChecked, setEnsembleChecked] = useState(false);
  const ensembleTried = useRef(false);

  // "Re-optimize top-k at DFT" fan-out form (Stage D unit D2a). All local UI state;
  // nothing is stored until the k child jobs are created on trigger.
  const [reoptK, setReoptK] = useState(4);
  const [reoptMethod, setReoptMethod] = useState(DEFAULT_REOPT_METHOD);
  const [reoptFreq, setReoptFreq] = useState(true); // Opt+Freq (default) vs Opt-only
  // Implicit solvation for the DFT re-opt: none (gas phase, default), CPCM (fast
  // electrostatic continuum) or SMD (adds the Cramer–Truhlar CDS term). The chosen
  // model is passed straight to `buildReoptInput`'s keyword-line emit.
  const [reoptSolvation, setReoptSolvation] = useState<"none" | "cpcm" | "smd">("none");
  const [reoptSolvent, setReoptSolvent] = useState("water");
  const [reoptBusy, setReoptBusy] = useState(false);
  const [reoptMsg, setReoptMsg] = useState<string | null>(null);
  // D2b read side: the xTB-vs-DFT comparison over this GOAT job's re-opt children.
  // Derived at read time from `read_conformer_reoptimization` + the D1 ensemble;
  // never stored. `null` = no children yet (no fan-out launched).
  const [reoptAgg, setReoptAgg] = useState<ReoptComparison | null>(null);
  const [reoptAggBusy, setReoptAggBusy] = useState(false);
  // Destination-group picker for the DFT re-opt fan-out children (unit 2b): defaults to THIS
  // GOAT job's group (the source, already in hand), overridable. Not the active sidebar group.
  const reoptGroupPicker = useGroupPicker(job?.group_id ?? null);

  useEffect(() => {
    if (!job || !isTerminalSuccessStatus(job.status) || ensembleTried.current) return;
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
      })
      .finally(() => setEnsembleChecked(true));
  }, [job?.status, jobId]);

  // The fragment this GOAT job ran on (its single-fragment snapshot) and the
  // per-conformer ΔE (kcal/mol) — both derived, memoised so the viewer below
  // keeps a stable scene reference and doesn't redraw every render.
  const snapshotFragment = useMemo(
    () => (job?.scene_json ? (deserializeScene(job.scene_json)?.fragments[0] ?? null) : null),
    [job?.scene_json],
  );
  const deltas = useMemo(() => (ensemble ? deltaEKcal(ensemble) : []), [ensemble]);
  // Boltzmann populations (xTB/GFN2-level, 298.15 K) — DERIVED at render, never
  // stored (a cached scalar drifts from its source). `cumulative` is a running
  // sum down the energy-sorted list; it's what D2's k-selection ("how many
  // conformers to DFT re-optimise") reads, so it earns its column now. A NaN
  // weight (no energy) contributes nothing to the running sum and renders "—".
  const weights = useMemo(
    () => (ensemble ? boltzmannWeights(ensemble) : []),
    [ensemble],
  );
  const cumulative = useMemo(() => {
    let acc = 0;
    return weights.map((w) => {
      if (Number.isNaN(w)) return NaN;
      acc += w;
      return acc;
    });
  }, [weights]);
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

  // Apply a chosen conformer geometry to the scene store (2.5.1b path): replace the
  // live fragment in place if it's still there, else seed a fresh single-fragment
  // scene — decided purely by `planConformerApply`, which refuses (no throw, a
  // banner) on a composition mismatch. That refusal IS the atom count/element-order
  // post-condition (rule #9) — carrying a geometry with a different composition is
  // rejected, never assumed away. Returns true on success. `provenance` labels the op.
  const applyConformer = (
    conf: Conformer,
    provenance: FragmentGeometryVia & { via: "conformer" },
  ): boolean => {
    if (!snapshotFragment || !job?.scene_json) {
      setError("This job has no fragment snapshot to apply a conformer to.");
      return false;
    }
    const plan = planConformerApply(
      useSceneStore.getState().scene,
      snapshotFragment,
      conf,
    );
    if (plan.action === "refuse") {
      setError(plan.reason);
      return false;
    }
    if (plan.action === "replace") {
      useSceneStore
        .getState()
        .replaceFragmentAtoms(plan.fragmentId, plan.atoms, provenance);
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
    return true;
  };

  // "Use this conformer" — the RAW xTB frame from the GOAT ensemble (2.5.1b).
  const useConformer = () => {
    const conf = ensemble?.[selectedConf];
    if (!conf) {
      setError("This job has no fragment snapshot to apply a conformer to.");
      return;
    }
    if (
      applyConformer(conf, {
        via: "conformer",
        conformerIndex: conf.index,
        deltaEKcal: null,
      })
    ) {
      onUseConformer();
    }
  };

  // Carry the DFT-RE-OPTIMISED geometry of a re-opt child downstream (D3). Unlike
  // `useConformer` (the raw xTB frame), the source is the child job's PARSED FINAL
  // DFT geometry (Phase 3 `results.final_geometry`), so the user carries the higher-
  // level structure forward. Only offered for a completed clean child (the D2b rows).
  // The op-log records `level: "DFT"` + method so the level is never ambiguous.
  const useDftConformer = async (
    childJobId: string,
    conformerIndex: number,
    method: string,
  ) => {
    setError(null);
    try {
      const res = await invoke<ParsedResults | null>("read_job_results", {
        id: childJobId,
      });
      const geo = res?.final_geometry;
      if (!geo || geo.elements.length === 0) {
        setError("This re-opt job has no parsed final DFT geometry yet.");
        return;
      }
      const dftConf: Conformer = {
        atoms: geo.elements.map((element, i) => ({
          element,
          x: geo.xyz_angstrom[i][0],
          y: geo.xyz_angstrom[i][1],
          z: geo.xyz_angstrom[i][2],
        })),
        energy: NaN, // geometry only — the scene carries no energy
        index: conformerIndex,
      };
      if (
        applyConformer(dftConf, {
          via: "conformer",
          conformerIndex,
          deltaEKcal: null,
          level: "DFT",
          method,
        })
      ) {
        onUseConformer();
      }
    } catch (e) {
      setError(String(e));
    }
  };

  // "Re-optimize top-k at DFT" (Stage D unit D2a): fan out k queued DFT re-opt
  // children over the LOWEST-energy conformers. The child inputs are built AND
  // charge-checked FIRST (buildReoptInput throws on any (c,m)/geometry mismatch —
  // the anion footgun), so if any one fails we create NOTHING; only once all k
  // inputs are proven do we create + queue them. Each child is tagged back to this
  // GOAT job + its conformer index. All k share one mode (no mixing within a fan-out).
  const reoptTopK = async () => {
    if (!job || !ensemble) return;
    const k = Math.max(1, Math.min(reoptK, ensemble.length));
    setReoptBusy(true);
    setReoptMsg(null);
    setError(null);
    try {
      const opts = {
        method: reoptMethod.trim() || DEFAULT_REOPT_METHOD,
        freq: reoptFreq,
        // TS narrows `reoptSolvation` to "cpcm" | "smd" inside this branch, which is
        // exactly the shape `solvation.model` wants — no `as const` needed.
        ...(reoptSolvation !== "none"
          ? { solvation: { model: reoptSolvation, solvent: reoptSolvent.trim() } }
          : {}),
      };
      // Build + charge-check ALL k inputs before creating any job (create boundary:
      // a mismatch aborts the whole fan-out, having created nothing).
      const built = ensemble.slice(0, k).map((conf) => ({
        conf,
        input: buildReoptInput(job.input_content, conf, opts),
      }));
      // All proven → create each as a queued child and submit into the sequential
      // queue (concurrency 1).
      for (const { conf, input } of built) {
        const child = await invoke<Job>("create_reopt_job", {
          sourceEnsembleJobId: job.id,
          sourceConformerIndex: conf.index,
          title: `re-opt #${conf.index + 1} — ${job.title}`,
          inputContent: input,
        });
        // Move each fan-out child into the picked group (default = this job's group) before
        // submitting (unit 2b). No-op when untouched + ungrouped source.
        await reoptGroupPicker.assignPicked(child.id);
        // The fan-out runs on this machine: it has no "Run on" choice of its own.
        await invoke("submit_job", { id: child.id, target: null });
      }
      setReoptMsg(
        `Queued ${built.length} DFT re-opt job${built.length === 1 ? "" : "s"} ` +
          `(${reoptFreq ? "Opt+Freq" : "Opt-only"}${reoptSolvation !== "none" ? `, ${reoptSolvation.toUpperCase()} ${reoptSolvent.trim()}` : ""}) ` +
          // ADR-024 o20.2: the fan-out has no "Run on" choice — say where its jobs run.
          `to run locally on this machine.`,
      );
      // Show the (provisional) aggregate immediately — the new children appear as
      // queued/running and are surfaced as such, not weighted.
      void loadReoptAgg();
    } catch (e) {
      // A charge/geometry post-condition failure lands here — no job was created.
      setError(`Re-opt fan-out aborted (no jobs created): ${String(e)}`);
    } finally {
      setReoptBusy(false);
    }
  };

  // Read-time aggregate of this GOAT job's DFT re-opt children (D2b). Needs the D1
  // ensemble (for the xTB side); recomputed on demand, never cached in the DB.
  const loadReoptAgg = useCallback(async () => {
    if (!job || !ensemble || !isGoatInput(job.input_content)) return;
    setReoptAggBusy(true);
    try {
      const raw = await invoke<ReoptAggregateRaw>("read_conformer_reoptimization", {
        sourceJobId: jobId,
      });
      setReoptAgg(raw.children.length ? aggregateReopt(raw, ensemble) : null);
    } catch {
      setReoptAgg(null);
    } finally {
      setReoptAggBusy(false);
    }
  }, [job, ensemble, jobId]);

  // Load the aggregate once the ensemble is available (a GOAT job with children).
  useEffect(() => {
    void loadReoptAgg();
  }, [loadReoptAgg]);

  const loadJob = useCallback(async () => {
    try {
      setJob(await invoke<Job>("get_job", { id: jobId }));
    } catch (e) {
      setError(String(e));
    }
  }, [jobId]);

  // Re-run the authoritative parse on a completed job whose parse previously failed
  // (e.g. after a parser fix landed). The command is idempotent and returns the reloaded
  // record, so on success the status advances to `parsed` and the stale error clears in
  // one round-trip. Only offered for `completed && error_message` (see the button gate).
  const [reparsing, setReparsing] = useState(false);
  const reparse = useCallback(async () => {
    setReparsing(true);
    setError(null);
    try {
      setJob(await invoke<Job>("reparse_job", { jobId }));
    } catch (e) {
      setError(String(e));
    } finally {
      setReparsing(false);
    }
  }, [jobId]);

  // Open the view (ADR-024 o16.7, `openJobView`): read the row, attach `job:log` /
  // `job:convergence` / `job:log-reset` / `job:status` FIRST, then open the live log
  // (`watch_job_log`), backfill what is on disk — never for a remote live job, whose log the
  // poller re-streams from offset 0 — and only then (if `autoRun`) submit, so no early line is
  // lost. The close drops every listener, a late-resolving one included, and closes the watch
  // only if this view opened it.
  useEffect(() => {
    const submit = async () => {
      // The ref guards against React StrictMode's double-mount.
      if (!autoRun || didSubmit.current) return;
      didSubmit.current = true;
      try {
        const response = await invoke<SubmitResponse | null>("submit_job", {
          id: jobId,
          target: runTarget,
        });
        // A remote submit answers its attempt: Rust's words for a failure, and the `%pal` notice.
        if (response?.failure) setError(response.failure);
        if (response?.notice) setNotice(response.notice);
      } catch (e) {
        setError(String(e));
      }
    };
    return openJobView(
      tauriBridge,
      jobId,
      {
        job: setJob,
        lines: (more) =>
          setLines((prev) => {
            const next = prev.concat(more);
            return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
          }),
        convergence: (more) => setEvents((prev) => prev.concat(more)),
        reset: () => {
          setLines([]);
          setEvents([]);
        },
        status: (status) => {
          setJob((prev) => (prev ? { ...prev, status } : prev));
          // Reload the full record on every status: a terminal one brings error_message /
          // completed_at / results, and a remote job's check changes only its error_message.
          loadJob();
        },
        // Seed only if live events haven't already populated (for a running local job a
        // small duplicate window is possible — acceptable since Phase 1).
        backfill: (existing, past) => {
          if (existing.length) setLines((prev) => (prev.length ? prev : existing));
          if (past.length) setEvents((prev) => (prev.length ? prev : past));
        },
        error: setError,
      },
      submit,
    );
  }, [jobId, autoRun, runTarget, loadJob]);

  // The job's remote record (ADR-024 o19.6): the listener first, then `remote_job_state`, both
  // merged by `seq` (`trackRemoteStates`). A local job simply never has one.
  const [remoteHeld, setRemoteHeld] = useState<HeldRemoteState>(NOTHING_HELD);
  useEffect(() => {
    setRemoteHeld(NOTHING_HELD);
    const tracker = trackRemoteStates(
      tauriBridge,
      (held) => setRemoteHeld(held.get(jobId) ?? NOTHING_HELD),
      setError,
    );
    void tracker.read([jobId]);
    return tracker.dispose;
  }, [jobId]);

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

  // Export the job's folder to a chosen location as a readable, job-named projection +
  // manifest.json (the single-job sibling of group export). The Curated/Full choice mirrors
  // the group export chooser; the canonical <UUID>/ dir is untouched (Rust only reads it).
  const [exportOpen, setExportOpen] = useState(false);
  const [exportBusy, setExportBusy] = useState(false);
  const runJobExport = async (copyMode: CopyMode) => {
    if (exportBusy) return;
    setExportOpen(false);
    setExportBusy(true);
    try {
      const path = await exportJob(jobId, copyMode);
      if (path) {
        await message(`Exported “${job?.title ?? "job"}” to:\n${path}`, {
          title: "Export complete",
        });
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setExportBusy(false);
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

  // The remote actions `remoteView` offers (ADR-024 o19.7). Each answer is shown in Rust's words
  // (a submit failure, the `%pal` notice; a refusal is the error text); the row and the record
  // follow through `job:status` and `job:remote-state`.
  const [remoteBusy, setRemoteBusy] = useState<RemoteAction | null>(null);
  const runRemoteAction = async (action: RemoteAction) => {
    setRemoteBusy(action);
    setError(null);
    setNotice(null);
    try {
      if (action === "retry_submit") {
        const response = await invoke<SubmitResponse>("retry_remote_submit", { id: jobId });
        if (response.failure) setError(response.failure);
        if (response.notice) setNotice(response.notice);
      } else if (action === "withdraw") {
        // Confirmed like Delete job (ADR-024 o20.3), and sent only if the view still offers it
        // once the dialog closed — the record may have changed meanwhile.
        await confirmThenWithdraw({
          confirm: () =>
            confirm(
              `Withdraw "${job?.title ?? "this job"}"? It is cancelled and will not run on the server.`,
              { title: "Withdraw job", kind: "warning" },
            ),
          currentActions: () => (remoteRef.current?.kind === "live" ? remoteRef.current.actions : []),
          withdraw: async () => {
            await invoke<WithdrawReport>("withdraw_remote_job", { id: jobId });
          },
        });
      } else {
        await invoke<StatusStep>("retry_remote_fetch", { id: jobId });
      }
      await loadJob();
    } catch (e) {
      setError(String(e));
    } finally {
      setRemoteBusy(null);
    }
  };
  const remote = job ? remoteView(job, remoteHeld.state) : null;
  // The latest view, for a decision taken after an await (the withdraw re-check, o20.3).
  const remoteRef = useRef(remote);
  remoteRef.current = remote;

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
            exportOpen ? (
              <span className="row" style={{ gap: 4 }}>
                <span className="muted">Export:</span>
                <button
                  className="btn btn-sm"
                  disabled={exportBusy}
                  title="Only the pinned scientific artifacts (no .gbw/cubes/scratch)"
                  onClick={() => runJobExport("curated")}
                >
                  Curated…
                </button>
                <button
                  className="btn btn-sm"
                  disabled={exportBusy}
                  title="The whole job folder, verbatim"
                  onClick={() => runJobExport("full")}
                >
                  Full…
                </button>
                <button className="icon-btn" title="Cancel" onClick={() => setExportOpen(false)}>
                  ✕
                </button>
              </span>
            ) : (
              <button
                className="btn btn-sm"
                disabled={exportBusy}
                title="Export this job's folder — named after the job — to a chosen location (+ manifest.json)"
                onClick={() => setExportOpen(true)}
              >
                {exportBusy ? "Exporting…" : "Export Folder…"}
              </button>
            )
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
            <InlineRename
              value={job.title}
              onCommit={async (title) => {
                try {
                  const updated = await invoke<Job>("rename_job", { id: job.id, title });
                  setJob(updated);
                } catch (e) {
                  setError(String(e));
                }
              }}
            />
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
      {notice ? (
        <div className="banner ok" style={{ marginBottom: 10 }}>
          {notice}
        </div>
      ) : null}
      {remote ? <RemotePanel view={remote} busy={remoteBusy} onAction={runRemoteAction} /> : null}
      {/* A live remote job's error_message is the remote panel's text (the server's facts), not
          an error banner. */}
      {job?.error_message && remote?.kind !== "live" ? (
        <div className="banner err" style={{ marginBottom: 10, whiteSpace: "pre-wrap" }}>
          {job.error_message}
          {/* Re-parse is offered ONLY for a completed job carrying a parse-failure
              message — the calculation ran fine, only OUR parse of it did not, and a
              parser fix may now succeed. Not shown for a `failed` calculation (nothing to
              re-parse) nor an already-`parsed` job (no stale error). */}
          {job.status === "completed" ? (
            <div style={{ marginTop: 8 }}>
              <button type="button" onClick={reparse} disabled={reparsing}>
                {reparsing ? "Re-parsing…" : "Re-parse results"}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}

      {/* A GOAT conformer search renders the ensemble panel below, NOT the
          single-structure dashboard — its "N optimization cycles" trajectory is
          the internal opt of one candidate, not conformers, and is misleading for a
          conformer search (debugging/017). Non-GOAT jobs show ResultsCard as before. */}
      {job && showsSingleStructureResults(job.input_content) ? (
        <ResultsCard
          jobId={jobId}
          jobTitle={job.title}
          status={job.status}
          onOpenJob={onOpenJob}
        />
      ) : null}

      {/* 2D relaxed-surface scan (Stage 4b): a job whose INPUT is a 2-coordinate scan has no
          `results.scan` (the 1D reader stands down on the 3-column .dat), so its surface is
          shown separately here, gated on the input — NOT on results. A 1D scan is a different
          branch (its ScanProfilePanel lives inside ResultsCard, unchanged). */}
      {job && parseScanCoordinates(job.input_content)?.length === 2 ? (
        <ScanSurface2dPanel jobId={jobId} jobTitle={job.title} onOpenJob={onOpenJob} />
      ) : null}

      {/* A GOAT job that finished but produced no readable ensemble: say so plainly,
          rather than falling back to the misleading trajectory dashboard. */}
      {job &&
      isGoatInput(job.input_content) &&
      !ensemble &&
      isTerminalSuccessStatus(job.status) &&
      ensembleChecked ? (
        <div className="banner muted" style={{ marginBottom: 10 }}>
          This is a GOAT conformer search, but no conformer ensemble
          (<code>input.finalensemble.xyz</code>) could be read from its job folder.
        </div>
      ) : null}

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
                  <span className="ensemble-pop mono">
                    {Number.isNaN(weights[i])
                      ? "—"
                      : `${(weights[i] * 100).toFixed(1)}%`}
                  </span>
                  <span className="ensemble-cum muted mono">
                    {Number.isNaN(cumulative[i])
                      ? "—"
                      : `Σ ${(cumulative[i] * 100).toFixed(1)}%`}
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
            ΔE relative to the lowest conformer (kcal/mol). Population = Boltzmann
            weight at 298.15&nbsp;K; Cumulative (Σ) is the running total down the
            energy-sorted list. These populations are <strong>xTB/GFN2-level</strong>{" "}
            — DFT re-optimisation of the lowest few (which will re-rank and
            re-weight them) is below.
          </div>

          {/* Stage D unit D2a — DFT re-opt fan-out (create side). Re-optimises the
              lowest-energy k conformers at DFT; the cumulative-% at k is shown so the
              user SEES how much xTB population those k cover (Fork 3). */}
          <div className="reopt-form">
            <div className="reopt-head section-title">Re-optimize top-k at DFT</div>
            <div className="reopt-controls">
              <label className="reopt-ctl">
                k
                <input
                  type="number"
                  min={1}
                  max={ensemble.length}
                  value={reoptK}
                  onChange={(e) =>
                    setReoptK(
                      Math.max(1, Math.min(Number(e.target.value) || 1, ensemble.length)),
                    )
                  }
                  style={{ width: 56 }}
                />
                <span className="muted mono" style={{ fontSize: 12 }}>
                  {(() => {
                    const cum = cumulative[Math.min(reoptK, ensemble.length) - 1];
                    return Number.isFinite(cum)
                      ? `≈ ${(cum * 100).toFixed(1)}% of xTB population`
                      : "";
                  })()}
                </span>
              </label>
              <label className="reopt-ctl">
                Method
                <input
                  type="text"
                  value={reoptMethod}
                  onChange={(e) => setReoptMethod(e.target.value)}
                  style={{ width: 120 }}
                />
              </label>
              <label className="reopt-ctl">
                Mode
                <select
                  value={reoptFreq ? "optfreq" : "opt"}
                  onChange={(e) => setReoptFreq(e.target.value === "optfreq")}
                >
                  <option value="optfreq">Opt+Freq (ΔG path)</option>
                  <option value="opt">Opt-only (quick screen)</option>
                </select>
              </label>
              <label className="reopt-ctl reopt-smd">
                Solvation
                <select
                  value={reoptSolvation}
                  onChange={(e) =>
                    setReoptSolvation(e.target.value as "none" | "cpcm" | "smd")
                  }
                >
                  <option value="none">None (gas phase)</option>
                  <option value="cpcm">CPCM</option>
                  <option value="smd">SMD</option>
                </select>
                <input
                  type="text"
                  value={reoptSolvent}
                  disabled={reoptSolvation === "none"}
                  onChange={(e) => setReoptSolvent(e.target.value)}
                  placeholder="solvent"
                  style={{ width: 96 }}
                />
              </label>
              <label className="reopt-ctl" title="Destination group for the re-opt children">
                Group
                <GroupSelect
                  groups={reoptGroupPicker.groups}
                  value={reoptGroupPicker.pickedGroupId}
                  onChange={reoptGroupPicker.onChange}
                  aria-label="Destination group for the re-opt children"
                />
              </label>
              <button
                className="btn btn-primary btn-sm"
                onClick={reoptTopK}
                disabled={reoptBusy}
              >
                {reoptBusy ? "Queuing…" : `Re-optimize top ${Math.min(reoptK, ensemble.length)}`}
              </button>
            </div>
            <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
              Creates {Math.min(reoptK, ensemble.length)} job
              {Math.min(reoptK, ensemble.length) === 1 ? "" : "s"} in the queue — each
              inherits this molecule's charge/multiplicity.{" "}
              {reoptFreq
                ? "Opt+Freq on a heavy molecule is not cheap."
                : "Opt-only skips the frequencies (no ΔG / no TS check)."}
            </div>
            {reoptMsg ? (
              <div className="banner ok" style={{ marginTop: 6 }}>
                {reoptMsg}
              </div>
            ) : null}
          </div>

          {/* Stage D unit D2b — DFT re-optimisation aggregate: xTB ranking vs DFT
              ranking over the SAME re-optimized subset (comparable columns, no
              cross-level arithmetic). Recomputed at read time, never stored. */}
          {reoptAgg ? (
            <div className="reopt-agg">
              <div className="reopt-agg-head">
                <span className="section-title">
                  DFT re-optimisation — xTB vs{" "}
                  {reoptAgg.mode === "dG" ? "ΔG (DFT)" : "ΔE (DFT)"}
                </span>
                <button
                  className="btn btn-sm"
                  onClick={loadReoptAgg}
                  disabled={reoptAggBusy}
                >
                  {reoptAggBusy ? "…" : "Refresh"}
                </button>
              </div>

              {reoptAgg.provisional ? (
                <div className="banner warn" style={{ marginTop: 6 }}>
                  {reoptAgg.terminalCount} of {reoptAgg.totalChildren} complete —{" "}
                  <strong>provisional, not for decisions</strong>. The ranking will
                  change as the remaining jobs finish.
                </div>
              ) : null}
              {reoptAgg.modeInconsistent ? (
                <div className="banner warn" style={{ marginTop: 6 }}>
                  Mixed set — some children ran Freq and some did not. Weighted on
                  electronic energy (ΔE); no single ΔG is honest here.
                </div>
              ) : null}
              {reoptAgg.notMinimumValidated && !reoptAgg.modeInconsistent ? (
                <div className="muted" style={{ fontSize: 12, marginTop: 6 }}>
                  Opt-only (no Freq): these geometries are{" "}
                  <strong>not frequency-validated as minima</strong>, and ΔE carries no
                  thermal/entropic terms.
                </div>
              ) : null}

              {reoptAgg.rows.length ? (
                <table className="reopt-table">
                  <thead>
                    <tr>
                      <th>Conf.</th>
                      <th>xTB #</th>
                      <th>xTB ΔE</th>
                      <th>xTB pop</th>
                      <th>DFT #</th>
                      <th>{reoptAgg.mode === "dG" ? "ΔG (DFT)" : "ΔE (DFT)"}</th>
                      <th>DFT pop</th>
                      <th>Σ</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {reoptAgg.rows.map((r) => (
                      <tr
                        key={r.jobId}
                        className={r.rankChanged ? "reopt-reordered" : undefined}
                      >
                        <td className="mono">#{r.conformerIndex + 1}</td>
                        <td className="mono">{r.xtbRank}</td>
                        <td className="mono">+{r.xtbDeltaKcal.toFixed(2)}</td>
                        <td className="mono">{(r.xtbWeight * 100).toFixed(1)}%</td>
                        <td className="mono">
                          {r.dftRank}
                          {r.rankChanged ? (
                            <span className="reopt-arrow" title="rank changed under DFT">
                              {" ⚑"}
                            </span>
                          ) : null}
                        </td>
                        <td className="mono">+{r.dftDeltaKcal.toFixed(2)}</td>
                        <td className="mono">{(r.dftWeight * 100).toFixed(1)}%</td>
                        <td className="mono muted">{(r.dftCumulative * 100).toFixed(1)}%</td>
                        <td>
                          {/* Carry THIS child's DFT-re-optimised geometry downstream.
                              Every row is a completed clean child (D2b `included`), so
                              all are usable; the DFT rank-1 is the default best. */}
                          <button
                            className={
                              "btn btn-sm" + (r.dftRank === 1 ? " btn-primary" : "")
                            }
                            title={`Use the ${r.method ?? "DFT"}-optimised geometry of conformer #${r.conformerIndex + 1}`}
                            onClick={() =>
                              useDftConformer(
                                r.jobId,
                                r.conformerIndex,
                                r.method ?? "DFT",
                              )
                            }
                          >
                            {r.dftRank === 1 ? "Use best (DFT)" : "Use (DFT)"}
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <div className="muted" style={{ fontSize: 12, marginTop: 6 }}>
                  No conformer has a usable DFT energy yet.
                </div>
              )}

              {/* The caption must claim exactly what happened — the strong "wrong
                  minimum" wording ONLY when the xTB minimum truly is not a DFT
                  minimum; otherwise the honest tail-reorder / dismissed-co-minimum
                  wording (D3 scope-fix). */}
              {reoptAgg.minimumChanged ? (
                <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                  ⚑ DFT re-ranked the ensemble — the xTB minimum is <strong>not</strong>{" "}
                  the DFT minimum. Building on the xTB best would have used the wrong
                  conformer; this is exactly why the ensemble is re-optimized.
                </div>
              ) : reoptAgg.dismissedRoseToTop ? (
                <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                  ⚑ The xTB minimum held, but a conformer xTB ranked lower is a DFT
                  co-minimum (≤{CO_MINIMUM_TIE_KCAL.toFixed(2)}&nbsp;kcal/mol) — xTB
                  under-weighted it. Worth carrying forward alongside the xTB best.
                </div>
              ) : reoptAgg.reordered ? (
                <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                  DFT re-ranked conformers below the top; the xTB minimum held.
                </div>
              ) : null}

              {reoptAgg.excluded.length ? (
                <div className="reopt-excluded muted" style={{ fontSize: 12, marginTop: 8 }}>
                  <div>Excluded from the weighting (honest-or-absent):</div>
                  <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
                    {reoptAgg.excluded.map((e) => (
                      <li key={e.jobId}>
                        <span className="mono">#{e.conformerIndex + 1}</span> — {e.reason}
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}

              <div className="muted" style={{ fontSize: 12, marginTop: 6 }}>
                xTB and DFT populations are computed over the same re-optimized subset
                — comparable columns at two energy levels. They are never combined.
              </div>
            </div>
          ) : null}
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
