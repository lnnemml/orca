//! Wire @monaco-editor/react to the *bundled* monaco-editor package.
//!
//! By default @monaco-editor/react fetches Monaco from a CDN (jsdelivr). That is
//! fatal for an offline desktop app, so we hand it the local npm package via
//! `loader.config({ monaco })` and provide Vite's base editor web worker. The
//! base worker is enough for our custom Monarch language (tokenization runs on
//! the main thread; we ship no TS/JSON/CSS language services).

import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
// The package `exports` map rewrites `monaco-editor/*` -> `./esm/vs/*`, so the
// worker is imported WITHOUT the `esm/vs/` prefix.
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";

self.MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

loader.config({ monaco });
