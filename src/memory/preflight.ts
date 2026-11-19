//! The memory and disk preflight's pure view logic (ADR-024 o26.7, o26.13.1, o26.13.6, o26.14.3).
//! Every number comes from Rust (`memory_estimate`); this file only words it. Warn, never block:
//! nothing here can disable Create or Create & Run.

import type { MemoryCheck, MemoryEstimateReply, ServerProfile, SubmitResponse } from "../types";

/** The builder's remote default `%maxcore` (o26.5: the remote default stays 2000). */
export const REMOTE_DEFAULT_MAXCORE_MIB = 2000;

/** The builder's `%maxcore` default for the selected Run On target (o26.5, o26.13.1): the local
 *  fitted value for this machine (`target` null), 2000 for a server. Applied only while the user
 *  has not touched the field (the form's job). */
export function builderMaxcoreDefault(target: string | null, localDefault: number): number {
  return target == null ? localDefault : REMOTE_DEFAULT_MAXCORE_MIB;
}

/** o26.13.6: a reply is applied only when it answers the latest request. */
export function isLatestReply(latestSeq: number, reply: Pick<MemoryEstimateReply, "seq">): boolean {
  return reply.seq === latestSeq;
}

/** The gates of New Job's Create and Create & Run. The memory reply is listed so the rule is explicit:
 *  **it never blocks** (o26.7) — only an empty buffer, a create in flight and the two input-content
 *  blocks (constraints, scan) do. */
export function createAllowed(
  gates: { hasContent: boolean; creating: boolean; constraintBlock: string | null; scanBlock: string | null },
  _memory: MemoryEstimateReply | null,
): boolean {
  return gates.hasContent && !gates.creating && !gates.constraintBlock && !gates.scanBlock;
}

const MIB_PER_GIB = 1024;

export function gib(mib: number): string {
  return `${(mib / MIB_PER_GIB).toFixed(1)} GiB`;
}

export function gibFromKib(kib: number): string {
  return gib(kib / 1024);
}

export function gibFromBytes(bytes: number): string {
  return gib(bytes / (1024 * 1024));
}

/** One verdict line of the warning row, in words. */
export interface VerdictLine {
  key: "local" | "server";
  label: string;
  /** "warns": RAM or disk over the line; "unknown": a fact is not known; "not_estimated": no U. */
  verdict: "warns" | "fits" | "unknown" | "not_estimated";
  /** U and how it was made; null when there is none or the RAM is unknown (o26.14.3). */
  estimate: string | null;
  /** The target's RAM (and its threshold), or why it is unknown. */
  ram: string;
  /** "measured at the last connection test, <date>" for a server; null for this machine. */
  measured: string | null;
  disk: string;
  /** Why there is no estimate ("cannot read the input's memory setting …"). */
  reason: string | null;
  /** P for the button "Set %maxcore to P" — only when the RAM warns and P exists. */
  proposal: number | null;
  /** The target is too small for n ranks at any useful %maxcore (no button). */
  tooSmall: string | null;
}

/** Whether a check needs to be shown: something warns, or something is not known. */
export function needsAttention(c: MemoryCheck): boolean {
  return c.warns || c.ram.state === "unknown" || c.disk.state === "unknown" || c.estimate.state === "not_estimated";
}

/** Word one target's check. */
export function verdictLine(key: "local" | "server", label: string, c: MemoryCheck): VerdictLine {
  const ramWarns = c.ram.state === "known" && c.ram.warns === true;
  const ramText =
    c.ram.state === "unknown"
      ? key === "server"
        ? c.ram.reason // Rust's "server memory unknown — re-run the connection test"
        : `this machine's memory unknown: ${c.ram.reason}`
      : `RAM ${gibFromKib(c.ram.mem_total_kib)} (warns above 0.9 × = ${gib(c.ram.threshold_mib)})`;
  // o26.14.3: no U when the RAM is unknown — an estimate with nothing to compare it to says nothing.
  const estimate =
    c.ram.state === "unknown" || c.estimate.state !== "estimated"
      ? null
      : `estimate ≈ ${gib(c.estimate.estimate_mib)} (${c.estimate.ranks} ranks × (%maxcore ${c.estimate.maxcore_mib}` +
        ` + 100) + 140 MiB${c.estimate.maxcore_from_input ? "" : ", ORCA's default %maxcore"}; from the cap, not an upper bound)`;
  const disk =
    c.disk.state === "unknown"
      ? `free disk unknown: ${c.disk.reason}`
      : `free disk ${gibFromBytes(c.disk.avail_bytes)}${c.disk.warns ? ` — below ${gibFromBytes(c.disk.min_free_bytes)}` : ""}`;
  const verdict: VerdictLine["verdict"] = c.warns
    ? "warns"
    : c.estimate.state === "not_estimated"
      ? "not_estimated"
      : c.ram.state === "unknown" || c.disk.state === "unknown"
        ? "unknown"
        : "fits";
  const ranks = c.estimate.state === "estimated" ? c.estimate.ranks : null;
  return {
    key,
    label,
    verdict,
    estimate,
    ram: ramText,
    measured: key === "server" && c.facts_measured_at ? `measured at the last connection test, ${c.facts_measured_at}` : null,
    disk,
    reason: c.estimate.state === "not_estimated" ? c.estimate.reason : null,
    proposal: ramWarns && c.proposal_mib != null ? c.proposal_mib : null,
    tooSmall:
      ramWarns && c.too_small_for_ranks && ranks != null
        ? `this target is too small for ${ranks} ranks at any useful %maxcore`
        : null,
  };
}

/** The row's lines (o26.13.1): one when the selected target is this machine ("Create" and "Create &
 *  Run" both run here); two when a server is selected — "Create (runs locally)" and "Create & Run on
 *  <server>". Empty (the row hides) when nothing warns and every fact is known. */
export function verdictLines(reply: MemoryEstimateReply | null, serverName: string | null): VerdictLine[] {
  if (!reply) return [];
  const lines = reply.server
    ? [
        verdictLine("local", "Create (runs locally)", reply.local),
        verdictLine("server", `Create & Run on ${serverName ?? reply.server.profile_id}`, reply.server),
      ]
    : [verdictLine("local", "This machine (Create and Create & Run)", reply.local)];
  const checks = reply.server ? [reply.local, reply.server] : [reply.local];
  return checks.some(needsAttention) ? lines : [];
}

/** One target's part of the collapsed summary: `<name> ≈U > threshold` for a RAM warning (the disk
 *  and unread facts after a `;`), `<target>: memory unknown` when the RAM is unknown (no U, o26.14.3),
 *  the disk or the missing estimate otherwise; null when this target needs no attention. */
function summaryPart(warnName: string, bareName: string, c: MemoryCheck): string | null {
  if (!needsAttention(c)) return null;
  const facts: string[] = [];
  // An unread RAM says so and shows no U (o26.14.3); the disk and the estimate still follow.
  if (c.ram.state === "unknown") facts.push("memory unknown");
  if (c.disk.state === "known" && c.disk.warns) {
    facts.push(`free disk ${gibFromBytes(c.disk.avail_bytes)} < ${gibFromBytes(c.disk.min_free_bytes)}`);
  } else if (c.disk.state === "unknown") {
    facts.push("disk unknown");
  }
  if (c.estimate.state === "not_estimated") facts.push("not estimated");
  if (c.ram.state === "known" && c.ram.warns === true && c.estimate.state === "estimated") {
    const head = `${warnName} ≈${gib(c.estimate.estimate_mib)} > ${gib(c.ram.threshold_mib)}`;
    return facts.length ? `${head}; ${facts.join("; ")}` : head;
  }
  return facts.length ? `${bareName}: ${facts.join("; ")}` : null;
}

/** The closed disclosure's one line (Anton's request after the 5.5 gate): `⚠ Memory: Create (local)
 *  ≈32.9 GiB > 13.7 GiB · Create & Run (uni) ≈98.5 GiB > 56.5 GiB`, a target with an unread RAM as
 *  `uni: memory unknown` (a low or unread disk after it, `uni: memory unknown; disk unknown`). The `⚠` only when a verdict warns. Null when the row hides (nothing warns,
 *  every fact known) — the same gate as {@link verdictLines}. */
export function memorySummary(reply: MemoryEstimateReply | null, serverName: string | null): string | null {
  if (!reply) return null;
  const parts = reply.server
    ? [
        summaryPart("Create (local)", "this machine", reply.local),
        summaryPart(`Create & Run (${serverName ?? reply.server.profile_id})`, serverName ?? reply.server.profile_id, reply.server),
      ]
    : [summaryPart("this machine", "this machine", reply.local)];
  const present = parts.filter((p): p is string => p != null);
  if (present.length === 0) return null;
  const warns = reply.local.warns || (reply.server?.warns ?? false);
  return `${warns ? "⚠ " : ""}Memory: ${present.join(" · ")}`;
}

/** What a submit answer shows (o26.6): the failure, the `%pal` notice and the memory notice — the
 *  last two side by side, neither overwriting the other. */
export function submitNotices(response: SubmitResponse | null): {
  failure: string | null;
  notice: string | null;
  memoryNotice: string | null;
} {
  return {
    failure: response?.failure ?? null,
    notice: response?.notice ?? null,
    memoryNotice: response?.memory_notice ?? null,
  };
}

/** A verified profile's stored host facts (schema v23) for the Settings list: " · RAM 62.7 GiB ·
 *  73.0 GiB free (measured <date>)"; a NULL fact is "unknown", never 0. */
export function hostFactsSuffix(p: Pick<ServerProfile, "mem_total_kib" | "disk_avail_bytes" | "host_facts_at">): string {
  const ram = p.mem_total_kib != null ? `RAM ${gibFromKib(p.mem_total_kib)}` : "RAM unknown";
  const disk = p.disk_avail_bytes != null ? `${gibFromBytes(p.disk_avail_bytes)} free` : "free disk unknown";
  return ` · ${ram} · ${disk}${p.host_facts_at ? ` (measured ${p.host_facts_at})` : " — re-run the connection test"}`;
}
