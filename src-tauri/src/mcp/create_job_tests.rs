//! Tests for `create_job`'s pipeline (ADR-027 decisions 3–5; M3 amendment, items 1 and 3; unit M3a
//! Part A): the order of its steps, step C by the row's state, the argument bounds and the result
//! cap. The task around it is tested in `server::create_job_task_tests`.

use std::sync::Arc;

use super::audit::{bounded_reasons, AUDIT_REASONS_MAX};
use super::create_job::*;
use super::create_job_test_support::{source_job, FakeStepB, Hand, World, HEADER};
use super::steps::McpReason;
use super::tools::{result_text, size_of_text, ToolError, RESULT_CAP_BYTES};
use crate::mcp::preflight::MaxcoreSource;
use crate::models::job::JobStatus;

fn run(world: &World, step_b: &Arc<FakeStepB>, args: CreateJobArgs) -> Result<CreateJobResult, ToolError> {
    run_create_job(&world.env(step_b.clone()), args)
}

/// A SUBMITTED call runs step A, then step B with no lock held on a committed `submitting` draft,
/// then step C, and reports each: the app's `%maxcore` and the source's charge and multiplicity, step
/// B's message, the job's status after step C. The audit log has the SUBMITTED row and step C's
/// outcome; the emits are step A's (`draft`) and step C's (`queued`).
#[test]
fn a_submitted_call_runs_a_then_b_then_c() {
    let w = World::new("cj-submitted", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let r = run(&w, &step_b, w.args()).expect("create_job");

    assert_eq!(r.disposition, Outcome::Submitted, "{r:?}");
    let job_id = r.job_id.clone().expect("a job");
    assert_eq!(*step_b.seen.lock().unwrap(), vec![(true, true)], "step B: no lock held, step A committed first");
    let set = r.set_by_app.clone().expect("set by the app");
    assert_eq!((set.maxcore_source, set.charge, set.multiplicity, set.atom_count), (MaxcoreSource::ServerProposal, 0, 1, 3));
    assert_eq!(set.charge_mult_from, "job:opt");
    let submit = r.submit.clone().expect("steps B and C");
    assert_eq!((submit.step_b.as_str(), submit.refunded, submit.job_status.as_deref()), ("queued", false, Some("queued")));

    assert_eq!(w.mcp_jobs(), vec![(job_id.clone(), "queued".into(), Some(w.grant_id.clone()), None)]);
    assert_eq!(w.charged(), 1);
    let audit = w.audit();
    assert_eq!(audit.len(), 2, "{audit:?}");
    assert_eq!((audit[0].disposition.as_deref(), audit[0].job_id.as_deref()), (Some("submitted"), Some(job_id.as_str())));
    assert_eq!(audit[0].grant_id.as_deref(), Some(w.grant_id.as_str()));
    assert_eq!(audit[1].disposition, None);
    assert_eq!(audit[1].reasons, vec![McpReason::state("queued"), McpReason::step_b("queued")]);
    assert_eq!(w.emit.statuses(), vec![JobStatus::Draft, JobStatus::Queued]);
    // The stored input is the app's: the header, `%maxcore` and the source's block.
    let input: String = w.db.conn.query_row("SELECT input_content FROM jobs WHERE id = ?1", [&job_id], |r| r.get(0)).unwrap();
    assert!(input.starts_with("! wB97X-3c CPCM(DMF) Opt\n%maxcore ") && input.contains("* xyz 0 1"), "{input}");
}

/// Control (M3 amendment, item 1, M3a): a step-B refusal not refunded. Step B refused before it took
/// the job, so the row is still the charged `draft`: step C refunds it — `grant_id` NULL, the counter
/// back to 0, the reason "submit refused: …" — and the call ends PROPOSED. NEGATIVE CONTROL: step C's
/// refund condition never true → the charge stays → red.
#[test]
fn a_step_b_refusal_is_refunded() {
    let w = World::new("cj-refund", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Refuse));
    let r = run(&w, &step_b, w.args()).expect("create_job");
    let job_id = r.job_id.clone().unwrap();
    let money = (w.charged(), w.mcp_jobs());
    assert_eq!(
        money,
        (0, vec![(job_id.clone(), "draft".to_string(), None, None)]),
        "MONEY-REFUND: a step-B refusal kept its charge"
    );
    assert_eq!(r.disposition, Outcome::Proposed);
    assert!(r.submit.as_ref().unwrap().refunded);
    assert_eq!(r.reasons, vec![McpReason::submit_refused("the slot is busy")]);
    let stored: String = w.db.conn.query_row("SELECT mcp_reasons FROM jobs WHERE id = ?1", [&job_id], |r| r.get(0)).unwrap();
    assert!(stored.contains("submit refused: the slot is busy"), "{stored}");
    assert_eq!(w.emit.statuses(), vec![JobStatus::Draft, JobStatus::Draft]);
}

/// Control (M3 amendment, item 3): a step-C refund leaving the log at SUBMITTED only. The refund's
/// outcome row is written in step C's transaction: `proposed`, with the reason appended to the job.
/// NEGATIVE CONTROL: drop the outcome row from `finish_submit_audited_conn` → red.
#[test]
fn a_refund_leaves_its_outcome_row() {
    let w = World::new("cj-refund-audit", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Refuse));
    let job_id = run(&w, &step_b, w.args()).unwrap().job_id.unwrap();
    let rows: Vec<(Option<String>, Vec<McpReason>, Option<String>)> =
        w.audit().into_iter().map(|a| (a.disposition, a.reasons, a.job_id)).collect();
    assert_eq!(
        rows,
        vec![
            (Some("submitted".into()), vec![], Some(job_id.clone())),
            (Some("proposed".into()), vec![McpReason::submit_refused("the slot is busy")], Some(job_id)),
        ],
        "AUDIT-REFUND: a refund left the log at SUBMITTED only"
    );
}

/// Control (M3 amendment, item 1, M3a): step C refunding a job that already left `draft`. Step B
/// handed the job over (the row is `queued`); step C keeps the charge, only clears the flag, and its
/// outcome row says the state. NEGATIVE CONTROL: drop the `draft` check from step C's refund
/// condition → refunded → red.
#[test]
fn step_c_never_refunds_a_job_that_left_draft() {
    let w = World::new("cj-left-draft", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let r = run(&w, &step_b, w.args()).unwrap();
    let job_id = r.job_id.clone().unwrap();
    assert_eq!(
        (w.charged(), w.mcp_jobs()),
        (1, vec![(job_id, "queued".to_string(), Some(w.grant_id.clone()), None)]),
        "MONEY-LEFT-DRAFT: step C refunded a job that had left draft"
    );
    assert_eq!(r.disposition, Outcome::Submitted);
}

/// A step B that panics is a refusal, so step C still runs: no job is left `submitting`.
#[test]
fn a_panicking_step_b_still_reaches_step_c() {
    let w = World::new("cj-panic", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Panic));
    let r = run(&w, &step_b, w.args()).expect("create_job");
    assert_eq!(r.submit.as_ref().unwrap().step_b, "step B panicked");
    assert_eq!(w.mcp_jobs()[0].3, None, "the flag is cleared");
    assert_eq!(w.charged(), 0, "refunded: the panic happened before any hand-over");
}

/// A PROPOSED call: an uncharged MCP draft with its reasons, emitted and audited, and no step B. On
/// `dark` (memory unknown, and not in the grant) the reasons are P4 and P5.
#[test]
fn a_proposed_call_is_a_draft_with_its_reasons_and_no_step_b() {
    let w = World::new("cj-proposed", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let r = run(&w, &step_b, CreateJobArgs { target: "dark".into(), ..w.args() }).unwrap();
    assert_eq!(r.disposition, Outcome::Proposed);
    let rows: Vec<&str> = r.reasons.iter().map(|x| x.row.as_str()).collect();
    assert_eq!(rows, ["P4", "P5"], "{r:?}");
    assert_eq!(step_b.calls(), 0);
    assert_eq!(r.set_by_app.as_ref().unwrap().maxcore_source, MaxcoreSource::ServerDefault);
    let job_id = r.job_id.unwrap();
    assert_eq!(w.mcp_jobs(), vec![(job_id.clone(), "draft".into(), None, None)]);
    let audit = w.audit();
    assert_eq!((audit.len(), audit[0].disposition.as_deref(), audit[0].job_id.as_deref()), (1, Some("proposed"), Some(job_id.as_str())));
    assert_eq!(w.emit.statuses(), vec![JobStatus::Draft]);
}

/// A REFUSED call writes its audit row and nothing else; no step B, no emit, no `set_by_app`.
#[test]
fn a_refused_call_creates_nothing_but_its_audit_row() {
    let w = World::new("cj-refused", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let r = run(&w, &step_b, CreateJobArgs { header: format!("{HEADER}%pal nprocs 4 end\n"), ..w.args() }).unwrap();
    assert_eq!((r.disposition, r.job_id.clone(), r.set_by_app.clone()), (Outcome::Refused, None, None));
    assert_eq!(r.reasons.iter().map(|x| x.row.as_str()).collect::<Vec<_>>(), ["R3"]);
    assert!(w.mcp_jobs().is_empty());
    assert_eq!(w.audit().len(), 1);
    assert_eq!(w.audit()[0].disposition.as_deref(), Some("refused"));
    assert!(w.emit.statuses().is_empty());
    assert_eq!(step_b.calls(), 0);
}

/// The R4 source-charge reason end to end (M3 amendment, item 1; the M2b verifier's F8): a source
/// whose `* xyz` line carries coordinate text reaches the model and the audit log as a kind and a
/// line. (The control is `classify_tests::r4_never_quotes_the_source_charge_line`.)
#[test]
fn an_r4_reason_reaches_the_model_and_the_log_without_the_source_line() {
    let w = World::new("cj-r4", 1);
    let bad = "! wB97X-3c CPCM(DMF) Opt\n* xyz 0 1 O 0.4242 0.0\nO 0.0 0.0 0.0\nH 0.0 0.76 0.59\nH 0.0 -0.76 0.59\n*\n";
    source_job(&w.db.conn, &w.db.dir, "bad", bad);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let r = run(&w, &step_b, CreateJobArgs { source: "job:bad".into(), ..w.args() }).unwrap();
    assert_eq!(r.disposition, Outcome::Refused);
    let text = serde_json::to_string(&(r, w.audit())).unwrap();
    assert!(!text.contains("0.4242"), "{text}");
    assert!(text.contains("line 2"), "{text}");
}

/// An unknown group is an argument error: nothing is written, and no audit row (M3 amendment, item 3).
/// A known one files the job.
#[test]
fn a_group_must_exist_and_files_the_job() {
    let w = World::new("cj-group", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let missing = run(&w, &step_b, CreateJobArgs { group_id: Some("nope".into()), ..w.args() });
    assert!(matches!(&missing, Err(ToolError::Refused(m)) if m.contains("does not exist")), "{missing:?}");
    assert!(w.mcp_jobs().is_empty() && w.audit().is_empty());

    w.db.conn.execute("INSERT INTO groups (id, name) VALUES ('g1', 'study')", []).unwrap();
    let r = run(&w, &step_b, CreateJobArgs { group_id: Some("g1".into()), ..w.args() }).unwrap();
    let group: Option<String> =
        w.db.conn.query_row("SELECT group_id FROM jobs WHERE id = ?1", [r.job_id.unwrap()], |r| r.get(0)).unwrap();
    assert_eq!(group.as_deref(), Some("g1"));
}

/// Control (M3 amendment, item 1, M3a): an over-long header accepted. Every argument has its bound
/// and is refused over it before anything is read or written, with no audit row; at the bound it is
/// taken. NEGATIVE CONTROL: drop the header's bound → the call is classified and audited → red.
#[test]
fn an_over_long_argument_is_refused_before_anything_is_written() {
    let w = World::new("cj-bounds", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let long_header = format!("{HEADER}#{}\n", "x".repeat(HEADER_MAX_BYTES));
    let over = run(&w, &step_b, CreateJobArgs { header: long_header, ..w.args() });
    let written = (w.mcp_jobs().len(), w.audit().len());
    assert!(
        matches!(&over, Err(ToolError::Refused(m)) if m.contains("`header`")) && written == (0, 0),
        "ARGS-HEADER: an over-long header was accepted: {over:?}, written {written:?}"
    );

    let cases = [
        CreateJobArgs { source: format!("smiles:{}", "C".repeat(SOURCE_MAX_BYTES)), ..w.args() },
        CreateJobArgs { target: "t".repeat(TARGET_MAX_BYTES + 1), ..w.args() },
        CreateJobArgs { group_id: Some("g".repeat(GROUP_ID_MAX_BYTES + 1)), ..w.args() },
        CreateJobArgs { title: Some("t".repeat(TITLE_MAX_BYTES + 1)), ..w.args() },
        CreateJobArgs { header: String::new(), ..w.args() },
        CreateJobArgs { source: String::new(), ..w.args() },
    ];
    for args in cases {
        let r = run(&w, &step_b, args.clone());
        assert!(matches!(r, Err(ToolError::Refused(_))), "{args:?} → {r:?}");
    }
    assert_eq!((w.mcp_jobs().len(), w.audit().len()), (0, 0));

    // At the bound: a header of exactly HEADER_MAX_BYTES is classified (and refused here: R1 by its
    // padding line), not refused as too long.
    let at = format!("{HEADER}{}\n", "x".repeat(HEADER_MAX_BYTES - HEADER.len() - 1));
    assert_eq!(at.len(), HEADER_MAX_BYTES);
    let r = run(&w, &step_b, CreateJobArgs { header: at, ..w.args() }).unwrap();
    assert_eq!(r.disposition, Outcome::Refused);
    // A title at its bound is taken.
    let r = run(&w, &step_b, CreateJobArgs { title: Some("t".repeat(TITLE_MAX_BYTES)), ..w.args() }).unwrap();
    assert_eq!(r.disposition, Outcome::Submitted);
}

/// Control (M3 amendment, item 1, M3a): a result over the cap. The largest call — a header at its
/// bound made of `*` lines, each one an R2 reason — gives a result that lists 20 reasons and counts
/// the rest, far under the cap; and the largest result the fields allow (every reason, the source
/// label and step B's message at their cuts, full of characters JSON escapes to 6 bytes) fits too.
/// NEGATIVE CONTROL: stop bounding the reasons → thousands of reasons → over the cap → red.
#[test]
fn the_largest_result_stays_under_the_cap() {
    let w = World::new("cj-cap", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let stars = (HEADER_MAX_BYTES - HEADER.len()) / 2;
    let header = format!("{HEADER}{}", "*\n".repeat(stars));
    let r = run(&w, &step_b, CreateJobArgs { header, ..w.args() });
    let fits = r.as_ref().map(|r| (r.reasons.len(), size_of_text(&result_text(r).unwrap()) <= RESULT_CAP_BYTES));
    assert_eq!(fits.as_ref().ok(), Some(&(AUDIT_REASONS_MAX + 1, true)), "RESULT-CAP: the result is not bounded: {:?}", r.as_ref().map(|r| r.reasons_count));
    // Each `*` line is an R2 reason; the R1 they also break is one reason with a count.
    assert_eq!(r.unwrap().reasons_count, stars + 1);

    let wide = "\u{1}".repeat(2_000);
    let many: Vec<McpReason> = (0..200).map(|_| McpReason { row: wide.clone(), message: wide.clone() }).collect();
    let largest = CreateJobResult {
        disposition: Outcome::Proposed,
        job_id: Some("x".repeat(36)),
        reasons: bounded_reasons(&many),
        reasons_count: many.len(),
        set_by_app: Some(SetByApp {
            maxcore_mib: u64::MAX,
            maxcore_source: MaxcoreSource::ServerProposal,
            charge: i64::MIN,
            multiplicity: i64::MIN,
            charge_mult_from: super::tools::clip(&wide, SOURCE_MAX_BYTES).0,
            atom_count: usize::MAX,
        }),
        submit: Some(SubmitReport {
            step_b: super::tools::clip(&wide, STEP_B_MESSAGE_MAX_BYTES).0,
            refunded: true,
            job_status: Some("cancelling".into()),
            step_c_error: Some(super::tools::clip(&wide, STEP_B_MESSAGE_MAX_BYTES).0),
        }),
    };
    let bytes = size_of_text(&result_text(&largest).unwrap());
    assert!(bytes <= RESULT_CAP_BYTES, "RESULT-CAP: the largest result is {bytes} bytes");
}

/// The audit row's argument text stays under the column's 8192-character CHECK whatever the
/// arguments, and says when the header was cut.
#[test]
fn the_audit_args_are_bounded_and_say_so() {
    let args = CreateJobArgs {
        header: "\u{1}".repeat(HEADER_MAX_BYTES),
        source: "\u{1}".repeat(SOURCE_MAX_BYTES),
        target: "\u{1}".repeat(TARGET_MAX_BYTES),
        group_id: Some("\u{1}".repeat(GROUP_ID_MAX_BYTES)),
        title: Some("\u{1}".repeat(TITLE_MAX_BYTES)),
    };
    let text = audit_args(&args);
    assert!(text.len() < crate::db::MCP_AUDIT_ARGS_MAX_CHARS, "{} bytes", text.len());
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!((v["header_bytes"].as_u64(), v["header_cut"].as_bool()), (Some(HEADER_MAX_BYTES as u64), Some(true)));
}
