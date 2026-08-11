import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import { createViewer, GLModel, VolumeData, elementColors, type GLViewer, type GLShape } from "3dmol";

import { dataUrlToBytes } from "../export/png";

import type { Scene, SceneAtom } from "../scene/types";
import type { AtomId } from "../scene/ids";
import {
  buildViewerAtomTable,
  buildViewerFeed,
  compositionSignature,
  fragmentRanges,
  type ViewerAtomTable,
} from "../scene/scene";
import { measureSelection, formatMeasurementValue } from "../scene/measure";
import { highlightRadius, vdwTableDrift } from "./highlight";
import { DEFAULT_THEME, cpkColorDrift, type ViewerTheme } from "./theme";
import { parseXyzCoords, applyCoordsToAtoms, drawableBondCount } from "./frozenTopology";
import {
  filterDrawnBonds,
  applyGeometricBondOrders,
  type BondKey,
  type FilterableAtom,
  type OrderableAtom,
} from "./bond-display";
import { bondOrderEstimate } from "../scene/bond-edit";
import { makeDragController, type WorldDelta } from "./fragment-drag";
import { postSidecar } from "../sidecar-client";
import { chooseRotateOverlay, type RotateOverlay } from "./rotate-overlay";

// Side-effect: force 3Dmol onto its direct-canvas WebGL path so it renders in
// the WebKitGTK webview (must run before the first createViewer).
import "./3dmol-setup";

/**
 * Module-level count of how many 3Dmol viewers this module has created — the
 * concrete **remount witness** (2.5.2e-2). Entering/leaving fullscreen only
 * toggles a CSS class on an ancestor; `MoleculeViewer` keeps its tree position,
 * so React keeps the instance and this counter does NOT tick. If a refactor
 * ever moved the component between two JSX branches, a fullscreen toggle would
 * remount it and this number would climb — the dev log makes that visible.
 */
let viewerCreateCount = 0;

interface MoleculeViewerProps {
  /** Flat xyz — the existing single-structure path (Molecules screen, previews). */
  xyzData?: string;
  /** Multi-fragment path: takes precedence over `xyzData` when present. */
  scene?: Scene;
  /**
   * Ordered global atom indices to highlight (2.5.2a). Optional; a changed
   * `selection` re-draws the highlight spheres only — never reloads the model or
   * re-`zoomTo`s. Ignored unless `onAtomPick` is also given (picking off). Holds
   * stable {@link AtomId}s (2c2): a halo resolves an id straight to its atom, so
   * the highlight follows the physical atom across a fragment removal.
   */
  selection?: AtomId[];
  /**
   * Atom-pick callback (2.5.2a; AtomId in 2c1). **Presence of this prop is what
   * turns clickability on** — without it the viewer is display-only (Molecules
   * screen, the Job-detail conformer panel), exactly as before. Receives a
   * {@link AtomPick}: the stable {@link AtomId} of the clicked atom (resolved
   * through the feed's table — never a raw viewer index leaking out as an app id),
   * plus the raw `atom.index` labelled `viewerIndex` for diagnostics only. The
   * viewer is a dumb renderer (ADR-010 / ADR-011): the identity it returns is the
   * one the core minted, mapped back from what 3Dmol drew.
   */
  onAtomPick?: (pick: AtomPick) => void;
  /**
   * Label every atom with its **global 0-based index** (2.5.2e-1). Default
   * false, so the Molecules screen and the Job-detail conformer panel are
   * unchanged. Only the global index is ever shown in the 3D view — the local
   * index lives in `AtomInspector`, where the fragment gives it context; two
   * numbers on an atom would reintroduce exactly the ambiguity the single
   * end-to-end index space exists to avoid. Selected atoms are always numbered,
   * even with this off, so a pick is legible immediately.
   */
  showAtomNumbers?: boolean;
  /**
   * Per-atom FORMAL CHARGES (geometric-editor completion), keyed by stable AtomId.
   * A nonzero charge is drawn as a small `+1` / `−1` label on the atom. DISPLAY-ONLY
   * bookkeeping — not part of the Scene, never an ORCA input (ORCA reads geometry +
   * total charge). Absent/empty → no charge labels (unchanged view).
   */
  formalCharges?: ReadonlyMap<AtomId, number>;
  /**
   * Viewer colour theme (2.5.2e-2). Default `dark` — the pre-2.5.2e-2 look. The
   * background is set via `setBackgroundColor` (no model reload, no zoomTo) and
   * every overlay colour (halo, labels, measurement) is read from it, so a light
   * background doesn't leave dark label rectangles behind.
   */
  theme?: ViewerTheme;
  /**
   * Global indices of the atoms that an edit would MOVE (2.5.2d) — the mask. Drawn
   * as a **solid translucent glow**, distinct in FORM from the selection halo (a
   * wireframe cage): the halo says "I picked this", the mask says "this will
   * move". They share the chartreuse hue on purpose — no hue is ≥30° from both
   * every element colour AND the halo (the element-safe band IS the halo's; see
   * theme.test.ts), so the distinction is carried by form, not hue. Shown only
   * while an edit is available.
   */
  maskHighlight?: number[];
  /**
   * Steric-clash highlight (unit 3.2): the stable {@link AtomId}s of atoms in an
   * inter-fragment vdW clash after a move. Drawn as a distinct **magenta danger
   * glow** — visually apart from the selection halo and the edit mask (see the
   * `CLASH_*` constants). Resolves an id straight to its atom (like `selection`), so
   * it follows the physical atom. A warning only — nothing here blocks Run/Apply.
   */
  clashHighlight?: AtomId[];
  /**
   * The two picked atoms `[P, Q]` of an active "Rotate about axis" edit (unit 3.3),
   * drawn as a distinct extended **axis cylinder** through P→Q so the eye reads the
   * rotation axis (not a bond). Resolves ids straight to atoms; an absent id draws
   * nothing. Purely decorative — a display of the axis the panel is turning about.
   */
  axisHighlight?: [AtomId, AtomId] | null;
  /**
   * Which single overlay to draw for the `axisHighlight` pair (unit 3.3b): the axis
   * cylinder (with the Å label on the axis midpoint) OR the distance measurement
   * line + label — **never both** (they drew on the same two atoms and read as one
   * wrong line). Ignored when `axisHighlight` is null (the measurement is untouched
   * outside Rotate). Default `"axis"` (the panel opens for rotating).
   */
  rotateOverlay?: RotateOverlay;
  /**
   * EPHEMERAL coordinate-only overlay (unit 3.3) — the live preview of a rigid
   * fragment rotation. A Scene with the SAME composition as `scene` but the rotating
   * fragment's atoms turned. Rendered through the **frozen-topology coordinate-update
   * path** (like the Move-mode drag and mode animation): the live model atoms' x/y/z
   * are set from it and re-styled WITHOUT `addModel`/re-perception/`zoomTo`, so bonds
   * don't flicker and the camera holds while the angle is turned live. The store/Scene
   * is untouched (ADR-010); on Apply the committed `scene` changes and the model effect
   * rebuilds at the final coords; on Cancel (`null`) the committed coords are restored.
   */
  ephemeralScene?: Scene | null;
  /**
   * Trajectory playback (unit 3.8). When true, a change of `xyzData` that keeps
   * the SAME atom count updates coordinates in place WITHOUT re-`zoomTo` — so the
   * camera stays put while the frames advance (a per-frame `zoomTo` would make
   * the molecule jump every tick). A change of atom count still re-`zoomTo`s.
   * Default false → the Molecules/preview path is byte-for-byte unchanged (it
   * always `zoomTo`s a newly-shown molecule).
   *
   * The viewer is still a **dumb renderer** (ADR-011): it is handed one frame's
   * geometry and draws it. It does NOT hold the frame list, a timer, or 3Dmol's
   * `setCoordinates`/`animate` frame apparatus — the current frame number is
   * application state in `TrajectoryPlayer`.
   */
  preserveCameraOnUpdate?: boolean;
  /**
   * FREEZE bond topology from this reference geometry (an xyz string). A vibration is
   * the same molecule — its bond graph is a function of the EQUILIBRIUM only — but
   * 3Dmol perceives bonds from each frame's distances, so an animated stretch makes
   * bonds flicker (an over-compressed bond blinks, an over-stretched one detaches).
   *
   * When set (unit 3.14), the model is built **once** from this reference so 3Dmol
   * perceives bonds and assigns `atom.index` exactly as for any static structure, and
   * then each `xyzData` frame only **updates the atom coordinates** — the topology is
   * frozen by construction (3Dmol never re-perceives it). The **app decides** the
   * topology (by choosing the equilibrium reference); the viewer draws (ADR-011); no
   * second bond perception exists (ADR-010). Only used on the `xyzData` path.
   * Trajectory playback deliberately does NOT set this — there bonds can genuinely
   * form/break, so it keeps re-perceiving per frame.
   *
   * (Unit 3.13 instead parsed each frame with `assignBonds:false` and set bonds by
   * hand; that drew nothing — `assignBonds:false` leaves `atom.index` unset and
   * 3Dmol's stick gate is `atom.index < atom2.index`. See `frozenTopology.ts`.)
   */
  bondTopologyReference?: string;
  /**
   * Orbital isosurface (unit 3.15). The cube's TEXT — 3Dmol parses the whole grid
   * (`VolumeData`) into a mesh. When set (and no `scene`), the viewer draws the
   * molecule FROM the cube's atoms plus two isosurfaces at ±`orbitalIsoValue` in
   * distinct colours (the two wavefunction phases). The **app owns** which orbital,
   * the isovalue, and visibility (ADR-011); the viewer draws. Changing the isovalue
   * redraws only the surfaces (the cube is parsed once, cached).
   */
  orbitalCube?: string;
  /** Isosurface level (Å⁻³·²). A display choice; the sign is the wavefunction PHASE,
   * not charge. Ignored unless `orbitalCube` is set. */
  orbitalIsoValue?: number;
  /** Molecule representation (unit 3.16): `stick` (default) or `line`. App-owned
   * (ADR-011). Honoured on the orbital, mode-animation and single-xyz paths; the
   * scene editor is always ball-and-stick. */
  representation?: Representation;
  /**
   * Rigid-body fragment drag — "Move mode" (Phase 4.2 Stage 3, unit 3.1). When
   * true (scene path only), a mouse-drag STARTING on an atom grabs the dragged
   * atom's PERCEIVED CONNECTED COMPONENT (Stage 3.x — resolved ONCE on mousedown
   * via the sidecar) and moves it rigidly in the plane of the screen at 60fps — a
   * VIEWER-ONLY ephemeral overlay (the Scene is untouched, ADR-010). For a fully
   * bonded fragment the component IS the whole fragment (identical to before a bond
   * was ever broken). Camera rotation is suppressed for that drag (the grab
   * intercepts the mousedown before 3Dmol). On release, the TOTAL delta is handed
   * up via {@link onFragmentDrag} as ONE op. A drag on empty space still rotates the
   * camera; a click still picks. When false, mouse behaviour is unchanged (rotate /
   * pick). Requires `onAtomPick` (a pickable scene) and `onFragmentDrag`.
   */
  moveMode?: boolean;
  /**
   * Release callback for a rigid drag (unit 3.1; Stage 3.x). Passes the grabbed
   * fragment's id, the stable {@link AtomId}s of the MOVING SET (the dragged atom's
   * perceived connected component — the whole fragment when nothing is broken), and
   * the TOTAL world displacement (Å). Called exactly once per drag, on mouseup
   * (never per frame). The app commits it as one `translate-atoms` op.
   */
  onFragmentDrag?: (
    fragmentId: string,
    atomIds: AtomId[],
    dx: number,
    dy: number,
    dz: number,
  ) => void;
  /**
   * Atom-pick callback for the **xyzData / frame** paths (results & trajectory), where
   * there is no Scene/AtomId table. Emits the raw **0-based viewer index**, which on
   * these paths equals the `final_geometry` / frame atom index — the identity the
   * results bond-order readout (and `mayer_bond_orders`) key on. Presence of this prop
   * arms picking on the xyz/frozen-topology models (harmless on the scene path, which
   * uses `onAtomPick` instead). No highlight overlay is drawn on these paths.
   */
  onXyzAtomPick?: (index: number) => void;
  /**
   * Honest-note callback (Stage 3.x): the sidecar could not resolve the dragged
   * atom's connected component, so the drag fell back to moving the WHOLE fragment.
   * Called at most once per drag, on release, only when a real move was committed
   * under the fallback — so the user is never left with a silent whole-fragment
   * move that should have been a component move. `message` is human-facing.
   */
  onFragmentDragFallback?: (message: string) => void;
  /**
   * Bond DISPLAY filter (unit bond-display-control) — DISPLAY-ONLY, app-owned
   * (ADR-010), NOT in the Scene. After 3Dmol perceives bonds at `addModel`, bonds
   * that fail `shouldDrawBond` are removed from the live atom array before styling
   * (the frozenTopology technique — no second perception). `hiddenBonds` is the set
   * of manually hidden AtomId pairs (`bondKey`); the pair key survives re-perception
   * and index shifts. `showCationBonds` overrides the default s-block-cation exclusion.
   * The geometry (xyz / Scene / ORCA input) is untouched. See `viewer/bond-display.ts`.
   */
  hiddenBonds?: ReadonlySet<BondKey>;
  showCationBonds?: boolean;
  style?: React.CSSProperties;
}

/**
 * The result of clicking an atom (2c1). `atomId` is the stable identity the rest
 * of the app keys on; `viewerIndex` is 3Dmol's raw `atom.index` — **viewer
 * space**, carried only for diagnostics and never to be used as an app id (that
 * is the coupling 2c1 severs). A consumer that still needs a positional global
 * index resolves `atomId` back through a `ViewerAtomTable` — it does not read
 * `viewerIndex`.
 */
export interface AtomPick {
  atomId: AtomId;
  /** Raw 3Dmol `atom.index` — VIEWER SPACE, diagnostics only. */
  viewerIndex: number;
}

/** Isosurface colours — the two wavefunction phases. Sign is PHASE, not charge; the
 * label says so. Blue = positive lobe, red = negative. */
const ORBITAL_POS_COLOR = "#3b6fd4";
const ORBITAL_NEG_COLOR = "#d43b3b";
const ORBITAL_ISO_OPACITY = 0.85;

/** Selection-halo wireframe opacity (2.5.2e-1). The colour comes from the theme
 * (`haloColor`); a constant-radius halo was invisible on carbon, so the RADIUS
 * is per-element via `highlightRadius`, and it's a wireframe cage (reads over
 * CPK red O / grey C where a solid sphere washed out). */
const HALO_OPACITY = 0.85;

/** Mask "will-move" glow (2.5.2d) — a SOLID translucent sphere (not a wireframe
 * cage), a touch larger than the halo, so it reads as a soft region distinct in
 * FORM from the selection halo. Reuses the theme halo colour (see the
 * `maskHighlight` prop note on why hue can't distinguish them). */
const MASK_OPACITY = 0.22;
const MASK_RADIUS_BOOST = 0.15;

/** Steric-clash "danger glow" (unit 3.2) — a SOLID sphere in a distinct **deep
 * magenta**, larger and more opaque than the mask glow. Distinct from the selection
 * halo (wireframe chartreuse cage) and the edit mask (translucent chartreuse) in
 * BOTH hue and form: no CPK element colour is magenta, so a clash reads on any atom
 * — deliberately not reusing the theme's chartreuse `haloColor` (the Pd/Pt-vs-halo
 * colour collision already bit once; see wiki/modules/theme.ts). A theme-independent
 * semantic colour, like the orbital-phase colours. */
const CLASH_COLOR = "#ff2d95";
const CLASH_OPACITY = 0.4;
const CLASH_RADIUS_BOOST = 0.3;

/** The flat molecule style. **Two representations only** (unit 3.16): the default
 * ball-and-stick, and thin **lines** — lines expose a core 1s isosurface that hides
 * inside an atom's drawn sphere (the occlusion the orbital panel needs). Fresh object
 * per call (3Dmol may retain the reference). */
export type Representation = "stick" | "line";
const baseStyle = (representation: Representation = "stick") =>
  representation === "line" ? { line: {} } : { stick: {}, sphere: { scale: 0.3 } };

/**
 * Ball-and-stick base style for the scene path, honouring a theme's CPK element
 * overrides (2.5.2e-3a). When the theme has overrides (light/white), atoms are
 * coloured by a `colorscheme` map = 3Dmol's default element colours WITH the
 * theme's low-contrast elements darkened (so CPK hydrogen isn't white-on-white).
 * With no overrides (dark/black) it returns the exact `baseStyle()` object — a
 * true no-op, so the dark themes render byte-identically to before.
 */
function cpkBaseStyle(theme: ViewerTheme) {
  const overrides = theme.elementColorOverrides;
  if (Object.keys(overrides).length === 0) return baseStyle();
  const map = { ...elementColors.defaultColors, ...overrides };
  const colorscheme = { prop: "elem", map } as unknown as { prop: string };
  return { stick: { colorscheme }, sphere: { scale: 0.3, colorscheme } };
}

/**
 * Apply the scene's ball-and-stick styling to the current model: CPK for fragment
 * 0, a flat palette colour for fragments 1+ (each reads as one object). Extracted
 * (unit 3.1) so BOTH the model-rebuild effect AND the ephemeral drag redraw style
 * identically — a drag frame must not flash fragment 0's CPK onto the others.
 * `setStyle` also nulls the model's cached geometry, so sticks redraw at whatever
 * coordinates the atoms currently hold (the frozen-topology update path).
 */
function applySceneStyle(viewer: GLViewer, scene: Scene, theme: ViewerTheme) {
  viewer.setStyle({}, cpkBaseStyle(theme));
  const palette = theme.fragmentPalette;
  fragmentRanges(scene).forEach((range, fragmentIndex) => {
    if (fragmentIndex === 0) return; // fragment 0 → leave on CPK colours
    const color = palette[(fragmentIndex - 1) % palette.length];
    const indices: number[] = [];
    for (let i = range.start; i < range.end; i++) indices.push(i);
    viewer.setStyle({ index: indices }, { stick: { color }, sphere: { scale: 0.3, color } });
  });
}

/** Stable empty-set default for `hiddenBonds` (module-level so an unspecified prop
 * keeps the same reference across renders and doesn't churn the model effect). */
const EMPTY_HIDDEN_BONDS: ReadonlySet<BondKey> = new Set();

/**
 * Apply the bond DISPLAY filter to a model 3Dmol just perceived — remove the bonds
 * that shouldn't be drawn (s-block-cation coordinate bonds by default; manually
 * hidden AtomId pairs), IN PLACE on the live atoms so the stick pass draws only the
 * survivors. This filters the perception 3Dmol already did (like `frozenTopology`);
 * it is NOT a second perception. `resolveId` maps a viewer index to its AtomId (the
 * scene feed's `ViewerAtomTable`), or `() => undefined` where there is no table (the
 * mode-animation path — only the element-based cation rule can apply there).
 */
function applyBondFilter(
  model: GLModel,
  resolveId: (viewerIndex: number) => AtomId | undefined,
  hidden: ReadonlySet<BondKey>,
  showCationBonds: boolean,
): void {
  filterDrawnBonds(
    model.selectedAtoms({}) as unknown as FilterableAtom[],
    resolveId,
    hidden,
    { showCationBonds },
  );
}

/**
 * Depth (in the frame `screenOffsetToModel` expects) of an atom at world coords —
 * the exact first step of 3Dmol's own `modelToScreen` chain (measured pixel-exact,
 * `wiki/debugging/013`). Passing this as the `modelz` argument makes a drag track
 * the GRABBED atom to 0 px error at any camera orientation. Reaches two 3Dmol
 * internals (`modelGroup.matrixWorld`, the `Vector3` constructor) by runtime probe,
 * not a typed import — so it returns `undefined` if a 3Dmol upgrade moves them,
 * and the caller falls back to the default (scene-centre) depth (≤~6% tracking lag
 * at a strongly rotated camera — still usable for a coarse-position affordance).
 */
function grabbedAtomDepth(
  viewer: GLViewer,
  atom: { x: number; y: number; z: number },
): number | undefined {
  const internals = viewer as unknown as {
    modelGroup?: { matrixWorld?: unknown };
    rotationGroup?: { position?: { constructor: new (x: number, y: number, z: number) => { applyMatrix4(m: unknown): { z: number } } } };
  };
  const matrixWorld = internals.modelGroup?.matrixWorld;
  const Vec3 = internals.rotationGroup?.position?.constructor;
  if (!matrixWorld || !Vec3) return undefined;
  return new Vec3(atom.x, atom.y, atom.z).applyMatrix4(matrixWorld).z;
}

/** Radius (Å) of the thick, solid cylinder marking a dihedral's j–k axis
 * (2.5.2e-2) — chunky enough to read as the axis against the thin dashed i–j /
 * k–l lines. */
const AXIS_RADIUS = 0.05;

/** A pointer within this many pixels of an atom's projected centre grabs it in Move
 * mode (unit 3.1). Coarse on purpose — the drag is a rough placement, refined by the
 * editor/constraints; a generous radius makes small atoms (H) easy to grab. */
const GRAB_RADIUS_PX = 22;

/** A pointer that stays within this many pixels of the grab point is a CLICK (pick);
 * crossing it starts a DRAG (move). Small, so a deliberate drag registers at once. */
const DRAG_THRESHOLD_PX = 3;

const xyz = (a: SceneAtom) => ({ x: a.x, y: a.y, z: a.z });
const midpoint = (a: SceneAtom, b: SceneAtom) => ({
  x: (a.x + b.x) / 2,
  y: (a.y + b.y) / 2,
  z: (a.z + b.z) / 2,
});

/**
 * Draw a short arc near an angle's VERTEX (2.5.2e-2), between the two rays
 * `vertex→a` and `vertex→b`, as a fan of solid line segments along the great
 * circle from one ray to the other. This is what tells the eye WHICH picked atom
 * is the vertex — without a second number on the atom (the "one number per atom"
 * rule from e-1 holds). Radius scales with the shorter arm so it never overshoots
 * a bond. No-op when the rays are (anti)parallel (no arc to draw).
 */
function drawAngleArc(
  viewer: GLViewer,
  a: SceneAtom,
  vertex: SceneAtom,
  b: SceneAtom,
  color: string,
) {
  const u = [a.x - vertex.x, a.y - vertex.y, a.z - vertex.z];
  const w = [b.x - vertex.x, b.y - vertex.y, b.z - vertex.z];
  const nu = Math.hypot(u[0], u[1], u[2]);
  const nw = Math.hypot(w[0], w[1], w[2]);
  if (nu === 0 || nw === 0) return;
  const un = u.map((c) => c / nu);
  const wn = w.map((c) => c / nw);
  const dot = Math.max(-1, Math.min(1, un[0] * wn[0] + un[1] * wn[1] + un[2] * wn[2]));
  const omega = Math.acos(dot);
  const sinO = Math.sin(omega);
  if (sinO < 1e-6) return; // 0° / 180° — no arc plane is defined
  const radius = Math.min(0.6, 0.35 * Math.min(nu, nw));
  const N = 16;
  let prev: { x: number; y: number; z: number } | null = null;
  for (let i = 0; i <= N; i++) {
    const t = i / N;
    const s1 = Math.sin((1 - t) * omega) / sinO;
    const s2 = Math.sin(t * omega) / sinO;
    const p = {
      x: vertex.x + radius * (un[0] * s1 + wn[0] * s2),
      y: vertex.y + radius * (un[1] * s1 + wn[1] * s2),
      z: vertex.z + radius * (un[2] * s1 + wn[2] * s2),
    };
    if (prev) viewer.addLine({ start: prev, end: p, color });
    prev = p;
  }
}

/** A measurement value label (the "N Å" / "109°" chip). Extracted (unit 3.3b) so
 * the distance line AND the rotation-axis midpoint show the value in the SAME style
 * from the SAME formatter. */
function drawValueLabel(
  viewer: GLViewer,
  position: { x: number; y: number; z: number },
  text: string,
  theme: ViewerTheme,
) {
  viewer.addLabel(text, {
    position,
    backgroundColor: theme.labelBg,
    backgroundOpacity: 0.85,
    fontColor: theme.measurementText,
    fontSize: 13,
    inFront: true,
  });
}

/**
 * Draw the measurement geometry for the current pick list and a value label,
 * marking WHICH atom is the vertex/axis geometrically (2.5.2e-2) — never with a
 * second number (the "one number per atom" rule from e-1 holds):
 * - **distance:** one dashed line, label at the bond midpoint;
 * - **angle:** two dashed rays + a solid ARC at the vertex, label at the vertex;
 * - **dihedral:** the j–k axis as a thick solid cylinder, the i–j / k–l bonds as
 *   thin dashed lines, label at the axis midpoint.
 * Colours come from `theme`. No-op for 0/1 atoms or any degenerate pick
 * (`measureSelection` → `none`). Caller has already run `removeAllShapes` /
 * `removeAllLabels` and will `render()`.
 */
function drawMeasurement(
  viewer: GLViewer,
  scene: Scene,
  selection: AtomId[],
  theme: ViewerTheme,
) {
  const m = measureSelection(scene, selection);
  const label = formatMeasurementValue(m);
  if (m.kind === "none" || !label) return;

  const rows = scene.fragments.flatMap((f) => f.atoms);
  const pts = m.atoms.map((gi) => rows[gi]);
  if (pts.some((a) => a == null)) return; // stale index — bail (guarded upstream)

  const line = theme.measurementLine;

  if (m.kind === "dihedral") {
    // Emphasise the j–k axis (pts[1]–pts[2]) with a thick solid cylinder; the
    // two outer bonds stay thin dashed lines. This shows the rotation axis.
    viewer.addLine({ dashed: true, start: xyz(pts[0]), end: xyz(pts[1]), color: line });
    viewer.addCylinder({
      start: xyz(pts[1]),
      end: xyz(pts[2]),
      radius: AXIS_RADIUS,
      color: line,
    });
    viewer.addLine({ dashed: true, start: xyz(pts[2]), end: xyz(pts[3]), color: line });
  } else {
    for (let n = 0; n < pts.length - 1; n++) {
      viewer.addLine({ dashed: true, start: xyz(pts[n]), end: xyz(pts[n + 1]), color: line });
    }
    if (m.kind === "angle") drawAngleArc(viewer, pts[0], pts[1], pts[2], line);
  }

  const anchor =
    m.kind === "angle"
      ? xyz(pts[1]) // the vertex
      : m.kind === "dihedral"
        ? midpoint(pts[1], pts[2]) // middle of the j–k axis
        : midpoint(pts[0], pts[1]); // distance: midpoint of the bond

  drawValueLabel(viewer, anchor, label, theme);
}

/**
 * The Å value for the rotation axis pair `[P, Q]`, from `measure` distance — the
 * SAME source `drawMeasurement` uses, NEVER a second computation (unit 3.3b). Returns
 * `null` if the pair doesn't measure (degenerate / stale).
 */
function rotationAxisValueLabel(scene: Scene, axis: [AtomId, AtomId]): string | null {
  const m = measureSelection(scene, [axis[0], axis[1]]);
  return m.kind === "none" ? null : formatMeasurementValue(m) || null;
}

/** Radius of the rotation-axis cylinder (unit 3.3) — a touch thicker than the
 * dihedral axis so it reads as "the axis this turns about", and the amount (Å) it
 * extends beyond each picked atom so the line reads as an axis, not a P–Q bond. */
const ROTATE_AXIS_RADIUS = 0.06;
const ROTATE_AXIS_EXTEND = 0.7;

/**
 * Draw the rotation axis P→Q as an extended solid cylinder (unit 3.3). Extended a
 * little past both atoms so it reads as an AXIS the fragment spins about, not a
 * bond between them. Colour ties to the selection accent (these are picked atoms).
 * No-op if the two coincide (no direction).
 */
function drawRotationAxis(viewer: GLViewer, a: SceneAtom, b: SceneAtom, color: string) {
  const d = [b.x - a.x, b.y - a.y, b.z - a.z];
  const len = Math.hypot(d[0], d[1], d[2]);
  if (len < 1e-8) return;
  const u = [d[0] / len, d[1] / len, d[2] / len];
  const e = ROTATE_AXIS_EXTEND;
  viewer.addCylinder({
    start: { x: a.x - u[0] * e, y: a.y - u[1] * e, z: a.z - u[2] * e },
    end: { x: b.x + u[0] * e, y: b.y + u[1] * e, z: b.z + u[2] * e },
    radius: ROTATE_AXIS_RADIUS,
    color,
  });
}

/**
 * Ball-and-stick molecule viewer built on 3Dmol.js. Fills its parent; mouse
 * rotate/zoom/pan is 3Dmol's default behaviour. One WebGL context is created
 * on mount and released on unmount — 3Dmol holds it explicitly and it leaks
 * otherwise (see wiki/modules/visualization.md).
 *
 * Two geometry sources: a multi-fragment `scene` (preferred; coloured per
 * fragment by atom-index range — ADR-008 #2/#3) or a flat `xyzData` string (the
 * original single-structure path). `scene` wins when both are given.
 */
/** Imperative handle (unit 3.16): a PNG snapshot of the current 3D scene, for export.
 * The app requests it; the viewer produces it from what it drew (ADR-011). */
export interface MoleculeViewerHandle {
  toPngBytes: () => Uint8Array | null;
}

export const MoleculeViewer = forwardRef<MoleculeViewerHandle, MoleculeViewerProps>(
  function MoleculeViewer(
    {
      xyzData,
      scene,
      selection,
      onAtomPick,
      onXyzAtomPick,
      showAtomNumbers = false,
      formalCharges,
      theme = DEFAULT_THEME,
      maskHighlight,
      clashHighlight,
      axisHighlight,
      rotateOverlay = "axis",
      ephemeralScene,
      preserveCameraOnUpdate = false,
      bondTopologyReference,
      orbitalCube,
      orbitalIsoValue,
      representation = "stick",
      moveMode = false,
      onFragmentDrag,
      onFragmentDragFallback,
      hiddenBonds = EMPTY_HIDDEN_BONDS,
      showCationBonds = false,
      style,
    }: MoleculeViewerProps,
    ref,
  ) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewerRef = useRef<GLViewer | null>(null);
  // The single, persistent model of a frozen-topology animation (unit 3.14): built
  // once from `bondTopologyReference`, then coordinate-updated per frame. Keyed by the
  // reference string so it is rebuilt only when the molecule changes.
  const animRef = useRef<{ source: string; model: GLModel } | null>(null);
  // Orbital isosurface (unit 3.15): the cube parsed ONCE into a VolumeData (keyed by the
  // cube text) and the two ± phase surface shapes, so an isovalue change removes exactly
  // those two and redraws — no re-parse, and the selection-halo path is untouched.
  const volDataRef = useRef<{ source: string; vol: VolumeData } | null>(null);
  const isoShapesRef = useRef<GLShape[]>([]);
  // Last scene composition rendered — drives the zoom-only-on-composition-change
  // rule. `null` means "nothing/only-xyz rendered so far".
  const lastCompositionRef = useRef<string | null>(null);
  // The AtomId↔viewer-index table for the geometry CURRENTLY drawn (2c1). Set in
  // the model effect from the SAME `buildViewerFeed` call that builds the model,
  // so it can never name a different geometry than the one on screen. The pick
  // handler reads it to map 3Dmol's `atom.index` back to the stable AtomId. `null`
  // whenever the scene path is not active (xyz / orbital / animation paths draw no
  // pickable scene). Non-scene paths clear it so a stale table can't resolve a pick.
  const viewerTableRef = useRef<ViewerAtomTable | null>(null);
  // Latest onAtomPick, read through a ref so the model effect (which re-arms
  // setClickable) doesn't need onAtomPick in its dependency list — an inline
  // callback would otherwise rebuild the model on every render.
  const onAtomPickRef = useRef<typeof onAtomPick>(onAtomPick);
  onAtomPickRef.current = onAtomPick;
  // Latest onFragmentDrag, read through a ref so the drag effect (below) does not
  // re-attach its listeners when only this inline callback changes.
  const onFragmentDragRef = useRef<typeof onFragmentDrag>(onFragmentDrag);
  onFragmentDragRef.current = onFragmentDrag;
  // Latest onXyzAtomPick through a ref — armed on the xyz/frozen paths without
  // adding it to the model effect's deps (same pattern as onAtomPick).
  const onXyzAtomPickRef = useRef<typeof onXyzAtomPick>(onXyzAtomPick);
  onXyzAtomPickRef.current = onXyzAtomPick;
  const onFragmentDragFallbackRef =
    useRef<typeof onFragmentDragFallback>(onFragmentDragFallback);
  onFragmentDragFallbackRef.current = onFragmentDragFallback;
  // Picking is enabled iff a pick handler was provided. Captured as a boolean so
  // the model effect re-runs when it flips (it never flips in practice — a
  // screen either passes onAtomPick or doesn't — but keeps the effect honest).
  const pickable = onAtomPick != null;

  // 3D-scene PNG snapshot for export (unit 3.16). Re-render then read the WebGL buffer
  // back via 3Dmol's `pngURI()` (the path measured to work under WebKitGTK — debugging/009).
  useImperativeHandle(
    ref,
    () => ({
      toPngBytes() {
        const viewer = viewerRef.current;
        if (!viewer) return null;
        viewer.render();
        return dataUrlToBytes(viewer.pngURI());
      },
    }),
    [],
  );

  // Create the viewer once and wire up resize handling.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // Dev guard: our `highlight.ts` vdW table is a hand copy of 3Dmol's
    // `GLModel.vdwRadii` (the node test runner can't load the 3dmol bundle, so
    // we can't import it there). Here in the real webview 3Dmol IS loaded — warn
    // if a 3Dmol upgrade moved the table out from under our copy.
    if (import.meta.env.DEV) {
      // Two-directional drift guards (2.5.2e-3b): `changed` = our copy is stale,
      // `missing` = 3Dmol has an element we don't mirror (the direction that let
      // the ADR-007 metals slip past the one-way guard).
      const vdw = vdwTableDrift(
        GLModel.vdwRadii as Record<string, number | undefined>,
      );
      if (vdw.changed.length || vdw.missing.length) {
        console.warn(
          `[MoleculeViewer] highlight.ts vdW radii vs 3Dmol — changed: [${vdw.changed.join(", ")}], missing: [${vdw.missing.join(", ")}] — update VDW_RADII.`,
        );
      }
      const cpk = cpkColorDrift(
        elementColors.defaultColors as Record<string, string | number | undefined>,
      );
      if (cpk.changed.length || cpk.missing.length) {
        console.warn(
          `[MoleculeViewer] theme.ts CPK colours vs 3Dmol — changed: [${cpk.changed.join(", ")}], missing: [${cpk.missing.join(", ")}] — update CPK_ELEMENT_COLORS.`,
        );
      }
    }

    const viewer = createViewer(container, {
      backgroundColor: theme.background,
    });
    viewerRef.current = viewer;
    viewerCreateCount += 1;
    if (import.meta.env.DEV) {
      // Remount witness — see the note on `viewerCreateCount`. A fullscreen
      // toggle must NOT increment this; only a real mount/navigation does.
      console.debug(`[MoleculeViewer] viewer created (total #${viewerCreateCount})`);
    }

    // Keep the render surface in sync with the container (flex/split resizes AND
    // the fullscreen class toggle — the container's box changes, so this fires
    // and calls resize; no remount needed). Same mechanism the split-panel
    // resize already relies on.
    const observer = new ResizeObserver(() => viewer.resize());
    observer.observe(container);

    return () => {
      observer.disconnect();
      viewer.clear(); // drop models/shapes and release the WebGL context
      viewerRef.current = null;
      lastCompositionRef.current = null;
      viewerTableRef.current = null;
      animRef.current = null;
      volDataRef.current = null;
      isoShapesRef.current = [];
    };
    // theme.background is only the INITIAL colour; the [theme] effect below keeps
    // it in sync, so it's intentionally not a dep here (mount-once effect).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Background follows the theme — `setBackgroundColor`, no model reload, no
  // zoomTo. Runs on mount (after create) and whenever the theme changes.
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;
    viewer.setBackgroundColor(theme.background, 1);
    viewer.render();
  }, [theme]);

  // Re-render whenever the geometry changes (scene takes precedence over xyz).
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;

    // Orbital-isosurface path (unit 3.15): the MOLECULE is built here from the cube's
    // atoms (once per cube); the ± isosurfaces are drawn by the dedicated effect below so
    // an isovalue change doesn't rebuild the model. Depends on `orbitalCube`, not the
    // isovalue. Camera holds across MO switches (same molecule → same atom count).
    const orbCube = !scene && orbitalCube?.trim() ? orbitalCube : null;
    if (orbCube) {
      animRef.current = null;
      viewerTableRef.current = null; // not a pickable scene — no table
      viewer.removeAllModels();
      viewer.addModel(orbCube, "cube");
      viewer.setStyle({}, baseStyle(representation));
      const count = orbCube.trimStart().split(/\r?\n/, 1)[0].trim();
      const signature = `orbital:${count}`;
      if (signature !== lastCompositionRef.current) {
        viewer.zoomTo();
        lastCompositionRef.current = signature;
      }
      viewer.render();
      return;
    }

    // Frozen-topology animation path (unit 3.14): keep ONE model across frames — build
    // it from the equilibrium reference so 3Dmol perceives bonds + assigns index once,
    // then only UPDATE COORDINATES. Topology is frozen by construction; no model
    // rebuild, no manual bonds (which drew nothing — see the prop note / frozenTopology).
    const frozenRef =
      !scene && xyzData && xyzData.trim().length > 0 && bondTopologyReference?.trim()
        ? bondTopologyReference.trim()
        : null;
    if (frozenRef) {
      viewerTableRef.current = null; // frozen-topology animation — not a pickable scene
      let anim = animRef.current;
      const firstBuild = !anim || anim.source !== frozenRef;
      if (firstBuild) {
        viewer.removeAllModels();
        const model = viewer.addModel(frozenRef, "xyz"); // perceive bonds + set index, ONCE
        // Same DISPLAY-ONLY bond filter, applied ONCE at build (the per-frame update
        // only moves coordinates — it never re-perceives, so the filter persists for
        // the whole animation). No AtomId table on this path (it's an xyz-only mode
        // animation), so only the element-based cation rule applies (`() => undefined`
        // → manual hides are inert here).
        applyBondFilter(model, () => undefined, hiddenBonds, showCationBonds);
        // Results/trajectory picking (Mayer readout): emit the raw 0-based viewer
        // index (== the frame / final_geometry atom index). Armed once at build.
        viewer.setClickable({}, true, (atom: { index: number }) =>
          onXyzAtomPickRef.current?.(atom.index),
        );
        anim = { source: frozenRef, model };
        animRef.current = anim;
      }
      // Move the existing atoms to this frame; bonds/index/style are untouched.
      applyCoordsToAtoms(
        anim!.model.selectedAtoms({}) as Array<{ x: number; y: number; z: number }>,
        parseXyzCoords(xyzData!), // frozenRef truthy ⇒ xyzData is a non-empty string
      );
      // Geometric bond order — the SAME call the scene path makes (reuse, no second
      // impl): re-derive 1/2/3 from THIS frame's geometry so the results/trajectory
      // view draws multiplicity (butadiene → two C=C). Per frame, nothing stored.
      applyGeometricBondOrders(
        anim!.model.selectedAtoms({}) as unknown as OrderableAtom[],
        (elA, elB, d) => bondOrderEstimate(elA, elB, d).order,
      );
      anim!.model.setStyle({}, baseStyle(representation)); // (re)apply style + null the cached geometry
      if (firstBuild) {
        viewer.zoomTo(); // frame the molecule once; later frames keep the camera
        if (import.meta.env.DEV) {
          // Output check in the REAL webview: the 3.13 bug was zero DRAWN bonds while
          // the stored list looked fine. Warn if 3Dmol would draw no sticks.
          const drawn = drawableBondCount(
            anim!.model.selectedAtoms({}) as Array<{ index?: number; bonds: number[] }>,
          );
          if (drawn === 0) {
            console.warn(
              "[MoleculeViewer] frozen-topology model has 0 drawable bonds — sticks will not render.",
            );
          }
        }
      }
      viewer.render();
      return;
    }
    // Leaving the animation path: drop the persistent model and fall through.
    if (animRef.current) animRef.current = null;

    viewer.removeAllModels();
    // Default to no table; only the scene branch below installs one (the xyz and
    // empty branches draw nothing pickable).
    viewerTableRef.current = null;

    if (scene && scene.fragments.length > 0) {
      // Geometry AND its AtomId↔viewer-index table from ONE call (2c1): the model
      // 3Dmol draws and the table the pick handler resolves through are the same
      // object, built from the same atom sequence — they cannot disagree.
      const feed = buildViewerFeed(scene);
      const sceneModel = viewer.addModel(feed.xyz, "xyz");
      // DISPLAY-ONLY bond filter (unit bond-display-control): right after perception,
      // drop cation coordinate bonds + manually hidden AtomId pairs from the live
      // atoms — resolving each viewer index to its AtomId through the SAME feed table
      // picks resolve through. Geometry (feed.xyz) is untouched; only which sticks
      // draw changes. The ephemeral drag/rotate paths reuse this model without
      // re-perceiving, so the filtered bonds stay filtered across an animation.
      applyBondFilter(sceneModel, (vi) => feed.table.atomIdAt(vi), hiddenBonds, showCationBonds);
      // DISPLAY-ONLY geometric bond order (geometric-editor completion): overwrite
      // each surviving bond's order with the nearest single/double/triple from the
      // current geometry, so 3Dmol draws 2/3 parallel sticks for short bonds. Nothing
      // stored — re-derived from geometry every rebuild, like perception; ORCA reads
      // geometry + total charge, never bond order.
      applyGeometricBondOrders(
        sceneModel.selectedAtoms({}) as unknown as OrderableAtom[],
        (elA, elB, d) => bondOrderEstimate(elA, elB, d).order,
      );
      viewerTableRef.current = feed.table;
      // Ball-and-stick: CPK for fragment 0, a flat palette colour per fragment 1+
      // (each reads as one object). The `index` selector is the merged-xyz line
      // order == the Scene global index (ADR-008). Extracted to `applySceneStyle`
      // (unit 3.1) so the ephemeral drag redraw styles identically.
      applySceneStyle(viewer, scene, theme);

      // Arm atom picking (2.5.2a; AtomId in 2c1) — only when a pick handler is
      // present, and re-armed here because removeAllModels/addModel rebuilt the
      // atom objects that carry the clickable flag. 3Dmol's `atom.index` is the
      // VIEWER index; the identity we emit is the AtomId the table names for it.
      // Post-condition (domain rule #9): if the table has no id for this drawn
      // index, emit NOTHING rather than a guessed id — a click that can't be
      // resolved to a real atom is dropped, never mapped to the wrong atom.
      if (pickable) {
        viewer.setClickable({}, true, (atom: { index: number }) => {
          const atomId = viewerTableRef.current?.atomIdAt(atom.index);
          if (atomId === undefined) return;
          onAtomPickRef.current?.({ atomId, viewerIndex: atom.index });
        });
      }

      // Zoom only when the composition changed — not on a coordinate-only edit.
      const signature = compositionSignature(scene);
      if (signature !== lastCompositionRef.current) {
        viewer.zoomTo();
        lastCompositionRef.current = signature;
      }
    } else if (xyzData && xyzData.trim().length > 0) {
      const xyzModel = viewer.addModel(xyzData, "xyz");
      // Geometric bond order (reuse the scene-path call): 1/2/3 lines re-derived from
      // this frame's geometry — display-only, nothing stored.
      applyGeometricBondOrders(
        xyzModel.selectedAtoms({}) as unknown as OrderableAtom[],
        (elA, elB, d) => bondOrderEstimate(elA, elB, d).order,
      );
      // Results picking (Mayer readout): raw 0-based index. Re-armed each render
      // because addModel above rebuilt the atom objects that carry the flag.
      viewer.setClickable({}, true, (atom: { index: number }) =>
        onXyzAtomPickRef.current?.(atom.index),
      );
      viewer.setStyle({}, baseStyle(representation));
      if (preserveCameraOnUpdate) {
        // Trajectory playback: zoom only when the atom COUNT changes (a new
        // molecule), not on a coordinate-only frame advance — otherwise the
        // camera would reset every frame. The first line of an xyz IS the count.
        const count = xyzData.trimStart().split(/\r?\n/, 1)[0].trim();
        const signature = `xyz:${count}`;
        if (signature !== lastCompositionRef.current) {
          viewer.zoomTo();
          lastCompositionRef.current = signature;
        }
      } else {
        // Single-structure path (Molecules screen, previews) — unchanged
        // behaviour: always zoomTo a newly-shown molecule.
        viewer.zoomTo();
        lastCompositionRef.current = null;
      }
    } else {
      // Neither prop — render an empty viewer without crashing.
      lastCompositionRef.current = null;
    }

    viewer.render();
    // Rebuilt on geometry change, picking flip, OR theme change (CPK overrides +
    // per-fragment palette are re-applied). A theme switch keeps the same
    // composition signature, so the zoom guard fires no `zoomTo` — the camera is
    // preserved (background is handled in the separate [theme] effect below).
  }, [xyzData, scene, pickable, theme, preserveCameraOnUpdate, bondTopologyReference, orbitalCube, representation, hiddenBonds, showCationBonds]);

  // Orbital isosurfaces (unit 3.15) — drawn SEPARATELY from the model so changing the
  // isovalue redraws only the two ± surfaces (the cube is parsed once into a VolumeData,
  // cached by its text). Runs after the model effect (the molecule exists) and after the
  // overlay effect (which is guarded to leave these shapes alone). Removes exactly its own
  // two shapes on each change — never `removeAllShapes` (which would wipe selection halos).
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;
    // Clear previous surfaces (also the teardown when the cube goes away).
    for (const s of isoShapesRef.current) viewer.removeShape(s);
    isoShapesRef.current = [];
    if (!orbitalCube?.trim() || orbitalIsoValue == null || orbitalIsoValue <= 0) {
      viewer.render();
      return;
    }
    // Parse the cube ONCE (per cube text); reuse across isovalue changes.
    if (volDataRef.current?.source !== orbitalCube) {
      volDataRef.current = { source: orbitalCube, vol: new VolumeData(orbitalCube, "cube") };
    }
    const vol = volDataRef.current.vol;
    const pos = viewer.addIsosurface(vol, {
      isoval: orbitalIsoValue,
      color: ORBITAL_POS_COLOR,
      opacity: ORBITAL_ISO_OPACITY,
    });
    const neg = viewer.addIsosurface(vol, {
      isoval: -orbitalIsoValue,
      color: ORBITAL_NEG_COLOR,
      opacity: ORBITAL_ISO_OPACITY,
    });
    isoShapesRef.current = [pos, neg];
    viewer.render();
  }, [orbitalCube, orbitalIsoValue]);

  // The overlay effect — the SINGLE owner of every shape and label in the
  // viewer: selection halos, measurement lines/labels (2.5.2b), and atom-number
  // labels (2.5.2e-1). It must be the only place that calls
  // `removeAllShapes`/`removeAllLabels`: a second effect doing so would erase
  // this one's work (and vice-versa). Kept SEPARATE from the model rebuild so a
  // selection or numbering change never reloads the model or moves the camera —
  // `showAtomNumbers` is in the deps but NOT in the model effect's, so toggling
  // Numbers redraws labels only, no `zoomTo`, no `addModel`.
  //
  // `removeAllShapes`/`removeAllLabels` run BEFORE the `!scene` bail-out so a
  // scene going null (last fragment removed) clears halos and labels. Every
  // shape/label here is decoration — none is made clickable (3Dmol shapes and
  // labels default non-clickable and we never call `setClickable` on them), so a
  // label or halo lying over a selected atom can't intercept the pick; a repeat
  // click still toggles the atom off (the 2.5.2a picking path is untouched).
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;
    // The orbital viewer owns its shapes (the isosurfaces) via the isosurface effect —
    // leave them alone. This overlay path is for the scene editor (halos/measurement),
    // which the orbital viewer never uses.
    if (orbitalCube?.trim()) return;
    viewer.removeAllShapes();
    viewer.removeAllLabels();
    if (!scene) {
      viewer.render();
      return;
    }
    // The overlay is fed through the SAME table the geometry was built with (2c1):
    // the atoms in viewer order, the table naming their indices, and an id→atom map.
    // Since 2c2 the **selection is `AtomId[]`**, so a halo resolves id → atom
    // **directly** through `byId` — no positional round-trip (the id IS the atom).
    // The **mask** stays a positional global index (`EditPlan.mask` is the ASE
    // emit seam, positional by design), so it resolves index → AtomId → atom via
    // the table. The NUMBER a label shows is the atom's viewer index read from the
    // table, never a loop counter that merely coincides with 3Dmol's index.
    const atoms = scene.fragments.flatMap((f) => f.atoms);
    const table = buildViewerAtomTable(scene);
    const byId = new Map<AtomId, SceneAtom>(atoms.map((a) => [a.id, a]));
    const atomAtGlobalIndex = (gi: number): SceneAtom | undefined => {
      const id = table.atomIdAt(gi); // positional global index → AtomId (through the table)
      return id === undefined ? undefined : byId.get(id);
    };

    // Mask "will-move" glow (2.5.2d) — drawn FIRST (a soft solid sphere), so the
    // crisp selection cage sits on top of it where they overlap. Positional
    // (ASE-mask space), resolved through the table.
    for (const gi of maskHighlight ?? []) {
      const atom = atomAtGlobalIndex(gi);
      if (!atom) continue;
      viewer.addSphere({
        center: { x: atom.x, y: atom.y, z: atom.z },
        radius: highlightRadius(atom.element) + MASK_RADIUS_BOOST,
        color: theme.haloColor,
        opacity: MASK_OPACITY,
        wireframe: false,
      });
    }

    // Steric-clash danger glow (unit 3.2) — a SOLID magenta sphere, drawn before
    // the selection cage so a crisp halo still reads on top where they overlap.
    // `AtomId[]` → resolve directly (like `selection`); an id no longer in the scene
    // just draws nothing. A warning marker only.
    for (const id of clashHighlight ?? []) {
      const atom = byId.get(id);
      if (!atom) continue;
      viewer.addSphere({
        center: { x: atom.x, y: atom.y, z: atom.z },
        radius: highlightRadius(atom.element) + CLASH_RADIUS_BOOST,
        color: CLASH_COLOR,
        opacity: CLASH_OPACITY,
        wireframe: false,
      });
    }

    // Selection halos — wireframe spheres sized per element (see highlight.ts),
    // coloured by the theme. `selection` is `AtomId[]` (2c2) → resolve directly;
    // an id no longer in the scene (`filterSelection` guards) just draws nothing.
    for (const id of selection ?? []) {
      const atom = byId.get(id);
      if (!atom) continue;
      viewer.addSphere({
        center: { x: atom.x, y: atom.y, z: atom.z },
        radius: highlightRadius(atom.element),
        color: theme.haloColor,
        opacity: HALO_OPACITY,
        wireframe: true,
      });
    }

    // Rotation axis vs measurement — EXACTLY ONE overlay for the picked pair (unit
    // 3.3b). While the Rotate panel holds an axis `[P, Q]` the toggle picks which:
    // the extended axis cylinder (with the Å value on the axis midpoint) OR the
    // measurement distance line + label — never both (they drew on the same two
    // atoms and read as one wrong line). Outside Rotate (`axisHighlight` null) the
    // measurement is untouched. The Å number is ALWAYS the `measure` distance —
    // `rotationAxisValueLabel` reuses `measureSelection`, the same source
    // `drawMeasurement` uses, so it reads identically in both modes.
    const overlayPlan = chooseRotateOverlay(axisHighlight != null, rotateOverlay);
    if (overlayPlan.axis && axisHighlight) {
      const pa = byId.get(axisHighlight[0]);
      const qa = byId.get(axisHighlight[1]);
      if (pa && qa) {
        // Both endpoints are fixed points of the rotation (P is the pivot; Q lies on
        // the line), so drawing at committed coords stays correct during preview.
        drawRotationAxis(viewer, pa, qa, theme.axisColor);
        const value = rotationAxisValueLabel(scene, axisHighlight);
        if (value) drawValueLabel(viewer, midpoint(pa, qa), value, theme);
      }
    }
    if (overlayPlan.measure) {
      drawMeasurement(viewer, scene, selection ?? [], theme);
    }

    // Atom numbers — keyed by AtomId, valued by the table's viewer index (the same
    // positional 0-based number as before, but SOURCED from the table). Every atom
    // when the toggle is on; selected atoms ALWAYS (so a pick reads even with the
    // toggle off). Renaming the index SPACE in the UI is done in the panels; the
    // number the 3D view shows stays the 0-based viewer index.
    const numbered = new Map<AtomId, number>();
    if (showAtomNumbers) {
      for (const a of atoms) numbered.set(a.id, table.viewerIndexOf(a.id)!);
    }
    for (const id of selection ?? []) {
      const vi = table.viewerIndexOf(id);
      if (vi !== undefined) numbered.set(id, vi);
    }
    for (const [id, n] of numbered) {
      const atom = byId.get(id);
      if (!atom) continue;
      viewer.addLabel(String(n), {
        position: { x: atom.x, y: atom.y, z: atom.z },
        fontSize: 11,
        fontColor: theme.labelText,
        backgroundColor: theme.labelBg,
        backgroundOpacity: 0.6,
        inFront: true,
      });
    }

    // Formal-charge labels (geometric-editor completion) — a small `+1`/`−1` on any
    // atom carrying a nonzero formal charge, nudged off the number label. DISPLAY-ONLY
    // bookkeeping: re-read from the `formalCharges` map each overlay pass, never stored
    // in the Scene, never an ORCA input.
    for (const [id, q] of formalCharges ?? []) {
      if (q === 0) continue;
      const atom = byId.get(id);
      if (!atom) continue;
      viewer.addLabel(q > 0 ? `+${q}` : `−${Math.abs(q)}`, {
        position: { x: atom.x, y: atom.y + 0.35, z: atom.z },
        fontSize: 11,
        fontColor: theme.labelText,
        backgroundColor: theme.haloColor,
        backgroundOpacity: 0.7,
        inFront: true,
      });
    }

    viewer.render();
  }, [selection, scene, showAtomNumbers, formalCharges, theme, maskHighlight, clashHighlight, axisHighlight, rotateOverlay, orbitalCube]);

  // Ephemeral coordinate-only overlay — the live rotation preview (unit 3.3). Reuses
  // the SAME frozen-topology coordinate-update path the Move-mode drag and the mode
  // animation use: the live model atoms' coords are set from `ephemeralScene` (same
  // composition, one fragment turned) then `applySceneStyle` re-draws the sticks at
  // the new coords — NO `addModel`, no bond re-perception (so an inter-fragment
  // contact doesn't flicker a stick as the angle turns), no `zoomTo`. The Scene/store
  // is untouched (ADR-010). On `null` we restore the committed coords (Cancel makes
  // the preview vanish); on Apply the committed `scene` changes and the model effect
  // rebuilds at the final coords. A ref tracks whether we currently hold an overlay so
  // an unrelated re-render doesn't churn a restore.
  const ephemeralActiveRef = useRef(false);
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;
    if (orbitalCube?.trim()) return; // the orbital viewer owns its own model
    const model = viewer.getModel();
    if (!model) return;
    const live = model.selectedAtoms({}) as Array<{ x: number; y: number; z: number }>;
    const writeCoords = (src: Scene): boolean => {
      const atoms = src.fragments.flatMap((f) => f.atoms);
      if (atoms.length !== live.length) return false; // composition mismatch → skip, never corrupt
      for (let i = 0; i < live.length; i++) {
        live[i].x = atoms[i].x;
        live[i].y = atoms[i].y;
        live[i].z = atoms[i].z;
      }
      applySceneStyle(viewer, src, theme); // nulls cached geometry → sticks redraw at new coords
      viewer.render();
      return true;
    };
    if (ephemeralScene) {
      if (writeCoords(ephemeralScene)) ephemeralActiveRef.current = true;
    } else if (ephemeralActiveRef.current && scene) {
      // Preview cancelled/applied → put the committed coordinates back.
      writeCoords(scene);
      ephemeralActiveRef.current = false;
    }
  }, [ephemeralScene, scene, theme, orbitalCube]);

  // Rigid-body fragment drag — "Move mode" (unit 3.1). Active only on a pickable
  // scene with Move mode on and a drag callback wired. The whole interaction is a
  // VIEWER-ONLY ephemeral overlay: the Scene/store is untouched until mouseup, when
  // exactly ONE `translate-fragment` op is committed with the total delta (ADR-010;
  // the pure accumulate/commit logic is `fragment-drag.ts`, unit-tested without
  // jsdom). The mousedown listener is on the CONTAINER in the CAPTURE phase, so an
  // atom-grab `stopPropagation`s BEFORE 3Dmol's canvas mousedown — that suppresses
  // camera rotation for the drag (3Dmol's rotate is gated on state it sets in that
  // mousedown). A drag on empty space is not stopped → 3Dmol rotates as usual; a
  // click still picks (the drag commits nothing on a zero move).
  const dragCleanupRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    const container = containerRef.current;
    const viewer = viewerRef.current;
    if (!container || !viewer) return;
    if (!moveMode || !pickable || !scene || scene.fragments.length === 0) return;

    const ranges = fragmentRanges(scene);

    const onMouseDown = (e: MouseEvent) => {
      if (e.button !== 0) return; // left button only
      const model = viewer.getModel();
      if (!model) return;
      const atoms = model.selectedAtoms({}) as Array<{ x: number; y: number; z: number }>;
      if (atoms.length === 0) return;

      // Hit-test: nearest projected atom within the grab radius (page pixels — the
      // space `modelToScreen` returns; verified against the canvas rect, debugging/013).
      const screens = viewer.modelToScreen(
        atoms.map((a) => ({ x: a.x, y: a.y, z: a.z })),
      ) as Array<{ x: number; y: number }>;
      let best = -1;
      let bestD = GRAB_RADIUS_PX;
      for (let i = 0; i < screens.length; i++) {
        const d = Math.hypot(screens[i].x - e.pageX, screens[i].y - e.pageY);
        if (d < bestD) {
          bestD = d;
          best = i;
        }
      }
      if (best < 0) return; // empty space → let 3Dmol rotate (don't stopPropagation)

      const fi = ranges.findIndex((r) => best >= r.start && best < r.end);
      if (fi < 0) return;
      const range = ranges[fi];
      const fragmentId = scene.fragments[fi].id;

      // Grab: suppress the camera for this drag (beat 3Dmol's canvas mousedown).
      e.stopPropagation();
      e.preventDefault();

      // The grabbed fragment's live atom refs + their pre-drag coords, and the
      // grabbed atom's depth (for pixel-exact screen-plane tracking).
      const fragAtoms = atoms.slice(range.start, range.end);
      const orig = fragAtoms.map((a) => ({ x: a.x, y: a.y, z: a.z }));
      const modelz = grabbedAtomDepth(viewer, atoms[best]);
      const fragData = scene.fragments[fi].atoms; // canonical (element + pre-drag Å)
      const localGrabbed = best - range.start; // the grabbed atom's index IN the fragment

      // The MOVING SET as LOCAL indices into `fragAtoms`. `null` = "not resolved yet"
      // → treated as the WHOLE fragment (the backward-compatible fallback), so an
      // early drag frame or a sidecar failure still moves rigidly, never wrongly.
      // Narrowed ONCE, on mousedown (below), to the dragged atom's perceived
      // connected component. `settled` guards the async resolve from touching a
      // finished drag; `fetchFailed` drives the honest fallback note on release.
      let movingLocal: Set<number> | null = null;
      let settled = false;
      let fetchFailed = false;
      let lastDelta: WorldDelta | null = null;
      const inSet = (i: number) => movingLocal === null || movingLocal.has(i);

      // Draw the ephemeral overlay at `delta`: ONLY the moving-set atoms shift; the
      // rest stay pinned at `orig` (so after a bond break the other piece stays put).
      const drawAt = (delta: WorldDelta) => {
        for (let i = 0; i < fragAtoms.length; i++) {
          const move = inSet(i);
          fragAtoms[i].x = orig[i].x + (move ? delta[0] : 0);
          fragAtoms[i].y = orig[i].y + (move ? delta[1] : 0);
          fragAtoms[i].z = orig[i].z + (move ? delta[2] : 0);
        }
        applySceneStyle(viewer, scene, theme); // nulls cached geometry → sticks redraw at new coords
        viewer.render();
      };
      const restoreCoords = () => {
        for (let i = 0; i < fragAtoms.length; i++) {
          fragAtoms[i].x = orig[i].x;
          fragAtoms[i].y = orig[i].y;
          fragAtoms[i].z = orig[i].z;
        }
        applySceneStyle(viewer, scene, theme);
        viewer.render();
      };

      // Resolve the component ONCE per mousedown (not per frame): send the FRAGMENT's
      // own xyz + the grabbed atom's local index; the sidecar returns the local
      // indices that travel with it. A fully bonded fragment → all of them (== the
      // whole fragment). On failure we keep the whole-fragment fallback and flag it.
      const fragXyz =
        `${fragData.length}\n\n` +
        fragData.map((a) => `${a.element} ${a.x} ${a.y} ${a.z}`).join("\n") +
        "\n";
      postSidecar<{ component: number[] }>("/geometry/connected-component", {
        xyz: fragXyz,
        atom: localGrabbed,
      })
        .then((res) => {
          if (settled) return; // the drag already finished → don't touch it
          movingLocal = new Set(res.component);
          if (lastDelta) drawAt(lastDelta); // narrow an already-moving ephemeral drag
        })
        .catch(() => {
          if (settled) return;
          fetchFailed = true; // fall back to the whole fragment; note it on release
        });

      // The moving set as stable AtomIds, for the ONE commit on release.
      const movingAtomIds = (): AtomId[] => {
        const ids: AtomId[] = [];
        for (let i = 0; i < fragAtoms.length; i++) {
          if (!inSet(i)) continue;
          const id = viewerTableRef.current?.atomIdAt(range.start + i);
          if (id !== undefined) ids.push(id);
        }
        return ids;
      };

      const controller = makeDragController({
        unproject: (dxPx, dyPx): WorldDelta => {
          const wd = viewer.screenOffsetToModel(dxPx, dyPx, modelz) as {
            x: number;
            y: number;
            z: number;
          };
          return [wd.x, wd.y, wd.z];
        },
        showEphemeral: (delta) => {
          lastDelta = delta;
          drawAt(delta);
        },
        commit: (fid, delta) => {
          onFragmentDragRef.current?.(fid, movingAtomIds(), delta[0], delta[1], delta[2]);
          // Honest note: a real move was committed but we could not perceive the
          // component, so it moved the whole fragment (never a silent wrong move).
          if (fetchFailed) {
            onFragmentDragFallbackRef.current?.(
              "Couldn't reach the chemistry sidecar to find the connected atoms — moved the whole fragment instead.",
            );
          }
        },
        restore: () => restoreCoords(),
      });
      controller.begin(fragmentId, [e.pageX, e.pageY]);

      // Distinguish a CLICK (→ pick the grabbed atom, since we intercepted 3Dmol's
      // own click handler) from a DRAG (→ move the fragment). A pointer that stays
      // within the threshold is a click; crossing it starts the ephemeral drag.
      let didDrag = false;
      const onMove = (me: MouseEvent) => {
        if (!didDrag) {
          if (Math.hypot(me.pageX - e.pageX, me.pageY - e.pageY) < DRAG_THRESHOLD_PX) return;
          didDrag = true;
        }
        controller.move([me.pageX, me.pageY]);
      };
      const onUp = (ue: MouseEvent) => {
        if (didDrag) {
          controller.end([ue.pageX, ue.pageY]); // commits ONE op with the total delta
        } else {
          controller.cancel(); // a click, not a drag → pick, don't move
          const atomId = viewerTableRef.current?.atomIdAt(best);
          if (atomId !== undefined) onAtomPickRef.current?.({ atomId, viewerIndex: best });
        }
        cleanup();
      };
      const cleanup = () => {
        settled = true; // a late connected-component resolve must not touch this drag
        window.removeEventListener("mousemove", onMove, true);
        window.removeEventListener("mouseup", onUp, true);
        dragCleanupRef.current = null;
      };
      // Track window listeners so a mid-drag unmount / Move-mode-off cancels cleanly.
      dragCleanupRef.current = () => {
        controller.cancel();
        cleanup();
      };
      window.addEventListener("mousemove", onMove, true);
      window.addEventListener("mouseup", onUp, true);
    };

    container.addEventListener("mousedown", onMouseDown, true); // capture — before 3Dmol's canvas mousedown
    return () => {
      container.removeEventListener("mousedown", onMouseDown, true);
      dragCleanupRef.current?.(); // cancel an in-flight drag on teardown (Move off / unmount)
    };
  }, [moveMode, pickable, scene, theme]);

  return <div ref={containerRef} className="molecule-viewer" style={style} />;
  },
);
