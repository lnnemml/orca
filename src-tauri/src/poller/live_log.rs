//! The remote live log (ADR-024 o16): who watches which job, and each watched job's stream state.
//!
//! **Pushed, like a local job's.** A remote job's `poll_log` chunks become the local `job:log` and
//! `job:convergence` events (o16.1): per job one [`LineAssembler`] (raw bytes in, complete lines out,
//! a split UTF-8 character carried) and one [`ConvergenceParser`] fed line by line, as the local tail
//! does. One poll is one batch.
//!
//! **The state machine** (o16.2–16.6), all under one mutex, which is **never held across an ssh
//! call**:
//! - `open` counts per job id, whatever the backend (an open of a draft or a local job is counted
//!   too). Every open re-streams: a job that has state gets it recreated at offset 0 and the view is
//!   told `job:log-reset` first, in the same locked step.
//! - `close` decrements, saturating at 0; the last close drops the state. So do the job turning
//!   terminal and its row vanishing ([`LiveLog::retain`], [`LiveLog::drop_state`]) — the count is
//!   kept, so a lost close leaks a count, never a stream.
//! - **State is created by the first step of a watched job** ([`LiveLog::begin`]: a log poll, or the
//!   drain after a fetch). An open creates none, so an open of a terminal job creates no state and a
//!   watched draft gets state only once the poller polls it, i.e. once it has coordinates.
//! - **Generation.** Every state carries a value from one counter that never resets within a launch.
//!   An open's reset and a `reset` chunk give the state a fresh value; a drop removes it, and any
//!   later state takes a fresh one. A step records the generation when it begins
//!   ([`Ticket`]) and applies its chunk only if the state still has it — so a chunk read before an
//!   open, a reset or a drop is discarded, also across drop-and-reopen (no ABA: the counter only
//!   grows).
//! - **Catch-up:** a chunk of exactly the cap means more is waiting; the planner polls again
//!   without the 2 s wait.
//! - A failed poll never reaches [`LiveLog::apply`]: state and offset stay as they were.
//! - **The drain** ([`LiveLog::drain`], o16.6) reads the downloaded, hash-verified copy from the
//!   stored offset to its end, then the assembler's unterminated last line, each step
//!   generation-checked, then drops the state.

use std::collections::HashMap;
use std::io;
use std::sync::{Mutex, MutexGuard};

use super::PollerSink;
use crate::convergence::{ConvergenceEvent, ConvergenceParser};
use crate::execution_backend::{LineAssembler, LogChunk};

/// Every watched job's live-log state, and the open counts. Managed app state in Part B; one per
/// launch, so everything here starts empty on launch (o16.2, like o item 4's state).
#[derive(Default)]
pub struct LiveLog {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Open views per job id; absent = 0.
    open: HashMap<String, u32>,
    states: HashMap<String, JobLog>,
    /// The launch-long generation counter (o16.3). Only grows.
    counter: u64,
}

impl Inner {
    fn fresh(&mut self) -> JobLog {
        self.counter += 1;
        JobLog {
            generation: self.counter,
            offset: 0,
            assembler: LineAssembler::new(),
            parser: ConvergenceParser::new(),
            catch_up: false,
        }
    }
}

/// One watched job's stream: where the next read starts, the carry, the parser.
struct JobLog {
    generation: u64,
    /// The byte offset of the next read: everything before it has gone through `assembler`.
    offset: u64,
    assembler: LineAssembler,
    parser: ConvergenceParser,
    /// The last chunk was exactly the cap: more is already waiting.
    catch_up: bool,
}

/// What a step recorded when it began: the state's generation and the offset to read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    pub generation: u64,
    pub offset: u64,
}

/// What [`LiveLog::apply`] did with one chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The state was reset, dropped or recreated since the step began: the chunk is stale and
    /// nothing was emitted (the next step reads from the state's own offset).
    Discarded,
    /// The log shrank below the offset (o16.4): the state is recreated at 0 and `job:log-reset` was
    /// emitted.
    Reset,
    /// The chunk's complete lines and the convergence points they finish were emitted.
    Lines { lines: usize, events: usize, catch_up: bool },
    /// The chunk does not continue the ticket's offset (rule #9: the reader's own post-condition
    /// failed); nothing was applied.
    Refused(String),
}

/// How a drain ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drained {
    /// Nobody watches the job: nothing was read (o16.6: no watcher, nothing to emit).
    NotWatched,
    /// The whole copy and its unterminated last line were emitted; the state is dropped.
    Done,
}

impl LiveLog {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Every update under the lock is a whole-value replace or a counter step, so a panic
        // elsewhere cannot leave it half-done; recover rather than wedge every view.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A view of job `job_id` opened (o16.2–16.3): count it, and if the job has state, recreate it
    /// at offset 0 and emit `job:log-reset` — in one locked step, so no chunk of the old stream can
    /// follow the reset.
    pub fn open(&self, job_id: &str, sink: &dyn PollerSink) {
        let mut inner = self.lock();
        *inner.open.entry(job_id.to_string()).or_insert(0) += 1;
        if inner.states.contains_key(job_id) {
            let fresh = inner.fresh();
            inner.states.insert(job_id.to_string(), fresh);
            sink.log_reset(job_id);
        }
    }

    /// A view closed: decrement, saturating at 0 (a close with no matching open is a no-op). The
    /// last close drops the state.
    pub fn close(&self, job_id: &str) {
        let mut inner = self.lock();
        match inner.open.get(job_id).copied() {
            None | Some(0) => {}
            Some(1) => {
                inner.open.remove(job_id);
                inner.states.remove(job_id);
            }
            Some(n) => {
                inner.open.insert(job_id.to_string(), n - 1);
            }
        }
    }

    /// The open views of `job_id`.
    pub fn open_count(&self, job_id: &str) -> u32 {
        self.lock().open.get(job_id).copied().unwrap_or(0)
    }

    /// Whether the job's last chunk was exactly the cap (o16.3 catch-up).
    pub fn catch_up(&self, job_id: &str) -> bool {
        self.lock().states.get(job_id).is_some_and(|s| s.catch_up)
    }

    /// Whether the job has stream state now.
    #[cfg(test)]
    pub(crate) fn has_state(&self, job_id: &str) -> bool {
        self.lock().states.contains_key(job_id)
    }

    /// Drop the job's state, keeping its count: the job turned terminal or its row vanished.
    pub fn drop_state(&self, job_id: &str) {
        self.lock().states.remove(job_id);
    }

    /// Drop the state of every job `keep` rejects — the poller passes "is a non-terminal remote
    /// job"; counts are kept.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.lock().states.retain(|id, _| keep(id));
    }

    /// Begin a step for `job_id`: `None` when nobody watches it; else its ticket, creating the state
    /// at offset 0 if it has none (the first step of a watched job).
    pub fn begin(&self, job_id: &str) -> Option<Ticket> {
        let mut inner = self.lock();
        if inner.open.get(job_id).copied().unwrap_or(0) == 0 {
            return None;
        }
        if !inner.states.contains_key(job_id) {
            let fresh = inner.fresh();
            inner.states.insert(job_id.to_string(), fresh);
        }
        let state = &inner.states[job_id];
        Some(Ticket { generation: state.generation, offset: state.offset })
    }

    /// Apply one chunk read for `ticket` (o16.3): under the lock, re-check the generation — a stale
    /// chunk is discarded — then reset, or assemble, parse, advance the offset and emit, all in one
    /// locked step. `cap` is the read's cap: a chunk of exactly `cap` bytes asks for catch-up.
    pub fn apply(&self, job_id: &str, ticket: Ticket, chunk: &LogChunk, cap: u64, sink: &dyn PollerSink) -> Applied {
        let mut inner = self.lock();
        match inner.states.get(job_id) {
            Some(state) if state.generation == ticket.generation => {}
            _ => return Applied::Discarded,
        }
        if chunk.reset {
            let fresh = inner.fresh();
            inner.states.insert(job_id.to_string(), fresh);
            sink.log_reset(job_id);
            return Applied::Reset;
        }
        let read = chunk.bytes.len() as u64;
        if chunk.offset != ticket.offset + read {
            return Applied::Refused(format!(
                "a chunk of {read} bytes read from offset {} ends at {}, not {}",
                ticket.offset,
                chunk.offset,
                ticket.offset + read
            ));
        }
        let Some(state) = inner.states.get_mut(job_id) else {
            return Applied::Discarded;
        };
        let lines = state.assembler.push(chunk);
        let events: Vec<ConvergenceEvent> = lines.iter().filter_map(|line| state.parser.feed(line)).collect();
        state.offset = chunk.offset;
        state.catch_up = read == cap;
        let applied = Applied::Lines { lines: lines.len(), events: events.len(), catch_up: state.catch_up };
        emit(sink, job_id, lines, events);
        applied
    }

    /// End of the stream for `ticket`: emit the assembler's unterminated last line (and what it
    /// completes) and drop the state — if the generation is unchanged. `false`: the state moved on
    /// and nothing was done.
    pub fn finish(&self, job_id: &str, ticket: Ticket, sink: &dyn PollerSink) -> bool {
        let mut inner = self.lock();
        let Some(state) = inner.states.get_mut(job_id).filter(|s| s.generation == ticket.generation) else {
            return false;
        };
        let lines: Vec<String> = state.assembler.take_partial().into_iter().collect();
        let events: Vec<ConvergenceEvent> = lines.iter().filter_map(|line| state.parser.feed(line)).collect();
        emit(sink, job_id, lines, events);
        inner.states.remove(job_id);
        true
    }

    /// Drain the job's stream from a **local, verified** copy of its log (o16.6), before the
    /// terminal `job:status`: `read(offset)` from the stored offset until it returns no bytes, then
    /// [`LiveLog::finish`]; each step generation-checked like a poll (an open meanwhile restarts the
    /// stream at 0 from the same copy). Runs only for a watched job. A read error ends the drain
    /// with the state left as it was.
    pub fn drain(
        &self,
        job_id: &str,
        read: &mut dyn FnMut(u64) -> io::Result<LogChunk>,
        cap: u64,
        sink: &dyn PollerSink,
    ) -> io::Result<Drained> {
        loop {
            let Some(ticket) = self.begin(job_id) else {
                return Ok(Drained::NotWatched);
            };
            let chunk = read(ticket.offset)?;
            if chunk.bytes.is_empty() && !chunk.reset {
                if self.finish(job_id, ticket, sink) {
                    return Ok(Drained::Done);
                }
                continue;
            }
            if let Applied::Refused(why) = self.apply(job_id, ticket, &chunk, cap, sink) {
                return Err(io::Error::other(why));
            }
        }
    }

    /// Whether the mutex is free right now — the tests' check that no step holds it across a call.
    #[cfg(test)]
    pub(crate) fn is_unlocked(&self) -> bool {
        self.inner.try_lock().is_ok()
    }

    /// Whether the mutex is held right now — the recording sink's check that every log, convergence
    /// and reset event is emitted inside the locked step that decided it (o16.3).
    #[cfg(test)]
    pub(crate) fn is_locked(&self) -> bool {
        matches!(self.inner.try_lock(), Err(std::sync::TryLockError::WouldBlock))
    }
}

/// One step's events, through the sink (each a no-op when empty, as the local emitters are).
fn emit(sink: &dyn PollerSink, job_id: &str, lines: Vec<String>, events: Vec<ConvergenceEvent>) {
    if !lines.is_empty() {
        sink.log(job_id, lines);
    }
    if !events.is_empty() {
        sink.convergence(job_id, events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poller::{Event, RecordingSink};

    fn chunk(offset: u64, bytes: &[u8]) -> LogChunk {
        LogChunk { offset: offset + bytes.len() as u64, bytes: bytes.to_vec(), reset: false }
    }

    /// One poll's worth: begin, then apply `bytes` read from the ticket's offset.
    fn poll(live: &LiveLog, sink: &RecordingSink, bytes: &[u8]) -> Applied {
        let t = live.begin("j").expect("watched");
        live.apply("j", t, &chunk(t.offset, bytes), 1 << 20, sink)
    }

    /// o16.8: open, open, close keeps polling; a close at 0 stays at 0; the last close drops the
    /// state. NEGATIVE CONTROL: make `close` drop the state on every close and the second `begin`
    /// returns a fresh ticket at offset 0 → red.
    #[test]
    fn open_open_close_keeps_the_stream_and_a_close_at_zero_stays_zero() {
        let (live, sink) = RecordingSink::with_live();
        live.close("j");
        assert_eq!(live.open_count("j"), 0, "a close with no open is a no-op");
        live.open("j", &sink);
        live.open("j", &sink);
        assert_eq!(poll(&live, &sink, b"a\n"), Applied::Lines { lines: 1, events: 0, catch_up: false });
        live.close("j");
        assert_eq!(live.open_count("j"), 1);
        assert_eq!(live.begin("j").map(|t| t.offset), Some(2), "still streaming from where it was");
        live.close("j");
        assert_eq!(live.open_count("j"), 0);
        assert!(!live.has_state("j"), "the last close drops the state");
        assert_eq!(live.begin("j"), None, "and nothing is polled");
        live.close("j");
        assert_eq!(live.open_count("j"), 0, "saturates at 0");
    }

    /// o16.8: a second open emits one reset and the next poll asks for offset 0. An open with no
    /// state emits nothing (no view has lines to drop). NEGATIVE CONTROL: skip the state reset in
    /// `open` and the next ticket's offset is 2, not 0 → red.
    #[test]
    fn a_second_open_resets_once_and_the_next_poll_reads_from_zero() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        assert!(sink.take().is_empty(), "an open without state emits nothing");
        poll(&live, &sink, b"a\n");
        sink.take();
        live.open("j", &sink);
        assert_eq!(sink.take(), [Event::Reset("j".into())], "exactly one reset");
        assert_eq!(live.begin("j").map(|t| t.offset), Some(0));
    }

    /// o16.8: an open issued while a poll is in flight makes that poll's chunk stale — discarded,
    /// nothing emitted after the reset — and the next poll asks for offset 0. NEGATIVE CONTROL: drop
    /// the generation check in `apply` and the stale line follows the reset → red.
    #[test]
    fn a_chunk_read_before_an_open_is_discarded() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        poll(&live, &sink, b"line 1\n");
        let in_flight = live.begin("j").unwrap();
        live.open("j", &sink); // the poll's ssh call is still running
        sink.take();
        let stale = live.apply("j", in_flight, &chunk(in_flight.offset, b"line 2\n"), 1 << 20, &sink);
        assert_eq!(stale, Applied::Discarded);
        assert!(sink.take().is_empty(), "no line of the stale chunk is emitted");
        assert_eq!(live.begin("j").map(|t| t.offset), Some(0));
    }

    /// o16.8 (ABA): open, a poll begins, the last close drops the state, open again, then the poll
    /// returns — discarded, because the counter outlived the dropped state. NEGATIVE CONTROL: give a
    /// new state a per-job generation that restarts at 1 instead of the launch-long counter and the
    /// stale chunk is applied → red.
    #[test]
    fn drop_and_reopen_during_a_poll_discards_its_chunk() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        let in_flight = live.begin("j").unwrap();
        live.close("j");
        live.open("j", &sink);
        assert!(live.begin("j").is_some(), "the new view's first step makes a new state");
        let stale = live.apply("j", in_flight, &chunk(0, b"old\n"), 1 << 20, &sink);
        assert_eq!(stale, Applied::Discarded);
        assert!(!sink.take().iter().any(|e| matches!(e, Event::Log(..))));
    }

    /// o16.8: a reset chunk recreates the state and emits `job:log-reset` once; the stream restarts
    /// at 0 and nothing of the old carry is spliced on.
    #[test]
    fn a_reset_chunk_recreates_the_state_and_emits_one_reset() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        poll(&live, &sink, b"done\npartial");
        sink.take();
        let t = live.begin("j").unwrap();
        assert_eq!(live.apply("j", t, &LogChunk::reset(), 1 << 20, &sink), Applied::Reset);
        assert_eq!(sink.take(), [Event::Reset("j".into())]);
        assert_eq!(poll(&live, &sink, b"new\n"), Applied::Lines { lines: 1, events: 0, catch_up: false });
        assert_eq!(sink.take(), [Event::Log("j".into(), vec!["new".into()])]);
    }

    /// o16.8: a chunk boundary inside a multi-byte character yields the same lines as one chunk.
    #[test]
    fn a_split_character_is_emitted_whole() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        poll(&live, &sink, b"E(SCF) \xC3");
        poll(&live, &sink, b"\x85 converged\n");
        assert_eq!(sink.take(), [Event::Log("j".into(), vec!["E(SCF) Å converged".into()])]);
    }

    /// o16.8: a terminal job's state is dropped with the count > 0; the count stays.
    #[test]
    fn a_dropped_state_keeps_the_count() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        live.open("k", &sink);
        poll(&live, &sink, b"x\n");
        live.begin("k");
        live.retain(|id| id == "k");
        assert!(!live.has_state("j") && live.has_state("k"));
        assert_eq!(live.open_count("j"), 1);
    }

    #[test]
    fn a_chunk_of_exactly_the_cap_asks_for_catch_up() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        let t = live.begin("j").unwrap();
        live.apply("j", t, &chunk(0, b"abcd"), 4, &sink);
        assert!(live.catch_up("j"));
        let t = live.begin("j").unwrap();
        live.apply("j", t, &chunk(4, b"ef"), 4, &sink);
        assert!(!live.catch_up("j"));
    }

    /// A chunk that does not continue the ticket's offset is refused, not spliced in.
    #[test]
    fn a_chunk_that_skips_bytes_is_refused() {
        let (live, sink) = RecordingSink::with_live();
        live.open("j", &sink);
        let t = live.begin("j").unwrap();
        let skipped = LogChunk { offset: 10, bytes: b"ab".to_vec(), reset: false };
        assert!(matches!(live.apply("j", t, &skipped, 1 << 20, &sink), Applied::Refused(_)));
        assert_eq!(live.begin("j").map(|t| t.offset), Some(0));
        assert!(sink.take().is_empty());
    }

    /// The drain reads to the end, emits the unterminated last line, drops the state; for an
    /// unwatched job it reads nothing.
    #[test]
    fn the_drain_reads_to_the_end_and_only_for_a_watched_job() {
        let file = b"one\ntwo\nthr".to_vec();
        let reads = std::cell::Cell::new(0);
        let mut read = |offset: u64| {
            reads.set(reads.get() + 1);
            let start = offset as usize;
            let end = (start + 3).min(file.len());
            Ok(chunk(offset, &file[start..end]))
        };
        let (live, sink) = RecordingSink::with_live();
        assert_eq!(live.drain("j", &mut read, 3, &sink).unwrap(), Drained::NotWatched);
        assert_eq!(reads.get(), 0, "an unwatched job's copy is not read");

        live.open("j", &sink);
        poll(&live, &sink, b"one\n");
        sink.take();
        assert_eq!(live.drain("j", &mut read, 3, &sink).unwrap(), Drained::Done);
        let lines: Vec<String> = sink.take().into_iter().flat_map(|e| match e { Event::Log(_, l) => l, _ => vec![] }).collect();
        assert_eq!(lines, ["two", "thr"], "from the stored offset, the unterminated last line included");
        assert!(!live.has_state("j"));
    }
}
