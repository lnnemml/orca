/**
 * The live-progress line for an xtb pre-optimization (2.5.5-fix-2). Kept pure so
 * the display contract is testable without a window: the current optimization
 * cycle once xtb reports one, `starting…` before the first cycle (the pre-cycle
 * window is exactly where the empty-xcontrol hang lived), and always a ticking
 * elapsed clock so "very long" is visible immediately instead of after minutes of
 * silence.
 */
export function formatXtbProgress(cycle: number | null, elapsedSecs: number): string {
  const head = cycle != null ? `optimization cycle ${cycle}` : "starting…";
  return `${head} · ${elapsedSecs}s`;
}
