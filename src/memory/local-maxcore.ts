//! The fan-out accessor (ADR-024 o26.5, o26.13.4): a frontend cache of the **local** default
//! `%maxcore` — Rust's `local_fitted_maxcore().default_maxcore_mib` (P for the current CPU preset;
//! 300 when P is below it; 2000 when the host's RAM is unreadable). Loaded at app start and re-read
//! when the Settings CPU preset is saved. Until it loads, and if the call fails, it yields 2000 —
//! today's value, so nothing regresses.
//!
//! Read **synchronously** (`localMaxcore()`), so the fan-out builders (`scene/optts.ts`,
//! `scene/reopt.ts` and the connectivity children through it, `crest/reopt.ts`; the scan/NEB refines
//! go through `buildOptTSInput`) stay synchronous; components subscribe with `useLocalMaxcore()`.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { LocalFittedMaxcore } from "../types";

/** The value before the cache loads (and after a failed load): the builder's former constant. */
export const FALLBACK_LOCAL_MAXCORE_MIB = 2000;

let cached: number | null = null;
const listeners = new Set<() => void>();

/** The local default `%maxcore`, MiB — synchronous, 2000 until loaded. */
export function localMaxcore(): number {
  return cached ?? FALLBACK_LOCAL_MAXCORE_MIB;
}

/** Set the cache (null = back to the fallback) and notify subscribers. Tests use it directly. */
export function setLocalMaxcore(value: number | null): void {
  cached = value;
  for (const notify of listeners) notify();
}

function subscribe(notify: () => void): () => void {
  listeners.add(notify);
  return () => listeners.delete(notify);
}

/** Re-read Rust's local default (at app start; after the CPU preset is saved). A failed call keeps
 *  the current value — never a guessed number. */
export async function reloadLocalMaxcore(
  call: () => Promise<LocalFittedMaxcore> = () => invoke<LocalFittedMaxcore>("local_fitted_maxcore"),
): Promise<void> {
  try {
    const reply = await call();
    setLocalMaxcore(reply.default_maxcore_mib);
  } catch (e) {
    console.error("[local_fitted_maxcore]", e);
  }
}

/** The local default as React state: re-renders when the cache loads or changes. */
export function useLocalMaxcore(): number {
  return useSyncExternalStore(subscribe, localMaxcore, localMaxcore);
}
