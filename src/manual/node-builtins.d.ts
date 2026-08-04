//! Minimal ambient types for the few Node built-ins the CORPUS test reader needs
//! (`render.corpus.test.ts`). This project deliberately has NO `@types/node` dependency
//! (see `vite.config.ts`, which `@ts-expect-error`s `process`); the corpus gate is the
//! only consumer of `fs`/`path`, so we declare exactly its surface here rather than pull
//! a new devDependency. Runtime is provided by Node under vitest; this is types-only.

declare module "node:fs" {
  export function readFileSync(path: string, encoding: string): string;
  export function existsSync(path: string): boolean;
  export function statSync(path: string): { isDirectory(): boolean };
  export function readdirSync(
    path: string,
    opts: { withFileTypes: true },
  ): { name: string; isDirectory(): boolean }[];
}
declare module "node:path" {
  export function join(...parts: string[]): string;
  export function resolve(...parts: string[]): string;
}
declare const process: { env: Record<string, string | undefined>; cwd(): string };
