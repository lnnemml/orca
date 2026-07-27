import { useEffect, useRef } from "react";
import { createViewer, type GLViewer } from "3dmol";

// Side-effect: force 3Dmol onto its direct-canvas WebGL path so it renders in
// the WebKitGTK webview (must run before the first createViewer).
import "./3dmol-setup";

interface MoleculeViewerProps {
  /** Standard xyz: atom count, comment line, then `element x y z` rows. */
  xyzData: string;
  style?: React.CSSProperties;
}

// Match the log console (#0d0f13) so the viewer sits inside the dark theme.
const BACKGROUND = "#0d0f13";

/**
 * Ball-and-stick molecule viewer built on 3Dmol.js. Fills its parent; mouse
 * rotate/zoom/pan is 3Dmol's default behaviour. One WebGL context is created
 * on mount and released on unmount — 3Dmol holds it explicitly and it leaks
 * otherwise (see wiki/modules/visualization.md).
 */
export function MoleculeViewer({ xyzData, style }: MoleculeViewerProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewerRef = useRef<GLViewer | null>(null);

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
    };
  }, []);

  // Re-render whenever the coordinates change.
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;
    viewer.removeAllModels();
    if (xyzData.trim().length > 0) {
      viewer.addModel(xyzData, "xyz");
      viewer.setStyle({}, { stick: {}, sphere: { scale: 0.3 } });
      viewer.zoomTo();
    }
    viewer.render();
  }, [xyzData]);

  return <div ref={containerRef} className="molecule-viewer" style={style} />;
}
