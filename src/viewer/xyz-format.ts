/**
 * Standard-`.xyz`-string ↔ ORCA-coordinate-line formatting. NOT an ORCA-input
 * parser (that lives in `src/scene/scene.ts` — `sceneFromOrcaInput` /
 * `injectSceneIntoInput`); these two are kept because their live consumers work
 * in xyz *strings*, not Scenes: `import-file.ts` (file/sidecar xyz → atom lines)
 * and `MoleculesScreen` (imported atom lines → the xyz string stored on a
 * library molecule). The ORCA-input parsers this file used to hold
 * (`parseChargeMult`) and its siblings (`parse-xyz-from-input.ts`,
 * `inject-xyz-into-input.ts`) were removed in 2.5.0d when NewJobScreen moved to
 * the scene store — completing the ADR-008 consolidation.
 */

/**
 * Parse standard xyz text (`count`, comment, then `element x y z` rows) into
 * ORCA coordinate lines. Returns `null` if the first line isn't a positive atom
 * count or no valid coordinate rows follow.
 */
export function xyzToAtomLines(xyz: string): string[] | null {
  const lines = xyz.split(/\r?\n/);
  if (lines.length < 3) return null;
  const count = Number(lines[0].trim());
  if (!Number.isInteger(count) || count <= 0) return null;

  const atoms: string[] = [];
  for (let i = 2; i < lines.length; i++) {
    const t = lines[i].trim();
    if (t.length === 0) continue;
    const parts = t.split(/\s+/);
    if (parts.length < 4) continue;
    const [element, x, y, z] = parts;
    if (![x, y, z].every((n) => Number.isFinite(Number(n)))) continue;
    atoms.push(`${element}   ${x}   ${y}   ${z}`);
  }
  return atoms.length > 0 ? atoms : null;
}

/**
 * Build a standard xyz string (atom count, comment, then `element x y z` rows)
 * from ORCA coordinate lines — the inverse of {@link xyzToAtomLines}.
 */
export function atomLinesToXyz(atomLines: string[], comment = ""): string {
  return `${atomLines.length}\n${comment}\n${atomLines.join("\n")}\n`;
}
