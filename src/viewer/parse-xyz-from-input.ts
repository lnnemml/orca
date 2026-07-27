/**
 * Extract the `* xyz charge multiplicity ... *` coordinate block from an ORCA
 * input file and return it as a standard xyz string:
 *
 *   3
 *   extracted from ORCA input
 *   O   0.000000   0.000000   0.117790
 *   H   0.000000   0.755453  -0.471161
 *   H   0.000000  -0.755453  -0.471161
 *
 * Returns `null` when there is no inline coordinate block — including the
 * `* xyzfile charge mult filename.xyz` form, where the geometry lives in an
 * external file we can't read on the frontend.
 */
export function extractXyzFromInput(inputContent: string): string | null {
  const lines = inputContent.split(/\r?\n/);

  // Find the opening marker: first non-space char is '*', followed by a
  // coordinate keyword. `xyzfile` must be checked before `xyz` since it also
  // starts with "xyz".
  let start = -1;
  for (let i = 0; i < lines.length; i++) {
    const rest = lines[i].trim();
    if (!rest.startsWith("*")) continue;
    const keyword = rest.slice(1).trim().toLowerCase();
    if (keyword.startsWith("xyzfile")) return null; // external file — unreadable here
    if (keyword.startsWith("xyz")) {
      start = i;
      break;
    }
  }
  if (start === -1) return null;

  // Collect `element x y z` lines until the closing '*'.
  const coords: string[] = [];
  for (let i = start + 1; i < lines.length; i++) {
    const t = lines[i].trim();
    if (t.startsWith("*")) break; // closing delimiter
    if (t.length === 0 || t.startsWith("#")) continue; // tolerate blanks/comments
    const parts = t.split(/\s+/);
    if (parts.length < 4) continue;
    const [element, x, y, z] = parts;
    if (![x, y, z].every((n) => Number.isFinite(Number(n)))) continue;
    coords.push(`${element}   ${x}   ${y}   ${z}`);
  }
  if (coords.length === 0) return null;

  return `${coords.length}\nextracted from ORCA input\n${coords.join("\n")}\n`;
}
