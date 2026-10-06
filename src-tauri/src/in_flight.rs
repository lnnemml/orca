//! The per-job in-flight guard (ADR-024 o item 4): **at most one remote operation per job at a
//! time** — submit, retry, withdraw and label today; the poller's collect and fetch in unit 5.3 B2.
//! Two operations on the same job must not interleave: a double-clicked Submit, a Retry while a
//! Withdraw is still talking to the server, a poller tick collecting a job a withdraw is
//! cancelling — each would race the server/database state machine of o item 3.
//!
//! The guard is **in memory only** and starts empty on every launch (o item 4): an operation that
//! was in flight when the app stopped is not "still running" after a restart; the label call and
//! the classifier decide such a job from the server's facts.
//!
//! Two shapes over one set: a command **refuses** a busy job ([`InFlight::acquire`], an
//! [`AppError`] naming the job), the poller **skips** it ([`InFlight::try_acquire`], `None`). The
//! returned [`InFlightGuard`] owns its slot: dropping it — on return, on an early `?`, or while a
//! panic unwinds — frees the job. It is `Send + 'static`, so a command moves it into the
//! `spawn_blocking` closure that does the ssh work and the job stays claimed until that work ends.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::AppError;

/// The set of job ids with an operation in flight. Managed app state; cheap to clone (an `Arc`).
#[derive(Debug, Clone, Default)]
pub struct InFlight(Arc<Mutex<HashSet<String>>>);

/// Holds one job's slot in [`InFlight`] until dropped.
#[derive(Debug)]
pub struct InFlightGuard {
    set: Arc<Mutex<HashSet<String>>>,
    job_id: String,
}

/// Lock the set, recovering it from a poisoned mutex: the set is only ever touched by one
/// `insert` or `remove` under the lock, so a panic elsewhere cannot leave it half-updated — and a
/// guard must still be able to free its job while a panic unwinds.
fn lock(set: &Mutex<HashSet<String>>) -> MutexGuard<'_, HashSet<String>> {
    set.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl InFlight {
    /// Claim `job_id`, or `None` when an operation on it is already in flight (the poller's
    /// shape: skip the job this tick).
    pub fn try_acquire(&self, job_id: &str) -> Option<InFlightGuard> {
        lock(&self.0)
            .insert(job_id.to_string())
            .then(|| InFlightGuard { set: Arc::clone(&self.0), job_id: job_id.to_string() })
    }

    /// Claim `job_id`, or refuse with an error naming it (a command's shape: the user asked for
    /// something that cannot run now).
    pub fn acquire(&self, job_id: &str) -> Result<InFlightGuard, AppError> {
        self.try_acquire(job_id).ok_or_else(|| {
            AppError::Conflict(format!("an operation on job {job_id} is already in progress; try again when it ends"))
        })
    }

    /// Whether an operation on `job_id` is in flight now.
    // Not routed yet: the poller (unit 5.3 B2) and the job list's busy state (B3) read it.
    #[allow(dead_code)]
    pub fn is_busy(&self, job_id: &str) -> bool {
        lock(&self.0).contains(job_id)
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        lock(&self.set).remove(&self.job_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_claim_on_the_same_job_is_refused_while_the_first_is_held() {
        let in_flight = InFlight::default();
        let first = in_flight.acquire("j1").unwrap();
        assert!(in_flight.try_acquire("j1").is_none(), "the poller skips a busy job");
        let refused = in_flight.acquire("j1").unwrap_err().to_string();
        assert!(refused.contains("already in progress") && refused.contains("j1"), "{refused}");
        assert!(in_flight.is_busy("j1"));
        drop(first);
        assert!(!in_flight.is_busy("j1"));
    }

    #[test]
    fn different_jobs_are_independent() {
        let in_flight = InFlight::default();
        let _a = in_flight.acquire("a").unwrap();
        let b = in_flight.acquire("b").expect("another job is not blocked");
        drop(b);
        assert!(in_flight.is_busy("a"), "freeing b leaves a claimed");
        assert!(in_flight.try_acquire("a").is_none());
    }

    /// NEGATIVE CONTROL target: make `Drop` not remove the id and every claim after a drop is
    /// refused — this test and the panic test below go red.
    #[test]
    fn dropping_the_guard_frees_the_job() {
        let in_flight = InFlight::default();
        for _ in 0..3 {
            let guard = in_flight.acquire("j1").expect("freed by the previous drop");
            drop(guard);
        }
        assert!(!in_flight.is_busy("j1"));
    }

    /// The guard frees its job while a panic unwinds (a bug in an operation must not wedge the
    /// job until the next launch), also when the guard was moved into another thread's closure —
    /// the shape of a command's `spawn_blocking`.
    #[test]
    fn a_panic_while_held_frees_the_job() {
        let in_flight = InFlight::default();
        let guard = in_flight.acquire("j1").unwrap();
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _held = guard;
            panic!("an operation panicked");
        }));
        assert!(unwound.is_err());
        assert!(!in_flight.is_busy("j1"), "freed during the unwind");

        let guard = in_flight.acquire("j1").unwrap();
        let joined = std::thread::spawn(move || {
            let _held = guard;
            panic!("a blocking task panicked");
        })
        .join();
        assert!(joined.is_err());
        assert!(in_flight.acquire("j1").is_ok(), "freed when the thread unwound");
    }

    /// One claim wins among threads racing for the same job.
    #[test]
    fn exactly_one_of_racing_claims_wins() {
        let in_flight = InFlight::default();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (in_flight, barrier) = (in_flight.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    barrier.wait();
                    let won = in_flight.try_acquire("j1");
                    barrier.wait(); // every thread has tried before any guard drops
                    won.is_some()
                })
            })
            .collect();
        let winners = handles.into_iter().map(|h| h.join().unwrap()).filter(|won| *won).count();
        assert_eq!(winners, 1);
        assert!(!in_flight.is_busy("j1"), "the winner's guard dropped with its thread");
    }
}
