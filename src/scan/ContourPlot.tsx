// The ONLY plotly-touching file — a thin, swappable wrapper (m0 de-risk isolation). If plotly
// renders heavy/blank in WebKitGTK (the m0 live gate), only THIS file swaps to a d3-contour
// fallback; `ScanSurface2dPanel` keeps the same `<ContourPlot …>` props.
//
// Uses `plotly.js-cartesian-dist-min` — the SVG cartesian bundle (scatter/heatmap/contour),
// **NOT** the WebGL (`scattergl`) traces — to avoid WebKitGTK WebGL issues (the reason plotly
// was chosen over recharts, which has no contour trace).
//
// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-expect-error — the cartesian dist has no bundled types; we treat it as the Plotly module.
import Plotly from "plotly.js-cartesian-dist-min";
import createPlotlyComponent from "react-plotly.js/factory";

import { nearestIndex } from "./nearestIndex";

const Plot = createPlotlyComponent(Plotly);

export interface ContourPlotProps {
  /** Coordinate-1 values → the Y axis (the OUTER scan loop). */
  axis1: number[];
  /** Coordinate-2 values → the X axis (the INNER scan loop). */
  axis2: number[];
  /** `z[i1][i2]` — the value to colour (ΔE kcal/mol) at `(axis1[i1], axis2[i2])`. */
  z: number[][];
  x2Label: string;
  x1Label: string;
  colorbarTitle: string;
  /** Orientation labels (reactant / product / stepwise corners) — facts, NOT a TS claim. */
  annotations?: Array<{ i1: number; i2: number; text: string }>;
  /** Fired when a grid node is clicked, in grid indices (i1 = coord1, i2 = coord2). */
  onNodeClick: (i1: number, i2: number) => void;
  height?: number;
}

/**
 * A filled contour + heatmap of `z` over `(axis2 = x, axis1 = y)`, with a colorbar, hover, and
 * an overlaid scatter of the N₁×N₂ grid nodes as **clickable markers**. Clicking a marker maps
 * its flat row-major index back to `(i1, i2)` and calls `onNodeClick` — the panel turns that into
 * `nodeRow` → the point geometry → OptTS. No node is preselected (the user picks the col).
 */
export function ContourPlot({
  axis1,
  axis2,
  z,
  x2Label,
  x1Label,
  colorbarTitle,
  annotations,
  onNodeClick,
  height = 420,
}: ContourPlotProps) {
  const n2 = axis2.length;

  // Grid-node markers in ROW-MAJOR order (i1 outer, i2 inner), so a marker's pointNumber k
  // maps back to (i1, i2) = (⌊k/N2⌋, k mod N2) — the same order as `nodeRow`.
  const markerX: number[] = [];
  const markerY: number[] = [];
  for (let i1 = 0; i1 < axis1.length; i1++) {
    for (let i2 = 0; i2 < n2; i2++) {
      markerX.push(axis2[i2]);
      markerY.push(axis1[i1]);
    }
  }

  const data = [
    {
      type: "contour" as const,
      x: axis2,
      y: axis1,
      z,
      colorscale: "Viridis",
      contours: { coloring: "heatmap" as const },
      colorbar: { title: { text: colorbarTitle }, thickness: 12 },
      hovertemplate: `${x2Label}: %{x:.3f}<br>${x1Label}: %{y:.3f}<br>ΔE: %{z:.2f} kcal/mol<extra></extra>`,
    },
    {
      type: "scatter" as const,
      mode: "markers" as const,
      x: markerX,
      y: markerY,
      marker: { size: 6, color: "rgba(255,255,255,0.35)", line: { width: 0 } },
      hoverinfo: "skip" as const,
      showlegend: false,
    },
  ];

  const layout = {
    height,
    margin: { l: 56, r: 16, t: 16, b: 44 },
    xaxis: { title: { text: x2Label } },
    yaxis: { title: { text: x1Label } },
    annotations: (annotations ?? []).map((a) => ({
      x: axis2[a.i2],
      y: axis1[a.i1],
      text: a.text,
      showarrow: false,
      font: { size: 10, color: "#fff" },
      bgcolor: "rgba(0,0,0,0.45)",
      borderpad: 2,
    })),
  };

  return (
    <Plot
      data={data}
      layout={layout}
      config={{ displayModeBar: false, responsive: true }}
      style={{ width: "100%" }}
      onClick={(e: { points?: Array<{ x?: number; y?: number }> }) => {
        // Any point (contour OR marker) carries the click's DATA-space x/y — so `[0]` is enough;
        // the old marker-only gate (`curveNumber === 1`) silently dropped every click that wasn't
        // pixel-exact on a 6px node (the m2 bug). SNAP to the nearest node: x = coord2 (X axis),
        // y = coord1 (Y axis) — do NOT swap, or OptTS gets the transposed node.
        const p = e.points?.[0];
        if (p?.x == null || p?.y == null) return;
        onNodeClick(nearestIndex(axis1, p.y), nearestIndex(axis2, p.x));
      }}
    />
  );
}
