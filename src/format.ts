//! Small display formatters shared across job screens.

/** Final SCF energy in Hartree, fixed to 6 decimals; `—` when absent. */
export function formatEnergy(energy: number | null): string {
  return energy == null ? "—" : energy.toFixed(6);
}

/** Wall time (seconds) as `35.4s` / `2m 15s` / `1h 5m`; `—` when absent. */
export function formatWallTime(seconds: number | null): string {
  if (seconds == null) return "—";
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  if (seconds < 3600) {
    const m = Math.floor(seconds / 60);
    const s = Math.round(seconds % 60);
    return `${m}m ${s}s`;
  }
  const h = Math.floor(seconds / 3600);
  const m = Math.round((seconds % 3600) / 60);
  return `${h}h ${m}m`;
}

/** SQLite `datetime('now')` timestamp rendered compactly. */
export function formatTimestamp(ts: string): string {
  return ts.replace("T", " ").replace(/\.\d+Z?$/, "");
}
