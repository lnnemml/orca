import { describe, expect, it } from "vitest";

import {
  builderMaxcoreDefault,
  createAllowed,
  hostFactsSuffix,
  isLatestReply,
  memorySummary,
  submitNotices,
  verdictLines,
} from "./preflight";
import { localCheck, reply, uniCheck, uniUnknown } from "./test-fixtures";
import type { SubmitResponse } from "../types";

describe("verdictLines (ADR-024 o26.7, o26.13.1, o26.14.3)", () => {
  // CONTROL: show U when the RAM is unknown (drop the `c.ram.state === "unknown"` guard on the
  // estimate) → red. NULL is never read as 0 either: the RAM text is Rust's "unknown" reason.
  it("NULL server facts read 'server memory unknown', with no U and no button", () => {
    const lines = verdictLines(reply(1, localCheck(), uniUnknown()), "uni");
    const server = lines.find((l) => l.key === "server")!;
    expect(server.ram).toBe("server memory unknown — re-run the connection test");
    expect(server.estimate).toBeNull();
    expect(server.proposal).toBeNull();
    expect(server.verdict).toBe("unknown");
    expect(server.disk).toBe("free disk unknown: server free disk unknown — re-run the connection test");
    expect(JSON.stringify(server)).not.toMatch(/\b0\.0 GiB/);
  });

  // CONTROL: always one line (drop the server line) or always two → red.
  it("two verdicts with a server selected, one for this machine", () => {
    const two = verdictLines(reply(1, localCheck({}, null), uniCheck({}, null)), "uni");
    expect(two.map((l) => l.label)).toEqual(["Create (runs locally)", "Create & Run on uni"]);
    const one = verdictLines(reply(1, localCheck({}, null)), null);
    expect(one.map((l) => l.label)).toEqual(["This machine (Create and Create & Run)"]);
  });

  it("hides the row when nothing warns and every fact is known", () => {
    expect(verdictLines(reply(1, localCheck()), null)).toEqual([]);
    expect(verdictLines(reply(1, localCheck(), uniCheck()), "uni")).toEqual([]);
    expect(verdictLines(null, null)).toEqual([]);
  });

  it("words a warning with U, the RAM, the date, the disk and P", () => {
    const [local, server] = verdictLines(reply(1, localCheck({}, null), uniCheck({}, null)), "uni");
    expect(local.verdict).toBe("warns");
    expect(local.estimate).toContain("≈ 32.9 GiB (8 ranks × (%maxcore 4096 + 100) + 140 MiB, ORCA's default %maxcore");
    expect(local.ram).toBe("RAM 15.2 GiB (warns above 0.9 × = 13.7 GiB)");
    expect(local.measured).toBeNull();
    expect(local.proposal).toBe(1600);
    expect(server.estimate).toContain("≈ 98.5 GiB (24 ranks");
    expect(server.measured).toBe("measured at the last connection test, 2026-11-18 10:00:00");
    expect(server.disk).toBe("free disk 73.0 GiB");
    expect(server.proposal).toBe(2300);
  });

  it("says why there is no estimate, and when the target is too small, with no button", () => {
    const unread = localCheck({ estimate: { state: "not_estimated", reason: "cannot read the input's memory setting: 2 %maxcore lines" }, warns: false });
    const [line] = verdictLines(reply(1, unread), null);
    expect(line.verdict).toBe("not_estimated");
    expect(line.reason).toBe("cannot read the input's memory setting: 2 %maxcore lines");
    expect(line.estimate).toBeNull();
    const tiny = localCheck({ proposal_mib: null, too_small_for_ranks: true }, null);
    const [small] = verdictLines(reply(1, tiny), null);
    expect(small.tooSmall).toBe("this target is too small for 8 ranks at any useful %maxcore");
    expect(small.proposal).toBeNull();
  });
});

describe("the button's gate (5.5 F3)", () => {
  // CONTROL (`run_p5fu_controls.sh`, `preflight-button-without-ram-warning`): `proposal` offered
  // whenever P exists, whatever warns (`ramWarns &&` dropped) → a disk-only warning offers it, red.
  it("offers P only when the RAM warns — never for a disk-only or an unread-RAM verdict", () => {
    const diskOnly = localCheck({
      disk: { state: "known", avail_bytes: 5 * 1024 ** 3, min_free_bytes: 10 * 1024 ** 3, warns: true },
      warns: true,
    });
    expect(diskOnly.ram.state === "known" && diskOnly.ram.warns).toBe(false);
    expect(diskOnly.proposal_mib).toBe(1600); // Rust sent a P; the view still must not offer it
    const [line] = verdictLines(reply(1, diskOnly), null);
    expect(line.verdict).toBe("warns");
    expect(line.proposal).toBeNull();
    // An unread RAM never offers a button, even if a P were sent.
    expect(verdictLines(reply(1, localCheck(), { ...uniUnknown(), proposal_mib: 2300 }), "uni")[1].proposal).toBeNull();
    // And the RAM warning does offer it.
    expect(verdictLines(reply(1, localCheck({}, null)), null)[0].proposal).toBe(1600);
  });
});

describe("memorySummary (the collapsed disclosure's line)", () => {
  // CONTROL: the threshold dropped from the warning part → red.
  it("names each warning target with U and its threshold, and hides with the row", () => {
    expect(memorySummary(reply(1, localCheck({}, null), uniCheck({}, null)), "uni")).toBe(
      "⚠ Memory: Create (local) ≈32.9 GiB > 13.7 GiB · Create & Run (uni) ≈98.5 GiB > 56.5 GiB",
    );
    expect(memorySummary(reply(1, localCheck({}, null)), null)).toBe("⚠ Memory: this machine ≈32.9 GiB > 13.7 GiB");
    expect(memorySummary(reply(1, localCheck(), uniCheck()), "uni")).toBeNull();
    expect(memorySummary(null, null)).toBeNull();
    // Hidden exactly when the row is.
    for (const r of [reply(1, localCheck()), reply(1, localCheck({}, null), uniUnknown()), reply(1, localCheck(), uniUnknown())]) {
      expect(memorySummary(r, "uni") == null).toBe(verdictLines(r, "uni").length === 0);
    }
  });

  it("says an unread RAM with no U, and a low disk or a missing estimate in words", () => {
    // CONTROLS (`run_p5fu_controls.sh`): an early return on the unread RAM (`row-summary-unknown-ram-drops-disk`)
    // or the words dropped (`row-summary-unknown-ram-silent`) → red.
    expect(memorySummary(reply(1, localCheck(), uniUnknown()), "uni")).toBe("Memory: uni: memory unknown; disk unknown");
    // The unread RAM with a read, low disk: the disk still follows, and no U.
    const unreadRamLowDisk = uniCheck({
      ram: { state: "unknown", reason: "server memory unknown — re-run the connection test" },
      disk: { state: "known", avail_bytes: 5 * 1024 ** 3, min_free_bytes: 10 * 1024 ** 3, warns: true },
      warns: true,
      proposal_mib: null,
    });
    expect(memorySummary(reply(1, localCheck(), unreadRamLowDisk), "uni")).toBe(
      "⚠ Memory: uni: memory unknown; free disk 5.0 GiB < 10.0 GiB",
    );
    // The unread RAM with the disk read and fine: the RAM alone.
    const unreadRamDiskFine = uniCheck({
      ram: { state: "unknown", reason: "server memory unknown — re-run the connection test" },
      warns: false,
      proposal_mib: null,
    });
    expect(memorySummary(reply(1, localCheck(), unreadRamDiskFine), "uni")).toBe("Memory: uni: memory unknown");
    const lowDisk = localCheck({
      disk: { state: "known", avail_bytes: 5 * 1024 ** 3, min_free_bytes: 10 * 1024 ** 3, warns: true },
      warns: true,
    });
    expect(memorySummary(reply(1, lowDisk), null)).toBe("⚠ Memory: this machine: free disk 5.0 GiB < 10.0 GiB");
    const unread = localCheck({ estimate: { state: "not_estimated", reason: "x" }, warns: false });
    expect(memorySummary(reply(1, unread), null)).toBe("Memory: this machine: not estimated");
    const both = localCheck({
      disk: { state: "known", avail_bytes: 5 * 1024 ** 3, min_free_bytes: 10 * 1024 ** 3, warns: true },
    }, null);
    expect(memorySummary(reply(1, both), null)).toBe("⚠ Memory: this machine ≈32.9 GiB > 13.7 GiB; free disk 5.0 GiB < 10.0 GiB");
  });
});

describe("the builder default (o26.5, o26.13.1)", () => {
  // CONTROL: 2000 for Local too → red.
  it("is the local fitted value for this machine and 2000 for a server", () => {
    expect(builderMaxcoreDefault(null, 1600)).toBe(1600);
    expect(builderMaxcoreDefault("p1", 1600)).toBe(2000);
  });
});

describe("sequence numbers (o26.13.6)", () => {
  it("only the latest request's reply is applied", () => {
    expect(isLatestReply(3, { seq: 3 })).toBe(true);
    expect(isLatestReply(3, { seq: 2 })).toBe(false);
  });
});

describe("Create is never disabled by the preflight (o26.7)", () => {
  // CONTROL: `&& !(memory?.local.warns || memory?.server?.warns)` in createAllowed → red.
  it("a warning reply leaves Create and Create & Run enabled", () => {
    const gates = { hasContent: true, creating: false, constraintBlock: null, scanBlock: null };
    const warning = reply(1, localCheck({}, null), uniCheck({}, null));
    expect(warning.local.warns && warning.server!.warns).toBe(true);
    expect(createAllowed(gates, warning)).toBe(true);
    expect(createAllowed(gates, reply(2, localCheck(), uniUnknown()))).toBe(true);
    expect(createAllowed({ ...gates, scanBlock: "no Opt" }, null)).toBe(false);
  });
});

describe("submit notices (o26.6)", () => {
  // CONTROL: the memory notice folded into `notice` (or dropped) → red.
  it("keeps the %pal notice and the memory notice apart", () => {
    const response = {
      outcome: { outcome: "enqueued", tsp_id: 3 },
      pal: { input_nprocs: null, nprocs: 24, mask_cpus: 24, rewritten: true },
      notice: "[OrcaStudio] %pal nprocs aligned to 24",
      memory_notice: "[OrcaStudio] memory: the job's estimate is 98.5 GiB",
      failure: null,
    } as SubmitResponse;
    expect(submitNotices(response)).toEqual({
      failure: null,
      notice: "[OrcaStudio] %pal nprocs aligned to 24",
      memoryNotice: "[OrcaStudio] memory: the job's estimate is 98.5 GiB",
    });
    expect(submitNotices(null)).toEqual({ failure: null, notice: null, memoryNotice: null });
  });
});

describe("Settings: a profile's stored host facts", () => {
  it("words them, and a NULL fact as unknown", () => {
    expect(hostFactsSuffix({ mem_total_kib: 65795232, disk_avail_bytes: 78360924160, host_facts_at: "2026-11-18 10:00:00" })).toBe(
      " · RAM 62.7 GiB · 73.0 GiB free (measured 2026-11-18 10:00:00)",
    );
    expect(hostFactsSuffix({ mem_total_kib: null, disk_avail_bytes: null, host_facts_at: null })).toBe(
      " · RAM unknown · free disk unknown — re-run the connection test",
    );
  });
});
