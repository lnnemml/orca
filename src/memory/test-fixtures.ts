/** Test-only fixtures for the memory preflight (imported by the vitest files only). The numbers are
 *  Rust's for the recorded hosts (wiki/orca/memory.md): the laptop 15 988 408 KiB, uni 65 795 232 KiB,
 *  uni's root 78 360 924 160 B free. */
import type { MemoryCheck, MemoryEstimateReply } from "../types";

/** A laptop check: 8 ranks; `maxcore` in the input or ORCA's 4096. */
export function localCheck(over: Partial<MemoryCheck> = {}, maxcore: number | null = 1600): MemoryCheck {
  const m = maxcore ?? 4096;
  const u = 8 * (m + 100) + 140;
  return {
    estimate: { state: "estimated", ranks: 8, maxcore_mib: m, maxcore_from_input: maxcore != null, estimate_mib: u },
    ram: { state: "known", mem_total_kib: 15988408, threshold_mib: 14052, warns: u > 14052 },
    disk: { state: "known", avail_bytes: 101075353600, min_free_bytes: 10737418240, warns: false },
    proposal_mib: 1600,
    too_small_for_ranks: false,
    facts_measured_at: null,
    warns: u > 14052,
    ...over,
  };
}

/** A uni check: 24 ranks on the 0-23 mask. */
export function uniCheck(over: Partial<MemoryCheck> = {}, maxcore: number | null = 2000): MemoryCheck {
  const m = maxcore ?? 4096;
  const u = 24 * (m + 100) + 140;
  return {
    estimate: { state: "estimated", ranks: 24, maxcore_mib: m, maxcore_from_input: maxcore != null, estimate_mib: u },
    ram: { state: "known", mem_total_kib: 65795232, threshold_mib: 57827, warns: u > 57827 },
    disk: { state: "known", avail_bytes: 78360924160, min_free_bytes: 10737418240, warns: false },
    proposal_mib: 2300,
    too_small_for_ranks: false,
    facts_measured_at: "2026-11-18 10:00:00",
    warns: u > 57827,
    ...over,
  };
}

/** uni never tested since v23: Rust's NULL facts. */
export function uniUnknown(maxcore: number | null = null): MemoryCheck {
  return uniCheck(
    {
      ram: { state: "unknown", reason: "server memory unknown — re-run the connection test" },
      disk: { state: "unknown", reason: "server free disk unknown — re-run the connection test" },
      proposal_mib: null,
      facts_measured_at: null,
      warns: false,
    },
    maxcore,
  );
}

export function reply(seq: number, local: MemoryCheck, server: MemoryCheck | null = null): MemoryEstimateReply {
  return { seq, local, server: server ? { ...server, profile_id: "p1" } : null };
}
