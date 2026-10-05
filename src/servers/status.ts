/**
 * Pure display mapping for server profiles and connection-test reports (ADR-024 n). Every
 * decision here was made in Rust — whether the profile is a run target, and why not; whether a
 * test passed — this module only turns those values into words. It never re-derives a rule.
 */
import type {
  ConnTestCheck,
  ConnTestReport,
  ConnTestWarning,
  ServerProfile,
} from "../types";

export type Tone = "ok" | "warn" | "err" | "muted";

export interface StatusLine {
  label: string;
  tone: Tone;
}

export interface ProfileStatus {
  /** The headline: whether submits may target this profile. Driven ONLY by `run_target`. */
  runTarget: StatusLine;
  /** The secondary line: when the last full-pass connection test stamped the profile. */
  verification: StatusLine;
}

/**
 * The headline never says "verified": a verified profile without a core mask (or with a mask
 * outside the measured cores) is still not a run target, and the UI says so with Rust's reason.
 */
export function profileStatus(p: ServerProfile): ProfileStatus {
  const runTarget: StatusLine = p.run_target.is_run_target
    ? { label: "Run target", tone: "ok" }
    : {
        label: `Not a run target: ${p.run_target.reason ?? "no reason reported"}`,
        tone: "warn",
      };
  const verification: StatusLine = p.verified_at
    ? { label: `Connection test passed ${p.verified_at} UTC`, tone: "muted" }
    : { label: "Not verified", tone: "muted" };
  return { runTarget, verification };
}

const CHECK_LABELS: Record<ConnTestCheck, string> = {
  orca: "ORCA",
  cores: "CPU cores (nproc)",
  core_mask: "Core mask within the cores",
  kill_user_processes: "KillUserProcesses = false (logind)",
  root: "Remote root (created, canonical, ext4)",
};

export function checkLabel(check: ConnTestCheck): string {
  return CHECK_LABELS[check];
}

export function warningText(w: ConnTestWarning): string {
  switch (w.kind) {
    case "sudo_group":
      return "The profile user is in the sudo group; the server account should have no sudo.";
    case "groups_undetermined":
      return `Group membership is undetermined (${w.detail}).`;
    case "open_mpi_not_reported":
      return `OpenMPI reported no version (${w.detail}); none is recorded.`;
  }
}

/** One line saying what the test concluded and what it wrote. */
export function outcomeSummary(report: ConnTestReport): StatusLine {
  switch (report.outcome) {
    case "verified":
      return { label: "Connection test passed; the profile is verified.", tone: "ok" };
    case "conflict":
      return {
        label: `The checks passed, but the profile changed during the test, so it was not verified: ${report.reason}`,
        tone: "warn",
      };
    case "not_passed":
      return {
        label: "Connection test did not pass; the verification was cleared.",
        tone: "err",
      };
    case "failed":
      return {
        label: `The test could not run; the verification was cleared. ${report.reason}`,
        tone: "err",
      };
  }
}
