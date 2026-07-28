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
  deserializeScene,
  mergeToAtomLines,
  sceneFromOrcaInput,
  xyzMatchesScene,
} from "./scene";
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
