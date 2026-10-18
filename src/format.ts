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

/** SQLite's `datetime('now')` form, exactly: `YYYY-MM-DD HH:MM:SS`. */
const SQLITE_DATETIME = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/;

/**
 * A stored SQLite `datetime('now')` timestamp — **UTC on the laptop's clock** (measured, ADR-024
 * o22.14.6; `db::tests::sqlite_datetime_now_is_utc_under_a_non_utc_tz`) — shown in the laptop's local
 * time as `YYYY-MM-DD HH:MM:SS`. Parsed explicitly as UTC (`T` + `Z`), never `new Date(ts)` on the
 * bare form, whose parse is implementation-defined (V8 reads it as local time). A value of any other
 * shape, or one that does not parse, is shown raw — never "Invalid Date".
 */
export function formatLaptopUtc(ts: string): string {
  if (!SQLITE_DATETIME.test(ts)) return ts;
  const d = new Date(`${ts.replace(" ", "T")}Z`);
  if (Number.isNaN(d.getTime())) return ts;
  const two = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ` +
    `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`
  );
}
