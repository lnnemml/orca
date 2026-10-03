//! The d′ race model (ADR-024 d′, l): the wrapper and the cancel script race on two markers,
//! and in no interleaving may ORCA run under `.cancelled` without being killed.
//!
//! - Wrapper: write `.started`, then check `.cancelled` — it runs ORCA iff its check saw no
//!   `.cancelled`.
//! - Cancel: write `.cancelled`, then check `.started` — it kills the job iff its check saw
//!   `.started`.
//!
//! Each script's two steps keep their order; the model enumerates every merge of the two
//! sequences (C(4,2) = 6) and checks the invariant. Pure, no processes. The real scripts are
//! run in both sequential orders by Part B.

#![cfg(test)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    WrapperWritesStarted,
    WrapperChecksCancelled,
    CancelWritesCancelled,
    CancelChecksStarted,
}

/// The wrapper's order (ADR-024 l start sequence: 1 `.started` → 2 `.cancelled` check).
const WRAPPER_STEPS: [Step; 2] = [Step::WrapperWritesStarted, Step::WrapperChecksCancelled];
/// The cancel script's order (step 1 writes `.cancelled`, then it looks at `.started`).
const CANCEL_STEPS: [Step; 2] = [Step::CancelWritesCancelled, Step::CancelChecksStarted];

#[derive(Debug, Default)]
struct World {
    started: bool,
    cancelled: bool,
    orca_runs: bool,
    killed: bool,
}

fn run(schedule: &[Step]) -> World {
    let mut world = World::default();
    for step in schedule {
        match step {
            Step::WrapperWritesStarted => world.started = true,
            Step::WrapperChecksCancelled => world.orca_runs = !world.cancelled,
            Step::CancelWritesCancelled => world.cancelled = true,
            Step::CancelChecksStarted => world.killed = world.started,
        }
    }
    world
}

/// Every merge of `a` and `b` that keeps each one's own order.
fn interleavings(a: &[Step], b: &[Step]) -> Vec<Vec<Step>> {
    match (a.split_first(), b.split_first()) {
        (None, _) => vec![b.to_vec()],
        (_, None) => vec![a.to_vec()],
        (Some((a0, a_rest)), Some((b0, b_rest))) => {
            let mut all = Vec::new();
            for mut tail in interleavings(a_rest, b) {
                tail.insert(0, *a0);
                all.push(tail);
            }
            for mut tail in interleavings(a, b_rest) {
                tail.insert(0, *b0);
                all.push(tail);
            }
            all
        }
    }
}

#[test]
fn orca_never_runs_unkilled_under_cancelled() {
    let schedules = interleavings(&WRAPPER_STEPS, &CANCEL_STEPS);
    // Guard against a vacuous enumeration: exactly the 6 distinct merges.
    assert_eq!(schedules.len(), 6);
    for (i, s) in schedules.iter().enumerate() {
        assert!(!schedules[..i].contains(s), "duplicate schedule {s:?}");
    }

    for schedule in schedules {
        let world = run(&schedule);
        assert!(
            !(world.orca_runs && !world.killed),
            "ORCA runs under .cancelled and is never killed: {schedule:?} -> {world:?}"
        );
    }
}
