import { useEffect, useRef } from "react";
import { createViewer, GLModel, VolumeData, elementColors, type GLViewer, type GLShape } from "3dmol";

import type { Scene, SceneAtom } from "../scene/types";
import { compositionSignature, fragmentRanges, mergeToXyz } from "../scene/scene";
import { measureSelection, formatMeasurementValue } from "../scene/measure";
import { highlightRadius, vdwTableDrift } from "./highlight";
import { DEFAULT_THEME, cpkColorDrift, type ViewerTheme } from "./theme";
import { parseXyzCoords, applyCoordsToAtoms, drawableBondCount } from "./frozenTopology";

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
   * re-`zoomTo`s. Ignored unless `onAtomPick` is also given (picking off).
   */
  selection?: number[];
  /**
   * Atom-pick callback (2.5.2a). **Presence of this prop is what turns
   * clickability on** — without it the viewer is display-only (Molecules screen,
   * the Job-detail conformer panel), exactly as before. Receives the 0-based
   * `atom.index` (== merged-xyz line == Scene global index; never `atom.serial`
   * — ADR-008 decision 3).
   */
  onAtomPick?: (globalIndex: number) => void;
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
  style?: React.CSSProperties;
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

/** Ball-and-stick — the same style the viewer has always used. Fresh object per
 * call (3Dmol may retain the reference). */
const baseStyle = () => ({ stick: {}, sphere: { scale: 0.3 } });

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

/** Radius (Å) of the thick, solid cylinder marking a dihedral's j–k axis
 * (2.5.2e-2) — chunky enough to read as the axis against the thin dashed i–j /
 * k–l lines. */
const AXIS_RADIUS = 0.05;

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
  selection: number[],
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

  viewer.addLabel(label, {
    position: anchor,
    backgroundColor: theme.labelBg,
    backgroundOpacity: 0.85,
    fontColor: theme.measurementText,
    fontSize: 13,
    inFront: true,
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
export function MoleculeViewer({
  xyzData,
  scene,
  selection,
  onAtomPick,
  showAtomNumbers = false,
  theme = DEFAULT_THEME,
  maskHighlight,
  preserveCameraOnUpdate = false,
  bondTopologyReference,
  orbitalCube,
  orbitalIsoValue,
  style,
}: MoleculeViewerProps) {
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
  // Latest onAtomPick, read through a ref so the model effect (which re-arms
  // setClickable) doesn't need onAtomPick in its dependency list — an inline
  // callback would otherwise rebuild the model on every render.
  const onAtomPickRef = useRef<typeof onAtomPick>(onAtomPick);
  onAtomPickRef.current = onAtomPick;
  // Picking is enabled iff a pick handler was provided. Captured as a boolean so
  // the model effect re-runs when it flips (it never flips in practice — a
  // screen either passes onAtomPick or doesn't — but keeps the effect honest).
  const pickable = onAtomPick != null;

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
      viewer.removeAllModels();
      viewer.addModel(orbCube, "cube");
      viewer.setStyle({}, baseStyle());
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
      let anim = animRef.current;
      const firstBuild = !anim || anim.source !== frozenRef;
      if (firstBuild) {
        viewer.removeAllModels();
        const model = viewer.addModel(frozenRef, "xyz"); // perceive bonds + set index, ONCE
        anim = { source: frozenRef, model };
        animRef.current = anim;
      }
      // Move the existing atoms to this frame; bonds/index/style are untouched.
      applyCoordsToAtoms(
        anim!.model.selectedAtoms({}) as Array<{ x: number; y: number; z: number }>,
        parseXyzCoords(xyzData!), // frozenRef truthy ⇒ xyzData is a non-empty string
      );
      anim!.model.setStyle({}, baseStyle()); // (re)apply style + null the cached geometry
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

    if (scene && scene.fragments.length > 0) {
      viewer.addModel(mergeToXyz(scene), "xyz");
      // Base ball-and-stick with CPK element colours for every atom — with the
      // theme's low-contrast element overrides folded in (light/white), so
      // fragment 0 (the CPK fragment) is legible on the background...
      viewer.setStyle({}, cpkBaseStyle(theme));
      // ...then override fragments 1+ with a flat palette colour on BOTH stick
      // and sphere so each fragment reads as one object. Fragment 0 keeps CPK.
      // The palette is the THEME's (darker on light themes); the `index` selector
      // takes the 0-based atom index, which for an xyz model is the merged-xyz
      // line order == the Scene global index (ADR-008).
      const palette = theme.fragmentPalette;
      fragmentRanges(scene).forEach((range, fragmentIndex) => {
        if (fragmentIndex === 0) return; // fragment 0 → leave on CPK colours
        const color = palette[(fragmentIndex - 1) % palette.length];
        const indices: number[] = [];
        for (let i = range.start; i < range.end; i++) indices.push(i);
        viewer.setStyle(
          { index: indices },
          { stick: { color }, sphere: { scale: 0.3, color } },
        );
      });

      // Arm atom picking (2.5.2a) — only when a pick handler is present, and
      // re-armed here because removeAllModels/addModel rebuilt the atom objects
      // that carry the clickable flag. `atom.index` is the pick identity.
      if (pickable) {
        viewer.setClickable({}, true, (atom: { index: number }) => {
          onAtomPickRef.current?.(atom.index);
        });
      }

      // Zoom only when the composition changed — not on a coordinate-only edit.
      const signature = compositionSignature(scene);
      if (signature !== lastCompositionRef.current) {
        viewer.zoomTo();
        lastCompositionRef.current = signature;
      }
    } else if (xyzData && xyzData.trim().length > 0) {
      viewer.addModel(xyzData, "xyz");
      viewer.setStyle({}, baseStyle());
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
  }, [xyzData, scene, pickable, theme, preserveCameraOnUpdate, bondTopologyReference, orbitalCube]);

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
    const rows = scene.fragments.flatMap((f) => f.atoms);

    // Mask "will-move" glow (2.5.2d) — drawn FIRST (a soft solid sphere), so the
    // crisp selection cage sits on top of it where they overlap.
    for (const gi of maskHighlight ?? []) {
      const atom = rows[gi];
      if (!atom) continue;
      viewer.addSphere({
        center: { x: atom.x, y: atom.y, z: atom.z },
        radius: highlightRadius(atom.element) + MASK_RADIUS_BOOST,
        color: theme.haloColor,
        opacity: MASK_OPACITY,
        wireframe: false,
      });
    }

    // Selection halos — wireframe spheres sized per element (see highlight.ts),
    // coloured by the theme.
    for (const gi of selection ?? []) {
      const atom = rows[gi]; // stale index → undefined; validateSelection guards
      if (!atom) continue;
      viewer.addSphere({
        center: { x: atom.x, y: atom.y, z: atom.z },
        radius: highlightRadius(atom.element),
        color: theme.haloColor,
        opacity: HALO_OPACITY,
        wireframe: true,
      });
    }

    drawMeasurement(viewer, scene, selection ?? [], theme);

    // Atom numbers — the GLOBAL 0-based index only. Every atom when the toggle
    // is on; selected atoms ALWAYS (so a pick reads even with the toggle off).
    const numbered = new Set<number>();
    if (showAtomNumbers) rows.forEach((_, i) => numbered.add(i));
    for (const gi of selection ?? []) if (rows[gi]) numbered.add(gi);
    for (const gi of numbered) {
      const atom = rows[gi];
      viewer.addLabel(String(gi), {
        position: { x: atom.x, y: atom.y, z: atom.z },
        fontSize: 11,
        fontColor: theme.labelText,
        backgroundColor: theme.labelBg,
        backgroundOpacity: 0.6,
        inFront: true,
      });
    }

    viewer.render();
  }, [selection, scene, showAtomNumbers, theme, maskHighlight, orbitalCube]);

  return <div ref={containerRef} className="molecule-viewer" style={style} />;
}
