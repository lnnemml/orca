//! The poller's `AppHandle` shell (unit 5.3 B2 Part B): the event sink, the loop thread, and the
//! poller the commands use. Everything it calls is the `AppHandle`-free core ([`super::run`],
//! [`super::Poller`], [`super::log_step`]); this file only sleeps, spawns and emits.
//!
//! **The loop** ([`start`]) is one named thread started at the end of `lib.rs` setup, after
//! `DbState`, `InFlight`, `LiveLog` and `PollerMemory` are managed; it resumes every non-terminal
//! remote job at launch (ADR-024 o item 4: the first tick finds every status step due). Every
//! [`TICK`] it runs [`super::run::tick`] — one database query when there is nothing to poll — and
//! hands each due step to **its own short-lived thread**. A pool would add a queue in front of steps
//! that are already bounded: the in-progress mark allows one step per job, and there are as many
//! steps as non-terminal remote jobs (each profile has one slot). The loop thread itself never waits
//! for a step, so a 300 s download blocks nothing but its own job.
//!
//! **On exit** the loop thread and the step threads are not joined: the process ends and takes them
//! with it, so exit never waits for a step. An ssh or rsync child that a step started runs in its
//! own process group (`SystemRunner`) and is left to the OS, exactly as for the remote commands of
//! B1; whatever it was doing is redone from the server's facts on the next launch (a download is
//! verified against the server's listing before it is used).

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::live_log::{Applied, LiveLog};
use super::plan::{Periods, PollerMemory};
use super::run::{run_step, tick, Planned, StepRan};
use super::{LogStep, Poller, PollerSink};
use crate::commands::settings::DbState;
use crate::convergence::ConvergenceEvent;
use crate::execution_backend::FetchPolicy;
use crate::in_flight::InFlight;
use crate::local_backend::{emit_convergence, emit_log, emit_status};
use crate::models::job::JobStatus;
use crate::remote::ssh::SystemRunner;

/// How often the loop plans. Short enough for the 2 s log period (a log poll starts at most this
/// late) and for catch-up (the next chunk follows within it).
pub const TICK: Duration = Duration::from_millis(500);

/// The event that tells a view to drop its live log before a re-stream (ADR-024 o16.3). B3's
/// listener binds to this exact name.
pub const LOG_RESET_EVENT: &str = "job:log-reset";

/// Payload of `job:log-reset` (ADR-024 o16.3): drop every line and convergence point held for the
/// job; the stream restarts from offset 0.
#[derive(Clone, Serialize)]
pub(crate) struct LogResetPayload {
    pub(crate) job_id: String,
}

/// The poller's events through the app: exactly the local `job:log`, `job:convergence` and
/// `job:status` payloads (their one emitters), plus `job:log-reset`.
pub struct AppSink(pub AppHandle);

impl PollerSink for AppSink {
    fn log(&self, job_id: &str, mut lines: Vec<String>) {
        emit_log(&self.0, job_id, &mut lines);
    }
    fn convergence(&self, job_id: &str, mut events: Vec<ConvergenceEvent>) {
        emit_convergence(&self.0, job_id, &mut events);
    }
    fn log_reset(&self, job_id: &str) {
        let _ = self.0.emit(LOG_RESET_EVENT, LogResetPayload { job_id: job_id.to_string() });
    }
    fn status(&self, job_id: &str, status: JobStatus) {
        emit_status(&self.0, job_id, status);
    }
}

/// Run `f` with the app's poller: the real ssh/rsync runner, the shared filter without `.gbw` (no
/// per-job opt-in exists yet), the app's managed state and an [`AppSink`].
pub fn with_poller<T>(app: &AppHandle, f: impl FnOnce(&Poller<'_>) -> T) -> T {
    let db = app.state::<DbState>();
    let in_flight = app.state::<InFlight>();
    let memory = app.state::<PollerMemory>();
    let live = app.state::<LiveLog>();
    let sink = AppSink(app.clone());
    let poller = Poller {
        db: &db,
        runner: &SystemRunner,
        in_flight: &in_flight,
        memory: &memory,
        live: &live,
        sink: &sink,
        policy: FetchPolicy::SMALL_ONLY,
    };
    f(&poller)
}

/// Start the loop thread. Call once, after the poller's state is managed.
pub fn start(app: AppHandle) -> std::io::Result<()> {
    std::thread::Builder::new().name("remote-poller".into()).spawn(move || loop {
        let planned = {
            let db = app.state::<DbState>();
            let live = app.state::<LiveLog>();
            let memory = app.state::<PollerMemory>();
            tick(&db, &live, &memory, &Periods::INITIAL, Instant::now())
        };
        match planned {
            Ok(steps) => steps.into_iter().for_each(|step| spawn_step(&app, step)),
            Err(e) => eprintln!("[OrcaStudio] remote poller: tick failed: {e}"),
        }
        std::thread::sleep(TICK);
    })?;
    Ok(())
}

/// Run one planned step on its own thread. If the thread cannot be started, the step's
/// in-progress mark is cleared here, so the job is planned again on a later tick.
fn spawn_step(app: &AppHandle, planned: Planned) {
    let job_id = planned.job_id.clone();
    let worker = app.clone();
    let spawned = std::thread::Builder::new().name(format!("poll-{job_id}")).spawn(move || {
        // A failed check or poll is routine while the host is away and is not logged; a database
        // error and a chunk the live log refused (a reader's post-condition) are.
        match with_poller(&worker, |poller| run_step(poller, &planned)) {
            StepRan::Status(Err(e)) => eprintln!("[OrcaStudio] remote poller: status step of job {}: {e}", planned.job_id),
            StepRan::Log(LogStep::Applied(Applied::Refused(why))) => {
                eprintln!("[OrcaStudio] remote poller: log chunk of job {} refused: {why}", planned.job_id)
            }
            _ => {}
        }
    });
    if let Err(e) = spawned {
        eprintln!("[OrcaStudio] remote poller: could not start a step of job {job_id}: {e}");
        app.state::<PollerMemory>().end_step(&job_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body of the item starting at `signature` in this file, up to the first line that closes
    /// it at `indent`.
    fn body<'a>(src: &'a str, signature: &str, close: &str) -> &'a str {
        let start = src.find(signature).unwrap_or_else(|| panic!("{signature} exists"));
        &src[start..start + src[start..].find(close).expect("it ends")]
    }

    /// The B3 wire contract of `job:log-reset`: the name and `{ "job_id": … }` exactly.
    /// NEGATIVE CONTROL: `LOG_RESET_EVENT = "job:log_reset"` → red.
    #[test]
    fn the_log_reset_event_and_its_payload() {
        assert_eq!(LOG_RESET_EVENT, "job:log-reset");
        assert_eq!(
            serde_json::to_value(LogResetPayload { job_id: "j1".into() }).unwrap(),
            serde_json::json!({ "job_id": "j1" })
        );
    }

    /// Each `AppSink` method goes to the one local emitter of its event (the exact local payload),
    /// and `log_reset` to `LOG_RESET_EVENT` — `AppHandle`-bound, so pinned in the source.
    /// NEGATIVE CONTROL: `log_reset` emitting the literal `"job:log_reset"` → red.
    #[test]
    fn the_app_sink_maps_each_event_to_its_local_emitter() {
        let src = include_str!("app.rs");
        let sink = body(src, concat!("impl PollerSink ", "for AppSink"), "\n}\n");
        for (method, emitter) in [
            ("fn log(", "emit_log(&self.0, job_id, &mut lines)"),
            ("fn convergence(", "emit_convergence(&self.0, job_id, &mut events)"),
            ("fn status(", "emit_status(&self.0, job_id, status)"),
            ("fn log_reset(", "self.0.emit(LOG_RESET_EVENT, LogResetPayload {"),
        ] {
            let m = body(sink, method, "\n    }");
            assert!(m.contains(emitter), "{method} goes to `{emitter}`");
            assert_eq!(m.matches("emit").count(), 1, "{method}: one emit");
        }
    }

    /// The shell is `AppHandle`-bound, so its two pairing rules are pinned in the source: a step
    /// runs only through `run_step` (which ends it on every path), and a step whose thread could not
    /// start is ended here. NEGATIVE CONTROL: drop the `end_step` from the spawn-failure branch → red.
    #[test]
    fn every_handed_out_step_is_ended() {
        let src = include_str!("app.rs");
        let start = src.find(concat!("fn ", "spawn_step(")).expect("spawn_step exists");
        let body = &src[start..start + src[start..].find("\n}\n").expect("spawn_step ends")];
        assert!(body.contains(concat!("run_", "step(poller, &planned)")), "a step runs through run_step");
        let failed = body.find("if let Err(e) = spawned").expect("the spawn failure is handled");
        assert!(body[failed..].contains(concat!(".end_", "step(&job_id)")), "a step that could not start is ended");
        let loop_start = src.find(concat!("pub fn ", "start(")).expect("start exists");
        let loop_body = &src[loop_start..loop_start + src[loop_start..].find("\n}\n").expect("start ends")];
        assert!(loop_body.contains(concat!("spawn_", "step(&app, step)")), "the loop hands every step to spawn_step");
        assert!(!loop_body.contains(concat!("run_", "step(")), "the loop thread never runs a step itself");
    }
}
