/**
 * Shared molecular-file import for the New Job and Molecules screens.
 *
 * `.xyz` is parsed locally (fast, no round-trip); every other supported format
 * is converted to xyz by the sidecar's `/convert` endpoint (ASE). Both paths
 * end in normalised ORCA coordinate lines. Throws `Error(message)` on any
 * failure so the caller can surface it in its existing banner.
 */

import { xyzToAtomLines } from "./xyz-format";
import { postSidecar } from "../sidecar-client";

/** File-picker `accept` list — kept in sync with {@link EXT_TO_FORMAT} + xyz. */
export const IMPORT_ACCEPT = ".xyz,.pdb,.cif,.mol,.sdf,.gen";

/** File extension → sidecar `/convert` `from_format`. `.xyz` is handled locally
 * and so is deliberately absent here. */
const EXT_TO_FORMAT: Record<string, string> = {
  pdb: "pdb",
  cif: "cif",
  mol: "mol",
  sdf: "sdf",
  gen: "gen",
};

export interface ImportedStructure {
  /** ORCA coordinate lines (`element x y z`). */
  atomLines: string[];
}

/** Read a molecular-structure file into ORCA coordinate lines. */
export async function importStructureFile(
  file: File,
): Promise<ImportedStructure> {
  const text = await readFileText(file);
  const ext = file.name.split(".").pop()?.toLowerCase() ?? "";

  if (ext === "xyz") {
    const atoms = xyzToAtomLines(text);
    if (!atoms) throw new Error(`"${file.name}" is not a valid .xyz file`);
    return { atomLines: atoms };
  }

  const fromFormat = EXT_TO_FORMAT[ext];
  if (!fromFormat) throw new Error(`Unsupported file type: .${ext || "?"}`);

  const xyz = await convertToXyz(text, fromFormat);
  const atoms = xyzToAtomLines(xyz);
  if (!atoms) {
    throw new Error(`Converted structure from "${file.name}" had no atoms`);
  }
  return { atomLines: atoms };
}

function readFileText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error(`Could not read "${file.name}"`));
    reader.readAsText(file);
  });
}

/** Convert non-xyz content to xyz via the sidecar; returns the xyz text. */
async function convertToXyz(content: string, fromFormat: string): Promise<string> {
  const data = await postSidecar<{ content: string }>("/convert", {
    content,
    from_format: fromFormat,
    to_format: "xyz",
  });
  return data.content;
}
