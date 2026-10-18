/**
 * Where `cancelling` (ADR-024 o22.5, o22.14.8) must appear in the screens, as pure values so a
 * test can pin them: the Jobs screen's status chips and the status bar's active count.
 */
import type { Job, JobStatus } from "../types";

/** The Jobs screen's status filter chips, in state-machine order. */
export const STATUS_CHIPS = [
  "draft",
  "queued",
  "running",
  "cancelling",
  "completed",
  "parsed",
  "failed",
  "cancelled",
] as const satisfies readonly JobStatus[];

// Compile-time: a status added to `JobStatus` without a chip is a type error here, not a filter
// that silently cannot select it.
type MissingChip = Exclude<JobStatus, (typeof STATUS_CHIPS)[number]>;
const EVERY_STATUS_HAS_A_CHIP: [MissingChip] extends [never] ? true : never = true;
void EVERY_STATUS_HAS_A_CHIP;

/**
 * The status bar's counts. A `cancelling` job still runs on the server (some of its processes are
 * alive, o22.5), so it counts as running — the bar never says "idle" while the server is busy.
 */
export function queueCounts(jobs: readonly Pick<Job, "status">[]): { running: number; queued: number } {
  return {
    running: jobs.filter((j) => j.status === "running" || j.status === "cancelling").length,
    queued: jobs.filter((j) => j.status === "queued").length,
  };
}
