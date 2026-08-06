import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { InputEditor } from "../editor/InputEditor";
import { MoleculeViewer, type AtomPick } from "../viewer/MoleculeViewer";
import { VIEWER_THEMES, viewerTheme } from "../viewer/theme";
import { importStructureFile, IMPORT_ACCEPT } from "../viewer/import-file";
import { InputBuilderForm } from "../input-builder/InputBuilderForm";
import { useSceneStore } from "../scene/store";
import { FragmentList } from "../scene/FragmentList";
import { HistoryPanel } from "../scene/HistoryPanel";
import { EditorDock, type DockSection } from "../scene/EditorDock";
import { AtomInspector } from "../scene/AtomInspector";
import { EditPanel } from "../scene/EditPanel";
import { ConstraintPanel } from "../scene/ConstraintPanel";
import {
  parseConstraintsBlock,
  inspectConstraintsBlock,
  injectConstraints,
  constraintIndexIssues,
  constraintFromSelection,
  sameConstraint,
} from "../scene/constraints";
import {
  planEdit,
  swapToAlternative,
  maskRoleViolation,
  explainSplitViolation,
} from "../scene/edit-plan";
import { postSidecar } from "../sidecar-client";
import {
  toggleAtom,
  validateSelection,
  selectionSurvives,
} from "../scene/selection";
import { placeFragment } from "../scene/placement";
import {
  FRAGMENT_LIBRARY,
  libraryFragmentToScene,
  type LibraryFragment,
} from "../scene/fragment-library";
import {
  atomCount,
  buildViewerAtomTable,
  compositionSignature,
  injectSceneIntoInput,
  mergeToAtomLines,
  nextAtomIdFor,
  parseAtomLines,
  replaceAllAtoms,
  xtbResultApplies,
  mergeToXyz,
  sceneFromAtomLines,
  sceneFromOrcaInput,
  sceneFromXyz,
  serializeScene,
  totalCharge,
  xyzMatchesScene,
} from "../scene/scene";
import { formatXtbProgress } from "../scene/xtb-progress";
import { restoreSceneLog, type LogRejection } from "../scene/restore";
import { serializeLog, type Op } from "../scene/oplog";
import { goatInputForFragment } from "../scene/ensemble";
import type { RawFragment, Scene, SceneFragment } from "../scene/types";
import {
  CATEGORY_LABELS,
  ORCA_TEMPLATES,
  type OrcaTemplate,
} from "../templates/orca-templates";
import type { Job, Molecule, SidecarStatus } from "../types";

/** Charge with an explicit sign: `0`, `-1`, `+1`. */
function signed(n: number): string {
  return n > 0 ? `+${n}` : String(n);
}

/** Human label for a constraint kind, for the range-block message. */
function constraintTypeLabel(
  kind: "distance" | "angle" | "dihedral" | "cartesian",
): string {
  return kind === "distance"
    ? "a distance constraint"
    : kind === "angle"
      ? "an angle constraint"
      : kind === "dihedral"
        ? "a dihedral constraint"
        : "a Cartesian constraint";
}

interface NewJobScreenProps {
  /** A draft job was created; parent navigates to the Jobs list. */
  onCreatedDraft: () => void;
  /** Open a job's detail screen, optionally auto-running it. */
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
  /** A library molecule to preload into the editor on mount ("Use" action). */
  initialMolecule?: Molecule;
  /** An existing job to seed a new iteration from ("New iteration" action):
   * its input + fragment snapshot, nothing from its results/status/dir. */
  initialJob?: Job;
  /** Keep the scene store as-is on mount (a conformer was just applied to it —
   * "Use this conformer"); skip the usual reset. */
  keepScene?: boolean;
}

export function NewJobScreen({
  onCreatedDraft,
  onOpenDetail,
  initialMolecule,
  initialJob,
  keepScene,
}: NewJobScreenProps) {
  // Seed title/content from an iterated job (marked so kinship shows in the job
  // list) or a library molecule; both before first paint via useState.
  const [title, setTitle] = useState(
    initialJob ? `${initialJob.title} (iteration)` : initialMolecule?.name ?? "",
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [content, setContent] = useState(initialJob?.input_content ?? "");
  // Set when an iterated job's fragment snapshot was discarded because it no
  // longer matched its input (distinct from a plain no-snapshot job — no note).
  const [snapshotRejected, setSnapshotRejected] = useState(false);
  const [logRejected, setLogRejected] = useState<LogRejection | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [smiles, setSmiles] = useState("");
  const [generating, setGenerating] = useState(false);
  // Formula carried from the last SMILES generation, used when saving to the
  // library. It is library-molecule metadata (RDKit's molecular formula),
  // orthogonal to the Scene geometry — SceneFragment has no formula concept — so
  // it stays a separate state rather than moving into the fragment.
  const [formula, setFormula] = useState(initialMolecule?.formula ?? "");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  // Input Builder and Templates behave as a two-section accordion: at most one
  // open at a time, both closed by default so the editor + viewer are visible
  // immediately. Opening one closes the other; picking a template or generating
  // an input collapses the accordion (the user has what they wanted).
  const [openSection, setOpenSection] = useState<
    "builder" | "templates" | null
  >(null);
  // The editor workspace dock (unit 2b-ux): which right-dock sections are open.
  // Session-only state (not persisted — a fresh screen starts viewer-first with
  // just Fragments open, so Add Fragment is discoverable). Each toggles alone.
  const [openDock, setOpenDock] = useState<Record<string, boolean>>({
    selection: false,
    edit: false,
    fragments: true,
    constraints: false,
    history: false,
    actions: false,
  });
  const toggleDock = (id: string) =>
    setOpenDock((o) => ({ ...o, [id]: !o[id] }));
  const anyDockOpen = Object.values(openDock).some(Boolean);
  // Library molecules for the Add-Fragment "From library" source (lazy-loaded
  // when the panel opens).
  const [libMolecules, setLibMolecules] = useState<Molecule[]>([]);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // ── Scene store: the single source of truth for geometry (ADR-008) ──────────
  // Selectors return the stored objects directly, so an unchanged scene keeps
  // its identity and MoleculeViewer does not redraw on every keystroke.
  const scene = useSceneStore((s) => s.scene);
  const log = useSceneStore((s) => s.log);
  const resetNotice = useSceneStore((s) => s.resetNotice);
  const addFragment = useSceneStore((s) => s.addFragment);
  const commit = useSceneStore((s) => s.commit);
  const installLog = useSceneStore((s) => s.installLog);
  const seedScene = useSceneStore((s) => s.seedScene);
  const collapseFromText = useSceneStore((s) => s.collapseFromText);
  const undo = useSceneStore((s) => s.undo);
  const dismissResetNotice = useSceneStore((s) => s.dismissResetNotice);

  // ── Atom selection (2.5.2a) — the geometry editor's pick list ───────────────
  // Ordered global atom indices, held in component state (NOT the scene store —
  // the store stays a pure geometry wrapper; ADR-008 #10). 2.5.2b reads this
  // list positionally for distance/angle/dihedral.
  const [selection, setSelection] = useState<number[]>([]);
  // ── TEMPORARY SEAM 2c1→2c2 ──────────────────────────────────────────────────
  // The viewer now speaks AtomId (2c1); selection/measure/edit-plan/constraints
  // still key on the positional global index. This is the ONE explicit, named
  // adapter that converts the pick's AtomId back to a global position, through the
  // same table the viewer draws with (`buildViewerAtomTable` — the pure derivation
  // of the current scene, so it agrees with the geometry that produced the pick).
  // We deliberately do NOT read `pick.viewerIndex` (that is viewer space — using it
  // as an app id is the coupling 2c1 severs). TODO(2c2): delete this adapter — the
  // AtomId flows straight into an AtomId-keyed selection and `selectionSurvives`
  // gains its removal dividend.
  const onAtomPick = (pick: AtomPick) => {
    const current = useSceneStore.getState().scene;
    if (!current) return;
    const globalIndex = buildViewerAtomTable(current).viewerIndexOf(pick.atomId);
    if (globalIndex === undefined) return;
    setSelection((sel) => toggleAtom(sel, globalIndex));
  };
  const clearSelection = () => setSelection([]);

  // Show every atom's global index in the 3D view (2.5.2e-1). Off by default;
  // selected atoms are numbered regardless (MoleculeViewer). Toggling this
  // redraws labels only — no model reload, no camera move.
  const [showNumbers, setShowNumbers] = useState(false);

  // Viewer theme (2.5.2e-2) — persisted in the `settings` table under
  // `viewer_theme`; loaded on mount, saved on change. `viewerTheme` falls back to
  // dark for an unknown/absent value.
  const [themeId, setThemeId] = useState<string>("dark");
  const theme = viewerTheme(themeId);
  useEffect(() => {
    invoke<Record<string, string>>("get_settings")
      .then((s) => setThemeId(s.viewer_theme ?? "dark"))
      .catch(() => {});
  }, []);
  const changeTheme = (id: string) => {
    setThemeId(id);
    invoke("set_setting", { key: "viewer_theme", value: id }).catch(() => {});
  };

  // Fullscreen viewer (2.5.2e-2) — a VIEW mode, deliberately NOT persisted. Only
  // a CSS class on the viewer container changes (position: fixed); MoleculeViewer
  // keeps its tree position, so it is NOT remounted and the camera survives. A
  // ref mirrors the state for the keydown handler so Esc priority doesn't depend
  // on effect mount order.
  const [fullscreen, setFullscreen] = useState(false);
  const fullscreenRef = useRef(false);
  fullscreenRef.current = fullscreen;
  // A clean canvas in either mode is just closing the dock sections (the icon
  // rail stays) — no separate rail-collapse state since unit 2b-ux.

  // ── Edit mode (2.5.2d) ──────────────────────────────────────────────────────
  // `previewScene` is the sidecar's proposed geometry shown ONLY in the viewer —
  // the store Scene and Monaco are untouched until Apply (the 2.5.1 decision).
  // Undo is now the operation log's undo (deep, not one-step) — unit 2b.
  const [previewScene, setPreviewScene] = useState<Scene | null>(null);
  // "Move the other fragment instead" — flip to the plan's alternative orientation
  // (2.5.2d-2). Reset when the selection/scene changes (the plan is different).
  const [preferAlternative, setPreferAlternative] = useState(false);
  // The base plan considers both chain orientations; `effectivePlan` applies the
  // user's orientation choice. Both drive the edit UI AND the mask glow.
  const basePlan = scene ? planEdit(scene, selection) : null;
  const editPlan =
    preferAlternative && basePlan?.kind === "ready" && basePlan.alternative
      ? swapToAlternative(basePlan)
      : basePlan;
  const fragmentName = (id: string | undefined | null) =>
    (id && scene?.fragments.find((f) => f.id === id)?.name) || null;
  const movingFragmentName =
    editPlan?.kind === "ready" ? fragmentName(editPlan.movingFragmentId) : null;
  const alternativeFragmentName =
    editPlan?.kind === "ready"
      ? fragmentName(editPlan.alternative?.movingFragmentId)
      : null;
  // Preview + orientation choice are transient: drop them when selection/scene changes.
  useEffect(() => {
    setPreviewScene(null);
    setPreferAlternative(false);
  }, [selection, scene]);

  // ── Intra-fragment mask resolution (2.5.3b) ─────────────────────────────────
  // A `needs-split` plan carries only cut/moving/within; the bond-graph split is
  // the sidecar's job (perception the browser can't do). Resolve it here so the
  // SAME mask drives the viewer glow AND set-internal — one source, not two.
  // The effect is RACE-GUARDED: if the selection changes mid-fetch, cleanup sets
  // `cancelled`, so a mask computed for an old pick never lands on the new one.
  const [splitMask, setSplitMask] = useState<number[] | null>(null);
  const [splitError, setSplitError] = useState<string | null>(null);
  const [splitResolving, setSplitResolving] = useState(false);
  const splitPlan = editPlan?.kind === "needs-split" ? editPlan : null;
  const splitKey = splitPlan
    ? `${splitPlan.op}:${splitPlan.indices.join(",")}`
    : null;
  useEffect(() => {
    if (!scene || !splitPlan) {
      setSplitMask(null);
      setSplitError(null);
      setSplitResolving(false);
      return;
    }
    let cancelled = false;
    setSplitMask(null);
    setSplitError(null);
    setSplitResolving(true);
    postSidecar<{ mask: number[] }>("/geometry/rotatable-mask", {
      xyz: mergeToXyz(scene),
      cut: splitPlan.cut,
      moving: splitPlan.moving,
      within: splitPlan.within,
    })
      .then((resp) => {
        if (cancelled) return;
        // Re-run the reference-atom rule on the RESOLVED mask (2.5.4a fix): the
        // split doesn't know which atoms were references, so a reference can land
        // on the moving side (butane angle(3,1,2) → mask ∋ ref 3) and set-internal
        // would 422 at Apply. The SAME pure check as `orientationFor`; on a
        // violation, refuse HERE with an explanation in terms of the selection,
        // never the sidecar's inter-fragment wording.
        const refs = splitPlan.indices.filter((i) => i !== splitPlan.moving);
        const violation = maskRoleViolation(resp.mask, splitPlan.moving, refs);
        if (violation) {
          setSplitError(
            explainSplitViolation(scene, splitPlan.cut, splitPlan.moving, violation),
          );
        } else {
          setSplitMask(resp.mask);
        }
        setSplitResolving(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setSplitError(e instanceof Error ? e.message : String(e));
        setSplitResolving(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [splitKey, scene]);
  // EditPanel hands up the typed op + its resultant snapshot; the store appends
  // it (→ the Scene→Monaco effect injects the new coords). Undo/redo are the
  // log's, not a local one-step stash (unit 2b).
  const applyEdit = (op: Op, newScene: Scene) => {
    commit(op, newScene);
    setSaved(false);
  };

  // React to a composition change via `compositionSignature` (the same
  // primitive the viewer uses to decide when to re-zoom). A coordinate-only edit
  // (same signature) leaves the selection untouched. On a real change, ask
  // `selectionSurvives`: an unchanged signature or a pure append (a new fragment
  // is always appended LAST, so existing indices don't shift) keeps the
  // selection; any other change (fragment removed, composition changed, scene
  // cleared) clears it OUTRIGHT — no remap. `validateSelection` stays a second
  // echelon (mainly the append path and defensive `scene → null`), because a
  // range-only check survives an index shift and would silently re-point a pick.
  const lastSelCompRef = useRef<string | null>(null);
  useEffect(() => {
    const sig = scene ? compositionSignature(scene) : null;
    const prev = lastSelCompRef.current;
    if (sig === prev) return;
    lastSelCompRef.current = sig;
    if (!selectionSurvives(prev, sig)) {
      setSelection((sel) => (sel.length ? [] : sel));
    } else {
      setSelection((sel) => validateSelection(sel, scene));
    }
  }, [scene]);

  // ── Constraint composition-change warning (2.5.4b, ЗАХИСТ 2) ────────────────
  // A constraint written at 38 atoms and then a fragment removed → 33 atoms is a
  // silent trap: Scene→Monaco rewrites ONLY the coordinate block, so the `%geom`
  // indices stay as written — now pointing at different atoms (or out of range →
  // ORCA segfaults). We do NOT remap or rewrite them (there is no operational
  // definition of "the same atom" after a removal — the same 2.5.2a call we made
  // for selection). Instead: when the composition signature moves while
  // constraints exist, warn and let the user verify by eye. SAME
  // `compositionSignature`, no second notion of "composition changed".
  const [constraintCompWarn, setConstraintCompWarn] = useState(false);
  const contentRef = useRef(content);
  contentRef.current = content;
  const lastConstraintSigRef = useRef<string | null>(null);
  useEffect(() => {
    const sig = scene ? compositionSignature(scene) : null;
    const prev = lastConstraintSigRef.current;
    if (sig === prev) return;
    lastConstraintSigRef.current = sig;
    // Skip the initial signature (mount / job restore is not a "change").
    if (prev !== null && (parseConstraintsBlock(contentRef.current)?.length ?? 0) > 0) {
      setConstraintCompWarn(true);
    }
  }, [scene]);
  // The warning is meaningless with no constraints — clear it the moment the
  // block is emptied (via delete, or a manual edit in Monaco).
  useEffect(() => {
    if ((parseConstraintsBlock(content)?.length ?? 0) === 0) setConstraintCompWarn(false);
  }, [content]);

  // "Constrain selection" (2.5.4b): freeze the 2/3/4-atom coordinate. One data
  // path — parse the current text, append (no duplicate), write back through
  // `injectConstraints`; the panel re-reads the text. No parallel state.
  const constrainSelection = (value?: number) => {
    const c = constraintFromSelection(selection, value);
    if (!c) return;
    const ins = inspectConstraintsBlock(content);
    if (ins.kind === "unrecognised") return; // never rewrite a block we can't read (2.5.5)
    const existing = ins.kind === "parsed" ? ins.cs : [];
    if (existing.some((e) => sameConstraint(e, c))) return;
    setContent(injectConstraints(content, [...existing, c]));
    setError(null);
  };

  // ── xTB pre-optimization (2.5.5; off-thread 2.5.5-fix) ──────────────────────
  // Relax the WHOLE scene with GFN2-xTB while holding the text's constraints (the
  // source of truth), then replace all coordinates. `xtb_optimize` is now a
  // STARTER that returns immediately (the run lives on a Rust thread and reports
  // via `xtb:done`/`xtb:error` events), so the window stays responsive and Cancel
  // is actually deliverable. The applied geometry is a `replace-all-atoms` op on
  // the log (undo/redo via the log, like every other edit — unit 2b).
  const [xtbBusy, setXtbBusy] = useState(false);
  const [xtbError, setXtbError] = useState<string | null>(null);
  const [xtbErrorDir, setXtbErrorDir] = useState<string | null>(null);
  const [xtbNote, setXtbNote] = useState<string | null>(null);
  // Live progress (2.5.5-fix-2): the current opt cycle + a ticking elapsed counter,
  // so five minutes of silence never happens again — "very long" is visible now.
  const [xtbCycle, setXtbCycle] = useState<number | null>(null);
  const [xtbElapsed, setXtbElapsed] = useState(0);
  const xtbStartRef = useRef(0);
  // The scene the run was launched against. A result is applied ONLY to this exact
  // scene — the same stale-response guard as the split-mask fetch (2.5.3b): if the
  // scene changed while xtb ran, the result is dropped, never applied to a scene it
  // wasn't computed from (a mismatched atom count / wrong coordinates).
  const xtbSceneRef = useRef<Scene | null>(null);
  const runXtbPreopt = async () => {
    if (!scene) return;
    setXtbError(null);
    setXtbErrorDir(null);
    setXtbNote(null);
    setXtbCycle(null);
    setXtbElapsed(0);
    xtbStartRef.current = Date.now();
    xtbSceneRef.current = scene;
    setXtbBusy(true);
    try {
      await invoke("xtb_optimize", {
        xyz: mergeToXyz(scene),
        charge: totalCharge(scene),
        multiplicity: scene.multiplicity,
        constraints, // parsed from the text; unrecognised → button is disabled
        timeoutSecs: null,
      });
      // Started — the result/errors arrive as events (handled below).
    } catch (e) {
      // Synchronous rejection only: busy slot / bad input. No run started.
      setXtbBusy(false);
      xtbSceneRef.current = null;
      setXtbError(e instanceof Error ? e.message : String(e));
    }
  };
  const cancelXtb = () => {
    invoke("xtb_cancel").catch(() => {});
  };

  // Result/error events from the off-thread xtb run. Subscribed only while busy;
  // the `cancelled` flag set in cleanup drops a late event (the 2.5.3b pattern),
  // and the captured-scene identity check drops a result whose scene has since
  // changed (applying it would clobber the user's edit / mismatch atom count).
  useEffect(() => {
    if (!xtbBusy) return;
    let cancelled = false;
    const unlisten = Promise.all([
      listen<{
        xyz: string;
        wall_time_secs: number;
        held: { kind: string; deviation: number; unit: string }[];
      }>("xtb:done", (event) => {
        if (cancelled) return;
        setXtbBusy(false);
        const captured = xtbSceneRef.current;
        xtbSceneRef.current = null;
        if (!xtbResultApplies(captured, useSceneStore.getState().scene) || !captured) {
          setXtbNote("Pre-optimization discarded — the scene changed while it ran.");
          return;
        }
        try {
          const atoms = parseAtomLines(event.payload.xyz.trim().split("\n").slice(2));
          if (!atoms) throw new Error("xtb returned a geometry OrcaStudio couldn't parse");
          commit(
            { type: "replace-all-atoms", edit: { via: "xtb" } },
            replaceAllAtoms(captured, atoms),
          );
          setSaved(false);
          const held = event.payload.held.length
            ? " · held " +
              event.payload.held
                .map((h) => `${h.kind} ±${h.deviation.toFixed(3)}${h.unit}`)
                .join(", ")
            : "";
          setXtbNote(`Pre-optimized in ${event.payload.wall_time_secs.toFixed(1)}s${held}`);
        } catch (err) {
          setXtbError(err instanceof Error ? err.message : String(err));
        }
      }),
      listen<{ message: string; dir: string | null }>("xtb:error", (event) => {
        if (cancelled) return;
        setXtbBusy(false);
        xtbSceneRef.current = null;
        setXtbError(event.payload.message);
        setXtbErrorDir(event.payload.dir);
      }),
      listen<{ cycle: number }>("xtb:progress", (event) => {
        if (cancelled) return;
        setXtbCycle(event.payload.cycle);
      }),
    ]);
    return () => {
      cancelled = true;
      unlisten.then((fns) => fns.forEach((f) => f()));
    };
  }, [xtbBusy]);

  // Tick the elapsed counter every second while xtb runs, so the time is visibly
  // live (a stalled cycle number + a growing clock = "this is taking too long").
  useEffect(() => {
    if (!xtbBusy) return;
    const id = setInterval(
      () => setXtbElapsed(Math.round((Date.now() - xtbStartRef.current) / 1000)),
      1000,
    );
    return () => clearInterval(id);
  }, [xtbBusy]);

  // Esc — ONE handler, explicit priority (2.5.2e-2 decision): in fullscreen it
  // exits fullscreen and does NOTHING else; otherwise it clears the selection.
  // One Esc = one action. The precedence is a branch in a single handler, read
  // from `fullscreenRef`, so it does NOT depend on the mount order of separate
  // keydown effects (two listeners would race on who runs first).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (fullscreenRef.current) {
        setFullscreen(false);
        return; // exit fullscreen; selection is left untouched
      }
      setSelection((sel) => (sel.length ? [] : sel));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Lazy-load saved molecules whenever the Fragments dock section opens (its
  // "From library" source lists them).
  useEffect(() => {
    if (!openDock.fragments) return;
    invoke<Molecule[]>("list_molecules")
      .then(setLibMolecules)
      .catch(() => setLibMolecules([]));
  }, [openDock.fragments]);

  /**
   * The single road every "add a fragment" source takes (ADR-008): place the new
   * fragment clear of whatever is already there, then append it. Works for the
   * first fragment too — `placeFragment` on an empty scene is the identity and
   * `addFragment` seeds a one-fragment scene — so there is no "first replaces,
   * rest add" branch (d-1 removed exactly that split).
   */
  const addFragmentToScene = (fragment: RawFragment) => {
    const current = useSceneStore.getState().scene ?? {
      fragments: [],
      multiplicity: 1,
      nextAtomId: 0,
    };
    addFragment(placeFragment(current, fragment));
    setSaved(false);
  };

  // Initialise the (module-singleton) scene store for this screen. useLayoutEffect
  // (not useEffect) so the reset runs before paint — the screen remounts on every
  // navigation, and a plain effect would flash the previous visit's molecule.
  //  - keepScene → leave the store untouched (a conformer was just applied to it);
  //  - iterated job → restore its OPERATION LOG, cross-checked against the snapshot
  //    (the log is rejected loudly if it diverges — unit 2b main risk);
  //  - library molecule → a single "library" fragment (a seeded log);
  //  - otherwise → empty, clearing any scene left over from a prior visit.
  useLayoutEffect(() => {
    if (keepScene) {
      // "Use this conformer" already dispatched onto the store log; don't reset it.
    } else if (initialJob) {
      const { log: restoredLog, snapshotRejected: rejected, logRejected: logRej } =
        restoreSceneLog(
          initialJob.input_content,
          initialJob.scene_json,
          initialJob.scene_log_json ?? null,
        );
      installLog(restoredLog);
      setSnapshotRejected(rejected);
      setLogRejected(logRej);
    } else if (initialMolecule) {
      seedScene(
        sceneFromXyz(initialMolecule.xyz, {
          source: "library",
          sourceLabel: initialMolecule.id,
          name: initialMolecule.name,
          charge: initialMolecule.charge,
          multiplicity: initialMolecule.multiplicity,
        }),
        "library",
      );
    } else {
      seedScene(null, "text-adopt"); // empty log, clears any prior visit's scene
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Scene → content (ADR-008 #6): inject the merged coordinate block, leaving
  // the `!` line / `%` blocks / comments untouched. The guard skips the write
  // when the text already reflects this scene — preventing an echo after a
  // content→scene sync and never reformatting a manual edit.
  useEffect(() => {
    if (!scene) return;
    setContent((c) => {
      const parsed = sceneFromOrcaInput(c);
      if (parsed && xyzMatchesScene(scene, mergeToAtomLines(parsed))) return c;
      return injectSceneIntoInput(c, scene);
    });
  }, [scene]);

  // content → Scene (ADR-008 #6): on the same 500 ms debounce the preview used,
  // compare the editor's coordinate block against the scene via xyzMatchesScene
  // (parsed floats, never string compare). Match → leave the scene (the user was
  // editing keywords). Diverged → the text wins: collapse to it (a logged
  // CollapseFromText op — undoing it re-injects the old coords so no second
  // collapse fires, unit 2b test c). Block gone → clear the lineage (empty log).
  // Scene absent but a block appeared (template / generated input) → adopt it.
  useEffect(() => {
    const id = setTimeout(() => {
      const parsed = sceneFromOrcaInput(content);
      const current = useSceneStore.getState().scene;
      if (!parsed) {
        if (current) seedScene(null, "text-adopt");
        return;
      }
      if (!current) {
        seedScene(parsed, "text-adopt");
        return;
      }
      if (xyzMatchesScene(current, mergeToAtomLines(parsed))) return;
      collapseFromText(parsed.fragments[0].atoms);
    }, 500);
    return () => clearTimeout(id);
  }, [content, seedScene, collapseFromText]);

  const pickTemplate = (t: OrcaTemplate) => {
    setSelectedId(t.id);
    setContent(t.inputContent);
    if (!title.trim()) setTitle(t.name);
    setOpenSection(null); // got the input — collapse so the editor is visible
  };

  // Builder's "Generate Input" → replace editor content and collapse the panel.
  const handleGenerate = (newContent: string) => {
    setContent(newContent);
    setOpenSection(null);
  };

  // Add a curated reagent (BH₄⁻, H₂O, …) as a fragment.
  const addReagent = (lf: LibraryFragment) => {
    setError(null);
    addFragmentToScene(libraryFragmentToScene(lf));
  };

  // Add a saved library molecule as a fragment.
  const addLibraryMolecule = (m: Molecule) => {
    const s = sceneFromXyz(m.xyz, {
      source: "library",
      sourceLabel: m.id,
      name: m.name,
      charge: m.charge,
    });
    if (!s) return;
    setError(null);
    addFragmentToScene(s.fragments[0]);
    if (!title.trim()) setTitle(m.name);
  };

  // Import a structure file: `.xyz` is parsed locally, other formats are
  // converted to xyz by the sidecar (see import-file.ts). Added as an "import"
  // fragment (neutral charge 0).
  const importFile = async (file: File) => {
    try {
      const { atomLines } = await importStructureFile(file);
      setError(null);
      const base = file.name.replace(/\.[^.]+$/, "");
      const s = sceneFromAtomLines(atomLines, {
        source: "import",
        sourceLabel: file.name,
        name: base,
        charge: 0,
        multiplicity: 1,
      });
      if (!s) return;
      // Formula only means something for a single-molecule save; clear it when
      // this import *is* the whole molecule (empty scene), leave it otherwise.
      if (!useSceneStore.getState().scene) setFormula("");
      addFragmentToScene(s.fragments[0]);
      if (!title.trim()) setTitle(base);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  // Generate a 3D structure from SMILES via the sidecar (RDKit ETKDG + MMFF);
  // added as a "smiles" fragment with the formal charge RDKit derived.
  const generateFromSmiles = async () => {
    const s = smiles.trim();
    if (!s) return;
    setGenerating(true);
    setError(null);
    try {
      const status = await invoke<SidecarStatus>("get_sidecar_status");
      if (!status.port) throw new Error("Sidecar is not ready yet");
      const resp = await fetch(
        `http://127.0.0.1:${status.port}/smiles-to-3d`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ smiles: s }),
        },
      );
      if (!resp.ok) {
        let detail = `HTTP ${resp.status}`;
        try {
          const body = await resp.json();
          if (body?.detail) detail = String(body.detail);
        } catch {
          /* non-JSON error body — keep the status code */
        }
        throw new Error(detail);
      }
      const data = (await resp.json()) as {
        xyz: string;
        charge: number;
        formula: string;
      };
      const built = sceneFromXyz(data.xyz, {
        source: "smiles",
        sourceLabel: s,
        name: data.formula,
        charge: data.charge,
        multiplicity: 1,
      });
      if (!built) throw new Error("Sidecar returned a malformed structure");
      // Only the first molecule's formula is meaningful for Save to Library.
      if (!useSceneStore.getState().scene) setFormula(data.formula);
      addFragmentToScene(built.fragments[0]);
      if (!title.trim()) setTitle(data.formula);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setGenerating(false);
    }
  };

  // Save the current geometry as a new library molecule. Geometry (canonical)
  // comes from the scene; charge/multiplicity from the live `* xyz` header (so a
  // manual header edit is honoured, matching the pre-store behaviour); the
  // formula is whatever the last SMILES generation reported.
  const saveToLibrary = async () => {
    if (!scene) {
      setError("No coordinates in the input to save");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const header = sceneFromOrcaInput(content);
      const charge = header ? totalCharge(header) : totalCharge(scene);
      const multiplicity = header ? header.multiplicity : scene.multiplicity;
      await invoke<Molecule>("create_molecule", {
        name: title.trim() || "Untitled molecule",
        formula,
        xyz: mergeToXyz(scene, title.trim()),
        charge,
        multiplicity,
        tags: "",
      });
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const create = async (run: boolean) => {
    if (constraintBlockMessage) {
      // The single place the app refuses to run on input CONTENT — justified: the
      // alternative is ORCA reading past the atom array and crashing with no
      // diagnostic. The job's input is immutable once created, so a "draft" would
      // be an un-runnable landmine — block both Create and Create & Run.
      setError(constraintBlockMessage);
      return;
    }
    setCreating(true);
    setError(null);
    try {
      const job = await invoke<Job>("create_job", {
        title: title.trim() || "Untitled job",
        inputContent: content,
        // Persist the fragment layout so a later iteration restores it (ADR-008
        // #5). Written once, here; the job's input is immutable, so is this.
        sceneJson: scene ? serializeScene(scene) : null,
        // The operation log alongside it — history carries across iterations
        // (unit 2b). Both co-written; restore cross-checks them. Null when there
        // is no scene (empty log), keeping the two columns consistent.
        sceneLogJson: scene ? serializeLog(log) : null,
      });
      // The detail screen performs the actual submit (after attaching its log
      // listeners) so no early output lines are missed.
      if (run) onOpenDetail(job.id, true);
      else onCreatedDraft();
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  // "Find conformers" on one fragment: create + run a GOAT job. Its input is the
  // fragment ALONE (`goatInputForFragment`), and its `scene_json` is a
  // single-fragment scene of that SAME fragment — so the snapshot annotates its
  // own single-fragment input and `restoreScene` accepts it (ADR-008 #5; NOT the
  // whole scene, which wouldn't match — see wiki/modules/scene.md). `%pal` is
  // added by the backend's `align_pal_nprocs` at submit, same as every job.
  const findConformers = async (fragment: SceneFragment) => {
    setError(null);
    try {
      const job = await invoke<Job>("create_job", {
        title: `Conformer search — ${fragment.name}`,
        inputContent: goatInputForFragment(fragment),
        sceneJson: serializeScene({
          fragments: [fragment],
          multiplicity: 1,
          nextAtomId: nextAtomIdFor([fragment]),
        }),
        // A GOAT search job carries no editing history — a New iteration of it
        // seeds a fresh log from its snapshot (the "legacy" restore branch).
        sceneLogJson: null,
      });
      onOpenDetail(job.id, true); // queue + run, then open its detail
    } catch (e) {
      setError(String(e));
    }
  };

  // ── ЗАХИСТ 1 — range-check constraints before any run ───────────────────────
  // ORCA does not validate constraint indices; an out-of-range one segfaults with
  // no error (verified, `wiki/orca/constraints.md`). Parse straight from the text
  // (the source of truth) and check against the geometry's atom count. An
  // unrecognised block (2.5.5) yields no parsed constraints — we can't validate
  // what we don't read, and the panel/buttons go read-only so we never rewrite it.
  const constraintsInspection = inspectConstraintsBlock(content);
  const constraints =
    constraintsInspection.kind === "parsed" ? constraintsInspection.cs : [];
  const constraintsUnrecognised = constraintsInspection.kind === "unrecognised";
  const indexIssues = scene
    ? constraintIndexIssues(constraints, atomCount(scene))
    : [];
  const constraintBlockMessage =
    indexIssues.length > 0 && scene
      ? "Can't create or run this job — " +
        `${indexIssues.length} constraint${indexIssues.length > 1 ? "s" : ""} ` +
        `reference atom indices that don't exist in this geometry (it has ` +
        `${atomCount(scene)} atoms, valid 0–${atomCount(scene) - 1}). ORCA does ` +
        "NOT range-check constraint indices — it segfaults instead of reporting " +
        "an error, so the run is blocked until they are fixed or removed: " +
        indexIssues
          .map(
            (it) =>
              `${constraintTypeLabel(it.constraint.kind)} on ` +
              `${it.badIndices.map((i) => `#${i}`).join(", ")}`,
          )
          .join("; ") +
        "."
      : null;

  const canCreate =
    content.trim().length > 0 && !creating && !constraintBlockMessage;

  // The Add-Fragment sources (reagents / import / SMILES / library) — the palette
  // half of the Fragments dock section. Reachable inside fullscreen (unit 2b-ux).
  const addFragmentSources = (
    <div className="add-fragment-body">
      <div className="add-source">
        <div className="add-source-title muted">Reagents</div>
        <div className="reagent-chips">
          {FRAGMENT_LIBRARY.map((lf) => (
            <button
              key={lf.key}
              className="chip"
              title={lf.provenance}
              onClick={() => addReagent(lf)}
            >
              {lf.name}{" "}
              <span className="muted">
                {signed(lf.charge)} · {lf.atoms.length}a
              </span>
            </button>
          ))}
        </div>
      </div>

      <div className="add-source">
        <div className="add-source-title muted">Import file or SMILES</div>
        <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
          <button className="btn btn-sm" onClick={() => fileInputRef.current?.click()}>
            Import file
          </button>
          <input
            className="input mono import-smiles"
            placeholder="SMILES, e.g. CCO"
            value={smiles}
            onChange={(e) => setSmiles(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") generateFromSmiles();
            }}
            spellCheck={false}
          />
          <button
            className="btn btn-sm"
            onClick={generateFromSmiles}
            disabled={generating || !smiles.trim()}
          >
            {generating ? "Generating…" : "Generate 3D"}
          </button>
        </div>
      </div>

      <div className="add-source">
        <div className="add-source-title muted">From library</div>
        {libMolecules.length === 0 ? (
          <div className="muted" style={{ fontSize: 13 }}>
            No saved molecules yet.
          </div>
        ) : (
          <div className="lib-mol-list">
            {libMolecules.map((m) => (
              <button
                key={m.id}
                className="lib-mol-row"
                onClick={() => addLibraryMolecule(m)}
              >
                <span>{m.name}</span>
                <span className="muted mono">
                  {m.formula || "—"} · {signed(m.charge)}
                </span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );

  // The six dock sections, in order of use (unit 2b-ux). Bodies are the EXISTING
  // panels with their existing props / empty states — nothing new invented; only
  // relocated from the old stacked rail into the viewer-first right dock.
  const dockSections: DockSection[] = [
    {
      id: "selection",
      label: "Selection & Measure",
      short: "Sel",
      glyph: "◎",
      body: scene ? (
        <AtomInspector
          scene={scene}
          selection={selection}
          onClear={clearSelection}
          onConstrain={constrainSelection}
          constrainDisabledReason={
            constraintsUnrecognised
              ? "The constraint block contains syntax OrcaStudio doesn't recognise — edit it in the input editor."
              : null
          }
        />
      ) : null,
    },
    {
      id: "edit",
      label: "Edit geometry",
      short: "Edit",
      glyph: "✎",
      body:
        scene && editPlan && selection.length >= 2 ? (
          <EditPanel
            scene={scene}
            plan={editPlan}
            movingFragmentName={movingFragmentName}
            alternativeFragmentName={alternativeFragmentName}
            splitMask={splitMask}
            splitError={splitError}
            splitResolving={splitResolving}
            onSwitchOrientation={() => setPreferAlternative((v) => !v)}
            onPreview={setPreviewScene}
            onApplied={applyEdit}
          />
        ) : null,
    },
    {
      id: "fragments",
      label: "Fragments",
      short: "Frag",
      glyph: "⬡",
      body: (
        <>
          {addFragmentSources}
          <FragmentList onFindConformers={findConformers} />
        </>
      ),
    },
    {
      id: "constraints",
      label: "Constraints",
      short: "Cons",
      glyph: "⊗",
      body: scene ? (
        <ConstraintPanel
          scene={scene}
          content={content}
          onChange={setContent}
          compositionChanged={constraintCompWarn}
          onDismissComposition={() => setConstraintCompWarn(false)}
        />
      ) : null,
    },
    {
      id: "history",
      label: "History",
      short: "Hist",
      glyph: "↺",
      body: <HistoryPanel />,
    },
    {
      id: "actions",
      label: "Actions",
      short: "Act",
      glyph: "▶",
      body: scene ? (
        <div className="xtb-panel">
          <div className="xtb-row">
            <button
              className="btn btn-sm"
              onClick={runXtbPreopt}
              disabled={xtbBusy || constraintsUnrecognised}
              title={
                constraintsUnrecognised
                  ? "The constraint block is unrecognised — fix it in the editor first."
                  : "GFN2-xTB relax the geometry, holding the constraints in the input text."
              }
            >
              {xtbBusy ? "Pre-optimizing…" : "xTB pre-optimize"}
            </button>
            {xtbBusy ? (
              <button className="btn btn-sm" onClick={cancelXtb}>
                Cancel
              </button>
            ) : null}
          </div>
          {xtbBusy ? (
            <div className="muted xtb-note xtb-progress">
              {formatXtbProgress(xtbCycle, xtbElapsed)}
            </div>
          ) : null}
          {constraintsUnrecognised ? (
            <div className="muted xtb-note">
              constraint block unreadable — fix it in the editor to pre-optimize
            </div>
          ) : null}
          {xtbNote ? <div className="muted xtb-note">{xtbNote}</div> : null}
          {xtbError ? (
            <div className="edit-error edit-error-severe xtb-error">
              <div className="xtb-error-msg">{xtbError}</div>
              {xtbErrorDir ? (
                <input
                  className="input mono xtb-error-dir"
                  readOnly
                  value={xtbErrorDir}
                  onFocus={(e) => e.currentTarget.select()}
                  title="Diagnostic files — select and copy this path"
                />
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null,
    },
  ];

  return (
    <div className="screen new-job">
      <div className="row" style={{ gap: 16 }}>
        <div className="field" style={{ flex: 1 }}>
          <label className="label" htmlFor="job-title">
            Job title
          </label>
          <input
            id="job-title"
            className="input"
            placeholder="e.g. water opt/freq"
            value={title}
            onChange={(e) => setTitle(e.currentTarget.value)}
            spellCheck={false}
          />
        </div>
        <div className="row" style={{ alignSelf: "flex-end" }}>
          <button
            className="btn"
            onClick={() => create(false)}
            disabled={!canCreate}
          >
            {creating ? "Creating…" : "Create Job"}
          </button>
          <button
            className="btn btn-primary"
            onClick={() => create(true)}
            disabled={!canCreate}
          >
            Create &amp; Run
          </button>
        </div>
      </div>

      {constraintBlockMessage ? (
        <div className="banner err constraint-block">{constraintBlockMessage}</div>
      ) : null}

      <input
        ref={fileInputRef}
        type="file"
        accept={IMPORT_ACCEPT}
        hidden
        onChange={(e) => {
          const f = e.currentTarget.files?.[0];
          if (f) importFile(f);
          e.currentTarget.value = ""; // allow re-picking the same file
        }}
      />

      {/* Add Fragment moved into the workspace dock's Fragments section (unit
          2b-ux) — reachable in fullscreen. Save to Library stays here. */}
      <div className="import-row">
        <button
          className="btn btn-sm"
          onClick={saveToLibrary}
          disabled={saving || !scene}
          title="Save the current geometry as a library molecule"
        >
          {saving ? "Saving…" : "Save to Library"}
        </button>
      </div>

      {error ? <div className="banner err">{error}</div> : null}
      {saved ? <div className="banner ok">Saved to library</div> : null}
      {snapshotRejected ? (
        <div className="banner warn">
          The fragment layout from the source job didn't match its input, so it
          was dropped — this starts from a single fragment.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={() => setSnapshotRejected(false)}
          >
            Dismiss
          </button>
        </div>
      ) : null}
      {logRejected && logRejected !== "legacy" ? (
        <div className="banner warn">
          The edit history saved with this job didn&apos;t match its geometry
          snapshot ({logRejected}), so it was not loaded — the geometry is intact,
          history begins here.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={() => setLogRejected(null)}
          >
            Dismiss
          </button>
        </div>
      ) : null}
      {resetNotice ? (
        <div className="banner warn">
          Coordinates were edited manually — {resetNotice.fragmentCount} fragments
          merged into one.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={() => undo()}
          >
            Undo
          </button>
          <button
            className="btn btn-sm"
            style={{ marginLeft: 6 }}
            onClick={() => dismissResetNotice()}
          >
            Dismiss
          </button>
        </div>
      ) : null}

      <div className="input-builder">
        <button
          className="builder-toggle"
          onClick={() =>
            setOpenSection((s) => (s === "builder" ? null : "builder"))
          }
          aria-expanded={openSection === "builder"}
        >
          <span className="builder-caret">
            {openSection === "builder" ? "▾" : "▸"}
          </span>
          Input Builder
          <span className="muted" style={{ marginLeft: 8 }}>
            method · basis · solvation → generates the input
          </span>
        </button>
        {openSection === "builder" ? (
          <InputBuilderForm
            currentContent={content}
            onGenerate={handleGenerate}
          />
        ) : null}
      </div>

      <div className="input-builder">
        <button
          className="builder-toggle"
          onClick={() =>
            setOpenSection((s) => (s === "templates" ? null : "templates"))
          }
          aria-expanded={openSection === "templates"}
        >
          <span className="builder-caret">
            {openSection === "templates" ? "▾" : "▸"}
          </span>
          Templates
          <span className="muted" style={{ marginLeft: 8 }}>
            ready-made inputs for common job types
          </span>
        </button>
        {openSection === "templates" ? (
          <div className="template-groups">
            {CATEGORY_LABELS.map(({ category, label }) => (
              <div key={category}>
                <div
                  className="template-group-title"
                  style={{ color: "var(--muted-2)", marginTop: 4 }}
                >
                  {label}
                </div>
                <div className="template-grid">
                  {ORCA_TEMPLATES.filter((t) => t.category === category).map(
                    (t) => (
                      <button
                        key={t.id}
                        className={
                          "template-card" +
                          (selectedId === t.id ? " selected" : "")
                        }
                        onClick={() => pickTemplate(t)}
                      >
                        <div className="tname">{t.name}</div>
                        <div className="tdesc">{t.description}</div>
                      </button>
                    ),
                  )}
                </div>
              </div>
            ))}
          </div>
        ) : null}
      </div>

      <div className={"editor-viewer-split" + (anyDockOpen ? " dock-open" : "")}>
        <div className="editor-wrap">
          <InputEditor value={content} onChange={setContent} />
        </div>
        {/* The workbench holds BOTH the viewer and the workspace dock, in ONE DOM
            structure for both modes (2.5.2e-3b / 2b-ux). The fullscreen toggle
            changes ONLY the className here — MoleculeViewer and the dock keep their
            tree positions, so React never remounts them and the camera survives. A
            ROW in both modes now (viewer-first): the viewer takes the width, the
            dock is a thin icon rail on the right (fullscreen just goes fixed). The
            viewer's ResizeObserver fires viewer.resize() on the box change — the
            one shared resize mechanism, same as before. */}
        <div
          className={
            "viewer-column" + (fullscreen ? " viewer-column-fullscreen" : "")
          }
        >
          <div className="viewer-panel">
            {scene ? (
              <>
                <div className="viewer-toolbar">
                  <label
                    className="viewer-toggle"
                    title="Label every atom with its global index"
                  >
                    <input
                      type="checkbox"
                      checked={showNumbers}
                      onChange={(e) => setShowNumbers(e.target.checked)}
                    />
                    Numbers
                  </label>
                  <select
                    className="viewer-theme-select"
                    value={themeId}
                    onChange={(e) => changeTheme(e.target.value)}
                    title="Viewer background theme"
                  >
                    {VIEWER_THEMES.map((t) => (
                      <option key={t.id} value={t.id}>
                        {t.label}
                      </option>
                    ))}
                  </select>
                  <button
                    className="btn btn-sm"
                    onClick={() => setFullscreen((f) => !f)}
                    title={
                      fullscreen ? "Exit fullscreen (Esc)" : "Fullscreen viewer"
                    }
                  >
                    {fullscreen ? "Exit" : "Expand"}
                  </button>
                </div>
                <MoleculeViewer
                  scene={previewScene ?? scene}
                  selection={selection}
                  onAtomPick={onAtomPick}
                  showAtomNumbers={showNumbers}
                  theme={theme}
                  maskHighlight={
                    editPlan?.kind === "ready"
                      ? editPlan.mask
                      : editPlan?.kind === "needs-split"
                        ? splitMask ?? undefined
                        : undefined
                  }
                />
              </>
            ) : (
              <div className="viewer-empty muted">No coordinates in input</div>
            )}
          </div>
          {/* The workspace dock — ONE instance, shared by both modes (unit 2b-ux;
              never duplicated: two AtomInspectors would be two copies of selection
              state). Viewer-first: a thin icon rail that expands per section, the
              SAME dock inside fullscreen so every section (incl. Add Fragment) is
              reachable without leaving it. The panels are unchanged — only moved. */}
          <EditorDock sections={dockSections} open={openDock} onToggle={toggleDock} />
        </div>
      </div>
    </div>
  );
}
