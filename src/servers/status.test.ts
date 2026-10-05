import { describe, it, expect } from "vitest";

import type { ConnTestReport } from "../types";
import { checkLabel, outcomeSummary, profileStatus, warningText } from "./status";
import { makeProfile } from "./test-fixtures";

const VERIFIED = {
  orca_version: "6.1.1",
  openmpi_version: "4.1.6",
  core_count: 48,
  verified_at: "2026-10-03 14:20:03",
};

describe("profileStatus — the headline is the run-target status from Rust", () => {
  it("an unverified profile is not a run target, with Rust's reason", () => {
    const s = profileStatus(makeProfile());
    expect(s.runTarget).toEqual({
      label: "Not a run target: the profile has not passed the connection test",
      tone: "warn",
    });
    expect(s.verification.label).toBe("Not verified");
  });

  it("a run target says so, and when it was verified (UTC)", () => {
    const s = profileStatus(
      makeProfile({ ...VERIFIED, run_target: { is_run_target: true, reason: null } }),
    );
    expect(s.runTarget).toEqual({ label: "Run target", tone: "ok" });
    expect(s.verification.label).toBe("Connection test passed 2026-10-03 14:20:03 UTC");
  });

  // Risk (4): a verified profile that is NOT a run target must not read as ready. NEGATIVE
  // CONTROL: map the headline from `verified_at` instead of `run_target` and this goes red.
  it("a verified profile without a mask is still not a run target", () => {
    const s = profileStatus(
      makeProfile({
        ...VERIFIED,
        core_mask: null,
        run_target: { is_run_target: false, reason: "the profile has no core mask" },
      }),
    );
    expect(s.runTarget).toEqual({
      label: "Not a run target: the profile has no core mask",
      tone: "warn",
    });
    expect(s.runTarget.label).not.toBe("Run target");
  });

  it("a missing reason is said, not invented", () => {
    const s = profileStatus(makeProfile({ run_target: { is_run_target: false, reason: null } }));
    expect(s.runTarget.label).toBe("Not a run target: no reason reported");
  });
});

describe("report wording", () => {
  const profile = makeProfile();
  it("each outcome has its own summary and tone", () => {
    const base = { profile, elapsed_ms: 1114 };
    const facts = { orca_version: "6.1.1", openmpi_version: "4.1.6", core_count: 48 };
    const cases: [ConnTestReport, string, string][] = [
      [{ ...base, outcome: "verified", checks: [], facts, warnings: [] }, "passed", "ok"],
      [
        { ...base, outcome: "conflict", reason: "changed", checks: [], facts, warnings: [] },
        "not verified: changed",
        "warn",
      ],
      [{ ...base, outcome: "not_passed", checks: [], warnings: [] }, "did not pass", "err"],
      [{ ...base, outcome: "failed", reason: "ssh exit 255" }, "ssh exit 255", "err"],
    ];
    for (const [report, text, tone] of cases) {
      const s = outcomeSummary(report);
      expect(s.label).toContain(text);
      expect(s.tone).toBe(tone);
    }
  });

  it("labels every check and warning", () => {
    expect(checkLabel("kill_user_processes")).toContain("KillUserProcesses");
    expect(warningText({ kind: "sudo_group" })).toContain("sudo");
    expect(warningText({ kind: "open_mpi_not_reported", detail: "rc 127" })).toContain("rc 127");
  });
});
