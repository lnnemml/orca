//! Save plumbing — the native save dialog (user chooses the location; NEVER the job
//! dir — rule #3, enforced again in Rust) + the Rust write commands (ADR-009 owns I/O).
//! Kept thin: the pure builders (`exporters.ts`) make the content, this just persists it.

import { save, open } from "@tauri-apps/plugin-dialog";
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

/** Curated = only the pinned scientific-artifact allowlist; Full = the whole job dir. */
export type CopyMode = "curated" | "full";

/**
 * Export a whole job group (and its sub-groups) to a folder the user picks — a
 * self-contained, readable, UUID-traceable projection with a `manifest.json` (ADR-021).
 * The canonical `<UUID>/` dirs are untouched (the Rust command only reads them).
 * Returns the created export path, or `null` if the user cancelled the folder picker.
 */
export async function exportGroup(groupId: string, mode: CopyMode): Promise<string | null> {
  const dir = await open({ directory: true, title: "Choose export destination" });
  if (!dir || typeof dir !== "string") return null;
  return invoke<string>("export_group", { groupId, destParent: dir, copyMode: mode });
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
