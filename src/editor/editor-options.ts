//! Monaco construction options for the ORCA input editor. Split into its own module
//! (type-only import of monaco-editor, erased at runtime) so a wiring test can pin the
//! settings the hover depends on WITHOUT dragging `monaco-setup` (which imports a Vite
//! `?worker` and touches `self`) into a node test.

import type { editor } from "monaco-editor";

/** `fixedOverflowWidgets` is load-bearing, not cosmetic: an ORCA input opens with the
 *  `!`-line as line 1, and Monaco lays a hover *above* the hovered line. Without this
 *  flag the hover renders inside the editor's `overflow:hidden` guard, so a popup placed
 *  above the top line is clipped and the user sees nothing (measured —
 *  `wiki/debugging/010-hover-clipped-on-top-line.md`). The flag moves overflow widgets to
 *  a body-level fixed overlay that escapes the clip. */
export const orcaEditorOptions: editor.IStandaloneEditorConstructionOptions = {
  fontFamily: "ui-monospace, 'SF Mono', Menlo, Consolas, monospace",
  fontSize: 13,
  minimap: { enabled: false },
  scrollBeyondLastLine: false,
  automaticLayout: true,
  tabSize: 2,
  renderWhitespace: "none",
  smoothScrolling: true,
  fixedOverflowWidgets: true,
};
