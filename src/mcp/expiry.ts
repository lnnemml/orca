/**
 * The grant editor's expiry (ADR-027 M4 amendment item 5): a preset computed at Create, or a custom
 * local date and time, turned into the UTC `YYYY-MM-DD HH:MM:SS` text `mcp_create_grant` takes.
 *
 * - **Presets** (1 h, 8 h — the default — 24 h, 7 d) are an exact span from an injected "now", the
 *   moment of Create.
 * - **A custom value** is split by a strict regex into its components and built with
 *   `new Date(y, m - 1, d, h, min)`; **never `new Date(string)`**, whose parse of a local form is
 *   engine-defined. The value comes from `composeExpiryChoice` (`McpGrantEditor.tsx`: the date field plus the hour
 *   and minute selects), but the strict form is enforced here, so a malformed text fails safe:
 *   anything but the strict form is refused.
 * - **The round-trip check (rule #9).** The built instant must read back as the same local
 *   components. A time skipped by a spring-forward change does not, whatever the engine resolved it
 *   to, and is refused: "does not exist — DST".
 * - **The fall-back hour.** A time that occurs twice is flagged `ambiguous`, and every result carries
 *   its UTC offset, so the time shown is the instant that will be stored, whichever offset the engine
 *   picked (V8 picks the earlier, summer one; JavaScriptCore is observed at the live gate).
 *
 * Everything takes its clock as a parameter; nothing reads `Date.now()`.
 */

export type ExpiryPreset = "1h" | "8h" | "24h" | "7d";

const HOUR_MS = 3_600_000;

export const EXPIRY_PRESETS: readonly { id: ExpiryPreset; label: string; ms: number }[] = [
  { id: "1h", label: "1 h", ms: HOUR_MS },
  { id: "8h", label: "8 h", ms: 8 * HOUR_MS },
  { id: "24h", label: "24 h", ms: 24 * HOUR_MS },
  { id: "7d", label: "7 days", ms: 7 * 24 * HOUR_MS },
];

export const DEFAULT_EXPIRY_PRESET: ExpiryPreset = "8h";

/** An expiry the editor can send, with what the form shows for it. */
export interface ResolvedExpiry {
  instant: Date;
  /** What `mcp_create_grant` takes: UTC, `YYYY-MM-DD HH:MM:SS`. */
  utc: string;
  /** The laptop's local time, `YYYY-MM-DD HH:MM`. */
  local: string;
  /** The local time's UTC offset, `UTC+03:00`. */
  offset: string;
  /** The local time occurs twice (the fall-back hour): `offset` says which one this is. */
  ambiguous: boolean;
}

export type ExpiryResult = ({ ok: true } & ResolvedExpiry) | { ok: false; error: string };

/** What the editor's expiry control holds. */
export type ExpiryChoice = { kind: "preset"; preset: ExpiryPreset } | { kind: "custom"; value: string };

const two = (n: number): string => String(n).padStart(2, "0");

/** `d` as UTC `YYYY-MM-DD HH:MM:SS` (SQLite's `datetime('now')` form). */
export function toUtcText(d: Date): string {
  return (
    `${d.getUTCFullYear()}-${two(d.getUTCMonth() + 1)}-${two(d.getUTCDate())} ` +
    `${two(d.getUTCHours())}:${two(d.getUTCMinutes())}:${two(d.getUTCSeconds())}`
  );
}

/** `d` in the laptop's local time, `YYYY-MM-DD HH:MM`. */
export function toLocalText(d: Date): string {
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}`;
}

/** The UTC offset in force at `d` locally, `UTC+03:00` / `UTC-04:30`. */
export function utcOffsetText(d: Date): string {
  const minutes = -d.getTimezoneOffset();
  const sign = minutes < 0 ? "-" : "+";
  const abs = Math.abs(minutes);
  return `UTC${sign}${two(Math.floor(abs / 60))}:${two(abs % 60)}`;
}

const sameLocalMinute = (a: Date, b: Date): boolean =>
  a.getFullYear() === b.getFullYear() &&
  a.getMonth() === b.getMonth() &&
  a.getDate() === b.getDate() &&
  a.getHours() === b.getHours() &&
  a.getMinutes() === b.getMinutes();

/**
 * Whether the local wall time of `d` also names another instant (a repeated, fall-back hour). The
 * other instant is one transition's shift away; shifts are 1 h, or 30 min in a few zones.
 */
function isAmbiguous(d: Date): boolean {
  return [HOUR_MS, HOUR_MS / 2].some(
    (shift) => sameLocalMinute(d, new Date(d.getTime() - shift)) || sameLocalMinute(d, new Date(d.getTime() + shift)),
  );
}

function resolved(instant: Date): ResolvedExpiry {
  return {
    instant,
    utc: toUtcText(instant),
    local: toLocalText(instant),
    offset: utcOffsetText(instant),
    ambiguous: isAmbiguous(instant),
  };
}

/** A preset's expiry, counted from `now` (the moment of Create). */
export function presetExpiry(preset: ExpiryPreset, now: Date): ResolvedExpiry {
  const span = EXPIRY_PRESETS.find((p) => p.id === preset);
  if (!span) throw new Error(`unknown expiry preset: ${String(preset)}`);
  return resolved(new Date(now.getTime() + span.ms));
}

/** `YYYY-MM-DD HH:MM` (what `composeExpiryChoice` builds from the date field and the hour and minute selects; a `T` for the space is accepted), optionally `:00`. */
const LOCAL_DATETIME = /^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})(?::(00))?$/;

/** A custom local date and time, strictly parsed, built from components, and round-trip checked. */
export function customExpiry(value: string): ExpiryResult {
  const m = LOCAL_DATETIME.exec(value.trim());
  if (!m) return { ok: false, error: `"${value}" is not a date and time of the form YYYY-MM-DD HH:MM` };
  const [y, mo, d, h, min] = m.slice(1, 6).map(Number);
  if (mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || min > 59) {
    return { ok: false, error: `${value} is not a valid date and time` };
  }
  const instant = new Date(y, mo - 1, d, h, min);
  if (Number.isNaN(instant.getTime())) return { ok: false, error: `${value} is not a valid date and time` };
  const dateBack = instant.getFullYear() === y && instant.getMonth() === mo - 1 && instant.getDate() === d;
  if (!dateBack) return { ok: false, error: `${value} is not a calendar date` };
  if (instant.getHours() !== h || instant.getMinutes() !== min) {
    return { ok: false, error: `${two(h)}:${two(min)} on ${y}-${two(mo)}-${two(d)} does not exist — DST` };
  }
  return { ok: true, ...resolved(instant) };
}

/** The editor's choice at the moment of Create (`now`). */
export function resolveExpiry(choice: ExpiryChoice, now: Date): ExpiryResult {
  return choice.kind === "preset" ? { ok: true, ...presetExpiry(choice.preset, now) } : customExpiry(choice.value);
}

/** The local time with its offset, and a note when it falls in the repeated hour. */
export function expiryShownText(e: ResolvedExpiry): string {
  const base = `${e.local} (${e.offset})`;
  return e.ambiguous ? `${base} — this hour occurs twice; this is the ${e.offset} one` : base;
}

/** The preset's line in the form: "8 h from Create — about 2026-11-21 22:00 (UTC+02:00)". */
export function presetHint(preset: ExpiryPreset, now: Date): string {
  const p = EXPIRY_PRESETS.find((x) => x.id === preset);
  return `${p?.label ?? preset} from Create — about ${expiryShownText(presetExpiry(preset, now))}`;
}
