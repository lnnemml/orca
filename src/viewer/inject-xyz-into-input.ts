/**
 * Replace or insert the `* xyz charge mult ... *` coordinate block in an ORCA
 * input string. Everything outside the block (the `!` line, `%` blocks,
 * comments) is preserved. Also replaces a `* xyzfile ... *` block if present.
 *
 * @param atomLines lines of the form `"O  0.0  0.0  0.117"` (element x y z).
 */
export function injectXyzIntoInput(
  currentContent: string,
  atomLines: string[],
  charge: number,
  multiplicity: number,
): string {
  const block = `* xyz ${charge} ${multiplicity}\n${atomLines.join("\n")}\n*`;
  const lines = currentContent.split(/\r?\n/);

  // Locate an existing coordinate block: an opening `*` followed by an `xyz`
  // (or `xyzfile`) keyword — the same marker `extractXyzFromInput` looks for.
  let start = -1;
  for (let i = 0; i < lines.length; i++) {
    const rest = lines[i].trim();
    if (!rest.startsWith("*")) continue;
    if (rest.slice(1).trim().toLowerCase().startsWith("xyz")) {
      start = i;
      break;
    }
  }

  if (start === -1) {
    // No block yet — append it at the end, separated by a blank line.
    const trimmed = currentContent.replace(/\s*$/, "");
    return trimmed.length > 0 ? `${trimmed}\n\n${block}\n` : `${block}\n`;
  }

  // Find the closing `*` (first `*` line after the opener); if there is none,
  // the block is taken to run to the end of the content.
  let end = lines.length - 1;
  for (let i = start + 1; i < lines.length; i++) {
    if (lines[i].trim().startsWith("*")) {
      end = i;
      break;
    }
  }

  return [...lines.slice(0, start), block, ...lines.slice(end + 1)].join("\n");
}
