import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job } from "../types";
import { sourceMethodIsXtb } from "../scene/optts";
import {
  DEFAULT_BUILDER_STATE,
  methodSliceOf,
  type MethodSlice,
} from "./build-input";
import { MethodPicker } from "./MethodPicker";

/** The buildOptTSInput options this control produces: `{}` = inherit (no override, the
 * byte-identical path), or `{ methodState }` = run the refine at the picked level. */
export type OptTSMethodOverride = { methodState?: MethodSlice };

/**
 * The OptTS-refine method control, shared by the three OptTS-derived spawn sites (1D scan, 2D
 * scan, NEB band). It wraps the reusable {@link MethodPicker} with the "Inherit from source"
 * default and the XTB-source note, and hands the parent the {@link OptTSMethodOverride} to pass
 * straight into `buildOptTSInput`.
 *
 * **Inherit = pass nothing.** The default is inherit → `onChange({})` → the caller passes `{}`
 * to `buildOptTSInput`, which re-uses the source's method verbatim (the byte-identical path of
 * `src/scene/optts.ts`, unchanged for anyone who does not touch this picker). Only an explicit
 * family selection produces `{ methodState }`. There is deliberately no attempt to reconstruct
 * the source method into a slice — that would be a back-door reverse-parse of the source `!`
 * line; inherit literally sends no override.
 */
export function OptTSMethodPicker({
  sourceIsXtb,
  onChange,
}: {
  /** True when the source ran at semi-empirical XTB — shows the "pick a DFT level" note. */
  sourceIsXtb?: boolean;
  onChange: (override: OptTSMethodOverride) => void;
}) {
  // Inherit is the default; the slice is seeded from the app defaults and only used once the
  // user turns inherit off by picking a family. "Disable, don't clear" — switching back to
  // Inherit keeps the configured slice, exactly like the form's xTB-tail handling.
  const [inherit, setInherit] = useState(true);
  const [slice, setSlice] = useState<MethodSlice>(() => methodSliceOf(DEFAULT_BUILDER_STATE));

  // Emit the override the parent will hand to buildOptTSInput. Called from the change handlers
  // (not an effect) so there is no mount-time emit — the parent's own default is `{}` (inherit),
  // which already matches, so an untouched picker never fires and never overrides.
  const emit = (nextInherit: boolean, nextSlice: MethodSlice) =>
    onChange(nextInherit ? {} : { methodState: nextSlice });

  return (
    <MethodPicker
      value={slice}
      onChange={(patch) => {
        const next = { ...slice, ...patch };
        setSlice(next);
        emit(inherit, next);
      }}
      inherit={{
        active: inherit,
        onChange: (active) => {
          setInherit(active);
          emit(active, slice);
        },
        note: sourceIsXtb ? (
          <div className="builder-note warn">
            This scan ran at semi-empirical XTB — a semi-empirical OptTS isn't
            publication-grade. Pick a DFT level (functional + basis) for the refined
            transition state.
          </div>
        ) : undefined,
      }}
    />
  );
}

/**
 * Read a source job's method once (on mount) and report whether it is semi-empirical XTB, for
 * the {@link OptTSMethodPicker} note. Best-effort: a fetch failure leaves it `false` (the note
 * simply does not show — it never blocks the refine).
 */
export function useSourceIsXtb(jobId: string): boolean {
  const [isXtb, setIsXtb] = useState(false);
  useEffect(() => {
    let live = true;
    invoke<Job>("get_job", { id: jobId })
      .then((j) => {
        if (live) setIsXtb(sourceMethodIsXtb(j.input_content));
      })
      .catch(() => {
        /* best-effort — no note on a read failure */
      });
    return () => {
      live = false;
    };
  }, [jobId]);
  return isXtb;
}
