//! Save plumbing — the native save dialog (user chooses the location; NEVER the job
//! dir — rule #3, enforced again in Rust) + the Rust write commands (ADR-009 owns I/O).
//! Kept thin: the pure builders (`exporters.ts`) make the content, this just persists it.

import { save } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

/** A filesystem-safe default filename: `{job}-{what}.{ext}` with spaces/odd chars tamed. */
export function exportName(jobTitle: string, what: string, ext: string): string {
  const base = (jobTitle || "job").trim().replace(/[^\w.-]+/g, "_").replace(/_+/g, "_").slice(0, 60);
  return `${base}-${what}.${ext}`;
}

/** Write text (xyz/CSV) via a save dialog. Returns false if the user cancelled. */
export async function saveText(defaultName: string, content: string, ext: string): Promise<boolean> {
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
  });
  if (!path) return false;
  await invoke("write_export_text", { path, content });
  return true;
}

/** Write binary bytes (PNG) via a save dialog. Returns false if the user cancelled. */
export async function saveBytes(defaultName: string, bytes: Uint8Array): Promise<boolean> {
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: "PNG", extensions: ["png"] }],
  });
  if (!path) return false;
  await invoke("write_export_bytes", { path, bytes: Array.from(bytes) });
  return true;
}
