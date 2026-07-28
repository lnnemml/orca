import { useEffect, useRef } from "react";
import { createViewer, type GLViewer } from "3dmol";

import type { Scene } from "../scene/types";
import { fragmentRanges, mergeToXyz } from "../scene/scene";
import { fragmentColor } from "./fragment-colors";

// Side-effect: force 3Dmol onto its direct-canvas WebGL path so it renders in
// the WebKitGTK webview (must run before the first createViewer).
import "./3dmol-setup";

interface MoleculeViewerProps {
  /** Flat xyz — the existing single-structure path (Molecules screen, previews). */
  xyzData?: string;
  /** Multi-fragment path: takes precedence over `xyzData` when present. */
  scene?: Scene;
  style?: React.CSSProperties;
}

// Match the log console (#0d0f13) so the viewer sits inside the dark theme.
const BACKGROUND = "#0d0f13";

/** Ball-and-stick — the same style the viewer has always used. Fresh object per
 * call (3Dmol may retain the reference). */
const baseStyle = () => ({ stick: {}, sphere: { scale: 0.3 } });

/**
 * A "composition signature" for a scene: atom count + the ordered fragment
 * id:size list, but NOT the coordinates. Two renders with the same signature
 * differ only in atom positions, so the camera must stay put (the 2.5.3 geometry
 * loop — type an angle, apply, look, adjust — is unusable if the view re-zooms
 * on every apply). A changed signature means atoms were added/removed and a
 * fresh `zoomTo` is warranted.
 */
function compositionSignature(scene: Scene): string {
  return scene.fragments.map((f) => `${f.id}:${f.atoms.length}`).join("|");
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
export function MoleculeViewer({ xyzData, scene, style }: MoleculeViewerProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewerRef = useRef<GLViewer | null>(null);
  // Last scene composition rendered — drives the zoom-only-on-composition-change
  // rule. `null` means "nothing/only-xyz rendered so far".
  const lastCompositionRef = useRef<string | null>(null);

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
  }, [xyzData, scene]);

  return <div ref={containerRef} className="molecule-viewer" style={style} />;
}
