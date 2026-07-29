# Wiki Index

Catalog of every page. Updated on every page creation/rename (see CLAUDE.md).

## Architecture

- [overview.md](architecture/overview.md) — three-layer architecture, data flow, job lifecycle
- [adr-001-tauri-react.md](architecture/adr-001-tauri-react.md) — Tauri 2 + React over Electron / PySide6
- [adr-002-python-sidecar.md](architecture/adr-002-python-sidecar.md) — FastAPI sidecar for chemistry logic
- [adr-003-execution-backend.md](architecture/adr-003-execution-backend.md) — ExecutionBackend abstraction (local/SSH/SLURM)
- [adr-004-sqlite-storage.md](architecture/adr-004-sqlite-storage.md) — SQLite as the single store (+ FTS5 for manual)
- [adr-005-system-ssh.md](architecture/adr-005-system-ssh.md) — system ssh/rsync over SSH libraries
- [adr-006-manual-integration.md](architecture/adr-006-manual-integration.md) — local ORCA manual indexing strategy
- [adr-007-reaction-modeling.md](architecture/adr-007-reaction-modeling.md) — from molecular modeling to reaction modeling: domain objects, geometry editor, reaction coordinate workflows
- [adr-008-scene-fragment-model.md](architecture/adr-008-scene-fragment-model.md) — Scene / SceneFragment model for multi-molecule geometry: one flat merged xyz, one 3Dmol model, `scene_json` snapshot
- [prior-art.md](architecture/prior-art.md) — how other tools (Avogadro 2, IQmol, Gabedit, Chemcraft, WebMO, GaussView) handle geometry, and what OrcaStudio does differently (scene-derived mask, unbroken index space)

## Modules

- [frontend.md](modules/frontend.md) — React UI: screens, state, key components
- [tauri-core.md](modules/tauri-core.md) — Rust core: commands, events, job state machine
- [sidecar.md](modules/sidecar.md) — Python service: endpoints, dependencies
- [execution-backends.md](modules/execution-backends.md) — LocalBackend / SshBackend details
- [parser.md](modules/parser.md) — result extraction: streaming regex + cclib
- [visualization.md](modules/visualization.md) — 3Dmol.js integration, cubes, spectra
- [scene.md](modules/scene.md) — Scene/SceneFragment pure core: merge to flat xyz, index-space invariant, reset-detection primitive (ADR-008)
- [manual-index.md](modules/manual-index.md) — manual indexing pipeline + keyword map

## ORCA domain knowledge

- [orca-basics.md](orca/orca-basics.md) — installation, invocation, MPI, environment
- [performance.md](orca/performance.md) — measured parallel scaling on the dev machine (i5-12500H), core-pinning presets, benchmark methodology
- [input-format.md](orca/input-format.md) — anatomy of an .inp file
- [output-files.md](orca/output-files.md) — what ORCA produces and what each file is for
- [gotchas.md](orca/gotchas.md) — accumulated traps and their solutions
- [goat.md](orca/goat.md) — `! XTB GOAT` conformer search: observed ensemble format, atom-order preservation, `%pal` cost (verified on ORCA 6.1.0)

## Chemistry (навчальні нотатки, українською)

- [README.md](chemistry/README.md) — що тут живе і як вести нотатки
- [reagent-geometry.md](chemistry/reagent-geometry.md) — геометрія реагентів-нуклеофілів: гідрид, чому BH₄⁻ тетраедричний, чому кут води 104.5°
- [conformers.md](chemistry/conformers.md) — конформери і GOAT: anti/gauche-бутан, чому один конформер зі SMILES — випадковий знімок

## Debugging

- [README.md](debugging/README.md) — format for solved-bug pages
- [001-monaco-offline-worker-resolve.md](debugging/001-monaco-offline-worker-resolve.md) — Monaco worker unresolved in Vite build (exports-map double-mapping) + CDN→bundled fix
- [002-webkitgtk-3dmol-offscreencanvas.md](debugging/002-webkitgtk-3dmol-offscreencanvas.md) — 3Dmol.js null WebGL context in WebKitGTK (OffscreenCanvas webgl2 returns null) + fix & MiniBrowser test
- [003-webkitgtk-select-styling.md](debugging/003-webkitgtk-select-styling.md) — `<select>` dark-on-dark in WebKitGTK (native GTK widget ignores inherited color) + appearance:none fix
- [004-mpi-ranks-escape-process-group.md](debugging/004-mpi-ranks-escape-process-group.md) — cancel orphans ORCA's MPI ranks (they escape the parent's process group) + sweep-by-cwd fix
- [005-stale-sidecar-hmr.md](debugging/005-stale-sidecar-hmr.md) — the app talks to a stale sidecar after frontend HMR (uvicorn stays old) → "Not Found"; version handshake + human errors + dev `--reload`

---

*Page count: 33. Last structural update: 2026-07-29 (+debugging/005-stale-sidecar-hmr).*
