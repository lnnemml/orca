# Measured: a SQLite `COMMIT` waits its own busy timeout (rollback journal)

**Measured 2026-11-21** on the dev machine, with the app's own bundled SQLite. Rule #10: how long
one `BEGIN IMMEDIATE` + `COMMIT` attempt can wait on another connection is a third-party fact,
accepted only from a run. This page records the probe behind the bound on step C's retry in
[ADR-027](adr-027-mcp-orchestration-server.md) (M3 amendment, "The 60 s exit backstop"), the
module page's "Step C's retry" ([mcp-server.md](../modules/mcp-server.md)) and the code
(`STEP_C_RETRY_WAITS` in `src-tauri/src/mcp/create_job.rs`).

## The question

Under OrcaStudio's SQLite (rollback journal, `busy_timeout` 5000 ms), does a `COMMIT` of a
`BEGIN IMMEDIATE` transaction wait its **own full busy timeout** while another connection holds a
read transaction? If so, one step C attempt can wait up to two busy timeouts (one at `BEGIN`, one at
`COMMIT`), not one.

## Which SQLite

`sqlite_version()` = **3.46.0** (source id `2024-05-23 13:25:27 96c92aba…`), `THREADSAFE=1`, no
compile-time busy/lock override. The probe crate pinned `rusqlite =0.32.1` with `bundled`
(`libsqlite3-sys 0.30.1`), with the repo's `Cargo.lock` copied in. Every connection printed
`busy_timeout=5000 journal_mode=delete` (the WAL control: `wal`). The app sets no `journal_mode`
pragma and no `busy_timeout`: both are the defaults, and a test reads them
(`create_job_tests::step_c_retry_fits_inside_the_exit_wait`).

## Method

A throwaway crate (not in the repo): `/tmp/claude-1000/-home-laptop-Documents-orca-studio/3301de98-2bff-422a-b676-33cab28c8639/scratchpad/sqlite-commit-probe/`
(`src/main.rs`, logs `run1.log` … `run4-s3-hold6000.log`).

```
S=<the scratch dir above>
cp /home/laptop/Documents/orca-studio/Cargo.lock $S/Cargo.lock
cd $S && CARGO_TARGET_DIR=$S/target cargo build --offline
PROBE_DIR=/dev/shm/sqlprobe ./target/debug/sqlite-commit-probe all      # S1, S2, S3, S5, S6 on tmpfs
S3_HOLD=4900 PROBE_DIR=/dev/shm/sqlprobe ./target/debug/sqlite-commit-probe s3
S3_HOLD=6000 PROBE_DIR=/dev/shm/sqlprobe ./target/debug/sqlite-commit-probe s3
```

The clean rows below are on tmpfs: SQLite's locking is POSIX-lock based, so it is the same as on
disk minus fsync noise (see "Caveats"). The essential code, condensed:

```rust
// equivalent of mcp/grant.rs::begin_immediate
fn begin_immediate(c: &Connection) -> Result<Transaction<'_>, Error> {
    Transaction::new_unchecked(c, TransactionBehavior::Immediate)
}
// writer: BEGIN IMMEDIATE, INSERT, COMMIT, each timed separately
let t0 = Instant::now();            let tx = begin_immediate(&w)?;        // BEGIN ms
let t1 = Instant::now();            tx.execute("INSERT INTO t(v) VALUES ('W')", [])?;
let t2 = Instant::now();            let r = tx.commit();                  // COMMIT ms
// reader (another connection, same file): a read transaction that keeps SHARED
r.execute_batch("BEGIN")?;
let _: String = r.query_row("SELECT v FROM t LIMIT 1", [], |x| x.get(0))?;   // then sleep(hold)
r.execute_batch("ROLLBACK")?;
// S3 adds a second writer W2 holding RESERVED (BEGIN IMMEDIATE) for S3_HOLD ms before it commits;
// a reader takes SHARED right after W2's commit, so W's BEGIN succeeds and its COMMIT then meets the reader.
```

## Results

| scenario | BEGIN ms | INSERT ms | COMMIT ms | outcome | row after the reader released |
|---|---|---|---|---|---|
| S1: reader holds 8 s (3 runs) | 0 | 0.1 | 5011 / 5011 / 5012 | COMMIT `Err` `DatabaseBusy` (code 3, ext 5) | **absent** |
| S2: reader holds 2 s (3 runs) | 0 | 0.0–0.1 | 2033 / 2032 / 2033 | COMMIT ok (the handler waits; it is not fail-fast) | present |
| S3: W2 holds RESERVED 4.0 s, reader then holds 8 s (3 runs) | 3939 | 0.2 | 5010–5011 | COMMIT busy; total ≈ 8950 | absent |
| S3: W2 holds 4.9 s (3 runs) | 4838 / 4842 / 4841 | 0.1–0.2 | 5010 / 5011 / 5010 | COMMIT busy; total **9848–9853** | absent |
| S3: W2 holds 6.0 s | 5014 | – | – | `BEGIN` busy alone (the txn never starts) | n/a |

- **The two waits are independent budgets.** One attempt measured BEGIN 4840 ms (waiting on another
  writer) + COMMIT 5010 ms = 9850 ms. BEGIN alone is capped at ≈ 5 s (6 s writer → BEGIN busy at 5014 ms).
- **The INSERT inside never waited** (≤ 0.3 ms, small transaction).
- The BEGIN wait is not a fixed 5 s: it ends when the other writer commits (3939 / 4838 ms), so BEGIN
  contributes min(holder's remaining time, 5 s). That is why the worst case per attempt is ≈ 10 s,
  and the achieved 3 × 9.85 + 1.25 = 30.8 s is within ≈ 0.5 s of the nominal figure below.
- In every S3 row the reader acquired SHARED before W's BEGIN returned (`reader_won_race=true`), the
  intended interleaving.

## Negative control and scaling

| scenario | COMMIT ms | outcome |
|---|---|---|
| S5: same as S1 under `journal_mode=WAL` (reader holds 8 s, 3 runs) | 0 / 0 / 0 | COMMIT ok at once — the reader is not waited for |
| S6: `busy_timeout` 300 ms, reader holds 2 s (3 runs) | 302 / 302 / 301 | COMMIT busy |

WAL turns the ≈ 5010 ms wait into 0 ms, so the probe distinguishes the mechanism: the wait is the
rollback journal's EXCLUSIVE-lock wait at `COMMIT`. S6 shows the wait scales with `busy_timeout`,
which is what the fast regression test uses.

## Verdict and the consequence for step C

**Yes.** Under the rollback journal a `COMMIT` waits its own full busy timeout while a reader holds
SHARED, then fails `SQLITE_BUSY` ("database is locked") and the transaction is rolled back (the row
is absent). Per attempt, SQLite lock waits are at most ≈ **2 × busy timeout** (nominal 10 s; ≈ 10.03 s with the
measured ~10–15 ms overshoot per wait).

Step C makes at most three attempts, after waits of 250 ms and 1 s:

> ≈ **31.25 s** of SQLite lock waits (nominal 3 × (5 s + 5 s) + 0.25 s + 1 s)

What was measured is each wait: a `COMMIT` busy at ≈ 5010–5014 ms, a `BEGIN` capped at ≈ 5014 ms, and
`BEGIN` + `COMMIT` in one attempt ≈ 9850–9859 ms. The three-attempt total is **arithmetic derived from
those measured waits**, not one run. Each wait overshoots its nominal 5 s by ~10–15 ms, so the strict
sum is ≈ 31.3 s.

The factor 2 assumes no statement *inside* the transaction waits: a mid-transaction cache spill would
need EXCLUSIVE and could wait on a reader. Step C's transaction is a few rows (`finish_in_tx` plus one
audit row), far below the cache, and the probe's INSERT took ≤ 0.3 ms.

Exit budget: nominal 31.25 s +
`terminate_on_exit` ≈ 14.5 s worst + sidecar stop ≈ 0.3 s ≈ **46 s**, inside the 60 s backstop
(`server::wiring::EXIT_FLOW_DEADLINE`), leaving ≈ **14 s** for step B and the `DbState` mutex waits.

## Caveats

- **Lock waits, not wall time.** The busy handler bounds *lock* waits only. Time in fsync or journal
  I/O comes on top and is unbounded. The first run on the real NVMe, with the machine at 88 % iowait
  from other work, gave a `COMMIT` with **no reader at all** of 741 / 14143 / 2891 ms, a WAL COMMIT of
  2178 ms, and a 300 ms-timeout COMMIT failing after 2480 ms (`run1.log`). The ≈ 31.25 s figure covers
  SQLite lock waits, not how long step C takes on a loaded disk.
- **Not the in-process `DbState` mutex.** The wait for that mutex before each attempt is no SQLite
  wait and no timeout bounds it.
- **Drop-rollback only.** After a failed COMMIT the row was absent in every run (checked from a fresh
  connection after the reader released), because rusqlite's `Transaction` drop rolled it back. The
  writer connection was dropped at once, so whether the transaction is still open on that connection
  between the failed COMMIT and the drop was **not isolated** (UNDETERMINED). The app relies on the
  drop-rollback, which was observed.
- One host, one SQLite build; the numbers are from tmpfs for the clean rows.

## What would invalidate it

Re-run the probe if **rusqlite / `libsqlite3-sys` is bumped** (a different bundled SQLite) or the app
sets a **`journal_mode`** (WAL removes the COMMIT wait; the bound would then be lower, but the two-waits
reasoning in the code and the tests must be redone). Two tests turn red first:

- `create_job_tests::step_c_retry_fits_inside_the_exit_wait` reads `journal_mode` (`delete`) and
  `busy_timeout` (5000) off the app's connection and pins the nominal `31_250` ms;
- `create_job_tests::a_commit_waits_its_own_busy_timeout_under_the_rollback_journal` reproduces S6 on
  the bundled SQLite (300 ms timeout, a reader holding SHARED, COMMIT busy, row absent).
