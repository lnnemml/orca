# ADR-023: Server-agnostic remote execution via server profiles

**Status:** accepted · 2026-08-27

## Context
Phase 5 adds remote execution. Remote execution is **not for one machine**: the author runs
jobs on a university server today and may add other servers (a lab box, an HPC allocation, a
collaborator's node) later. A full mechanism study is 300–800 jobs, and which machine hosts
them is an operational choice, not a fixed fact. The design must therefore not hardcode any
host, path, or core layout.

This refines ADR-003 (the `ExecutionBackend` trait) and rides on ADR-005 (system `ssh`/`rsync`).

## Decision
1. **One `SshBackend`, parameterized by a `ServerProfile`** — host alias (reusing
   `~/.ssh/config`, per ADR-005), remote ORCA absolute path (rule #1), remote scratch dir,
   core-pinning mask (rule #8). **Not a type per server.** Any number of servers are the same
   code with different profile data.
2. **`enum Backend { Local(LocalBackend), Ssh(SshBackend) }`, static dispatch.** Per-job
   selection is a `match` on `jobs.backend_id` (a later Phase 5 migration). `enum` over
   `Box<dyn>`: the backend set is closed and owned by us at compile time, and a `match` on
   `backend_id` **is** the runtime "Run on:" selector — no dynamic polymorphism is needed to
   choose per job.
3. **Server profiles are runtime config** in settings/SQLite (a `server_profiles` table),
   user-managed: add / edit / connection-test / delete. The app stores **no credentials** —
   a profile is a host alias plus paths; auth stays with the user's SSH setup (ADR-005).
4. **Per-server specs are established per profile by a rule-#10 measurement**, not assumed.
   A **connection-test** verifies the profile in our terms (rule #9 post-condition) before it
   is usable: resolve the remote ORCA path, confirm the remote OpenMPI version (rule #2), read
   `nproc`/topology for the taskset probe (rule #8). A profile that has not passed the test is
   not offered as a run target.

## Rationale
- **Universality is the requirement.** A study may span machines; hardcoding a host would mean
  a code change per server. Profiles make "add a server" a settings action, not a build.
- **`enum` over `dyn`.** The set is closed (`Local`, `Ssh`, later `Slurm`) and we ship every
  variant. `enum` imposes **no object-safety corset** on the still-maturing `poll_log(offset)`
  / `fetch_results(policy)` signatures — their real shape is forced by `SshBackend`, so keeping
  signature freedom until then is worth more than literal `dyn` fidelity to ADR-003's selector.
  `enum ↔ dyn` is a local change in the dispatch layer if that judgement ever flips.
- **Specs as data, not code.** Rule #10 forbids accepting a third-party fact from memory; each
  server is its own set of measured facts. They belong in per-profile rows established once per
  server via the connection-test, never baked into the binary.

## Consequences
- **Unit 5.0 (the trait extraction) is unaffected.** Trait methods take `&self`; per-server
  config lives in the `SshBackend` instance, so the trait signatures do not change. 5.0 stays a
  pure, behaviour-preserving local extraction — no profiles, no `enum`, no `backend_id` yet.
  The `enum` and the `dyn`-vs-`enum` call both land in the `SshBackend` unit, where they matter.
- **New persistence, in a later Phase 5 unit:** a `server_profiles` table + `jobs.backend_id`
  (default `'local'`). Not 5.0.
- **Each new server is a live gate.** Adding a profile requires a real SSH session the user
  authorizes; the connection-test measures and records the profile's specs. The university
  server is simply the **first** profile — its specs remain UNDETERMINED until that gate runs
  (see the prober finding, 2026-08-27: no server spec is recorded anywhere yet; `~/.ssh/config`
  holds only `github.com`). The eventual measurement is homed under `wiki/orca/` (a
  `remote-server.md` sibling of `performance.md`), one section per profiled host.
- **Extends, does not supersede.** ADR-003 still owns the trait; ADR-005 still owns the
  transport. This ADR names how `SshBackend` is parameterized and how servers are configured.
