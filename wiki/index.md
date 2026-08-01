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
- [adr-013-manual-indexing-ownership.md](architecture/adr-013-manual-indexing-ownership.md) — manual indexing ownership: **Rust** writes `orcastudio.db` and does the sectioning (Markdown/ATX, not HTML); the app never fetches the manual over the network (a per-version script does); narrows ADR-006 (three decisions), does not edit it
- [adr-014-ai-integration-boundary.md](architecture/adr-014-ai-integration-boundary.md) — AI integration boundary: AI never inside the numerical pipeline (geometric constants **retrieved, never recalled**); three authority tiers T1 explain / T2 draft / T3 orchestrate (MCP, after Phase 4.5) mapped onto ADR-007's L1–L4; methodology as executable guard; commands as a pollable API; narrows ADR-007 §"AI integration", does not edit it
- [prior-art.md](architecture/prior-art.md) — how other tools (Avogadro 2, IQmol, Gabedit, Chemcraft, WebMO, GaussView) handle geometry, and what OrcaStudio does differently (scene-derived mask, unbroken index space)
- [ai-landscape.md](architecture/ai-landscape.md) — register of **agentic AI layers** over computational chemistry (Bunsen, El Agente over ORCA, ChemGraph, Aitomia), each with a uniform Who/Drives-what/Interface/Compute/Access/Delta record; why they do not change OrcaStudio's need, and what they confirm (agents built **over** validated physics); the JOSS statement-of-need source. Sibling of prior-art.md (builders/viewers)
- [proposals/editor-architecture-2026-07-30.md](architecture/proposals/editor-architecture-2026-07-30.md) — author's design proposal for the editor architecture (source document for ADR-010/011; includes §11 rejected alternatives). Input, not a decision — not edited.

## Modules

- [frontend.md](modules/frontend.md) — React UI: screens, state, key components
- [tauri-core.md](modules/tauri-core.md) — Rust core: commands, events, job state machine
- [sidecar.md](modules/sidecar.md) — Python service: endpoints, dependencies
- [execution-backends.md](modules/execution-backends.md) — LocalBackend / SshBackend details
- [parser.md](modules/parser.md) — result extraction: streaming (Tier 1) + authoritative artifact readers (Tier 2)
- [artifact-readers.md](modules/artifact-readers.md) — `src-tauri/src/parse/`: two-layer tokenizer + typed accessors, canonical units held by the `Angstrom` type, post-conditions; the ADR-012 tier is **complete** — all four readers (`.property.txt`, `.hess`, `_trj.xyz`/`.xyz`, `orca_2json`) built and wired into `results.rs`
- [visualization.md](modules/visualization.md) — 3Dmol.js integration, cubes, spectra
- [results-ui.md](modules/results-ui.md) — post-calculation visualization: trajectory playback + IR spectrum + normal-mode animation; the frame/phase is app state, not the viewer's (ADR-011); IR panel is sticks (km/mol) + broadened curve (km/mol·cm⁻¹) on two labelled axes, single-source tooltip, display-only scale + inversion (3.10); click a peak → animate the mode, amplitude a display choice, imaginary = reaction coordinate (3.12)
- [scene.md](modules/scene.md) — Scene/SceneFragment pure core: merge to flat xyz, index-space invariant, reset-detection primitive (ADR-008)
- [manual-index.md](modules/manual-index.md) — manual indexing pipeline + keyword map
- [manual-sections.md](modules/manual-sections.md) — `src-tauri/src/manual/`: ATX sectioner (fence-aware, non-nested bodies) + `objects.inv` anchor map; three post-conditions (line conservation, anchors, label binding); the `#[ignore]` corpus gate. Writes no DB (storage is 4.3)

## ORCA domain knowledge

- [orca-basics.md](orca/orca-basics.md) — installation, invocation, MPI, environment
- [orca-plot.md](orca/orca-plot.md) — **measured** unit-3.15 gate: `orca_plot` non-interactive invocation (its `plot-inputfile` batch mode was unusable → drive the menu over stdin), cube size/time by grid (40³–100³ = 0.9–13.5 MB, sub-second), the `.cube` format, and the WebKitGTK isosurface render PASS (MiniBrowser)
- [performance.md](orca/performance.md) — measured parallel scaling on the dev machine (i5-12500H), core-pinning presets, benchmark methodology
- [input-format.md](orca/input-format.md) — anatomy of an .inp file
- [output-files.md](orca/output-files.md) — what ORCA produces and what each file is for
- [gotchas.md](orca/gotchas.md) — accumulated traps and their solutions
- [goat.md](orca/goat.md) — `! XTB GOAT` conformer search: observed ensemble format, atom-order preservation, `%pal` cost (verified on ORCA 6.1.0)
- [constraints.md](orca/constraints.md) — `%geom Constraints` block: B/A/D/C syntax, value/no-value, **0-based index base settled by a real ORCA 6.1.0 run**, out-of-range segfault
- [xtb.md](orca/xtb.md) — standalone GFN2-xTB pre-optimization: **1-based `$constrain` (≠ ORCA!)** settled by a real xtb 6.6.1 run, spring-held constraints + the measured tolerance, charge/multiplicity
- [parse-sources.md](orca/parse-sources.md) — **measured** parse sources for ORCA 6.1.0: cclib 1.8.1 crashes on 6.1 output; `.hess`/`.property.txt`/`_trj.xyz`/`orca_2json` inventory; per-quantity source table; the `.hess` frame is a pure translation of the reference (unit-3.12 Kabsch gate, no mode rotation) (probes: `parse_sources.py`, `hess_frame_kabsch.py`)
- [manual-sources.md](orca/manual-sources.md) — **measured** ORCA 6.1 manual source format (Sphinx+MyST, `_sources/*.md.txt`): the 140-path toctree walk (11 containers / 126 leaves / 3 no-source), the `//` normalization trap on the Structure-and-Reactivity branch, ATX sectioning, heterogeneous "Keywords" markup (`:::{table}` pipe tables vs annotated ` ```orca ` blocks), the `(label)=`→`#slug` anchor rule (46/46) + `objects.inv` exists, eval-rst review-trigger = 0 in body sample (probe: `scripts/fetch-manual.py`)

## Chemistry (навчальні нотатки, українською)

- [README.md](chemistry/README.md) — що тут живе і як вести нотатки
- [reagent-geometry.md](chemistry/reagent-geometry.md) — геометрія реагентів-нуклеофілів: гідрид, чому BH₄⁻ тетраедричний, чому кут води 104.5°
- [conformers.md](chemistry/conformers.md) — конформери і GOAT: anti/gauche-бутан, чому один конформер зі SMILES — випадковий знімок
- [burgi-dunitz.md](chemistry/burgi-dunitz.md) — траєкторія Бюрґі–Дунітца: чому нуклеофіл атакує карбоніл під ~107°, метод структурної кореляції, звʼязок зі стереоселективністю NaBH₄ і з d/θ/φ у редакторі
- [normal-modes.md](chemistry/normal-modes.md) — нормальні моди: що таке мода й що показує анімація; чому амплітуда довільна (вибір подання); чому анімація уявної моди показує координату реакції; чому моди беруться «як є» (ворота Кабша)
- [ir-spectrum.md](chemistry/ir-spectrum.md) — ІЧ-спектр: чому обчислений спектр — палички, а не крива; розширення лінії; FWHM як вибір побудови графіка; що означає площа під піком; уявна частота як діагноз перехідного стану; **чому обчислений спектр не схожий на експериментальний** (інтенсивність vs %T, два C=O, гармонічне завищення, O–H-димер, низькі моди й ентропія — зміряно на декскетопрофені)
- [orbitals.md](chemistry/orbitals.md) — орбіталі та ізоповерхні: що таке МО й ізоповерхня; чому дві кольорові поверхні — це знак **фази**, а не заряд; ізозначення як вибір подання; чому HOMO/LUMO — межові орбіталі, де живе реакційна здатність

## Debugging

- [README.md](debugging/README.md) — format for solved-bug pages
- [001-monaco-offline-worker-resolve.md](debugging/001-monaco-offline-worker-resolve.md) — Monaco worker unresolved in Vite build (exports-map double-mapping) + CDN→bundled fix
- [002-webkitgtk-3dmol-offscreencanvas.md](debugging/002-webkitgtk-3dmol-offscreencanvas.md) — 3Dmol.js null WebGL context in WebKitGTK (OffscreenCanvas webgl2 returns null) + fix & MiniBrowser test
- [003-webkitgtk-select-styling.md](debugging/003-webkitgtk-select-styling.md) — `<select>` dark-on-dark in WebKitGTK (native GTK widget ignores inherited color) + appearance:none fix
- [004-mpi-ranks-escape-process-group.md](debugging/004-mpi-ranks-escape-process-group.md) — cancel orphans ORCA's MPI ranks (they escape the parent's process group) + sweep-by-cwd fix
- [005-stale-sidecar-hmr.md](debugging/005-stale-sidecar-hmr.md) — the app talks to a stale sidecar after frontend HMR (uvicorn stays old) → "Not Found"; version handshake + human errors + dev `--reload`
- [006-xtb-empty-input-hang.md](debugging/006-xtb-empty-input-hang.md) — xTB pre-optimize hangs 300 s on a no-constraint run: an empty `xcontrol` passed via `--input` freezes xtb 6.6.1 before cycle 1; rejected hypotheses (molecule/OMP/opt level) + the `--input`-only-with-content fix
- [007-phase1-decisions-phase3-outgrew.md](debugging/007-phase1-decisions-phase3-outgrew.md) — the first real molecule (dexketoprofen, 33 atoms) exposed two Phase-1 decisions Phase 3 outgrew: `.screen.detail overflow:hidden` clipped the results screen, and the header energy read a 64 KB output tail that misses the final energy 164 KB back. Fixes: one scrolling layout; header energy from `results` (ADR-012) + a cross-check post-condition
- [008-frozen-bonds-drew-nothing.md](debugging/008-frozen-bonds-drew-nothing.md) — freezing bond topology (unit 3.13) drew atoms but NO bonds: `assignBonds:false` leaves `atom.index` unset and 3Dmol's stick gate is `atom.index < atom2.index`, so every cylinder was dropped. Fix: build once from equilibrium, update only coordinates. Lesson: the 3.13 test checked our input (bonded set) not the rendered output — 3.14 mirrors 3Dmol's draw gate
- [009-webkitgtk-png-export.md](debugging/009-webkitgtk-png-export.md) — unit-3.16 PNG-export gate under webkit2gtk-4.1: both paths PASS — recharts SVG→2D-canvas→PNG (`SVG_OK 6237`) and 3Dmol WebGL `pngURI()` readback (`PNG_OK 17388`); the `var(--…)` colour-resolution caveat; kill MiniBrowser by PID not `pkill -f`

---

*Page count: 59. Last structural update: 2026-08-01 (+architecture/adr-014-ai-integration-boundary.md and +architecture/ai-landscape.md — the AI integration boundary (T1/T2/T3 authority tiers, geometric constants retrieved-not-recalled) and the agentic-AI landscape register for the JOSS statement of need).*
