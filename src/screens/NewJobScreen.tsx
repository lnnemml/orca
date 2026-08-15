import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { InputEditor } from "../editor/InputEditor";
import { MoleculeViewer, type AtomPick } from "../viewer/MoleculeViewer";
import { VIEWER_THEMES, viewerTheme } from "../viewer/theme";
import { importStructureFile, IMPORT_ACCEPT } from "../viewer/import-file";
import { InputBuilderForm } from "../input-builder/InputBuilderForm";
import type { NebPayload } from "../reactions/NebBuilderSection";
import { hasNebKeyword } from "../scene/neb";
import { CrestPanel } from "../crest/CrestPanel";
import { useSceneStore } from "../scene/store";
import { FragmentList } from "../scene/FragmentList";
import { HistoryPanel } from "../scene/HistoryPanel";
import { EditorDock, type DockSection } from "../scene/EditorDock";
import { AtomInspector } from "../scene/AtomInspector";
import { EditPanel } from "../scene/EditPanel";
import { RotatePanel } from "../scene/RotatePanel";
import { GuidedPlacementPanel } from "../scene/GuidedPlacementPanel";
import { DEFAULT_ROTATE_OVERLAY, type RotateOverlay } from "../viewer/rotate-overlay";
import { bondKey, type BondKey } from "../viewer/bond-display";
import { ConstraintPanel } from "../scene/ConstraintPanel";
import { ScanPanel } from "../scene/ScanPanel";
import {
  parseConstraintsBlock,
  inspectConstraintsBlock,
  injectConstraints,
  constraintIndexIssues,
  constraintFromSelection,
  sameConstraint,
} from "../scene/constraints";
import {
  inspectScanBlock,
  injectScan,
  parseScanCoordinates,
  scanOptIssue,
  scanFromSelection,
} from "../scene/scan";
import { measureSelection } from "../scene/measure";
import {
  planEdit,
  swapToAlternative,
  maskRoleViolation,
  explainSplitViolation,
  type ComponentLookup,
} from "../scene/edit-plan";
import { type MoveMode } from "../scene/moving-set";
import { postSidecar } from "../sidecar-client";
import { toggleAtom, filterSelection } from "../scene/selection";
import {
  detectClashes,
  clashAtomIds,
  undeterminedElements,
  type ClashReport,
} from "../scene/clash";
import { placeFragment } from "../scene/placement";
import {
  FRAGMENT_LIBRARY,
  libraryFragmentToScene,
  type LibraryFragment,
} from "../scene/fragment-library";
import { userReagentToFragment, fragmentToXyz } from "../scene/reagent-catalog";
import {
  atomCount,
  compositionSignature,
  globalIndexOfAtom,
  injectSceneIntoInput,
  locateAtom,
  mergeToAtomLines,
  nextAtomIdFor,
  parseAtomLines,
  replaceAllAtoms,
  xtbResultApplies,
  mergeToXyz,
  sceneFromAtomLines,
  sceneFromOrcaInput,
  adoptPreservesScene,
  sceneFromXyz,
  serializeScene,
  totalCharge,
  xyzMatchesScene,
} from "../scene/scene";
import { formatXtbProgress } from "../scene/xtb-progress";
import { formalChargeConsistency } from "../scene/formal-charge";
import { restoreSceneLog, type LogRejection } from "../scene/restore";
import {
  resolveCarryForwardGeometry,
  geometryMatchesFinal,
  carryForwardProvenanceComment,
  withProvenanceComment,
} from "../scene/carryForward";
import { finalGeometryXyz } from "../export/exporters";
import { serializeLog, type Op } from "../scene/oplog";
import { goatInputForFragment } from "../scene/ensemble";
import type { RawFragment, Scene, SceneFragment } from "../scene/types";
import type { AtomId } from "../scene/ids";
import {
  CATEGORY_LABELS,
  ORCA_TEMPLATES,
  type OrcaTemplate,
} from "../templates/orca-templates";
import type { Group, Job, Molecule, ParsedResults, SidecarStatus } from "../types";
import { GroupSelect, resolveGroupAssignment } from "../groups/GroupSelect";

/** Charge with an explicit sign: `0`, `-1`, `+1`. */
function signed(n: number): string {
  return n > 0 ? `+${n}` : String(n);
}

/** The default span (end − start) for a "Scan this coordinate" default range — an
 * editable starting point: a 1 Å bond stretch, a 30° angle sweep, a 60° dihedral
 * sweep. The panel lets the user change start/end/points afterwards. */
const DEFAULT_SCAN_SPAN: Record<"distance" | "angle" | "dihedral", number> = {
  distance: 1.0,
  angle: 30,
  dihedral: 60,
};

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
  /** Assign-on-create (Phase 4.7.3): the active group in the Jobs sidebar, or null
   * when "All jobs"/"Ungrouped" is active. A newly created job inherits it via
   * `move_job` (pure composition — no Rust change). */
  activeGroupId?: string | null;
}

export function NewJobScreen({
  onCreatedDraft,
  onOpenDetail,
  initialMolecule,
  initialJob,
  keepScene,
  activeGroupId,
}: NewJobScreenProps) {
  // Seed title/content from an iterated job (marked so kinship shows in the job
  // list) or a library molecule; both before first paint via useState.
  const [title, setTitle] = useState(
    initialJob ? `${initialJob.title} (iteration)` : initialMolecule?.name ?? "",
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [content, setContent] = useState(initialJob?.input_content ?? "");
  // ── The coordinate block is a read-only projection of the Scene (unit 2d) ─────
  // Variant C (ADR-010 authority split: input text owns chemistry `!`/`%`, the
  // Scene owns geometry). A hand-edit of the `* xyz … *` rows never flows into the
  // Scene — the Monaco→Scene effect REVERTS the block to the projection (keeping
  // the `!`/`%` edits) instead of the pre-2d collapse. Geometry hand-editing moves
  // to two conscious doors, not a silent text-edit:
  //   • Import xyz as fragment — paste xyz → `addFragment` (the typical path);
  //   • Replace input — a one-shot escape: unlock the whole buffer, paste a
  //     different calculation, Adopt it as a fresh Scene (`text-adopt`). After
  //     adoption the block is read-only again.
  const [coordNotice, setCoordNotice] = useState(false); // a block hand-edit was reverted
  // A drag fell back to a whole-fragment move because the sidecar couldn't resolve
  // the connected component (Stage 3.x) — surfaced honestly, never a silent move.
  const [dragFallbackNotice, setDragFallbackNotice] = useState<string | null>(null);
  const [importXyzOpen, setImportXyzOpen] = useState(false);
  const [importXyz, setImportXyz] = useState("");
  const [replaceConfirm, setReplaceConfirm] = useState(false); // "Replace input?" prompt shown
  const [replaceMode, setReplaceMode] = useState(false); // buffer unlocked for a re-adopt
  const replaceSnapshotRef = useRef(""); // content before unlock, restored on Cancel
  // Set when an iterated job's fragment snapshot was discarded because it no
  // longer matched its input (distinct from a plain no-snapshot job — no note).
  const [snapshotRejected, setSnapshotRejected] = useState(false);
  const [logRejected, setLogRejected] = useState<LogRejection | null>(null);
  // Geometry carry-forward (the input-vs-output fix): when New iteration seeds from a completed
  // job, the geometry must be the parent's CONVERGED output (results.final_geometry), never the
  // seed (input_content/scene_json). `carryBanner` = the converged-output note (green); `carryRefusal`
  // = an honest refusal (scan/NEB/non-converged) shown as a warning, never a silent seed.
  const [carryBanner, setCarryBanner] = useState<string | null>(null);
  const [carryRefusal, setCarryRefusal] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // ── Destination group (explicit picker, Phase 4.7 unit 2a) ──────────────────
  // The group the created job lands in. DEFAULT = the active sidebar group, and it FOLLOWS
  // the active group until the user touches the picker — a `groupTouched` flag, NOT a naive
  // `useState(activeGroupId)` init that would freeze on the first render and ignore a later
  // sidebar switch. After an explicit pick the picker WINS (see `assignPickedGroup`).
  const [groups, setGroups] = useState<Group[]>([]);
  const [pickedGroupId, setPickedGroupId] = useState<string | null>(activeGroupId ?? null);
  const [groupTouched, setGroupTouched] = useState(false);
  useEffect(() => {
    invoke<Group[]>("list_groups")
      .then(setGroups)
      .catch(() => setGroups([]));
  }, []);
  useEffect(() => {
    if (!groupTouched) setPickedGroupId(activeGroupId ?? null);
  }, [activeGroupId, groupTouched]);
  // Set by the builder when the current buffer is a generated NEB-TS input; carries the
  // product.xyz + the two source job ids so `create()` can route to `create_neb_job`.
  // CLEARED whenever the buffer is replaced by anything else (adoptWholeInput) so a stale
  // payload can never turn a non-NEB input into a NEB job (the hasNebKeyword gate is the
  // second belt). Re-set AFTER adoptWholeInput inside handleGenerate for the NEB case.
  const [pendingNeb, setPendingNeb] = useState<NebPayload | null>(null);
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
    scan: false,
    history: false,
    actions: false,
  });
  const toggleDock = (id: string) =>
    setOpenDock((o) => ({ ...o, [id]: !o[id] }));
  const anyDockOpen = Object.values(openDock).some(Boolean);
  // Library molecules for the Add-Fragment "From library" source (lazy-loaded
  // when the panel opens).
  const [libMolecules, setLibMolecules] = useState<Molecule[]>([]);
  // User reagent catalog (Phase 4.2 tail-2): the "My reagents" palette subsection,
  // lazy-loaded with the Fragments dock. Kept apart from `libMolecules` — a reagent
  // is a molecules row with role reagent, listed by `list_reagents`, not mixed with
  // the substrate library or the curated built-ins.
  const [userReagents, setUserReagents] = useState<Molecule[]>([]);
  // "Save to my reagents" dialog state (app-owned). Charge is a REQUIRED input
  // (ADR-014 — never a silent 0); the xyz comes from a picked scene fragment or a
  // pasted block.
  const [reagentSaveOpen, setReagentSaveOpen] = useState(false);
  const [reagentName, setReagentName] = useState("");
  const [reagentCharge, setReagentCharge] = useState(""); // empty until the user types
  const [reagentSource, setReagentSource] = useState<"fragment" | "paste">("fragment");
  const [reagentFragmentId, setReagentFragmentId] = useState<string>("");
  const [reagentPasteXyz, setReagentPasteXyz] = useState("");
  const [reagentSaving, setReagentSaving] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // ── Scene store: the single source of truth for geometry (ADR-008) ──────────
  // Selectors return the stored objects directly, so an unchanged scene keeps
  // its identity and MoleculeViewer does not redraw on every keystroke.
  const scene = useSceneStore((s) => s.scene);
  const log = useSceneStore((s) => s.log);
  const addFragment = useSceneStore((s) => s.addFragment);
  const commit = useSceneStore((s) => s.commit);
  const installLog = useSceneStore((s) => s.installLog);
  const seedScene = useSceneStore((s) => s.seedScene);
  const translateAtoms = useSceneStore((s) => s.translateAtoms);
  const rotateFragment = useSceneStore((s) => s.rotateFragment);

  // ── Atom selection (2.5.2a; AtomId-native 2c2) — the geometry editor's pick list
  // Ordered stable AtomIds, held in component state (NOT the scene store — the store
  // stays a pure geometry wrapper; ADR-008 #10). measure/edit-plan/constraints
  // resolve these ids to positional indices at their own seams. The pick's AtomId
  // flows straight in — no viewer-index round-trip (the 2c1 seam is gone).
  const [selection, setSelection] = useState<AtomId[]>([]);
  const onAtomPick = (pick: AtomPick) =>
    setSelection((sel) => toggleAtom(sel, pick.atomId));
  const clearSelection = () => setSelection([]);

  // Show every atom's global index in the 3D view (2.5.2e-1). Off by default;
  // selected atoms are numbered regardless (MoleculeViewer). Toggling this
  // redraws labels only — no model reload, no camera move.
  const [showNumbers, setShowNumbers] = useState(false);

  // Move mode (Phase 4.2 Stage 3, unit 3.1): when on, a mouse-drag starting on an
  // atom rigidly moves that atom's whole fragment in the plane of the screen (the
  // 60fps motion is a viewer-only overlay; one `translate-fragment` op commits on
  // release — ADR-010). A drag on empty space still rotates; a click still picks.
  // Session-only (a view affordance, not persisted).
  const [moveMode, setMoveMode] = useState(false);
  // The MOVING SET granularity (unified moving-set unit): what a drag with NO explicit
  // selection moves — "fragment" (whole grabbed fragment, synchronous) or "selection"
  // (the grabbed atom's perceived connected component — a broken-off piece moves alone).
  // An explicit atom selection ALWAYS wins over this (THE ONE RULE in `resolveMovingSet`).
  // Session-only view affordance, owned here like `hiddenBonds` (not in the Scene).
  const [moveGranularity, setMoveGranularity] = useState<MoveMode>("fragment");

  // Steric-clash threshold (unit 3.2) — a LABELED display-choice (like the IR FWHM
  // slider), NOT a physical cutoff: flag inter-fragment atoms closer than
  // `k·(rᵢ+rⱼ)` of their vdW sum. App-owned (ADR-011 style), never in the Scene;
  // session-only. Default 0.65 — a common steric-overlap heuristic.
  const [clashK, setClashK] = useState(0.65);
  // Clashes are DERIVED over the committed scene + k + the text's active
  // constraints (a distance-constrained pair is an intentional contact, excluded).
  // Memoized so the highlight array keeps a stable reference (the overlay effect
  // redraws only when the clash set actually changes). Reuses the SAME
  // `inspectConstraintsBlock` parser the panel uses — no second constraint reader.
  const clashReport: ClashReport = useMemo(() => {
    if (!scene) return { clashes: [], undetermined: [] };
    const ins = inspectConstraintsBlock(content);
    const cs = ins.kind === "parsed" ? ins.cs : [];
    return detectClashes(scene, clashK, cs);
  }, [scene, clashK, content]);
  const clashIds = useMemo(() => clashAtomIds(clashReport), [clashReport]);
  const undetElements = useMemo(() => undeterminedElements(clashReport), [clashReport]);

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
  // ── Guided fragment placement (Phase 4.2 tail-1) — APP-OWNED, NOT in the Scene ──
  // The reagent whose approach geometry is being set (its `add-fragment` op is
  // already committed at rough placement); `null` = no guided flow open. The panel
  // reuses the shared `selection` pick list + the set-internal edit path.
  const [guidedReagent, setGuidedReagent] = useState<{ fragmentId: string; name: string } | null>(
    null,
  );
  // When on, clicking a reagent adds it AND opens the guided approach-geometry flow;
  // off = the plain rough add (unchanged behaviour). Session-only.
  const [guidedMode, setGuidedMode] = useState(false);
  // ── Rotate about axis (unit 3.3) — app-owned rotate state, NOT in the Scene ──
  // `rotateEphemeral` is the live rotation preview shown ONLY in the viewer (the
  // frozen-topology coordinate-update path); `rotateAxis` is the two picked atoms
  // the viewer draws the axis through. Both cleared on Apply/Cancel by `RotatePanel`.
  const [rotateEphemeral, setRotateEphemeral] = useState<Scene | null>(null);
  const [rotateAxis, setRotateAxis] = useState<[AtomId, AtomId] | null>(null);
  // Which overlay the viewer draws for the axis pair (unit 3.3b) — axis cylinder or
  // the P→Q distance line, never both. App-owned (like `clashK`), NOT in the Scene.
  // Defaults to the axis view and resets to it whenever the axis pair changes/clears.
  const [rotateOverlay, setRotateOverlay] = useState<RotateOverlay>(DEFAULT_ROTATE_OVERLAY);
  useEffect(() => {
    setRotateOverlay(DEFAULT_ROTATE_OVERLAY);
  }, [rotateAxis]);
  // ── Bond display control (unit bond-display-control) — DISPLAY-ONLY, app-owned,
  // NOT in the Scene (ADR-010). `hiddenBonds` = manually hidden bonds keyed by AtomId
  // PAIR (`bondKey`), so a hide survives re-perception / drag / rotate / index shifts.
  // `showCationBonds` overrides the default s-block-cation exclusion. Neither touches
  // geometry — Generate/Run are byte-identical whatever is hidden.
  const [hiddenBonds, setHiddenBonds] = useState<ReadonlySet<BondKey>>(new Set());
  const [showCationBonds, setShowCationBonds] = useState(false);
  // Per-atom FORMAL CHARGES (geometric-editor completion) — DISPLAY-ONLY bookkeeping
  // keyed by AtomId (like `hiddenBonds`, not in the Scene). ORCA is given only the
  // TOTAL charge (`totalCharge(scene) = Σ fragment.charge`); these annotations let the
  // chemist reason about where charge sits and their Σ is checked against that total.
  const [formalCharges, setFormalCharges] = useState<ReadonlyMap<AtomId, number>>(new Map());
  const setFormalCharge = (id: AtomId, value: number) =>
    setFormalCharges((prev) => {
      const next = new Map(prev);
      if (value === 0) next.delete(id);
      else next.set(id, value);
      return next;
    });
  // Σ-formal-vs-total, computed over the atoms CURRENTLY in the scene (a stale entry
  // for a removed atom is simply not summed). Only shown once any formal charge is set.
  const formalChargeList = scene
    ? scene.fragments.flatMap((f) => f.atoms.map((a) => formalCharges.get(a.id) ?? 0))
    : [];
  const chargeConsistency =
    scene && formalChargeList.some((q) => q !== 0)
      ? formalChargeConsistency(formalChargeList, totalCharge(scene))
      : null;
  // Toggle the bond between the two currently-selected atoms (by AtomId pair).
  const toggleSelectedBond = () => {
    if (selection.length !== 2) return;
    const key = bondKey(selection[0], selection[1]);
    setHiddenBonds((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };
  const selectedBondHidden =
    selection.length === 2 && hiddenBonds.has(bondKey(selection[0], selection[1]));
  // "Move the other fragment instead" — flip to the plan's alternative orientation
  // (2.5.2d-2). Reset when the selection/scene changes (the plan is different).
  const [preferAlternative, setPreferAlternative] = useState(false);
  // ── Component connectivity for a 2-atom pick (unified moving-set unit) ───────
  // A DISTANCE / FORM-BOND between two atoms of ONE fragment that sit in DIFFERENT
  // connected components (the Diels-Alder diene + dienophile in one xyz) must plan a
  // `needs-component-move`, not a `needs-split` (there is no bond to cut → the sidecar
  // would refuse "not bonded"). Perception has ONE home (the sidecar), so we resolve
  // both picked atoms' components here and INJECT them into `planEdit`. RACE-GUARDED.
  const [distanceComponents, setDistanceComponents] = useState<ComponentLookup | null>(null);
  const [componentsResolving, setComponentsResolving] = useState(false);
  const twoPickKey =
    selection.length === 2 ? `${selection[0]}:${selection[1]}` : "";
  useEffect(() => {
    if (!scene || selection.length !== 2) {
      setDistanceComponents(null);
      setComponentsResolving(false);
      return;
    }
    const gi0 = globalIndexOfAtom(scene, selection[0]);
    const gi1 = globalIndexOfAtom(scene, selection[1]);
    const loc0 = gi0 !== null ? locateAtom(scene, gi0) : null;
    const loc1 = gi1 !== null ? locateAtom(scene, gi1) : null;
    // Only a SAME-fragment pair can be a component move; inter-fragment (or a stale
    // id) is handled by `planEdit`'s existing paths — no components needed.
    if (!loc0 || !loc1 || gi0 === null || gi1 === null || loc0.fragment.id !== loc1.fragment.id) {
      setDistanceComponents(null);
      setComponentsResolving(false);
      return;
    }
    let cancelled = false;
    setDistanceComponents(null);
    setComponentsResolving(true);
    const frag = loc0.fragment;
    const fragStart = gi0 - loc0.localIndex; // the fragment's global base
    const fragXyz =
      `${frag.atoms.length}\n\n` +
      frag.atoms.map((a) => `${a.element} ${a.x} ${a.y} ${a.z}`).join("\n") +
      "\n";
    const fetchComp = (localAtom: number) =>
      postSidecar<{ component: number[] }>("/geometry/connected-component", {
        xyz: fragXyz,
        atom: localAtom,
      }).then((r) => r.component.map((li) => fragStart + li)); // local → global
    Promise.all([fetchComp(loc0.localIndex), fetchComp(loc1.localIndex)])
      .then(([c0, c1]) => {
        if (cancelled) return;
        setDistanceComponents(new Map([[gi0, c0], [gi1, c1]]));
        setComponentsResolving(false);
      })
      .catch(() => {
        if (cancelled) return;
        // Leave components unknown → `planEdit` falls back to `needs-split` (the
        // existing rotatable-mask path surfaces its own error honestly).
        setDistanceComponents(null);
        setComponentsResolving(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [twoPickKey, scene]);

  // The base plan considers both chain orientations AND (for a 2-atom same-fragment
  // distance) the injected connectivity; `editPlan` applies the user's orientation /
  // "move the other" choice. Both drive the edit UI AND the mask glow.
  const basePlan = scene
    ? planEdit(scene, selection, distanceComponents ?? undefined)
    : null;
  // `swapToAlternative` no-ops unless the plan is swappable (ready-with-alternative or
  // needs-component-move), so it is safe to call whenever the user asked to swap.
  const editPlan = preferAlternative && basePlan ? swapToAlternative(basePlan) : basePlan;
  const fragmentName = (id: string | undefined | null) =>
    (id && scene?.fragments.find((f) => f.id === id)?.name) || null;
  const componentMoveFragmentName =
    editPlan?.kind === "needs-component-move" && scene
      ? fragmentName(locateAtom(scene, editPlan.moving[0])?.fragment.id)
      : null;
  const movingFragmentName =
    editPlan?.kind === "ready"
      ? fragmentName(editPlan.movingFragmentId)
      : componentMoveFragmentName;
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
  // Gate the rotatable-mask fetch while a 2-atom pair's components are still resolving:
  // until then `basePlan` is the interim `needs-split` (no connectivity), and firing
  // rotatable-mask on a DISCONNECTED pair would 422 "not bonded" (the Diels-Alder bug).
  const splitPlan =
    editPlan?.kind === "needs-split" && !componentsResolving ? editPlan : null;
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

  // Prune the selection to atoms still in the scene on any scene change (2c2's
  // dividend). Because the pick list holds stable AtomIds, an atom survives ⟺ its
  // id is still present: `filterSelection` drops only the ids whose fragment was
  // removed and keeps everything else — so removing an UNRELATED fragment leaves
  // the selection intact (the positional guards this replaces had to clear on any
  // composition change; that clearing is gone). It returns the
  // same array reference when nothing is dropped, so a coordinate-only edit is a
  // no-op and doesn't churn state.
  useEffect(() => {
    setSelection((sel) => filterSelection(sel, scene));
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
    if (!scene) return;
    // Resolve the AtomId selection to ORCA indices NOW, at this emit seam (2c2):
    // the constraint is frozen into the text as positional indices (2.5.4a).
    const c = constraintFromSelection(scene, selection, value);
    if (!c) return;
    const ins = inspectConstraintsBlock(content);
    if (ins.kind === "unrecognised") return; // never rewrite a block we can't read (2.5.5)
    const existing = ins.kind === "parsed" ? ins.cs : [];
    if (existing.some((e) => sameConstraint(e, c))) return;
    setContent(injectConstraints(content, [...existing, c]));
    setError(null);
  };

  // "Scan this coordinate" (Stage A2): from the 2/3/4-atom selection, build a
  // relaxed-scan coordinate with an editable default range and write it through
  // `injectScan` — one data path, the ScanPanel re-reads the text. A scan is
  // singular, so this REPLACES any existing (recognised) scan; an unrecognised
  // block is never clobbered (mirror `constrainSelection`). Start defaults to the
  // current measured value; the end/points are sensible, editable defaults.
  const scanFromSelectionHandler = () => {
    if (!scene) return;
    if (inspectScanBlock(content).kind === "unrecognised") return; // don't clobber
    const m = measureSelection(scene, selection);
    if (m.kind === "none") return;
    const start = Number(m.value.toFixed(m.kind === "distance" ? 3 : 1));
    const end = Number((start + DEFAULT_SCAN_SPAN[m.kind]).toFixed(m.kind === "distance" ? 3 : 1));
    const s = scanFromSelection(scene, selection, { start, end, npoints: 10 });
    if (!s) return;
    setContent(injectScan(content, s));
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
    invoke<Molecule[]>("list_reagents")
      .then(setUserReagents)
      .catch(() => setUserReagents([]));
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

  // Geometry carry-forward resolution (the input-vs-output fix). The sync effect above seeded the
  // parent's SEED scene as a fast placeholder; here we fetch the parent's PARSED results and, for a
  // CONVERGED optimization, REPLACE it with a fresh scene from the converged OUTPUT — the same source
  // the viewer / `finalGeometryXyz` use (`results.final_geometry`), never `input_content`/`scene_json`.
  // Variant (a): the parent's edit LOG is NOT carried (it produced the seed, not the output) — a fresh
  // scene, with a provenance comment + banner so this reads as "from the output", not seed-editing. A
  // scan/NEB/non-converged source is an honest REFUSAL (a warning; never a silent seed); a single point
  // needs no change (its final geometry is its input).
  useEffect(() => {
    if (!initialJob) return;
    let live = true;
    (async () => {
      const results = await invoke<ParsedResults | null>("read_job_results", { id: initialJob.id }).catch(
        () => null,
      );
      if (!live) return;
      const cf = resolveCarryForwardGeometry(initialJob, results);
      if (!cf.ok) {
        setCarryRefusal(cf.reason);
        return;
      }
      // A single point's final geometry IS its input — the sync-seeded scene is already correct.
      if (cf.origin !== "converged" || !results) return;
      // Guard (defense-in-depth): the carried geometry MUST bit-match the parsed final frame; the
      // seed (2.364) would not (this is the negative-control target).
      if (!geometryMatchesFinal(cf.geometry, results)) {
        setCarryRefusal(
          "Carried geometry did not match the parent's parsed final geometry — refusing to seed (guard).",
        );
        return;
      }
      // Charge/multiplicity from the parent's input header (an .xyz carries none).
      const parent = sceneFromOrcaInput(initialJob.input_content);
      const xyz = finalGeometryXyz(cf.geometry, initialJob.title, results.final_energy_eh);
      const fresh = sceneFromXyz(xyz, {
        name: `${initialJob.title} (converged)`,
        charge: parent ? totalCharge(parent) : 0,
        multiplicity: parent ? parent.multiplicity : 1,
        source: "editor",
      });
      if (!fresh) return;
      seedScene(fresh, "text-adopt"); // fresh log — the parent's seed edit history is not carried
      setContent((c) => withProvenanceComment(c, carryForwardProvenanceComment(cf)));
      setCarryBanner(cf.note);
      // The converged-output seed supersedes any snapshot/log-reject warning from the sync restore.
      setSnapshotRejected(false);
      setLogRejected(null);
    })();
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialJob]);

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

  // content → Scene (ADR-008 #6): on the same 500 ms debounce the preview used.
  // Since unit 2d the coordinate block is a READ-ONLY PROJECTION of the Scene, so
  // a block edit never flows into the geometry — the collapse-from-text path is
  // gone. The four branches:
  //   • replaceMode → the buffer is deliberately unlocked (Replace input); the
  //     Adopt button commits it, so this effect stands down entirely.
  //   • no scene yet → a block typed/pasted into an empty editor SEEDS the Scene
  //     (`text-adopt`); this keeps template/generated-input adoption alive.
  //   • block matches the scene (parsed floats, tol 1e-6, never string compare) →
  //     leave it: the user edited `!`/`%` keywords (the common, allowed path).
  //   • block gone or diverged, scene present → the block is read-only; REVERT it
  //     from the Scene (keeping the `!`/`%` edits) and note the reverted edit.
  // The single locator is `sceneFromOrcaInput`/`injectSceneIntoInput` (no second
  // `* xyz` finder). Editing the geometry goes through the two doors below.
  useEffect(() => {
    const id = setTimeout(() => {
      if (replaceMode) return; // buffer unlocked; Adopt commits it, not this effect
      const parsed = sceneFromOrcaInput(content);
      const current = useSceneStore.getState().scene;
      if (!current) {
        if (parsed) seedScene(parsed, "text-adopt");
        return;
      }
      if (parsed && xyzMatchesScene(current, mergeToAtomLines(parsed))) return;
      // Read-only projection: a hand-edit (or deletion) of the block does NOT win.
      // Restore the projection, keeping everything outside the block, and Scene is
      // untouched (no collapse, geometry unchanged — the multi-fragment layout
      // survives). This is the pure core of manual gate m1.
      setContent((c) => injectSceneIntoInput(c, current));
      setCoordNotice(true);
    }, 500);
    return () => clearTimeout(id);
  }, [content, seedScene, replaceMode]);

  // Replace the whole buffer AND re-seed the Scene from its coordinate block in
  // one conscious act (unit 2d). A template / generated input is a fresh geometry,
  // so it is a deliberate `text-adopt` — NOT a block hand-edit — and must bypass
  // the read-only revert (else the effect would restore the previous scene's block
  // over the new one). Same door as "Replace input", reached from the builder.
  const adoptWholeInput = (newContent: string) => {
    setContent(newContent);
    // Any whole-buffer replace drops a pending NEB payload by default (template pick,
    // Replace input, a non-NEB generate). handleGenerate re-sets it AFTER this call for
    // the NEB case, so a NEB generate keeps its payload while everything else clears it.
    setPendingNeb(null);
    // Guard the fragment layout (debugging/014). "Generate Input" rewrites only the
    // `!`/`%` keyword lines over the SAME coordinates, so a blind `text-adopt` would
    // collapse a substrate+reagent scene into one "Molecule" fragment (breaking
    // rotate/move/clash). Keep the Scene when the adopted geometry matches it; only
    // a genuinely different (or absent) geometry re-seeds as a fresh single fragment.
    if (!adoptPreservesScene(useSceneStore.getState().scene, newContent)) {
      seedScene(sceneFromOrcaInput(newContent), "text-adopt"); // null → clears the scene
    }
    setReplaceMode(false);
    setCoordNotice(false);
  };

  const pickTemplate = (t: OrcaTemplate) => {
    setSelectedId(t.id);
    adoptWholeInput(t.inputContent);
    if (!title.trim()) setTitle(t.name);
    setOpenSection(null); // got the input — collapse so the editor is visible
  };

  // Builder's "Generate Input" → replace editor content and collapse the panel. For a
  // NEB-TS generate, `neb` carries the product.xyz + source job ids; set it AFTER
  // adoptWholeInput (which clears any prior payload) so the buffer + payload stay in sync.
  const handleGenerate = (newContent: string, neb?: NebPayload) => {
    adoptWholeInput(newContent);
    setPendingNeb(neb ?? null);
    setOpenSection(null);
  };

  // Add a curated reagent (BH₄⁻, H₂O, …) as a fragment.
  const addReagent = (lf: LibraryFragment) => {
    setError(null);
    addFragmentToScene(libraryFragmentToScene(lf));
  };

  // Guided placement (Phase 4.2 tail-1): after a reagent is added roughly, OPEN the
  // guided panel on the just-added fragment so the user sets its approach d/θ/φ in
  // one flow. The store update is synchronous, so the new fragment is the last one.
  // Shared by the built-in and user-reagent guided-mode paths.
  const openGuidedOnLastFragment = () => {
    const frags = useSceneStore.getState().scene?.fragments;
    const added = frags && frags.length ? frags[frags.length - 1] : undefined;
    if (added) {
      setGuidedReagent({ fragmentId: added.id, name: added.name });
      setSelection([]); // a clean pick list for the approach-geometry atoms
      setOpenDock((o) => ({ ...o, fragments: true }));
    }
  };

  const addReagentGuided = (lf: LibraryFragment) => {
    setError(null);
    addFragmentToScene(libraryFragmentToScene(lf));
    openGuidedOnLastFragment();
  };

  // Commit the guided placement's op sequence (add-fragment is already in the log):
  // one `replace-fragment-atoms` per given coordinate, in order (d, θ, φ), each with
  // its own resultant snapshot — so Undo unwinds them step by step (ADR-017).
  const applyGuided = (ops: { op: Op; scene: Scene }[]) => {
    ops.forEach(({ op, scene: resultScene }) => commit(op, resultScene));
    setGuidedReagent(null);
    setPreviewScene(null);
    setSaved(false);
  };

  // If the guided reagent leaves the scene (Undo past the add, a fragment removal),
  // close the flow — there is nothing to place.
  useEffect(() => {
    if (guidedReagent && !scene?.fragments.some((f) => f.id === guidedReagent.fragmentId)) {
      setGuidedReagent(null);
      setPreviewScene(null);
    }
  }, [scene, guidedReagent]);

  // ── Door 1: Import xyz as fragment (unit 2d) ────────────────────────────────
  // The typical way coordinate hand-editing survives the read-only block: paste a
  // plain xyz and it becomes a NEW fragment (a logged `add-fragment` op), placed
  // clear of what's there. The composition invariant (count + order) is preserved
  // by `addFragment`. Soft failure (a message, no throw) on unparseable xyz.
  const importXyzAsFragment = () => {
    const built = sceneFromXyz(importXyz, {
      source: "import",
      name: "Pasted xyz",
      charge: 0,
      multiplicity: 1,
    });
    if (!built) {
      setError("That doesn't parse as xyz (expected: count, comment, then `El x y z` rows).");
      return;
    }
    setError(null);
    addFragmentToScene(built.fragments[0]);
    setImportXyz("");
    setImportXyzOpen(false);
  };

  // ── Door 2: Replace input (unit 2d) — a one-shot, conscious escape ───────────
  // Unlock the WHOLE buffer once so the user can paste a different calculation
  // (another molecule / another job type), then Adopt it as a fresh Scene. The
  // block re-locks after adoption (review point 3: not a permanent unlock). Cancel
  // restores the pre-unlock buffer. Confirmed first because it discards the scene
  // lineage (the history starts over — a fresh `text-adopt` log).
  const beginReplace = () => setReplaceConfirm(true);
  const unlockReplace = () => {
    replaceSnapshotRef.current = content;
    setReplaceConfirm(false);
    setReplaceMode(true);
    setError(null);
  };
  const adoptReplace = () => {
    // A conscious repeat `text-adopt`: seedScene installs a FRESH log, so the old
    // scene does not leak into the new lineage (negative control c2).
    seedScene(sceneFromOrcaInput(content), "text-adopt");
    setReplaceMode(false);
    setCoordNotice(false);
  };
  const cancelReplace = () => {
    setContent(replaceSnapshotRef.current); // restore the buffer; the block re-locks
    setReplaceMode(false);
    setReplaceConfirm(false);
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

  // Add a user reagent (from "My reagents") as a fragment — its stored charge flows
  // into the scene total exactly like a built-in reagent's (no special case).
  const addUserReagent = (m: Molecule) => {
    const frag = userReagentToFragment(m);
    if (!frag) {
      setError(`"${m.name}" has no readable geometry.`);
      return;
    }
    setError(null);
    addFragmentToScene(frag);
    if (guidedMode) openGuidedOnLastFragment();
  };

  // Open the "Save to my reagents" dialog, pre-filled from a picked scene fragment
  // when there is exactly one (the common case) so the charge starts at the known
  // fragment charge — still editable and still REQUIRED (ADR-014).
  const openReagentSave = () => {
    const only = scene && scene.fragments.length === 1 ? scene.fragments[0] : null;
    setReagentName(only?.name ?? "");
    setReagentCharge(only ? String(only.charge) : "");
    setReagentSource(scene && scene.fragments.length > 0 ? "fragment" : "paste");
    setReagentFragmentId(only?.id ?? scene?.fragments[0]?.id ?? "");
    setReagentPasteXyz("");
    setReagentSaveOpen(true);
  };

  // The charge field must hold a valid INTEGER before Save is allowed — an empty or
  // non-integer charge is refused, never coerced to 0 (ADR-014 footgun).
  const reagentChargeValid =
    reagentCharge.trim() !== "" && Number.isInteger(Number(reagentCharge));

  const saveReagent = async () => {
    if (!reagentChargeValid) return;
    // Resolve the xyz from the chosen source: a picked scene fragment, or pasted text.
    let xyz: string | null = null;
    if (reagentSource === "fragment") {
      const frag = scene?.fragments.find((f) => f.id === reagentFragmentId);
      if (!frag) {
        setError("Pick a fragment to save.");
        return;
      }
      xyz = fragmentToXyz(frag, reagentName.trim() || frag.name);
    } else {
      const parsed = sceneFromXyz(reagentPasteXyz, { source: "import", name: "r", charge: 0 });
      if (!parsed) {
        setError("That doesn't parse as xyz (count, comment, then `El x y z` rows).");
        return;
      }
      xyz = reagentPasteXyz;
    }
    setReagentSaving(true);
    setError(null);
    try {
      await invoke<Molecule>("create_reagent", {
        name: reagentName.trim() || "Untitled reagent",
        xyz,
        charge: Number(reagentCharge),
      });
      const reagents = await invoke<Molecule[]>("list_reagents");
      setUserReagents(reagents);
      setReagentSaveOpen(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setReagentSaving(false);
    }
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

  // Assign the created job to the PICKED destination group (unit 2a — explicit picker over
  // the 4.7.3 implicit active-group assign). Pure composition of the existing `move_job`
  // command — no Rust change, same `null`-for-ungrouped semantics as `JobsScreen.moveJob`.
  //
  // The `groupTouched` guard preserves zero regression AND makes an explicit pick authoritative:
  //   • untouched + no active group (pickedGroupId == null) → NO-OP — exactly today's behavior,
  //     and it does NOT clobber a create-path default (a NEB-TS inherits its reactant's group in
  //     Rust, Unit 1);
  //   • untouched + an active group → assign it (as the 4.7.3 assign-on-create did);
  //   • an explicit pick → the picker WINS, including "(ungrouped)" (null), overriding any default.
  const assignPickedGroup = async (jobId: string) => {
    const decision = resolveGroupAssignment(groupTouched, pickedGroupId);
    if (!decision.assign) return;
    await invoke("move_job", { jobId, groupId: decision.groupId });
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
    if (scanBlockMessage) {
      // Same rationale as the constraint block: an immutable input that will
      // silently single-point instead of scanning must not be created or run.
      setError(scanBlockMessage);
      return;
    }
    setCreating(true);
    setError(null);
    try {
      // A NEB-TS input generated by the builder routes to the two-file `create_neb_job`
      // (it materializes product.xyz into the isolated dir and attaches the reactant's
      // pathway). BOTH gates required: a pending payload AND a `!` line that still carries
      // NEB — so a stale payload after the user replaced the buffer with a non-NEB input
      // falls through to the ordinary create_job.
      const isNeb = pendingNeb != null && hasNebKeyword(content);
      const job = isNeb
        ? await invoke<Job>("create_neb_job", {
            title: title.trim() || "Untitled job",
            inpContent: content,
            productXyzContent: pendingNeb.productXyz,
            reactantJobId: pendingNeb.reactantJobId,
            productJobId: pendingNeb.productJobId,
          })
        : await invoke<Job>("create_job", {
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
      // Assign the picked destination group before navigating (unit 2a).
      await assignPickedGroup(job.id);
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
      // A conformer search started under an active group belongs in it too.
      await assignPickedGroup(job.id);
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
  // A 2D scan reads as N coordinates — recognised, but the selection add-path (which
  // creates/replaces ONE coordinate) must not clobber it; the 2nd coordinate is edited in
  // the Scan panel. `inspectScanBlock` still flags >1 line as unrecognised, so distinguish
  // "we DO recognise a 2D scan" from a genuinely unrecognised block for an honest reason.
  const scanIsMultiCoord = (parseScanCoordinates(content)?.length ?? 0) > 1;
  const indexIssues = scene
    ? constraintIndexIssues(constraints, atomCount(scene))
    : [];
  const constraintBlockMessage =
    indexIssues.length > 0 && scene
      ? "Can't create or run this job — " +
        `${indexIssues.length} constraint${indexIssues.length > 1 ? "s" : ""} ` +
        `reference atom indices that don't exist in this geometry (it has ` +
        `${atomCount(scene)} atoms, valid ORCA index 0–${atomCount(scene) - 1}). ORCA does ` +
        "NOT range-check constraint indices — it segfaults instead of reporting " +
        "an error, so the run is blocked until they are fixed or removed: " +
        indexIssues
          .map(
            (it) =>
              `${constraintTypeLabel(it.constraint.kind)} on ` +
              `ORCA index ${it.badIndices.map((i) => `#${i}`).join(", ")}`,
          )
          .join("; ") +
        "."
      : null;

  // ── Scan Run-guard (Stage A2) ───────────────────────────────────────────────
  // A `%geom Scan` with no measured optimization keyword on the `!` line runs a
  // SINGLE POINT and silently ignores the scan (measured — `wiki/orca/scan.md`).
  // The job's input is immutable once created, so an input that will silently do
  // the wrong thing is a landmine — block Create AND Run, exactly as the
  // out-of-range constraint does. `scanOptIssue` is the same guard the panel shows.
  const scanBlockMessage = scanOptIssue(content);

  const canCreate =
    content.trim().length > 0 &&
    !creating &&
    !constraintBlockMessage &&
    !scanBlockMessage;

  // The Add-Fragment sources (reagents / import / SMILES / library) — the palette
  // half of the Fragments dock section. Reachable inside fullscreen (unit 2b-ux).
  const addFragmentSources = (
    <div className="add-fragment-body">
      <div className="add-source">
        <div className="add-source-title muted">Reagents</div>
        <label
          className="guided-mode-toggle"
          title="When on, adding a reagent opens the guided flow to set its approach distance / angle / dihedral to the substrate in one step."
        >
          <input
            type="checkbox"
            checked={guidedMode}
            onChange={(e) => setGuidedMode(e.target.checked)}
          />
          Guided placement — set d / θ / φ on add
        </label>
        {/* Curated built-ins — each carries a reference-geometry contract (its
            provenance is the tooltip). Visually the "Built-in" group. */}
        <div className="reagent-group-label muted">Built-in</div>
        <div className="reagent-chips">
          {FRAGMENT_LIBRARY.map((lf) => (
            <button
              key={lf.key}
              className="chip chip-curated"
              title={lf.provenance}
              onClick={() => (guidedMode ? addReagentGuided(lf) : addReagent(lf))}
            >
              {lf.name}{" "}
              <span className="muted">
                {signed(lf.charge)} · {lf.atoms.length}a
              </span>
            </button>
          ))}
        </div>

        {/* User reagents — a molecules row with role reagent (no reference contract:
            user provenance). Deliberately a SEPARATE group + distinct chip style so
            the two are never conflated. */}
        <div className="reagent-group-label muted reagent-group-user">
          My reagents
          <button
            className="btn reagent-save-btn"
            onClick={openReagentSave}
            title="Save a scene fragment or a pasted structure as a reusable reagent"
          >
            + Save
          </button>
        </div>
        {userReagents.length === 0 ? (
          <div className="muted reagent-empty">
            None yet — Save captures a fragment’s geometry + charge for reuse.
          </div>
        ) : (
          <div className="reagent-chips">
            {userReagents.map((m) => (
              <button
                key={m.id}
                className="chip chip-user"
                title={`User reagent — no reference geometry. ${m.name}`}
                onClick={() => addUserReagent(m)}
              >
                {m.name}{" "}
                <span className="muted">{signed(m.charge)}</span>
              </button>
            ))}
          </div>
        )}

        {reagentSaveOpen ? (
          <div className="reagent-save-dialog">
            <div className="reagent-save-row">
              <label>Name</label>
              <input
                className="input"
                value={reagentName}
                onChange={(e) => setReagentName(e.currentTarget.value)}
                placeholder="e.g. tetrabutylammonium"
                spellCheck={false}
              />
            </div>
            <div className="reagent-save-row">
              <label>
                Charge <span className="reagent-required">*required</span>
              </label>
              <input
                className={"input" + (reagentChargeValid ? "" : " input-invalid")}
                type="number"
                step="1"
                value={reagentCharge}
                onChange={(e) => setReagentCharge(e.currentTarget.value)}
                placeholder="e.g. +1, 0, -1"
                aria-label="reagent formal charge (required)"
              />
            </div>
            <div className="reagent-save-row">
              <label>Geometry from</label>
              <div className="reagent-source-toggle">
                <label>
                  <input
                    type="radio"
                    name="reagent-src"
                    checked={reagentSource === "fragment"}
                    disabled={!scene || scene.fragments.length === 0}
                    onChange={() => setReagentSource("fragment")}
                  />
                  a scene fragment
                </label>
                <label>
                  <input
                    type="radio"
                    name="reagent-src"
                    checked={reagentSource === "paste"}
                    onChange={() => setReagentSource("paste")}
                  />
                  pasted xyz
                </label>
              </div>
            </div>
            {reagentSource === "fragment" ? (
              <select
                className="input"
                value={reagentFragmentId}
                onChange={(e) => setReagentFragmentId(e.currentTarget.value)}
              >
                {(scene?.fragments ?? []).map((f) => (
                  <option key={f.id} value={f.id}>
                    {f.name} · {f.atoms.length}a · {signed(f.charge)}
                  </option>
                ))}
              </select>
            ) : (
              <textarea
                className="input mono paste-xyz-area"
                placeholder={"3\nname\nO 0 0 0\nH 0.76 0.59 0\nH -0.76 0.59 0"}
                value={reagentPasteXyz}
                onChange={(e) => setReagentPasteXyz(e.currentTarget.value)}
                spellCheck={false}
              />
            )}
            <div className="reagent-save-actions">
              <button
                className="btn btn-sm btn-primary"
                onClick={saveReagent}
                disabled={reagentSaving || !reagentChargeValid}
                title={reagentChargeValid ? undefined : "Enter the formal charge first"}
              >
                {reagentSaving ? "Saving…" : "Save reagent"}
              </button>
              <button
                className="btn btn-sm"
                onClick={() => setReagentSaveOpen(false)}
                disabled={reagentSaving}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : null}
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
        <div className="add-source-title muted">Paste xyz</div>
        {importXyzOpen ? (
          <div className="paste-xyz">
            <textarea
              className="input mono paste-xyz-area"
              placeholder={"3\nwater\nO  0.000  0.000  0.000\nH  0.757  0.586  0.000\nH -0.757  0.586  0.000"}
              value={importXyz}
              onChange={(e) => setImportXyz(e.currentTarget.value)}
              spellCheck={false}
              rows={5}
            />
            <div className="row" style={{ gap: 8 }}>
              <button
                className="btn btn-sm"
                onClick={importXyzAsFragment}
                disabled={!importXyz.trim()}
                title="Parse the xyz and add it as a new fragment"
              >
                Add as fragment
              </button>
              <button
                className="btn btn-sm"
                onClick={() => {
                  setImportXyz("");
                  setImportXyzOpen(false);
                }}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button className="btn btn-sm" onClick={() => setImportXyzOpen(true)}>
            Paste xyz…
          </button>
        )}
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
          onScan={scanFromSelectionHandler}
          scanDisabledReason={
            scanIsMultiCoord
              ? "A 2D scan is already set — edit its coordinates in the Scan panel below."
              : inspectScanBlock(content).kind === "unrecognised"
              ? "The scan block contains syntax OrcaStudio doesn't recognise — edit it in the input editor."
              : null
          }
          formalCharges={formalCharges}
          onSetFormalCharge={setFormalCharge}
          chargeConsistency={chargeConsistency}
        />
      ) : null,
    },
    {
      id: "edit",
      label: "Edit geometry",
      short: "Edit",
      glyph: "✎",
      body: scene ? (
        <div className="edit-section">
          {/* Move mode (unit 3.1): rough repositioning by dragging a fragment in the
              viewer. Coarse by design — exact distances/angles come from the
              measure + constraint tools below and the editor (division of labour). */}
          <label className="move-mode-toggle" title="Drag any atom of a fragment to move the whole fragment in the plane of the screen. A drag on empty space still rotates the camera.">
            <input
              type="checkbox"
              checked={moveMode}
              onChange={(e) => setMoveMode(e.target.checked)}
            />
            Move mode — drag a fragment to reposition it
          </label>
          {/* Moving-set granularity (unified moving-set unit) — what a drag with NO
              explicit atom selection moves. Fragment = the whole grabbed fragment
              (fast, no sidecar); Selection = the grabbed atom's connected component,
              so a broken-off piece moves alone. An explicit selection always wins. */}
          <div
            className="move-granularity"
            role="radiogroup"
            aria-label="Drag moves"
            title="With no atoms selected, a drag moves either the whole fragment or just the grabbed atom's connected component (a broken-off piece). Selecting atoms overrides this — a drag then moves exactly the selection."
          >
            <span className="muted">Drag moves:</span>
            <label className="move-granularity-opt">
              <input
                type="radio"
                name="move-granularity"
                checked={moveGranularity === "fragment"}
                disabled={!moveMode}
                onChange={() => setMoveGranularity("fragment")}
              />
              Fragment
            </label>
            <label className="move-granularity-opt">
              <input
                type="radio"
                name="move-granularity"
                checked={moveGranularity === "selection"}
                disabled={!moveMode}
                onChange={() => setMoveGranularity("selection")}
              />
              Selection / connected piece
            </label>
          </div>
          {/* Steric-clash sensitivity — a LABELED heuristic (unit 3.2), not a
              physical cutoff. Higher k flags contacts sooner. App-owned. */}
          <div className="clash-slider">
            <label className="clash-slider-label" htmlFor="clash-k">
              vdW overlap threshold
              <span className="muted"> — heuristic, not a physical cutoff</span>
            </label>
            <div className="clash-slider-row">
              <input
                id="clash-k"
                type="range"
                min={0.4}
                max={0.9}
                step={0.01}
                value={clashK}
                onChange={(e) => setClashK(Number(e.currentTarget.value))}
              />
              <span className="mono clash-slider-value">
                k = {clashK.toFixed(2)} · {clashReport.clashes.length} clash
                {clashReport.clashes.length === 1 ? "" : "es"}
              </span>
            </div>
          </div>
          {/* Bond display control (unit bond-display-control) — DISPLAY-ONLY. Cation
              coordinate bonds (Na⁺/K⁺/Mg²⁺…) are hidden by default; this reveals them.
              Hiding is a viewer choice — it never changes the geometry or the ORCA input. */}
          <div className="bond-display-controls">
            <label
              className="move-mode-toggle"
              title="s-block metal cations (Na⁺, K⁺, Mg²⁺, …) coordinate rather than bond covalently; 3Dmol draws a spurious stick. Off = hide those sticks (default). This is display-only."
            >
              <input
                type="checkbox"
                checked={showCationBonds}
                onChange={(e) => setShowCationBonds(e.target.checked)}
              />
              Show cation coordinate bonds
            </label>
            <button
              className="btn btn-sm"
              onClick={toggleSelectedBond}
              disabled={selection.length !== 2}
              title={
                selection.length === 2
                  ? "Hide/show the bond between the two selected atoms (kept by AtomId — survives moves)"
                  : "Select exactly two atoms to hide/show the bond between them"
              }
            >
              {selectedBondHidden ? "Show bond" : "Hide bond"} between selection
            </button>
            {hiddenBonds.size > 0 ? (
              <span className="muted bond-hidden-count">
                {hiddenBonds.size} bond{hiddenBonds.size === 1 ? "" : "s"} hidden ·{" "}
                <button className="link-btn" onClick={() => setHiddenBonds(new Set())}>
                  show all
                </button>
              </span>
            ) : null}
          </div>
          {editPlan && selection.length >= 2 ? (
            <EditPanel
              scene={scene}
              plan={editPlan}
              movingFragmentName={movingFragmentName}
              alternativeFragmentName={alternativeFragmentName}
              splitMask={splitMask}
              splitError={splitError}
              splitResolving={splitResolving || componentsResolving}
              components={distanceComponents ?? undefined}
              onSwitchOrientation={() => setPreferAlternative((v) => !v)}
              onPreview={setPreviewScene}
              onApplied={applyEdit}
            />
          ) : (
            <div className="muted" style={{ fontSize: 13 }}>
              Pick 2+ atoms for a distance / angle / dihedral edit — or use Move mode
              for rough placement.
            </div>
          )}
          {/* Rotate about axis (unit 3.3): a rigid whole-fragment spin about the
              approach axis two picked atoms define — sibling of the set-internal
              edit above, pure TS (no sidecar). Self-manages its active state on the
              same 2-atom pick. */}
          <RotatePanel
            scene={scene}
            selection={selection}
            overlay={rotateOverlay}
            onOverlayChange={setRotateOverlay}
            onEphemeral={setRotateEphemeral}
            onAxis={setRotateAxis}
            onApply={(fragmentId, axisAtoms, angleRad) => {
              rotateFragment(fragmentId, axisAtoms, angleRad);
              setSaved(false);
            }}
          />
        </div>
      ) : null,
    },
    {
      id: "fragments",
      label: "Fragments",
      short: "Frag",
      glyph: "⬡",
      body: (
        <>
          {scene && guidedReagent ? (
            <GuidedPlacementPanel
              scene={scene}
              reagentFragmentId={guidedReagent.fragmentId}
              reagentName={guidedReagent.name}
              selection={selection}
              onPreview={setPreviewScene}
              onApplied={applyGuided}
              onCancel={() => {
                setGuidedReagent(null);
                setPreviewScene(null);
              }}
            />
          ) : null}
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
      id: "scan",
      label: "Scan",
      short: "Scan",
      glyph: "⇢",
      body: scene ? (
        <ScanPanel scene={scene} content={content} onChange={setContent} />
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
        <>
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
        <CrestPanel
          scene={scene}
          onRefine={(input) => adoptWholeInput(input)}
        />
        </>
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
        <div className="field">
          <label className="label" htmlFor="job-group">
            Group
          </label>
          <GroupSelect
            id="job-group"
            groups={groups}
            value={pickedGroupId}
            onChange={(g) => {
              setGroupTouched(true);
              setPickedGroupId(g);
            }}
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

      {scanBlockMessage ? (
        <div className="banner err scan-block">
          Can&apos;t create or run this job — {scanBlockMessage}
        </div>
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
        {/* Door 2 (unit 2d): the deliberate escape from the read-only coordinate
            block — paste a whole different calculation. Hidden while already
            unlocked / confirming (the banners below own that flow). */}
        {!replaceMode && !replaceConfirm ? (
          <button
            className="btn btn-sm"
            onClick={beginReplace}
            title="Unlock the whole input to paste a different calculation (starts the scene over)"
          >
            Replace input…
          </button>
        ) : null}
      </div>

      {error ? <div className="banner err">{error}</div> : null}
      {saved ? <div className="banner ok">Saved to library</div> : null}
      {carryBanner ? (
        <div className="banner ok">
          {carryBanner}
          <button className="btn btn-sm" style={{ marginLeft: 10 }} onClick={() => setCarryBanner(null)}>
            Dismiss
          </button>
        </div>
      ) : null}
      {carryRefusal ? (
        <div className="banner warn">
          {carryRefusal}
          <button className="btn btn-sm" style={{ marginLeft: 10 }} onClick={() => setCarryRefusal(null)}>
            Dismiss
          </button>
        </div>
      ) : null}
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
      {/* The coordinate block is a read-only projection of the 3D scene (unit 2d):
          a hand-edit was reverted. Point at the two doors instead of silently
          discarding the keystrokes. */}
      {coordNotice ? (
        <div className="banner warn">
          The coordinate block is generated from the 3D scene — edit the geometry
          in the viewer, use <b>Paste xyz</b> to add a fragment, or{" "}
          <b>Replace input</b> for a different calculation. Your text edit to the
          coordinates was reverted.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={() => setCoordNotice(false)}
          >
            Dismiss
          </button>
        </div>
      ) : null}

      {/* Stage 3.x — a drag moved the WHOLE fragment because the sidecar could not
          resolve the connected component. Honest, dismissible; never silent. */}
      {dragFallbackNotice ? (
        <div className="banner warn">
          {dragFallbackNotice}
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={() => setDragFallbackNotice(null)}
          >
            Dismiss
          </button>
        </div>
      ) : null}

      {/* Steric-clash WARNING (unit 3.2) — a warning, never a block. Derived over
          the scene, so it clears itself when the fragments are pulled apart. The
          clashing atoms are glowing magenta in the viewer. */}
      {clashReport.clashes.length > 0 ? (
        <div className="banner warn clash-warn">
          <span className="clash-warn-dot" aria-hidden>
            ●
          </span>{" "}
          {clashReport.clashes.length} steric{" "}
          {clashReport.clashes.length === 1 ? "clash" : "clashes"} between fragments
          — coarse placement; refine with the editor or pull the fragments apart.
          The overlapping atoms are highlighted.
        </div>
      ) : null}

      {/* UNDETERMINED — a separate, quiet notice: an element with no cited vdW
          radius couldn't be steric-checked (rule #11: skipped, not guessed). */}
      {undetElements.length > 0 ? (
        <div className="banner muted clash-undetermined">
          Couldn&apos;t steric-check {undetElements.join(", ")}: no cited van der
          Waals radius, so pairs touching{" "}
          {undetElements.length === 1 ? "it" : "them"} were skipped (not guessed).
        </div>
      ) : null}

      {/* Replace input — the confirm step (discards the scene lineage). */}
      {replaceConfirm ? (
        <div className="banner warn">
          Replace the entire input and start the scene over? The current geometry
          and its edit history will be discarded.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={unlockReplace}
          >
            Unlock editor
          </button>
          <button
            className="btn btn-sm"
            style={{ marginLeft: 6 }}
            onClick={() => setReplaceConfirm(false)}
          >
            Cancel
          </button>
        </div>
      ) : null}

      {/* Replace input — the buffer is unlocked; Adopt commits it as a fresh scene. */}
      {replaceMode ? (
        <div className="banner warn">
          Editor unlocked — paste a new input (its own <code>* xyz … *</code>{" "}
          block), then Adopt it as a fresh scene.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={adoptReplace}
          >
            Adopt input
          </button>
          <button
            className="btn btn-sm"
            style={{ marginLeft: 6 }}
            onClick={cancelReplace}
          >
            Cancel
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
                  ephemeralScene={rotateEphemeral}
                  axisHighlight={rotateAxis}
                  rotateOverlay={rotateOverlay}
                  selection={selection}
                  onAtomPick={onAtomPick}
                  moveMode={moveMode}
                  moveGranularity={moveGranularity}
                  onFragmentDrag={translateAtoms}
                  onFragmentDragFallback={setDragFallbackNotice}
                  hiddenBonds={hiddenBonds}
                  showCationBonds={showCationBonds}
                  clashHighlight={clashIds}
                  showAtomNumbers={showNumbers}
                  formalCharges={formalCharges}
                  theme={theme}
                  maskHighlight={
                    editPlan?.kind === "ready"
                      ? editPlan.mask
                      : editPlan?.kind === "needs-split"
                        ? splitMask ?? undefined
                        : editPlan?.kind === "needs-component-move"
                          ? editPlan.moving
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
