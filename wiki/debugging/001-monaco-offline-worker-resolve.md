# 001-monaco-offline-worker-resolve.md — Vite build can't resolve the Monaco web worker

**Date:** 2026-07-26 · **Area:** frontend
**Symptom:** `vite build` fails with
`[vite]: Rollup failed to resolve import "monaco-editor/esm/vs/editor/editor.worker.js?worker"`.
`tsc` was clean (the ambient `vite/client` decl types the `?worker` suffix), so the error only
surfaced at bundle time. Trying without `.js` and with `.js` both failed identically.

**Root cause:** `monaco-editor`'s `package.json` `exports` map is:
```json
"./*.js": "./esm/vs/*.js",
"./*":    "./esm/vs/*.js"
```
So a deep import already containing `esm/vs/` gets **double-mapped**: the subpath
`./esm/vs/editor/editor.worker.js` resolves the `*` to `esm/vs/editor/editor.worker`, producing
`./esm/vs/esm/vs/editor/editor.worker.js` — a path that doesn't exist → resolution fails. Dev
mode was never reached because the build blocked first. (Separately, `@monaco-editor/react`
defaults to loading Monaco from a **CDN**, which is fatal for an offline desktop app.)

**Fix:** import the worker WITHOUT the `esm/vs/` prefix so the exports map maps it once:
`import EditorWorker from "monaco-editor/editor/editor.worker.js?worker"` → resolves to
`./esm/vs/editor/editor.worker.js`. Combined with `loader.config({ monaco })` and
`self.MonacoEnvironment.getWorker` in `src/editor/monaco-setup.ts` to force the bundled package
+ local worker instead of the CDN. Verified at runtime in Chrome: editor renders, ORCA Monarch
highlighting works, zero console/worker errors. (commit: Phase 1.2)

**Lesson / rule:** when a package ships an `exports` map with `"./*"` wildcards, import via the
**public** subpath the map expects, not the physical `esm/vs/...` path — the wildcard will
re-prefix it. And for any bundled-in-desktop JS lib that defaults to a CDN loader
(`@monaco-editor/react`), always pin it to the local package explicitly. Added to
`wiki/modules/frontend.md`.
