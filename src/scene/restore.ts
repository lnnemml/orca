/**
 * Reconcile a persisted `scene_json` snapshot against a job's `input_content`
 * when opening/iterating a job (ADR-008 #5, made concrete in 2.5.0d-3).
 *
 * The input text is **authoritative for geometry**; the snapshot only *annotates*
 * it with fragment boundaries. So a snapshot is honoured only when it describes
 * the same atoms as the input — verified with the very same `xyzMatchesScene`
 * primitive that guards the live Scene↔Monaco sync (no second comparison exists,
 * on purpose). A snapshot that is missing, malformed, wrong-version, or stale
 * falls back to a single fragment parsed straight from the input.
 *
 * `snapshotRejected` distinguishes the two "single fragment" outcomes: a plain
 * pre-v4 / no-snapshot job (`false`, not an anomaly) versus a snapshot that was
 * present but discarded because it didn't match the input (`true`, worth a note).
 */

import {
  atomCount,
  deserializeScene,
  mergeToAtomLines,
  sceneFromOrcaInput,
  serializeScene,
  xyzMatchesScene,
} from "./scene";
import {
  append,
  current,
  deserializeLog,
  emptyLog,
  type Op,
  type SceneLog,
} from "./oplog";
import type { Scene } from "./types";

export function restoreScene(
  inputContent: string,
  sceneJson: string | null,
): { scene: Scene | null; snapshotRejected: boolean } {
  // 1. Geometry from the text — the authority. No coordinate block → no scene.
  const fromText = sceneFromOrcaInput(inputContent);
  if (!fromText) return { scene: null, snapshotRejected: false };

  // 2. No snapshot at all (pre-v4 job, or a job saved without a scene): the
  //    single fragment parsed from the text is exactly right, not an anomaly.
  if (!sceneJson) return { scene: fromText, snapshotRejected: false };

  // 3. A snapshot that won't deserialize (corrupt / a future version) is discarded.
  const snapshot = deserializeScene(sceneJson);
  if (!snapshot) return { scene: fromText, snapshotRejected: true };

  // 4. A valid snapshot is honoured only if it annotates the *same* geometry the
  //    input holds — same primitive as the Monaco sync (parsed floats, tol 1e-6).
  if (xyzMatchesScene(snapshot, mergeToAtomLines(fromText))) {
    return { scene: snapshot, snapshotRejected: false };
  }
  return { scene: fromText, snapshotRejected: true };
}

/**
 * Why a persisted operation log was NOT honoured on restore:
 *  - `legacy` — the job predates `scene_log_json` (NULL). Not an anomaly; a fresh
 *    log is seeded so history "begins here", honestly.
 *  - `log-unreadable` — the column was present but didn't deserialize (corrupt /
 *    a future format). Fall back to a seeded log.
 *  - `log-diverged` — the log deserialized, but **its current snapshot does not
 *    equal the co-written `scene_json` snapshot**. `scene_json` is the map-minting
 *    contract (ADR-016 unit 1e): it is *more authoritative* than the history, so
 *    the log is **rejected loudly** and the snapshot is honoured. This is a write
 *    bug (the two persists were written together at `create_job`), surfaced, not
 *    swallowed.
 */
export type LogRejection = "legacy" | "log-unreadable" | "log-diverged";

export interface LogRestore {
  /** The log to install in the store (`scene` follows as `current(log)`). */
  log: SceneLog;
  scene: Scene | null;
  /** `null` when the persisted log was honoured; a reason when it was not. */
  logRejected: LogRejection | null;
  /** From `restoreScene`: the snapshot disagreed with the input text. */
  snapshotRejected: boolean;
}

/**
 * Restore a job's operation log on "New iteration" (unit 2b). Composes
 * `restoreScene` (input text ↔ snapshot, ADR-008 #5) with a **cross-check of the
 * persisted log against the snapshot** (the 2b main risk): the log is honoured
 * only if its current snapshot equals `scene_json`; otherwise the snapshot wins
 * and the log is rejected with a named reason. Either way an iteration boundary
 * (`restore-snapshot`) is appended on top so history carries across iterations.
 *
 * Pure and total — never throws; a `null` snapshot / log is a branch, not an
 * error (the `deserializeScene` / `deserializeLog` contract).
 */
export function restoreSceneLog(
  inputContent: string,
  sceneJson: string | null,
  sceneLogJson: string | null,
): LogRestore {
  const { scene: restored, snapshotRejected } = restoreScene(inputContent, sceneJson);

  // No coordinate block at all → the empty log (scene → null).
  if (!restored) {
    return { log: emptyLog(), scene: null, logRejected: null, snapshotRejected };
  }

  const iterationBoundary: Op = {
    type: "restore-snapshot",
    source: "new-iteration",
    fragmentCount: restored.fragments.length,
    atomCount: atomCount(restored),
  };

  // Seed a FRESH log from the reconciled scene — history "begins here".
  const seeded = (reason: LogRejection): LogRestore => {
    const log = append(emptyLog(), iterationBoundary, restored);
    return { log, scene: current(log), logRejected: reason, snapshotRejected };
  };

  if (sceneLogJson === null) return seeded("legacy");

  const persisted = deserializeLog(sceneLogJson);
  if (!persisted) return seeded("log-unreadable");

  // THE cross-check: the log's current snapshot must equal the co-written
  // scene_json snapshot (serialized-identical — they were written from one scene
  // at create_job). A mismatch means the log lies; reject it, honour the snapshot.
  const snapshot = sceneJson ? deserializeScene(sceneJson) : null;
  const logScene = current(persisted);
  const agree =
    snapshot !== null &&
    logScene !== null &&
    serializeScene(logScene) === serializeScene(snapshot);
  if (!agree) return seeded("log-diverged");

  // Honour the history: append the iteration boundary on top.
  const log = append(persisted, iterationBoundary, restored);
  return { log, scene: current(log), logRejected: null, snapshotRejected };
}
