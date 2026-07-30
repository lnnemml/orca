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
- [adr-009-process-orchestration.md](architecture/adr-009-process-orchestration.md) — running external binaries belongs to Rust, not the sidecar (complements ADR-002/003; the xTB precedent)
- [adr-010-editor-identity-state.md](architecture/adr-010-editor-identity-state.md) — editor identity & state model: `AtomId`, branded `OrcaIndex`/`AseIndex`, `IndexMap`, `emit_input`/`parse_output` paired, op-log fold; refines ADR-008 (three corrections; sidecar stays positional, Rust maps)
- [adr-011-editor-graphics-stack.md](architecture/adr-011-editor-graphics-stack.md) — editor graphics stack (wgpu→WASM→WebGL2/WebGPU, impostors, GPU picking): **proposed/deferred**, gated on a spike with verifiable exit criteria; until then 3Dmol stays a dumb renderer
- [adr-012-output-parsing-ownership.md](architecture/adr-012-output-parsing-ownership.md) — authoritative result parsing = own Rust parsers over structured artifacts (`.property.txt`/`.hess`/`_trj.xyz`/`orca_2json`), **not cclib** (crashes on ORCA 6.1); narrows ADR-002, moves the tier to Rust (ADR-009)
- [prior-art.md](architecture/prior-art.md) — how other tools (Avogadro 2, IQmol, Gabedit, Chemcraft, WebMO, GaussView) handle geometry, and what OrcaStudio does differently (scene-derived mask, unbroken index space)
- [proposals/editor-architecture-2026-07-30.md](architecture/proposals/editor-architecture-2026-07-30.md) — author's design proposal for the editor architecture (source document for ADR-010/011; includes §11 rejected alternatives). Input, not a decision — not edited.

## Modules

- [frontend.md](modules/frontend.md) — React UI: screens, state, key components
- [tauri-core.md](modules/tauri-core.md) — Rust core: commands, events, job state machine
- [sidecar.md](modules/sidecar.md) — Python service: endpoints, dependencies
- [execution-backends.md](modules/execution-backends.md) — LocalBackend / SshBackend details
- [parser.md](modules/parser.md) — result extraction: streaming (Tier 1) + authoritative artifact readers (Tier 2)
- [artifact-readers.md](modules/artifact-readers.md) — `src-tauri/src/parse/`: two-layer tokenizer + typed accessors, canonical units held by the `Angstrom` type, post-conditions; `.property.txt` built (the template), 3 others not started (ADR-012)
- [visualization.md](modules/visualization.md) — 3Dmol.js integration, cubes, spectra
- [results-ui.md](modules/results-ui.md) — post-calculation visualization (unit 3.8): trajectory playback + IR spectrum; the frame number is app state, not the viewer's (ADR-011)
- [scene.md](modules/scene.md) — Scene/SceneFragment pure core: merge to flat xyz, index-space invariant, reset-detection primitive (ADR-008)
- [manual-index.md](modules/manual-index.md) — manual indexing pipeline + keyword map

## ORCA domain knowledge

- [orca-basics.md](orca/orca-basics.md) — installation, invocation, MPI, environment
- [performance.md](orca/performance.md) — measured parallel scaling on the dev machine (i5-12500H), core-pinning presets, benchmark methodology
- [input-format.md](orca/input-format.md) — anatomy of an .inp file
- [output-files.md](orca/output-files.md) — what ORCA produces and what each file is for
- [gotchas.md](orca/gotchas.md) — accumulated traps and their solutions
- [goat.md](orca/goat.md) — `! XTB GOAT` conformer search: observed ensemble format, atom-order preservation, `%pal` cost (verified on ORCA 6.1.0)
- [constraints.md](orca/constraints.md) — `%geom Constraints` block: B/A/D/C syntax, value/no-value, **0-based index base settled by a real ORCA 6.1.0 run**, out-of-range segfault
- [xtb.md](orca/xtb.md) — standalone GFN2-xTB pre-optimization: **1-based `$constrain` (≠ ORCA!)** settled by a real xtb 6.6.1 run, spring-held constraints + the measured tolerance, charge/multiplicity
- [parse-sources.md](orca/parse-sources.md) — **measured** parse sources for ORCA 6.1.0: cclib 1.8.1 crashes on 6.1 output; `.hess`/`.property.txt`/`_trj.xyz`/`orca_2json` inventory; per-quantity source table (probe: `sidecar/probes/parse_sources.py`)

## Chemistry (навчальні нотатки, українською)

- [README.md](chemistry/README.md) — що тут живе і як вести нотатки
- [reagent-geometry.md](chemistry/reagent-geometry.md) — геометрія реагентів-нуклеофілів: гідрид, чому BH₄⁻ тетраедричний, чому кут води 104.5°
- [conformers.md](chemistry/conformers.md) — конформери і GOAT: anti/gauche-бутан, чому один конформер зі SMILES — випадковий знімок
- [burgi-dunitz.md](chemistry/burgi-dunitz.md) — траєкторія Бюрґі–Дунітца: чому нуклеофіл атакує карбоніл під ~107°, метод структурної кореляції, звʼязок зі стереоселективністю NaBH₄ і з d/θ/φ у редакторі
- [ir-spectrum.md](chemistry/ir-spectrum.md) — ІЧ-спектр: чому обчислений спектр — палички, а не крива; розширення лінії; FWHM як вибір побудови графіка; що означає площа під піком; уявна частота як діагноз перехідного стану

## Debugging

- [README.md](debugging/README.md) — format for solved-bug pages
- [001-monaco-offline-worker-resolve.md](debugging/001-monaco-offline-worker-resolve.md) — Monaco worker unresolved in Vite build (exports-map double-mapping) + CDN→bundled fix
- [002-webkitgtk-3dmol-offscreencanvas.md](debugging/002-webkitgtk-3dmol-offscreencanvas.md) — 3Dmol.js null WebGL context in WebKitGTK (OffscreenCanvas webgl2 returns null) + fix & MiniBrowser test
- [003-webkitgtk-select-styling.md](debugging/003-webkitgtk-select-styling.md) — `<select>` dark-on-dark in WebKitGTK (native GTK widget ignores inherited color) + appearance:none fix
- [004-mpi-ranks-escape-process-group.md](debugging/004-mpi-ranks-escape-process-group.md) — cancel orphans ORCA's MPI ranks (they escape the parent's process group) + sweep-by-cwd fix
- [005-stale-sidecar-hmr.md](debugging/005-stale-sidecar-hmr.md) — the app talks to a stale sidecar after frontend HMR (uvicorn stays old) → "Not Found"; version handshake + human errors + dev `--reload`
- [006-xtb-empty-input-hang.md](debugging/006-xtb-empty-input-hang.md) — xTB pre-optimize hangs 300 s on a no-constraint run: an empty `xcontrol` passed via `--input` freezes xtb 6.6.1 before cycle 1; rejected hypotheses (molecule/OMP/opt level) + the `--input`-only-with-content fix

---

*Page count: 46. Last structural update: 2026-07-30 (unit 3.8: +modules/results-ui.md — trajectory playback + IR spectrum, frame state in the app not the viewer; +chemistry/ir-spectrum.md — line broadening / FWHM / imaginary-frequency diagnosis).*
