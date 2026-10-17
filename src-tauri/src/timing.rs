//! Opt-in timing of a locked section (B4 measurement M4, `wiki/architecture/remote-live-run-b4.md`).
//!
//! **Off by default.** The gate is the environment variable `ORCASTUDIO_TIMING`, read once per
//! launch; it is on only for the value `1`. Off, a [`SectionTimer`] holds no `Instant`, reads no
//! clock and produces no line: the instrumented code pays one cached bool check per section.
//!
//! **It must not perturb what it measures.** Under the lock a timer only reads the monotonic clock
//! and stores integers — no I/O, no lock. The line is formatted and printed by the caller *after*
//! the guard is dropped ([`SectionTimer::print`]); `live_log.rs` pins that order in a source test.
//!
//! One stable, grep-able line per section:
//! `[timing] live_log.apply job=<id> lines=N conv=M lock_wait_us=… hold_us=… emit_us=…`
//! - `lock_wait`: from [`SectionTimer::start`] (before `lock()`) until [`SectionTimer::locked`];
//! - `hold`: from `locked` until [`SectionTimer::released`] (called right after the guard's drop);
//! - `emit`: the sum of every [`SectionTimer::time_emit`] call in the section.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The variable that turns the timing on.
pub const ENV_VAR: &str = "ORCASTUDIO_TIMING";

/// Whether a value of [`ENV_VAR`] turns the timing on: only exactly `1`.
pub fn gate_from_env_value(value: Option<&str>) -> bool {
    value == Some("1")
}

/// The gate, read once per launch.
pub fn enabled() -> bool {
    static GATE: OnceLock<bool> = OnceLock::new();
    *GATE.get_or_init(|| gate_from_env_value(std::env::var(ENV_VAR).ok().as_deref()))
}

/// The three durations of one section, in the units the line prints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sample {
    pub lock_wait: Duration,
    pub hold: Duration,
    pub emit: Duration,
}

/// The stable output line for one section.
pub fn format_line(label: &str, job_id: &str, lines: usize, conv: usize, sample: &Sample) -> String {
    format!(
        "[timing] {label} job={job_id} lines={lines} conv={conv} lock_wait_us={} hold_us={} emit_us={}",
        sample.lock_wait.as_micros(),
        sample.hold.as_micros(),
        sample.emit.as_micros()
    )
}

/// Times one locked section. Create it **before** `lock()`; see the module docs for the call order.
pub struct SectionTimer {
    /// `Some` only when the gate is on: set before `lock()`.
    started: Option<Instant>,
    locked_at: Option<Instant>,
    sample: Sample,
    lines: usize,
    conv: usize,
}

impl SectionTimer {
    /// A timer under the process-wide gate ([`enabled`]).
    pub fn start() -> Self {
        Self::with_gate(enabled())
    }

    /// A timer under an explicit gate — what [`SectionTimer::start`] calls, and what tests inject.
    pub fn with_gate(enabled: bool) -> Self {
        Self {
            started: enabled.then(Instant::now),
            locked_at: None,
            sample: Sample::default(),
            lines: 0,
            conv: 0,
        }
    }

    /// The guard was just obtained.
    pub fn locked(&mut self) {
        if let Some(started) = self.started {
            let now = Instant::now();
            self.sample.lock_wait = now - started;
            self.locked_at = Some(now);
        }
    }

    /// Run one emit call and add its duration to `emit`. Off, this is just `f()`.
    pub fn time_emit<R>(&mut self, f: impl FnOnce() -> R) -> R {
        if self.started.is_none() {
            return f();
        }
        let before = Instant::now();
        let out = f();
        self.sample.emit += before.elapsed();
        out
    }

    /// Record how many lines and convergence events the section emitted (plain integers, safe under
    /// the lock).
    pub fn counts(&mut self, lines: usize, conv: usize) {
        self.lines = lines;
        self.conv = conv;
    }

    /// The guard was just dropped.
    pub fn released(&mut self) {
        if let Some(locked_at) = self.locked_at {
            self.sample.hold = locked_at.elapsed();
        }
    }

    /// The line for this section; `None` when the gate is off.
    pub fn report(&self, label: &str, job_id: &str) -> Option<String> {
        self.started?;
        Some(format_line(label, job_id, self.lines, self.conv, &self.sample))
    }

    /// Print the section's line to stderr, if the gate is on. Call only after the guard is dropped.
    pub fn print(&self, label: &str, job_id: &str) {
        if let Some(line) = self.report(label, job_id) {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_value_1_turns_the_gate_on() {
        assert!(gate_from_env_value(Some("1")));
        for off in [None, Some(""), Some("0"), Some("true"), Some("yes"), Some(" 1"), Some("11")] {
            assert!(!gate_from_env_value(off), "{off:?} must leave the timing off");
        }
    }

    #[test]
    fn the_line_is_stable_and_carries_injected_values() {
        let sample = Sample {
            lock_wait: Duration::from_micros(12),
            hold: Duration::from_micros(3456),
            emit: Duration::from_micros(789),
        };
        assert_eq!(
            format_line("live_log.apply", "job-1", 5, 2, &sample),
            "[timing] live_log.apply job=job-1 lines=5 conv=2 lock_wait_us=12 hold_us=3456 emit_us=789"
        );
    }

    /// NEGATIVE CONTROL: invert the gate in `with_gate` (`!enabled`) and this goes red.
    #[test]
    fn with_the_gate_off_nothing_is_produced_and_the_emit_still_runs() {
        let mut t = SectionTimer::with_gate(false);
        t.locked();
        let ran = t.time_emit(|| 7);
        t.counts(3, 1);
        t.released();
        assert_eq!(ran, 7, "the emit runs whether or not the timing is on");
        assert_eq!(t.report("live_log.apply", "j"), None);
        assert_eq!(t.sample, Sample::default(), "no clock was read");
    }

    #[test]
    fn with_the_gate_on_the_three_durations_and_counts_are_recorded() {
        let mut t = SectionTimer::with_gate(true);
        std::thread::sleep(Duration::from_millis(2));
        t.locked();
        t.time_emit(|| std::thread::sleep(Duration::from_millis(3)));
        t.time_emit(|| std::thread::sleep(Duration::from_millis(3)));
        t.counts(4, 1);
        t.released();
        assert!(t.sample.lock_wait >= Duration::from_millis(2));
        assert!(t.sample.emit >= Duration::from_millis(6), "both emit calls add up");
        assert!(t.sample.hold >= t.sample.emit, "the hold contains the emit");
        let line = t.report("live_log.finish", "j").expect("on");
        assert!(line.starts_with("[timing] live_log.finish job=j lines=4 conv=1 lock_wait_us="), "{line}");
    }
}
