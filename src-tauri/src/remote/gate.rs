//! The per-host cap on concurrent ssh/rsync processes (ADR-024 o24.2–o24.4).
//!
//! sshd drops new connections past its `MaxStartups` (B4 M6: 15 at once lose 2–4 with rc 255), so
//! the app never runs more than `n` ssh/rsync processes against one host at a time. A [`Gate`] is
//! one **fair queue per host key**: a caller takes the next ticket `t` of its key and is admitted
//! **iff `t < released + n`**, where `released` counts the permits given back. `released` moves
//! only on a release (never on admission) and every release wakes all waiters, so the predicate is
//! monotone, admits a FIFO prefix of the tickets, loses no wakeup, and never lets a later arrival
//! overtake an earlier waiter.
//!
//! A [`Permit`] is an RAII guard: dropping it — on return, on an early `?`, while a panic unwinds —
//! gives the slot back. A key's entry is removed only when every ticket it handed out has been
//! released (`next_ticket == released`), so the map holds only hosts with calls in flight.
//!
//! The host key is read from the production argv shapes in one place ([`host_key`]); an argv
//! with no recognisable host shares one fallback key, so nothing runs ungated.
//!
//! The gate is in-process: it starts empty on every launch and does not count ssh/rsync orphaned
//! by a previous run (o18). Mutex poisoning is recovered (`into_inner`): the state is two counters
//! per key, each changed by a single statement under the lock, so a panic elsewhere cannot leave it
//! half-updated — and a permit must still be released while a panic unwinds.

use std::collections::HashMap;
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};

use super::ssh::SSH_PROGRAM;
use super::sync::RSYNC_PROGRAM;

/// At most this many ssh/rsync processes per host at a time (ADR-024 o24.2): room under sshd's
/// default `MaxStartups` 10 and `MaxSessions` 10 (both inferred for uni) for the user's own
/// sessions.
pub const MAX_CONCURRENT_REMOTE_CALLS: usize = 8;

/// The process-global gate every ssh and rsync goes through: `SystemRunner::run`
/// (`remote::ssh`), the one production spawn site of both — poller steps, commands and the
/// connection test share it, with no reserved permit (o24.3).
pub static REMOTE_CALLS: LazyLock<Gate> = LazyLock::new(|| Gate::new(MAX_CONCURRENT_REMOTE_CALLS));

/// What the gate counts by: the host string a process connects to, or the one shared key for an
/// argv whose host is not recognisable (gated, never ungated — o24.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HostKey {
    Host(String),
    Fallback,
}

/// The host a production argv connects to (o24.2):
/// - `ssh … -- <host> bash -s` ([`super::ssh::ssh_bash_argv`]): the argument right after `--`;
/// - `rsync … <local>/ <host>:<remote>/` or `rsync … <host>:<remote>/ <local>/`
///   ([`super::sync::upload_argv`], [`super::sync::download_argv`]): the `host:` prefix of the one
///   remote argument among the last two (the local end is always absolute, so it starts with `/`).
///
/// Anything else is [`HostKey::Fallback`].
pub fn host_key(program: &str, args: &[String]) -> HostKey {
    let host = if program == SSH_PROGRAM {
        args.iter().position(|a| a == "--").and_then(|i| args.get(i + 1)).map(String::as_str)
    } else if program == RSYNC_PROGRAM {
        let ends = &args[args.len().saturating_sub(2)..];
        let mut remote = ends.iter().filter(|a| !a.starts_with('/')).filter_map(|a| a.split_once(':'));
        match (remote.next(), remote.next()) {
            (Some((host, _)), None) => Some(host),
            _ => None,
        }
    } else {
        None
    };
    match host {
        Some(h) if !h.is_empty() && !h.starts_with('-') => HostKey::Host(h.to_string()),
        _ => HostKey::Fallback,
    }
}

/// One key's queue: tickets handed out, and permits given back.
#[derive(Debug, Default)]
struct Queue {
    next_ticket: u64,
    released: u64,
}

/// A fair per-key cap of `n` concurrent holders.
#[derive(Debug)]
pub struct Gate {
    n: u64,
    queues: Mutex<HashMap<HostKey, Queue>>,
    wake: Condvar,
}

/// One admitted call's slot in its key's queue, given back on drop.
#[derive(Debug)]
pub struct Permit<'g> {
    gate: &'g Gate,
    key: HostKey,
}

impl Gate {
    /// A gate admitting at most `n` holders per key at a time. `n = 0` would admit nobody and
    /// stall every call, so it is raised to 1.
    pub fn new(n: usize) -> Gate {
        Gate { n: n.max(1) as u64, queues: Mutex::new(HashMap::new()), wake: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<HostKey, Queue>> {
        self.queues.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Take the next ticket of `key` and block until it is admitted (`ticket < released + n`).
    pub fn acquire(&self, key: HostKey) -> Permit<'_> {
        let mut queues = self.lock();
        let queue = queues.entry(key.clone()).or_default();
        let ticket = queue.next_ticket;
        queue.next_ticket += 1;
        // The entry cannot be removed while this ticket is outstanding (released <= ticket <
        // next_ticket), so it is there on every wakeup.
        while queues.get(&key).is_some_and(|q| ticket >= q.released + self.n) {
            queues = self.wake.wait(queues).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        Permit { gate: self, key }
    }

    /// How many callers of `key` hold a ticket but are not admitted yet: the forced-interleaving
    /// tests wait on it (o24.10).
    #[cfg(test)]
    pub(crate) fn waiting(&self, key: &HostKey) -> u64 {
        self.lock().get(key).map_or(0, |q| q.next_ticket.saturating_sub(q.released + self.n))
    }

    /// How many callers of `key` are admitted and have not released yet.
    #[cfg(test)]
    pub(crate) fn holders(&self, key: &HostKey) -> u64 {
        self.lock().get(key).map_or(0, |q| q.next_ticket.min(q.released + self.n) - q.released)
    }

    /// Whether `key` has an entry (a ticket not yet released).
    #[cfg(test)]
    fn has_entry(&self, key: &HostKey) -> bool {
        self.lock().contains_key(key)
    }

    fn release(&self, key: &HostKey) {
        let mut queues = self.lock();
        if let Some(queue) = queues.get_mut(key) {
            queue.released += 1;
            if queue.released == queue.next_ticket {
                queues.remove(key);
            }
        }
        drop(queues);
        self.wake.notify_all();
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.gate.release(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::remote::ssh::ssh_bash_argv;
    use crate::execution_backend::FetchPolicy;
    use crate::remote::sync::{download_argv, upload_argv};

    const BOUND: Duration = Duration::from_secs(5);

    fn host(h: &str) -> HostKey {
        HostKey::Host(h.into())
    }

    /// A latch the holders park on until the test opens it (no sleeps decide the outcome).
    #[derive(Default)]
    struct Latch {
        open: Mutex<bool>,
        cv: Condvar,
    }

    impl Latch {
        fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.cv.wait(open).unwrap();
            }
        }

        fn open(&self) {
            *self.open.lock().unwrap() = true;
            self.cv.notify_all();
        }
    }

    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // o24.10: with n = 8 and 12 callers on one key the overlap REACHES exactly 8 and never exceeds
    // it, and all 12 get through. NEGATIVE CONTROLS: `Gate::new(9)` here → 9 hold at once, red;
    // `Gate::new(1)` → the overlap never reaches 8, red (the assertion is on the literal 8).
    #[test]
    fn eight_of_twelve_callers_hold_at_once_and_never_nine() {
        let gate = Arc::new(Gate::new(8));
        let latch = Arc::new(Latch::default());
        let (current, max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (done_tx, done) = mpsc::channel();
        for _ in 0..12 {
            let (gate, latch, current, max, done_tx) =
                (gate.clone(), latch.clone(), current.clone(), max.clone(), done_tx.clone());
            std::thread::spawn(move || {
                let permit = gate.acquire(host("uni"));
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                latch.wait();
                current.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
                done_tx.send(()).unwrap();
            });
        }
        // Every caller is either holding (parked on the latch) or queued: the split is the cap.
        wait_until("all 12 placed", || current.load(Ordering::SeqCst) as u64 + gate.waiting(&host("uni")) == 12);
        assert_eq!(current.load(Ordering::SeqCst), 8, "exactly 8 hold at once while they are parked");
        assert_eq!(gate.waiting(&host("uni")), 4);
        latch.open();
        for i in 0..12 {
            done.recv_timeout(BOUND).unwrap_or_else(|_| panic!("caller {i} of 12 never finished"));
        }
        assert_eq!(max.load(Ordering::SeqCst), 8, "the overlap reached 8 and never more");
        assert!(!gate.has_entry(&host("uni")), "the key's entry is dropped once every ticket is released");
    }

    // o24.10, FIFO under a forced interleaving: holder H (n = 1) has A queued behind it; H releases
    // and at once asks again, and must queue behind A. NEGATIVE CONTROL: a plain counting gate
    // (admit iff holders < n, decrement + notify on release) lets H straight back in → red. Run on
    // 500 fresh gates, each bounded, so a scheduler-dependent control is reliably red.
    #[test]
    fn a_releaser_that_asks_again_queues_behind_the_waiter() {
        for round in 0..500 {
            let gate = Arc::new(Gate::new(1));
            let order = Arc::new(Mutex::new(Vec::new()));
            // H: holds the one permit, then on `go` releases it and asks again at once.
            let (h_held_tx, h_held) = mpsc::channel();
            let (go_tx, go) = mpsc::channel::<()>();
            let (h_tx, h_done) = mpsc::channel();
            {
                let (gate, order) = (gate.clone(), order.clone());
                std::thread::spawn(move || {
                    let held = gate.acquire(host("uni"));
                    h_held_tx.send(()).unwrap();
                    go.recv().unwrap();
                    drop(held);
                    let _again = gate.acquire(host("uni"));
                    order.lock().unwrap().push("H");
                    h_tx.send(()).unwrap();
                });
            }
            h_held.recv_timeout(BOUND).unwrap_or_else(|_| panic!("round {round}: H never admitted"));
            let (a_tx, a_done) = mpsc::channel();
            {
                let (gate, order) = (gate.clone(), order.clone());
                std::thread::spawn(move || {
                    let _permit = gate.acquire(host("uni"));
                    order.lock().unwrap().push("A");
                    a_tx.send(()).unwrap();
                });
            }
            wait_until("A queued", || gate.waiting(&host("uni")) == 1);
            go_tx.send(()).unwrap();
            a_done.recv_timeout(BOUND).unwrap_or_else(|_| panic!("round {round}: A never admitted"));
            h_done.recv_timeout(BOUND).unwrap_or_else(|_| panic!("round {round}: H never re-admitted"));
            assert_eq!(*order.lock().unwrap(), ["A", "H"], "round {round}: H overtook the waiter A");
        }
    }

    // o24.10: two releases before either of two waiters runs admit both, with no third release.
    // NEGATIVE CONTROL: a head-of-line `serving` counter advanced on admission with no notify on
    // admission → the second waiter can sleep through its turn → red within the bound. 500 fresh
    // gates, each bounded.
    #[test]
    fn two_releases_admit_two_waiters_without_a_third() {
        for round in 0..500 {
            let gate = Arc::new(Gate::new(2));
            let (h1, h2) = (gate.acquire(host("uni")), gate.acquire(host("uni")));
            let (tx, admitted) = mpsc::channel();
            let latch = Arc::new(Latch::default());
            for name in ["A", "B"] {
                let (gate, tx, latch) = (gate.clone(), tx.clone(), latch.clone());
                std::thread::spawn(move || {
                    let _permit = gate.acquire(host("uni"));
                    tx.send(name).unwrap();
                    latch.wait(); // hold, so no third release can happen
                });
            }
            wait_until("both queued", || gate.waiting(&host("uni")) == 2);
            drop(h1);
            drop(h2);
            for _ in 0..2 {
                admitted.recv_timeout(BOUND).unwrap_or_else(|_| panic!("round {round}: a waiter slept through"));
            }
            latch.open();
        }
    }

    // o24.10: two keys never block each other. NEGATIVE CONTROL: one shared key (`acquire` keying
    // every call as `HostKey::Fallback`) → the second host waits on the first → red.
    #[test]
    fn two_hosts_do_not_block_each_other() {
        let gate = Arc::new(Gate::new(1));
        let _uni = gate.acquire(host("uni"));
        let (tx, admitted) = mpsc::channel();
        let g = gate.clone();
        std::thread::spawn(move || {
            let _other = g.acquire(host("other"));
            tx.send(()).unwrap();
        });
        admitted.recv_timeout(BOUND).expect("a second host is admitted while the first is full");
    }

    // Every key shares one condvar, so a release must wake ALL waiters: with `notify_one` the one
    // woken can be another key's waiter that still cannot go (it waited first), and this key's
    // waiter sleeps forever. Forced order: B (key "b", full) waits on the condvar first, then A
    // (key "a", full); "a" is released. NEGATIVE CONTROL: `notify_one` in `release` → A is never
    // woken → red within the bound (Linux wakes futex waiters of equal priority in FIFO order).
    #[test]
    fn a_release_wakes_its_own_keys_waiter_when_another_keys_waiter_sleeps_first() {
        for round in 0..50 {
            let gate = Arc::new(Gate::new(1));
            let (held_a, _held_b) = (gate.acquire(host("a")), gate.acquire(host("b")));
            let (tx, admitted) = mpsc::channel();
            let latch = Arc::new(Latch::default());
            for name in ["b", "a"] {
                let (g, tx, latch) = (gate.clone(), tx.clone(), latch.clone());
                std::thread::spawn(move || {
                    let _permit = g.acquire(host(name));
                    tx.send(name).unwrap();
                    latch.wait();
                });
                // Queued (ticket taken and on the condvar: one lock hold) before the next starts.
                wait_until("queued", || gate.waiting(&host(name)) == 1);
            }
            drop(held_a);
            let first = admitted.recv_timeout(BOUND).unwrap_or_else(|_| panic!("round {round}: the waiter of the released key slept"));
            assert_eq!(first, "a", "round {round}");
            latch.open();
        }
    }

    #[test]
    fn a_zero_cap_is_raised_to_one() {
        let gate = Gate::new(0);
        drop(gate.acquire(HostKey::Fallback));
        assert!(!gate.has_entry(&HostKey::Fallback));
    }

    // The key is the production argv's host. NEGATIVE CONTROL: take the argument BEFORE `--`, or
    // the local end of an rsync, and the assertions below go red.
    #[test]
    fn the_host_key_comes_from_all_three_production_argv_shapes() {
        let policy = FetchPolicy { include_gbw: false };
        let cases = [
            ("ssh", ssh_bash_argv("uni").unwrap(), host("uni")),
            ("ssh", ssh_bash_argv("anton@uni.example").unwrap(), host("anton@uni.example")),
            ("rsync", upload_argv("/home/a/job", "uni", "/srv/r/jobs/j").unwrap(), host("uni")),
            ("rsync", download_argv("anton@uni", "/srv/r/jobs/j", "/home/a/job", policy).unwrap(), host("anton@uni")),
        ];
        for (program, argv, want) in cases {
            assert_eq!(host_key(program, &argv), want, "{program} {argv:?}");
        }
        // Not a production shape: gated under the one fallback key, never ungated.
        for (program, argv) in [
            ("sh", vec!["-c".to_string(), "true".into()]),
            ("ssh", vec!["uni".to_string(), "true".into()]),
            ("rsync", vec!["/a/".to_string(), "/b/".into()]),
            ("rsync", vec!["x:/a/".to_string(), "y:/b/".into()]),
        ] {
            assert_eq!(host_key(program, &argv), HostKey::Fallback, "{program} {argv:?}");
        }
    }
}
