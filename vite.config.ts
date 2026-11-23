/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { configDefaults } from "vitest/config";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// Absolute project root (this file's directory). chokidar matches `ignored` globs against ABSOLUTE
// paths, so each pattern is anchored here: an unanchored `**/.claude/**` would ignore every file when
// the dev server runs from a checkout living under `.claude/worktrees/`.
// Accepted limit (measured): a root containing `{`, `(` or `|` fails open: the trees stay watched
// (ENOSPC risk returns) but `src/` is never wrongly ignored; chokidar defeats `\` escapes, so none is tried.
const root = decodeURIComponent(new URL(".", import.meta.url).pathname).replace(/\/$/, "");
const ignoredTree = (name: string) => `${root}/${name}/**`;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // vitest. A plain `npm test` run from the main checkout would also collect the tests of every
  // agent worktree under `.claude/worktrees/*/` (each a full copy of `src/`); exclude the whole tree.
  test: {
    exclude: [...configDefaults.exclude, ".claude/**"],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching non-frontend top-level trees (inotify ENOSPC: verifier
      //    worktrees under `.claude/` and the large sidecar/Rust/manual/wiki trees)
      ignored: ["src-tauri", ".claude", "sidecar", "target", "resources", "wiki"].map(ignoredTree),
    },
  },
}));
