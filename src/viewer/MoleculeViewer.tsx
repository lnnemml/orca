import { useEffect, useRef } from "react";
import { createViewer, GLModel, type GLViewer } from "3dmol";

import type { Scene, SceneAtom } from "../scene/types";
import { compositionSignature, fragmentRanges, mergeToXyz } from "../scene/scene";
import { measureSelection, formatMeasurementValue } from "../scene/measure";
import { highlightRadius, vdwTableDrift } from "./highlight";
import { fragmentColor } from "./fragment-colors";

// Side-effect: force 3Dmol onto its direct-canvas WebGL path so it renders in
// the WebKitGTK webview (must run before the first createViewer).
import "./3dmol-setup";

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
  style?: React.CSSProperties;
}

// Match the log console (#0d0f13) so the viewer sits inside the dark theme.
const BACKGROUND = "#0d0f13";

/** Highlight halo for a selected atom (2.5.2e-1). A **wireframe** sphere, sized
 * per element by `highlightRadius` (proportional to 3Dmol's drawn radius — a
 * constant-radius halo was invisible on carbon; see `highlight.ts`). Wireframe,
 * not a solid translucent sphere: the MiniBrowser screenshot showed a solid
 * magenta halo washing out over CPK red oxygen and grey carbon, while the cage
 * reads on all four of H/C/N/O. Colour is a saturated magenta (NOT `#ffffff`,
 * which is CPK hydrogen and vanishes on the light background e-2 adds). */
const HALO_COLOR = "#ff2d95";
const HALO_OPACITY = 0.85;

/** Atom-number label style (2.5.2e-1) — small, semi-transparent backing, drawn
 * in front. Non-clickable (3Dmol labels default so), like the measurement
 * labels. */
const NUMBER_FONT_COLOR = "#e6e6e6";
const NUMBER_BG = "#0d0f13";

/** Ball-and-stick — the same style the viewer has always used. Fresh object per
 * call (3Dmol may retain the reference). */
const baseStyle = () => ({ stick: {}, sphere: { scale: 0.3 } });

/** Measurement decoration (2.5.2b): dashed line between consecutive picks + a
 * value label. Decoration only — never made clickable (see the highlight
 * effect), so it can't intercept an atom pick. */
const MEASURE_COLOR = "#ffd34d";

const xyz = (a: SceneAtom) => ({ x: a.x, y: a.y, z: a.z });
const midpoint = (a: SceneAtom, b: SceneAtom) => ({
  x: (a.x + b.x) / 2,
  y: (a.y + b.y) / 2,
  z: (a.z + b.z) / 2,
});

/**
 * Draw the measurement geometry for the current pick list: one dashed line per
 * bond of the chain, and a single value label anchored where a chemist reads it
 * — the midpoint for a distance (i–j) and for a dihedral (the j–k axis), the
 * vertex for an angle. No-op for 0/1 atoms or any degenerate pick
 * (`measureSelection` → `none`). Caller has already run `removeAllShapes` /
 * `removeAllLabels` and will `render()`.
 */
function drawMeasurement(viewer: GLViewer, scene: Scene, selection: number[]) {
  const m = measureSelection(scene, selection);
  const label = formatMeasurementValue(m);
  if (m.kind === "none" || !label) return;

  const rows = scene.fragments.flatMap((f) => f.atoms);
  const pts = m.atoms.map((gi) => rows[gi]);
  if (pts.some((a) => a == null)) return; // stale index — bail (guarded upstream)

  for (let n = 0; n < pts.length - 1; n++) {
    viewer.addLine({
      dashed: true,
      start: xyz(pts[n]),
      end: xyz(pts[n + 1]),
      color: MEASURE_COLOR,
    });
  }

  const anchor =
    m.kind === "angle"
      ? xyz(pts[1]) // the vertex
      : m.kind === "dihedral"
        ? midpoint(pts[1], pts[2]) // middle of the j–k axis
        : midpoint(pts[0], pts[1]); // distance: midpoint of the bond

  viewer.addLabel(label, {
    position: anchor,
    backgroundColor: "#1b1d23",
    backgroundOpacity: 0.85,
    fontColor: MEASURE_COLOR,
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

    const viewer = createViewer(container, { backgroundColor: BACKGROUND });
    viewerRef.current = viewer;

    // Keep the render surface in sync with the container (flex/split resizes).
    const observer = new ResizeObserver(() => viewer.resize());
    observer.observe(container);

    return () => {
      observer.disconnect();
      viewer.clear(); // drop models/shapes and release the WebGL context
      viewerRef.current = null;
      lastCompositionRef.current = null;
    };
  }, []);

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

    // Selection halos — wireframe spheres sized per element (see highlight.ts).
    for (const gi of selection ?? []) {
      const atom = rows[gi]; // stale index → undefined; validateSelection guards
      if (!atom) continue;
      viewer.addSphere({
        center: { x: atom.x, y: atom.y, z: atom.z },
        radius: highlightRadius(atom.element),
        color: HALO_COLOR,
        opacity: HALO_OPACITY,
        wireframe: true,
      });
    }

    drawMeasurement(viewer, scene, selection ?? []);

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
        fontColor: NUMBER_FONT_COLOR,
        backgroundColor: NUMBER_BG,
        backgroundOpacity: 0.6,
        inFront: true,
      });
    }

    viewer.render();
  }, [selection, scene, showAtomNumbers]);

  return <div ref={containerRef} className="molecule-viewer" style={style} />;
}
