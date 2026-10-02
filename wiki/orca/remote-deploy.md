# Deploying ORCA 6.1.0 on a clean remote Linux server

**Purpose:** Phase 5 groundwork. Before the `SshBackend` (ADR-023) can run a calculation on a
remote host, ORCA itself must be brought up on that host. This page records the **deployment
procedure** that stood ORCA up on a fresh cloud box, so the future backend work starts from a known
sequence rather than rediscovering it. It is the *deployment* recipe — NOT the backend design (the
`SshBackend` prober targets are separate and **not yet measured**; see the caveat at the bottom).

---

## ⚠️ Source honesty (read before trusting any step)

This procedure was carried out in **one live session** on a rented **Hetzner CPX62** (16 shared AMD
vCPU, 32 GB RAM, Ubuntu 24.04) that **has since been destroyed**. The box is gone — nothing here can
be re-verified with a command right now. This page is therefore a **"procedure that worked once,
reconstructed from memory of the steps taken"**, NOT a run-verified specification in the sense of
domain rule #10. Where a step's exact form is uncertain it is flagged inline. Treat the whole page as
a strong starting hypothesis to be **re-probed on the real target server** (the university cluster)
before it is baked into `SshBackend` — do not promote any line here to "verified" without a fresh run.

The load-bearing *facts* it rests on (rule #2 version match, rule #1 absolute path, RPATH behaviour,
OpenMPI's root refusal) are each independently corroborated elsewhere in the wiki or by ORCA's own
tooling; the *sequence and the exact commands* are the memory-based part.

---

## Procedure (in order)

### 1. Build dependencies

```
apt install build-essential gfortran
```

Needed to compile OpenMPI from source (step 2). `gfortran` in particular — OpenMPI's Fortran
bindings are part of what ORCA's MPI launch touches.

### 2. OpenMPI — build from source, version must EXACTLY match the ORCA build (rule #2)

This is the single most failure-prone step and the one domain rule #2 exists for. The distro package
(`apt install openmpi-bin`) installs the **wrong version** — for our ORCA 6.1.0 build the required
OpenMPI is **4.1.6**, and Ubuntu's package is not it. Build 4.1.6 from source:

```
./configure --prefix=<install-prefix>
make -j<N>
make install
```

Then put `<install-prefix>/bin` on `$PATH`. **Check before proceeding:**

```
mpirun --version        # must print exactly: mpirun (Open MPI) 4.1.6
```

If this shows anything other than 4.1.6, STOP — ORCA's OpenMPI parallelization will silently fail
(rule #2, and the `wiki/orca/orca-basics.md` MPI notes). The exact version to build is dictated by
the ORCA build you are deploying; 4.1.6 is correct for **this** build, not a universal constant.

> Uncertain: the exact `./configure` flags used beyond `--prefix`. A plain prefix-only configure
> matched what the running system expected in the session; a real re-deploy should confirm no extra
> flag (e.g. a specific fabric/PMIx option) is needed on the target.

### 3. ORCA — rsync a working `/opt/orca/` from an existing machine (do NOT re-download)

Rather than fetching the ~20 GB distribution from the ORCA portal again, `rsync` the already-working
tree from a machine that has it:

```
rsync -a /opt/orca/ <remote>:/opt/orca/
```

Keep the destination path **identical to the source** (`/opt/orca`). Two facts confirmed in the
session made this clean:

- **RPATH inside the ORCA binaries is absolute**, so each binary finds its own `lib/` without help —
  **`LD_LIBRARY_PATH` is NOT required**. Only `PATH=/opt/orca` is needed on the remote (this mirrors
  what ORCA's own `setup`/environment script does). This is why keeping the path identical matters:
  an absolute RPATH is baked to `/opt/orca/...`, so moving the tree elsewhere would break it.
- ~20 GB transfers are the bulk of deploy time; rsync's incremental behaviour also makes a re-sync
  cheap if the tree changes.

Domain rule #1 still holds unchanged: ORCA is always invoked by its **full absolute path**
(`/opt/orca/orca input.inp`), never bare, or OpenMPI binding silently fails.

### 4. A non-root user is MANDATORY

OpenMPI **refuses to launch under root**:

```
mpirun has detected an attempt to run as root ...
```

So ORCA-under-MPI cannot run as root either. Create a dedicated unprivileged user, give it `$PATH`
(ORCA + the source-built OpenMPI), and run all calculations as that user. This is also the right
shape for `SshBackend`: the backend should SSH in as an ordinary user.

> The `OMPI_ALLOW_RUN_AS_ROOT` / `OMPI_ALLOW_RUN_AS_ROOT_CONFIRM` escape hatch exists, but for a
> backend the **correct** answer is a non-root user, not the override. Do not design the backend
> around running as root.

### 5. Probe #0 — a known-good job before any production run (rule #10)

Before trusting the deployment, run a **known-good reference** and check it in our own terms
(rule #9 post-condition, rule #10 verify-from-a-run). Use the small water `r2SCAN-3c` Opt+Freq that
already has an established reference, and confirm all three of:

1. `ORCA TERMINATED NORMALLY` in the output (+ the `.exit_code` marker — rule #6).
2. Final energy matches the reference: **≈ −76.4189 Ha** for the water r2SCAN-3c job.
3. `%pal nprocs` **actually parallelizes** — i.e. MPI really engaged. This is the real proof that
   the OpenMPI build matches the ORCA build (rule #2); a mismatched MPI often still "runs" on 1 core.

Only after Probe #0 passes is the host fit for real jobs.

---

## Performance reference (one session, planning aid — NOT a guarantee)

A single measured data point, useful for backend memory/time budgeting, not a benchmark:

| Job | CAM-B3LYP / ma-def2-TZVP / RIJCOSX / CPCM, full TD-DFT, 25 roots |
|---|---|
| System | 44-atom cation |
| Host | 16 shared AMD vCPU (Hetzner CPX62), 32 GB RAM |
| Wall-clock | ~9.5 min |
| Peak RSS | ~1.5 GB |

**Consequence for backend design:** jobs of this class are **cheap on memory** — 1.5 GB peak against
32 GB is nowhere near memory-bound. The bottleneck is **CPU/time, not RAM**. So a remote scheduler
can treat such jobs as CPU-limited (queue depth by cores/time), and need not gate on memory headroom
for the common single-molecule TD-DFT case. (Larger systems / bigger bases will shift this — one
point, not a curve.)

---

## What this page deliberately does NOT cover

- **`SshBackend` prober targets** — remote exit-code propagation, `ControlMaster` connection-reuse
  latency, `rsync` transfer behaviour for job dirs, etc. — are **not measured**. The temporary box
  was destroyed before those were probed; they must be settled on the **permanent target server**
  (rule #10) when the backend is actually built. Do not invent them from this page. This page is
  about getting ORCA *onto* a remote host, not about how the backend talks to it.
- The scientific work run on the box (the study itself) is a lab result, not part of this repo.

---

## Cross-references

- ADR-023: `wiki/architecture/adr-023-server-agnostic-remote-execution.md` — the `ServerProfile` +
  server-agnostic remote execution design this deployment feeds.
- `wiki/orca/remote-server-probe-commands.md` — the unit-5.1 connection-test that a `ServerProfile`
  runs against a host like this one (ORCA path / OpenMPI version / core count).
- `wiki/orca/orca-basics.md` — rule #1 (absolute path) and the MPI/environment notes.
- `wiki/orca/performance.md` — the dev-machine scaling probe and the taskset-mask methodology
  (rule #8); the perf point above is a remote complement, not a substitute.
- Domain rules #1, #2, #6, #9, #10 in `CLAUDE.md`.
