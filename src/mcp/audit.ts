/**
 * The audit view's pure display helpers (ADR-027 M4 amendment item 4). The rows, their order, the
 * page bound, the cut of `args` and the reading of `reasons` are Rust's (`mcp_list_audit`,
 * `mcp_audit_row`); this module only turns a row into the table's cells and keeps "Load older"
 * honest.
 */
import { formatLaptopUtc } from "../format";
import type { AuditPage, AuditReasons, AuditRowView } from "../types";

/** The page size the view asks for (Rust's default; it clamps to 1–200 itself). */
export const AUDIT_PAGE_SIZE = 50;

/** The row's time in the laptop's local time. */
export function auditTime(row: AuditRowView): string {
  return formatLaptopUtc(row.at);
}

/** The disposition column: Rust's word, or "outcome" for a row that records no disposition. */
export function dispositionLabel(row: AuditRowView): string {
  return row.disposition ?? "outcome";
}

/** The reasons cell: each `row: message`, or "reasons unreadable" with the raw text. */
export function reasonsText(reasons: AuditReasons): string {
  if (reasons.kind === "unreadable") {
    return `reasons unreadable: ${reasons.raw}${reasons.raw_clipped ? "…" : ""}`;
  }
  return reasons.list.length === 0 ? "—" : reasons.list.map((r) => `${r.row}: ${r.message}`).join("; ");
}

/** The first reason alone (the collapsed cell); the whole list is `reasonsText`. */
export function firstReasonText(reasons: AuditReasons): string {
  if (reasons.kind === "unreadable") return "reasons unreadable";
  const [first] = reasons.list;
  if (!first) return "—";
  const more = reasons.list.length > 1 ? ` (+${reasons.list.length - 1})` : "";
  return `${first.row}: ${first.message}${more}`;
}

/** The args cell. A cut one says so, and that the whole text is one `mcp_audit_row` away. */
export function argsCell(row: AuditRowView): { text: string; clipped: boolean; note: string | null } {
  if (!row.args_clipped) return { text: row.args, clipped: false, note: null };
  const shown = [...row.args].length;
  return {
    text: `${row.args}…`,
    clipped: true,
    note: `first ${shown.toLocaleString("en-US")} of ${row.args_chars.toLocaleString("en-US")} characters`,
  };
}

/**
 * "Load older": the older page appended below the rows shown. Keyset paging never repeats a row; a
 * row that is somehow already shown is not shown twice, and is counted rather than dropped silently.
 */
export function appendOlder(
  shown: readonly AuditRowView[],
  page: AuditPage,
): { rows: AuditRowView[]; repeated: number; nextBeforeId: number | null } {
  const ids = new Set(shown.map((r) => r.id));
  const fresh = page.rows.filter((r) => !ids.has(r.id));
  return { rows: [...shown, ...fresh], repeated: page.rows.length - fresh.length, nextBeforeId: page.next_before_id };
}
