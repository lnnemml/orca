//! The manual-open CHANNEL — a handler (set by `ManualDrawer`) that the editor's selection
//! panel calls to open a section in the drawer. Deliberately decoupled from Monaco: it was
//! the one part of the old hover that survives the trigger change (hover → selection)
//! unchanged, so it kept its own module (was `orca-hover.ts`, whose hover provider and
//! markdown `command:` link — the link that silently failed to open — are gone in 4.13).

import { orcaMapVersion, type SectionDescriptor } from "../manual/keyword-lookup";

/** React (`ManualDrawer`) sets this so the selection panel's "Open" button can open the
 *  section drawer. Direct function call — NOT a Monaco markdown command (that failed
 *  silently and was masked by a try/catch; the whole reason the trigger changed). */
let openHandler: ((d: SectionDescriptor) => void) | null = null;

export function setManualOpenHandler(fn: ((d: SectionDescriptor) => void) | null) {
  openHandler = fn;
}

/** Open a manual section in the drawer (no-op if nothing has registered a handler yet). */
export function openManualSection(d: SectionDescriptor) {
  openHandler?.(d);
}

/** The map's ORCA version, passed to `resolve_manual_section` so a stale map is caught. */
export { orcaMapVersion };
