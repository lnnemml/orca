# Agentic-AI landscape over computational chemistry

**What this page is.** A register of *agentic AI layers over computational chemistry* — systems
that take a scientific goal in natural language and drive quantum-chemistry / atomistic engines to
answer it. It is the sibling of [prior-art.md](prior-art.md): that page covers **builders / viewers /
geometry**; this one covers **the AI orchestration layer**. It exists in particular so a JOSS /
SoftwareX *statement of need* can be assembled from a standing record — reconstructing the field
after the fact is more expensive than keeping one line current.

**Discipline of this page (rule #10).** Every entry carries a **URL + date**. Anything not confirmed
by a source or a run is written as `unknown`, never guessed, or is parked in **To verify** as a bare
pointer. Fields are uniform across entries so the comparison sections below read off the columns
(Compute / Access / Drives what) rather than re-narrating each one.

Each entry has the same fields, in this order:
**Who / when** · **Drives what** · **Interface** · **Compute** · **Access** · **Delta vs OrcaStudio**.

---

## Bunsen (Schrödinger)

- **Who / when** — Schrödinger, Inc. Early-access introduced **2026-07-27**; full release expected
  later in 2026.
  ([press release](https://ir.schrodinger.com/press-releases/news-details/2026/Schrdinger-Introduces-Bunsen-an-AI-Co-Scientist-for-Molecular-Discovery/default.aspx),
  [product page](https://www.schrodinger.com/platform/products/bunsen/))
- **Drives what** — Schrödinger's **validated physics-based methods** for large-scale molecular
  discovery: the 2026 release domain is **drug discovery** (MacroDock, ESOL, FEP+, lipid bilayers,
  HMR, RetroSynth, GPCR / LiveDesign). The **QM engine Jaguar does not appear** in this release.
  Positioned explicitly as *executing validated physical methods*, unlike general AI systems that
  operate over information retrieval.
- **Interface** — `unknown` (natural-language scientific goal; part of the Schrödinger platform;
  specific UI not stated in the sources above).
- **Compute** — **cloud cluster**: a co-engineered NVIDIA + Google Cloud stack (GCP compute,
  NVIDIA BioNeMo Agent Toolkit, RTX PRO 6000 Blackwell).
- **Access** — **closed early access**; full release later 2026.
- **Delta vs OrcaStudio** — orthogonal on all three axes: drug-discovery domain (not mechanistic
  organic QM), cloud cluster (not a laptop), closed EA (not a local app). Not a competitor; a
  confirmation — see *What it confirms*.

## El Agente (Aspuru-Guzik group) — the nearest existing analog

- **Drives what** — **ORCA (v6.0.1) is the main quantum-chemistry engine for all calculations**,
  alongside RDKit, OpenBabel, xTB and Architector. This is why El Agente is OrcaStudio's closest
  existing neighbour: *an LLM agent over ORCA, with autonomous file handling and submission*, is
  exactly the T3 shape of [ADR-014](adr-014-ai-integration-boundary.md).
- **Who / when** — Zou, Cheng, Aldossary, … Aspuru-Guzik. arXiv:2505.02484 (**2025-05-05**),
  published in *Matter*, **July 2025**.
  ([arXiv](https://arxiv.org/abs/2505.02484),
  [Matter](https://www.cell.com/matter/fulltext/S2590-2385(25)00306-6)) A 2026 follow-up appears in
  search — *El Agente Quntur*, [arXiv:2602.04850](https://arxiv.org/abs/2602.04850) — contents not
  verified here (see To verify).
- **Interface** — `unknown` (LLM multi-agent system with transparent action-trace logs; concrete
  front end not stated in the sources checked).
- **Compute** — `unknown` (not stated in the abstract; runs the engines above wherever they are
  installed).
- **Access** — `unknown` (code-availability / license not confirmed in the sources checked).
- **Delta vs OrcaStudio** — stated plainly, without softening: **"AI over ORCA" already exists.**
  OrcaStudio's difference is *not* the presence of an agent. It is that the agent sits over a
  **desktop application with a local, free academic ORCA**, over **manual reaction-center
  construction** (geometric control — distances, angles of attack, dihedrals) and a **learning
  layer** — and that the **authority boundary is fixed architecturally** ([ADR-014](adr-014-ai-integration-boundary.md)),
  not left to depend on model strength. If this page does not say that, a JOSS reviewer will.

## ChemGraph (Argonne)

- **Who / when** — Argonne National Laboratory (`argonne-lcf`). arXiv:2506.06363 (**2025-06-06**).
  ([arXiv](https://arxiv.org/abs/2506.06363), [GitHub](https://github.com/argonne-lcf/ChemGraph))
- **Drives what** — via **ASE**: **NWChem, ORCA, Psi4** (DFT / coupled-cluster), **xTB** (through
  TBLite), and ML potentials **MACE / UMA** (plus generic ASE calculators). Tasks: structure
  generation, single-point, geometry optimisation, vibrational analysis, thermochemistry.
- **Interface** — CLI (`chemgraph -q "…"`), Jupyter notebooks, and a **Streamlit web UI** with 3D
  visualisation. Built on LangGraph + ReAct.
- **Compute** — self-hosted (runs wherever the ASE backends are installed; HPC-capable).
- **Access** — **open source, Apache-2.0.**
- **Delta vs OrcaStudio** — also drives ORCA, but as a **research framework** (CLI / notebook /
  Streamlit) rather than a desktop reaction-mechanism workstation; no unbroken pick→run index space
  ([ADR-008](adr-008-scene-fragment-model.md)), no learning layer, and the authority boundary is not
  an architectural fixture.

## Aitomia (Dral group)

- **Who / when** — Pavlo O. Dral et al., Xiamen University. arXiv:2505.08195 (**2025-05-13**),
  ChemRxiv (doi:10.26434/chemrxiv-2025-gnf13); publicly online since **2025-05-11**.
  ([arXiv](https://arxiv.org/abs/2505.08195),
  [ChemRxiv](https://chemrxiv.org/engage/chemrxiv/article-details/687ee36e728bf9025e95174f))
- **Drives what** — the **MLatom** platform plus DFT, semiempirical **GFN2-xTB**, and
  wavefunction methods through interfaces to **Gaussian, ORCA, PySCF, xtb**. Covers geometry
  optimisation, thermochemistry, spectra, molecular dynamics, and reaction modeling.
- **Interface** — **web-based cloud** platform with chatbots + AI agents.
- **Compute** — **cloud** (public instances "Aitomistic Lab@XMU" and "Aitomistic Hub").
- **Access** — **free public cloud** access; source / pricing model `unknown` from the sources
  checked.
- **Delta vs OrcaStudio** — cloud web assistant over many backends; OrcaStudio is a **local desktop**
  app centred on **one engine (ORCA) in depth** with first-class geometric control and a learning
  layer.

---

## What this does NOT change for OrcaStudio

Read off the columns above, not re-narrated:

- **(a) Domain.** Bunsen's 2026 domain is **drug discovery**; OrcaStudio's is **mechanistic organic
  QM with transition-state search** ([ADR-007](adr-007-reaction-modeling.md)). Orthogonal. (The
  academic agents — El Agente, ChemGraph, Aitomia — *do* overlap on domain; they are separated on
  the other axes and by form factor, below.)
- **(b) Compute.** The cloud entrants (**Bunsen, Aitomia**) need a cluster; OrcaStudio runs on a
  **laptop with free academic ORCA**. This axis does **not** separate ChemGraph or El Agente, which
  are self-hostable — so compute alone is not the argument against those two.
- **(c) Access.** **Bunsen** is closed early access and **Aitomia** is cloud-gated; OrcaStudio is a
  **local desktop app**. This axis does **not** separate **ChemGraph** (Apache-2.0) — so access
  alone is not the argument against it either.
- **(d) The honest separation from the open academic agents.** "AI over ORCA" already exists
  (El Agente drives ORCA 6.0.1; ChemGraph drives it via ASE). OrcaStudio is not differentiated by
  *having* an agent. It is a **desktop reaction-mechanism workstation** — manual reaction-center
  construction, an unbroken pick→run index space ([ADR-008](adr-008-scene-fragment-model.md)), a
  learning layer — with the **agent's authority fixed in architecture** ([ADR-014](adr-014-ai-integration-boundary.md)),
  not left to the strength of whatever model is loaded. That is the statement of need.

## What it confirms

The largest company in the field builds an agent **on top of validated physics, not instead of it**
— Bunsen is positioned precisely as *executing validated methods*, and its QM path runs through
Schrödinger's own engines, not through the language model. The academic agents converge on the same
line independently: **El Agente drives ORCA**, **ChemGraph drives ORCA / NWChem / Psi4 via ASE**,
**Aitomia drives Gaussian / ORCA / PySCF / xtb** — in every case the LLM *orchestrates* a
deterministic engine and reads what it returns; none of them let the model *emit* the physics. That
is exactly the boundary [ADR-014](adr-014-ai-integration-boundary.md) fixes: the AI never inside the
numerical pipeline. Four independent groups drawing the same line is the strongest available evidence
that the line is the right one.

---

## To verify

- *El Agente Quntur* ([arXiv:2602.04850](https://arxiv.org/abs/2602.04850), 2026) — a 2026 "research
  collaborator agent" follow-up; contents, engines driven, and access model not yet read.
- El Agente (2025) — interface, compute location, and code license unconfirmed.
- Aitomia — whether any component is open source, and the pricing model of the public instances.
- Bunsen — concrete interface, and whether the QM engine (Jaguar) enters a later release.
