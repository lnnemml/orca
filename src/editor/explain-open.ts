//! The explain CHANNEL — the T1 "Explain with Claude" action of the selection panel
//! (ADR-014 T1, ADR-015 (3)). Mirrors `manual-open.ts`: the selection panel calls
//! `requestExplain`, and `ManualDrawer` (which owns the drawer + the `resolve_manual_section`
//! bridge) registers the handler. Decoupled from Monaco so the wire is testable without jsdom.
//!
//! Tier-zero (ADR-014): this path produces ADVICE shown in the drawer. It exposes **no** way to
//! write into the editor — there is no insert handler here, by construction, not by prompt.

import type { KeySource } from "../types";
import type { SectionDescriptor } from "../manual/keyword-lookup";

/** What the panel hands the drawer. The drawer resolves `descriptor` → the section body and
 *  calls `explain_selection(word, line, section)` — exactly the three ADR-015 (3) fields. There
 *  is deliberately NO geometry / file field here or downstream. */
export interface ExplainRequest {
  word: string;
  line: string;
  descriptor: SectionDescriptor;
}

let explainHandler: ((req: ExplainRequest) => void) | null = null;

export function setExplainHandler(fn: ((req: ExplainRequest) => void) | null) {
  explainHandler = fn;
}

/** Ask the drawer to explain a selection (no-op until `ManualDrawer` has registered). */
export function requestExplain(req: ExplainRequest) {
  explainHandler?.(req);
}

/**
 * The appearance condition for the Explain action (TASK 3), as a pure predicate so it is
 * testable without the DOM: a usable key (`stored-in-keyring` or `from-environment`) AND a
 * resolved section to ground the answer. No key → the button is ABSENT, not an error on click.
 */
export function canExplain(
  keyState: KeySource["state"] | undefined,
  hasSection: boolean,
): boolean {
  const keyed = keyState === "stored-in-keyring" || keyState === "from-environment";
  return keyed && hasSection;
}
