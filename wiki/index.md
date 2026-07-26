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

## Modules

- [frontend.md](modules/frontend.md) — React UI: screens, state, key components
- [tauri-core.md](modules/tauri-core.md) — Rust core: commands, events, job state machine
- [sidecar.md](modules/sidecar.md) — Python service: endpoints, dependencies
- [execution-backends.md](modules/execution-backends.md) — LocalBackend / SshBackend details
- [parser.md](modules/parser.md) — result extraction: streaming regex + cclib
- [visualization.md](modules/visualization.md) — 3Dmol.js integration, cubes, spectra
- [manual-index.md](modules/manual-index.md) — manual indexing pipeline + keyword map

## ORCA domain knowledge

- [orca-basics.md](orca/orca-basics.md) — installation, invocation, MPI, environment
- [input-format.md](orca/input-format.md) — anatomy of an .inp file
- [output-files.md](orca/output-files.md) — what ORCA produces and what each file is for
- [gotchas.md](orca/gotchas.md) — accumulated traps and their solutions

## Chemistry (навчальні нотатки, українською)

- [README.md](chemistry/README.md) — що тут живе і як вести нотатки

## Debugging

- [README.md](debugging/README.md) — format for solved-bug pages
- [001-monaco-offline-worker-resolve.md](debugging/001-monaco-offline-worker-resolve.md) — Monaco worker unresolved in Vite build (exports-map double-mapping) + CDN→bundled fix

---

*Page count: 19. Last structural update: 2026-07-26 (Phase 1.2: +debugging/001-monaco-offline-worker-resolve).*
