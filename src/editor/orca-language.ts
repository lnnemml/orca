//! Minimal Monarch grammar for ORCA `.inp` files.
//!
//! We deliberately do NOT try to cover ORCA's 500+ keywords. Highlighting the
//! *structure* — the `!` keyword line, `%block ... end` sections, `#` comments,
//! and the `* xyz ... *` coordinate delimiters — already carries ~80% of the
//! value and never goes stale as ORCA adds methods.

import type { languages } from "monaco-editor";

/** The language id registered with Monaco. */
export const orcaLanguageId = "orca-inp";

/** Editor behaviour: `#` line comments, no bracket pairs to auto-close. */
export const orcaLanguageConfiguration: languages.LanguageConfiguration = {
  comments: { lineComment: "#" },
  brackets: [],
  surroundingPairs: [{ open: '"', close: '"' }],
  autoClosingPairs: [{ open: '"', close: '"' }],
};

/** Monarch tokenizer. ORCA input is case-insensitive, hence `ignoreCase`. */
export const orcaMonarchTokens: languages.IMonarchLanguage = {
  ignoreCase: true,
  defaultToken: "",
  tokenizer: {
    root: [
      // Full-line comment.
      [/#.*$/, "comment"],
      // The `!` simple-input line: everything after `!` is a directive.
      [/^\s*!.*$/, "keyword"],
      // Coordinate-block delimiter line: `* xyz 0 1` ... closing `*`.
      [/^\s*\*.*$/, "delimiter"],
      // `%block` section openers.
      [/%[a-zA-Z_][\w]*/, "keyword"],
      // Block terminator.
      [/\bend\b/, "keyword"],
      // Quoted strings (filenames, e.g. %moinp "prev.gbw").
      [/"([^"\\]|\\.)*"/, "string"],
      // Numbers: float / scientific / integer, optionally signed.
      [/[-+]?\d+\.\d*([eE][-+]?\d+)?/, "number"],
      [/[-+]?\.\d+([eE][-+]?\d+)?/, "number"],
      [/[-+]?\d+([eE][-+]?\d+)?/, "number"],
    ],
  },
};
