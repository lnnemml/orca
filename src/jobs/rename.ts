/**
 * The frontend UX **echo** of the Rust `rename_job` guard — the SAME contract, not a second,
 * divergent one. Rust does `title.trim()` then refuses an empty result; this does the identical
 * `String.prototype.trim()` (leading/trailing whitespace only — **no** internal-whitespace
 * collapsing, matching Rust's `str::trim`) then returns `null` when empty.
 *
 * So a non-null result here is exactly what Rust would STORE, and a `null` result is exactly what
 * Rust would REFUSE — the two can never disagree ("frontend showed OK, Rust refused", or the
 * reverse). This drives the disabled state / commit-skip so the user never fires a doomed rename;
 * the Rust command stays authoritative.
 */
export function sanitizeRenameInput(raw: string): string | null {
  const trimmed = raw.trim();
  return trimmed.length === 0 ? null : trimmed;
}
