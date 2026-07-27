/**
 * Small helpers for moving between standard `.xyz` text, ORCA coordinate lines,
 * and the `* xyz charge mult` header. Shared by the New Job and Molecules
 * screens.
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

/**
 * Pull the charge and multiplicity from an ORCA input's `* xyz charge mult`
 * header. Falls back to neutral singlet (`0 / 1`) when there is no such line or
 * the two integers can't be read.
 */
export function parseChargeMult(content: string): {
  charge: number;
  multiplicity: number;
} {
  for (const line of content.split(/\r?\n/)) {
    const t = line.trim();
    if (!t.startsWith("*")) continue;
    const rest = t.slice(1).trim();
    if (!rest.toLowerCase().startsWith("xyz")) continue;
    // `xyz <charge> <mult> ...` — the keyword may be `xyz` or `xyzfile`.
    const parts = rest.split(/\s+/);
    const charge = Number(parts[1]);
    const multiplicity = Number(parts[2]);
    if (Number.isInteger(charge) && Number.isInteger(multiplicity)) {
      return { charge, multiplicity };
    }
    break;
  }
  return { charge: 0, multiplicity: 1 };
}
