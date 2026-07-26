# ADR-003: ExecutionBackend abstraction

**Status:** accepted · 2026-07-26

## Context
Calculations must run locally today, on a remote server over SSH soon (author's explicit
requirement), possibly under SLURM later. Remote execution must be optional per-job, and
the rest of the app must not care where a job ran.

## Decision
All execution goes through a Rust trait:

```rust
trait ExecutionBackend {
    fn submit(&self, job: &Job) -> Result<JobHandle>;
    fn poll_log(&self, h: &JobHandle, offset: u64) -> Result<LogChunk>;
    fn status(&self, h: &JobHandle) -> Result<JobStatus>;
    fn fetch_results(&self, h: &JobHandle, policy: FetchPolicy) -> Result<()>;
    fn cancel(&self, h: &JobHandle) -> Result<()>;
}
```

Implementations: `LocalBackend` (Phase 1), `SshBackend` (Phase 5), `SlurmBackend` (future).
The job row in SQLite stores `backend_id`; the UI exposes it as a "Run on:" selector.

## Rationale
- Forces clean seams from day one; retrofitting remote execution later would touch
  everything.
- Reconciliation-on-startup (re-checking marker files) is uniform across backends,
  which is what makes jobs survive laptop sleep, app restarts, and SSH drops.

## Consequences
- `poll_log` is offset-based (pull), not stream-based (push), so the same interface works
  for local files and remote tail-over-ssh. Local polling at 250ms feels live.
- `FetchPolicy` controls large-artifact download (gbw opt-in) — designed in now, used in
  Phase 5.
