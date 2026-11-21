/**
 * The grant card's and the grant editor's pure logic (ADR-027 decision 5; M4 amendment item 5). Every
 * decision about a grant is Rust's (`mcp_create_grant` refuses a bad level, a count below 1, an expiry
 * not after now); this module turns a `GrantView` into words, and runs Create and Revoke around the
 * injected commands.
 *
 * **Replacing a grant.** Creating a grant revokes the active one (decision 5). Whether to confirm
 * "this replaces the active grant" is decided from a **fresh** `mcp_get_grant` read at the click,
 * never from the card's last poll: a grant made, revoked or expired since then must not slip past the
 * confirm (or raise a stale one).
 */
import { formatLaptopUtc } from "../format";
import type { GrantView } from "../types";

/** `max_jobs` and `max_in_flight` before Anton changes them. */
export const DEFAULT_MAX_JOBS = 5;
export const DEFAULT_MAX_IN_FLIGHT = 1;

export function grantStateLabel(g: GrantView): string {
  switch (g.state) {
    case "active":
      return "Active";
    case "expired":
      return "Expired";
    case "revoked":
      return "Revoked";
  }
}

/** "2 of 5 jobs charged". */
export function chargedLabel(g: GrantView): string {
  return `${g.charged_jobs} of ${g.max_jobs} jobs charged`;
}

/** "1 of 2 in flight (all grants)": decision 5 counts every grant's non-terminal jobs. */
export function inFlightLabel(g: GrantView): string {
  return `${g.in_flight} of ${g.max_in_flight} in flight (all grants)`;
}

/** The expiry (or revocation) in the laptop's local time. */
export function grantTimeLabel(g: GrantView): string {
  if (g.revoked_at !== null) return `revoked ${formatLaptopUtc(g.revoked_at)}`;
  return `${g.state === "expired" ? "expired" : "expires"} ${formatLaptopUtc(g.expires_at)}`;
}

/** Targets as shown: `local` stays, a server profile id is named by `profileName` when known. */
export function targetsLabel(g: GrantView, profileName: (id: string) => string | undefined): string {
  return g.targets.map((t) => (t === "local" ? "local" : (profileName(t) ?? t))).join(", ");
}

/** Whether Create must confirm the replacement: decided from the fresh read alone. */
export function replacesActive(fresh: GrantView | null): fresh is GrantView {
  return fresh?.state === "active";
}

/** The replacement's confirm. `changed`: the fresh read differs from what the card showed. */
export function replaceConfirmText(active: GrantView, changed: boolean): string {
  const note = changed ? " (The grant changed since this screen last read it.)" : "";
  return (
    `This replaces the active grant (${chargedLabel(active)}, ${grantTimeLabel(active)}): ` +
    `it is revoked, and Claude's running jobs are not cancelled.${note}`
  );
}

export function revokeConfirmText(g: GrantView): string {
  return `Revoke this grant (${chargedLabel(g)})? Claude can start no new job under it; running jobs are not cancelled.`;
}

/** What `mcp_create_grant` takes (camelCase at the `invoke` boundary). */
export interface CreateGrantArgs {
  targets: string[];
  levels: string[];
  maxJobs: number;
  maxInFlight: number;
  /** UTC `YYYY-MM-DD HH:MM:SS` (`expiry.ts`). */
  expiresAt: string;
}

/** The commands and the dialog Create runs through, injected. */
export interface CreateGrantDeps {
  /** `mcp_get_grant`: read at the click. */
  readGrant: () => Promise<GrantView | null>;
  confirm: (text: string) => Promise<boolean>;
  /** `mcp_create_grant`. */
  create: (args: CreateGrantArgs) => Promise<GrantView>;
}

export type CreateGrantOutcome =
  | { kind: "created"; grant: GrantView }
  /** Anton declined the replacement; `fresh` is what the read found, for the card. */
  | { kind: "declined"; fresh: GrantView | null };

/** Two views of a grant name the same state: same grant, same state, same counters. */
const sameGrant = (a: GrantView | null, b: GrantView | null): boolean =>
  a === null || b === null
    ? a === b
    : a.id === b.id && a.state === b.state && a.charged_jobs === b.charged_jobs && a.expires_at === b.expires_at;

/**
 * Create: read the grant **now**, confirm the replacement only if that read shows an active grant,
 * then create. `shown` is what the card displayed; it decides nothing, and only adds a note to the
 * confirm when the fresh read differs from it.
 */
export async function createGrantFlow(
  deps: CreateGrantDeps,
  args: CreateGrantArgs,
  shown: GrantView | null,
): Promise<CreateGrantOutcome> {
  const fresh = await deps.readGrant();
  if (replacesActive(fresh)) {
    const ok = await deps.confirm(replaceConfirmText(fresh, !sameGrant(shown, fresh)));
    if (!ok) return { kind: "declined", fresh };
  }
  return { kind: "created", grant: await deps.create(args) };
}
