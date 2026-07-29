import { useEffect, useRef } from "react";
import { createViewer, GLModel, type GLViewer } from "3dmol";

import type { Scene, SceneAtom } from "../scene/types";
import { compositionSignature, fragmentRanges, mergeToXyz } from "../scene/scene";
import { measureSelection, formatMeasurementValue } from "../scene/measure";
import { highlightRadius, vdwTableDrift } from "./highlight";
import { DEFAULT_THEME, type ViewerTheme } from "./theme";
import { fragmentColor } from "./fragment-colors";

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
  style?: React.CSSProperties;
}

/** Selection-halo wireframe opacity (2.5.2e-1). The colour comes from the theme
 * (`haloColor`); a constant-radius halo was invisible on carbon, so the RADIUS
 * is per-element via `highlightRadius`, and it's a wireframe cage (reads over
 * CPK red O / grey C where a solid sphere washed out). */
const HALO_OPACITY = 0.85;

/** Ball-and-stick — the same style the viewer has always used. Fresh object per
 * call (3Dmol may retain the reference). */
const baseStyle = () => ({ stick: {}, sphere: { scale: 0.3 } });

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
  style,
}: MoleculeViewerProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewerRef = useRef<GLViewer | null>(null);
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
      const drift = vdwTableDrift(
        GLModel.vdwRadii as Record<string, number | undefined>,
      );
      if (drift.length > 0) {
        console.warn(
          `[MoleculeViewer] highlight.ts vdW radii disagree with 3Dmol for: ${drift.join(", ")} — update VDW_RADII.`,
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
    viewer.removeAllModels();

    if (scene && scene.fragments.length > 0) {
      viewer.addModel(mergeToXyz(scene), "xyz");
      // Base ball-and-stick with CPK element colours for every atom...
      viewer.setStyle({}, baseStyle());
      // ...then override fragments 1+ with a flat palette colour on BOTH stick
      // and sphere so each fragment reads as one object. Fragment 0 keeps CPK.
      // The `index` selector takes the 0-based atom index, which for an xyz
      // model is the merged-xyz line order == the Scene global index (ADR-008).
      fragmentRanges(scene).forEach((range, fragmentIndex) => {
        const color = fragmentColor(fragmentIndex);
        if (!color) return; // fragment 0 → leave on CPK colours
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
      // Legacy single-structure path — unchanged behaviour: always zoomTo.
      viewer.addModel(xyzData, "xyz");
      viewer.setStyle({}, baseStyle());
      viewer.zoomTo();
      lastCompositionRef.current = null;
    } else {
      // Neither prop — render an empty viewer without crashing.
      lastCompositionRef.current = null;
    }

    viewer.render();
    // The model is rebuilt whenever geometry changes OR picking flips on/off.
  }, [xyzData, scene, pickable]);

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
    viewer.removeAllShapes();
    viewer.removeAllLabels();
    if (!scene) {
      viewer.render();
      return;
    }
    const rows = scene.fragments.flatMap((f) => f.atoms);

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
  }, [selection, scene, showAtomNumbers, theme]);

  return <div ref={containerRef} className="molecule-viewer" style={style} />;
}
