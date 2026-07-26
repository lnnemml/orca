# OrcaStudio

An integrated desktop environment for the [ORCA](https://www.faccts.de/orca/) quantum
chemistry package. Linux-first (developed on Linux Mint). Built with Tauri, React, Rust,
and a Python chemistry sidecar.

## Why

ORCA is one of the most powerful quantum chemistry packages available — and free for
academic use. But its power is atomized across dozens of command-line tools, a 1300-page
manual, and a terminal-only workflow. The barrier to entry is not the chemistry;
it's the tooling.

OrcaStudio wraps the full workflow into one place:

- **Build** — import structures (xyz, SMILES → 3D), manage a molecule library
- **Configure** — form-based input builder with a real editor underneath, template library
- **Run** — locally or on a remote server over SSH, with live SCF/optimization monitoring
- **Understand** — integrated ORCA manual with context-sensitive help right in the editor
- **Analyze** — orbitals, IR/UV-Vis spectra, normal modes, trajectories, all parsed
  automatically and stored in a local database that doubles as a lab journal

The goal: a chemist who has never opened a terminal should be able to run and understand
a DFT calculation. And a chemist learning quantum chemistry should learn *faster* because
the app shows what the numbers mean as they appear.

## Status

Early development. See [ROADMAP.md](ROADMAP.md) for the phased plan and current progress.

## Architecture (short version)

```
┌─────────────────────────────────────────────┐
│  React + TS (UI): editor, 3D viewer, plots  │
│  3Dmol.js · Monaco · recharts               │
├─────────────────────────────────────────────┤
│  Tauri / Rust core: job lifecycle, process  │
│  spawn, log tailing, SSH/rsync, SQLite      │
├─────────────────────────────────────────────┤
│  Python sidecar (FastAPI, localhost):       │
│  cclib parsing · RDKit · manual index       │
└─────────────────────────────────────────────┘
              │ spawns / monitors
              ▼
     ORCA (local or remote over SSH)
```

Full architecture and decision records: [`wiki/architecture/`](wiki/architecture/overview.md).

## Development

This project is developed with Claude Code and maintains an LLM-wiki
(see [`CLAUDE.md`](CLAUDE.md) and [`wiki/`](wiki/index.md)) — a compounding knowledge base
of architecture decisions, ORCA domain knowledge, and quantum chemistry study notes.

## Licensing notes

OrcaStudio does not bundle or redistribute ORCA or its documentation. Users install ORCA
themselves (free academic license via FAccTs) and point the app to the install path.
The manual index is built locally on the user's machine for personal use.
