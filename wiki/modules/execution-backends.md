# Module: Execution backends (src-tauri/src/backends/)

**Status:** not started · Trait defined in ADR-003

## LocalBackend (Phase 1)
- Job dir: `<data>/jobs/<job_id>/`
- Spawn: full ORCA path, stdout+stderr → `output.out`, then `echo $? > .exit_code`
  (wrap in a tiny runner script so the pattern matches remote exactly)
- Log: poll file by byte offset every 250ms (notify crate optional later)

## SshBackend (Phase 5)
- Upload: `rsync -az <job_dir>/ <host>:<scratch>/<job_id>/`
- Launch: `ssh <host> 'cd <dir> && nohup bash run.sh > /dev/null 2>&1 & echo $! > .pid'`
  where run.sh = orca invocation + `.exit_code` writer
- Poll: `ssh <host> "tail -c +<offset> <dir>/output.out"` every 5–10s; offset persisted
  in SQLite so polling resumes across app restarts
- Fetch: `rsync` back per FetchPolicy (output/xyz/hess always, gbw opt-in, cubes on demand)
- Cube generation remotely: `ssh <host> 'cd <dir> && <orca_bin_dir>/orca_plot ...'`

## Invariants (both backends)
- Completion = `.exit_code` present AND "ORCA TERMINATED NORMALLY" in output tail.
- Reconciliation on startup for any non-terminal job state.
- Concurrency 1 per backend by default.
