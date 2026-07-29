/**
 * One client for the Python sidecar (2.5.2d-1). All three callers — EditPanel,
 * import-file, smiles — go through `postSidecar`, so a failure turns into the
 * SAME human message everywhere instead of a bare `Not Found`.
 *
 * The parse logic (`describeSidecarError`) is a **pure function with tests**; the
 * fetch stays thin. The trap this exists to close: `npm run tauri dev` hot-reloads
 * the frontend but uvicorn (no `--reload` in a release build) keeps running old
 * code, so a newly-added route 404s and the user sees "Not Found" mid-workflow
 * with no hint why. See wiki/debugging/005.
 */

import { invoke } from "@tauri-apps/api/core";
import type { SidecarStatus } from "./types";

export type SidecarErrorKind =
  | "not-running" // no port / down / starting, or a network error
  | "missing-endpoint" // 404 — the running sidecar is an older build
  | "validation" // 4xx with a human `detail` (e.g. 422)
  | "server-error"; // 5xx — a real inconsistency (post-condition breach)

export interface SidecarError {
  kind: SidecarErrorKind;
  message: string;
}

/**
 * Map a sidecar failure to a `{ kind, message }` a human can act on. `status` is
 * an HTTP status code, or the string `"network"` for a fetch that never reached
 * the server. `detail` is FastAPI's `detail` field when present. `route` is the
 * path being called, so a 404 can name it. Pure — unit-tested.
 */
export function describeSidecarError(input: {
  status: number | "network";
  detail?: string | null;
  route: string;
}): SidecarError {
  const { status, detail, route } = input;

  if (status === "network") {
    return { kind: "not-running", message: "The chemistry sidecar isn't running." };
  }
  if (status === 404) {
    return {
      kind: "missing-endpoint",
      message: `The sidecar has no route ${route} — it is running an older build. Restart the app to pick up the new endpoints.`,
    };
  }
  if (status >= 500) {
    return {
      kind: "server-error",
      message:
        detail?.trim() ||
        "The sidecar hit an internal inconsistency (a post-condition failed).",
    };
  }
  // Any other 4xx (422 validation, 400, …) — the detail is already human.
  return {
    kind: "validation",
    message: detail?.trim() || `The sidecar rejected the request (HTTP ${status}).`,
  };
}

/** Message for a sidecar that isn't reachable at all (down/starting/no port). */
function unreachableMessage(status: SidecarStatus): string {
  if (status.status === "starting") {
    return "The chemistry sidecar is still starting — try again in a moment.";
  }
  return "The chemistry sidecar isn't running.";
}

/**
 * POST JSON to a sidecar route and return the parsed JSON. Throws an `Error`
 * whose message is `describeSidecarError(...).message` on any failure. A `stale`
 * sidecar is NOT pre-blocked — an existing route still works on it, and a
 * new/changed route 404s into the "older build, restart" message; the status bar
 * separately flags stale prominently.
 */
export async function postSidecar<T>(route: string, body: unknown): Promise<T> {
  const status = await invoke<SidecarStatus>("get_sidecar_status");
  if (
    !status.port ||
    status.status === "down" ||
    status.status === "starting"
  ) {
    throw new Error(unreachableMessage(status));
  }

  let resp: Response;
  try {
    resp = await fetch(`http://127.0.0.1:${status.port}${route}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error(describeSidecarError({ status: "network", route }).message);
  }

  if (!resp.ok) {
    let detail: string | null = null;
    try {
      const b = await resp.json();
      if (b?.detail) detail = String(b.detail);
    } catch {
      /* non-JSON error body — the status code carries the meaning */
    }
    throw new Error(
      describeSidecarError({ status: resp.status, detail, route }).message,
    );
  }
  return (await resp.json()) as T;
}
