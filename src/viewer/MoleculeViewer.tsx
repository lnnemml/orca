import { useEffect, useRef } from "react";
import { createViewer, type GLViewer } from "3dmol";

import type { Scene } from "../scene/types";
import { compositionSignature, fragmentRanges, mergeToXyz } from "../scene/scene";
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
  style?: React.CSSProperties;
}

// Match the log console (#0d0f13) so the viewer sits inside the dark theme.
const BACKGROUND = "#0d0f13";

/** Highlight sphere for a selected atom — translucent so the CPK/fragment
 * colour underneath still reads. Radius is a touch above the ball-and-stick
 * sphere (scale 0.3) so it reads as a halo, not a repaint. */
const HIGHLIGHT_COLOR = "#ffffff";
const HIGHLIGHT_RADIUS = 0.55;
const HIGHLIGHT_OPACITY = 0.35;

/** Ball-and-stick — the same style the viewer has always used. Fresh object per
 * call (3Dmol may retain the reference). */
const baseStyle = () => ({ stick: {}, sphere: { scale: 0.3 } });

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

  // Highlight the selected atoms with translucent spheres. Kept a SEPARATE
  // effect from the model rebuild so a selection change never reloads the model
  // or moves the camera (`removeAllShapes` + re-add spheres, no zoomTo). Spheres
  // — not setStyle — because a style override would clobber the per-fragment
  // colours applied above and we'd have to restore them by hand; the spheres
  // will also carry the measurement labels in 2.5.2b.
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer || !scene) return;
    viewer.removeAllShapes();
    const rows = scene.fragments.flatMap((f) => f.atoms);
    for (const globalIndex of selection ?? []) {
      const atom = rows[globalIndex];
      if (!atom) continue; // stale index — validateSelection normally prevents this
      viewer.addSphere({
        center: { x: atom.x, y: atom.y, z: atom.z },
        radius: HIGHLIGHT_RADIUS,
        color: HIGHLIGHT_COLOR,
        opacity: HIGHLIGHT_OPACITY,
      });
    }
    viewer.render();
  }, [selection, scene]);

  return <div ref={containerRef} className="molecule-viewer" style={style} />;
}
