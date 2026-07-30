import { useLayoutEffect, useRef, useState } from "react";

/**
 * Measure a container's content width via `ResizeObserver`.
 *
 * recharts' `ResponsiveContainer` measures 0×0 in WebKitGTK (Tauri's engine —
 * the same webview-mismeasurement class as the 3Dmol OffscreenCanvas and
 * `<select>` bugs, `wiki/debugging/002`/`003`), so every chart in the app passes
 * an explicit pixel `width` instead. This hook is the one owner of that
 * workaround — the convergence dashboard, the IR spectrum, and the trajectory
 * energy chart all read from it (extracted from `ConvergenceDashboard` so there
 * is exactly one copy).
 */
export function useContainerWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => setWidth(el.clientWidth);
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return { ref, width };
}
