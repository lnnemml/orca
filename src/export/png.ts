//! PNG helpers — browser-only (canvas / Image), so not node-tested; the WebKitGTK PNG
//! GATE (both paths) is measured in `wiki/debugging/009`. Two producers:
//!  * `svgToPngBytes` — a recharts `<svg>` → 2D-canvas → PNG (charts);
//!  * `dataUrlToBytes` — turn 3Dmol's `viewer.pngURI()` into bytes (the 3D scene).

/** Fixed export resolution: **2× the on-screen pixels** — named, not eyeballed. */
export const EXPORT_SCALE = 2;

/** Decode a `data:...;base64,…` URL to raw bytes. */
export function dataUrlToBytes(dataUrl: string): Uint8Array {
  const comma = dataUrl.indexOf(",");
  const bin = atob(dataUrl.slice(comma + 1));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

/**
 * Rasterize a recharts `<svg>` to PNG bytes. The standalone SVG has no `:root`, so
 * themed `var(--…)` colours are resolved to their computed values (rule #11: the export
 * carries real colours, not unresolved references), the background is forced white, and
 * the canvas is `EXPORT_SCALE`× for a crisp image.
 */
export async function svgToPngBytes(svg: SVGSVGElement): Promise<Uint8Array> {
  const w = svg.width?.baseVal?.value || svg.clientWidth || 640;
  const h = svg.height?.baseVal?.value || svg.clientHeight || 320;

  let s = new XMLSerializer().serializeToString(svg);
  const root = getComputedStyle(document.documentElement);
  s = s.replace(/var\((--[\w-]+)\)/g, (_m, name: string) => root.getPropertyValue(name).trim() || "#000");
  if (!/^<svg[^>]*xmlns=/.test(s)) s = s.replace("<svg", '<svg xmlns="http://www.w3.org/2000/svg"');

  const url = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(s);
  const img = new Image();
  await new Promise<void>((resolve, reject) => {
    img.onload = () => resolve();
    img.onerror = () => reject(new Error("could not rasterize the chart SVG"));
    img.src = url;
  });

  const canvas = document.createElement("canvas");
  canvas.width = Math.round(w * EXPORT_SCALE);
  canvas.height = Math.round(h * EXPORT_SCALE);
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2D canvas context");
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
  return dataUrlToBytes(canvas.toDataURL("image/png"));
}
