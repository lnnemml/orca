//! Tests for the grant core (ADR-027 decision 5; M1b controls).

use super::classify::{GrantStatus, Target};
use super::grant::{
    active_grant_conn, check_utc_text, create_grant_conn, get_grant_conn, grant_snapshot_conn, in_flight_conn,
    revoke_grant_conn, NewGrant,
};
use super::signature::MethodSignature;
use super::test_db::{grant, new_grant, TestDb, EXPIRES, LATER, LEVEL, NOW};
use crate::error::AppError;

fn unrevoked(db: &TestDb) -> Vec<String> {
    let mut stmt = db.conn.prepare("SELECT id FROM mcp_grants WHERE revoked_at IS NULL").unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

#[test]
fn a_grant_is_created_with_its_signatures_and_zero_charged() {
    let db = TestDb::new("grant-create");
    let g = grant(&db.conn, 5, 2);
    assert_eq!(g.targets, vec![Target::Local]);
    assert_eq!(g.levels, vec![MethodSignature::of_input(LEVEL)]);
    assert_eq!((g.max_jobs, g.max_in_flight, g.charged_jobs), (5, 2, 0));
    assert_eq!((g.created_at.as_str(), g.expires_at.as_str(), g.revoked_at.as_deref()), (NOW, EXPIRES, None));
    assert_eq!(get_grant_conn(&db.conn, &g.id).unwrap(), g, "it reads back equal");
}

/// Creating grant 2 revokes grant 1 in the same transaction. NEGATIVE CONTROL: drop the revoking
/// UPDATE in `create_grant_conn` → the expression index refuses the INSERT (or, without the index,
/// grant 1 stays unrevoked) and this is red.
#[test]
fn creating_a_grant_revokes_the_previous_one() {
    let db = TestDb::new("grant-revoke-prev");
    let g1 = grant(&db.conn, 5, 2);
    let g2 = create_grant_conn(&db.conn, &new_grant(3, 1), LATER).unwrap();
    assert_eq!(unrevoked(&db), vec![g2.id.clone()]);
    assert_eq!(get_grant_conn(&db.conn, &g1.id).unwrap().revoked_at.as_deref(), Some(LATER));
    assert_eq!(active_grant_conn(&db.conn, LATER).unwrap().map(|g| g.id), Some(g2.id));
}

/// Every refusal happens before any write: the existing grant stays the active one.
#[test]
fn a_refused_grant_writes_nothing_and_keeps_the_previous_one() {
    let db = TestDb::new("grant-refuse");
    let g1 = grant(&db.conn, 5, 2);
    let with = |f: &dyn Fn(&mut NewGrant)| {
        let mut n = new_grant(5, 2);
        f(&mut n);
        create_grant_conn(&db.conn, &n, NOW)
    };
    let cases: Vec<(&str, Result<_, AppError>)> = vec![
        ("a violating level line", with(&|n| n.levels = vec!["! wB97X-3c\n! D3BJ #a# x\n".into()])),
        ("an empty level", with(&|n| n.levels = vec!["! Opt Freq\n".into()])),
        ("no level", with(&|n| n.levels.clear())),
        ("no target", with(&|n| n.targets.clear())),
        ("max_jobs 0", with(&|n| n.max_jobs = 0)),
        ("max_in_flight 0", with(&|n| n.max_in_flight = 0)),
        ("expired", with(&|n| n.expires_at = "2026-11-19 11:59:59".into())),
        ("expiring now", with(&|n| n.expires_at = NOW.into())),
        ("a malformed expiry", with(&|n| n.expires_at = "2026-11-20T12:00:00Z".into())),
        ("an unknown server", with(&|n| n.targets = vec![Target::Server { profile_id: "nope".into() }])),
        ("a server named local", with(&|n| n.targets = vec![Target::Server { profile_id: "local".into() }])),
    ];
    for (what, r) in cases {
        assert!(r.is_err(), "{what} must be refused");
    }
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM mcp_grants", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "nothing was written");
    assert_eq!(unrevoked(&db), vec![g1.id], "the previous grant is still the unrevoked one");
}

#[test]
fn a_server_target_must_be_a_profile() {
    let db = TestDb::new("grant-server");
    db.conn
        .execute(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir) VALUES ('p-uni', 'uni', 'uni', '/opt/orca/orca', '/s')",
            [],
        )
        .unwrap();
    let mut n = new_grant(5, 2);
    n.targets = vec![Target::Local, Target::Server { profile_id: "p-uni".into() }, Target::Local];
    let g = create_grant_conn(&db.conn, &n, NOW).unwrap();
    assert_eq!(g.targets, vec![Target::Local, Target::Server { profile_id: "p-uni".into() }], "de-duplicated, in order");
}

#[test]
fn revoke_is_idempotent_and_keeps_the_first_time() {
    let db = TestDb::new("grant-revoke");
    let g = grant(&db.conn, 5, 2);
    assert_eq!(revoke_grant_conn(&db.conn, &g.id, LATER).unwrap().revoked_at.as_deref(), Some(LATER));
    assert_eq!(revoke_grant_conn(&db.conn, &g.id, "2026-11-19 14:00:00").unwrap().revoked_at.as_deref(), Some(LATER));
    assert!(matches!(revoke_grant_conn(&db.conn, "nope", LATER), Err(AppError::NotFound(_))));
    assert_eq!(active_grant_conn(&db.conn, LATER).unwrap(), None);
}

#[test]
fn the_active_grant_is_unrevoked_and_unexpired() {
    let db = TestDb::new("grant-active");
    assert_eq!(active_grant_conn(&db.conn, NOW).unwrap(), None);
    let g = grant(&db.conn, 5, 2);
    assert_eq!(active_grant_conn(&db.conn, NOW).unwrap().map(|g| g.id), Some(g.id.clone()));
    assert_eq!(active_grant_conn(&db.conn, EXPIRES).unwrap(), None, "expired at expires_at");
}

#[test]
fn the_snapshot_names_the_newest_grant_and_its_state() {
    let db = TestDb::new("grant-snapshot");
    assert_eq!(grant_snapshot_conn(&db.conn, NOW).unwrap(), None, "no grant ever");
    let g = grant(&db.conn, 5, 2);
    let s = grant_snapshot_conn(&db.conn, NOW).unwrap().unwrap();
    assert_eq!((s.id.as_str(), s.snapshot.status), (g.id.as_str(), GrantStatus::Active));
    assert_eq!(s.snapshot.levels, vec![MethodSignature::of_input(LEVEL)]);
    assert_eq!((s.snapshot.max_jobs, s.snapshot.max_in_flight, s.snapshot.charged_jobs, s.snapshot.in_flight), (5, 2, 0, 0));
    assert_eq!(grant_snapshot_conn(&db.conn, "2026-11-21 00:00:00").unwrap().unwrap().snapshot.status, GrantStatus::Expired);
    revoke_grant_conn(&db.conn, &g.id, LATER).unwrap();
    let s = grant_snapshot_conn(&db.conn, LATER).unwrap().unwrap();
    assert_eq!((s.id, s.snapshot.status), (g.id, GrantStatus::Revoked));
}

/// `in_flight` counts every non-terminal job with a grant, under any grant: an old grant's running
/// job and a draft still in step B included; terminal, refunded and user jobs excluded. NEGATIVE
/// CONTROLS: count only the current grant's jobs → red (the old grant's running job); exclude
/// drafts → red (the step-B draft).
#[test]
fn in_flight_counts_every_grant_and_the_step_b_draft() {
    let db = TestDb::new("grant-in-flight");
    let g1 = grant(&db.conn, 5, 5);
    db.job("old-running", "running", Some(&g1.id));
    db.job("old-done", "parsed", Some(&g1.id));
    let g2 = create_grant_conn(&db.conn, &new_grant(5, 5), LATER).unwrap();
    db.job("step-b", "draft", Some(&g2.id));
    db.conn.execute("UPDATE jobs SET mcp_state = 'submitting' WHERE id = 'step-b'", []).unwrap();
    db.job("queued", "queued", Some(&g2.id));
    db.job("cancelling", "cancelling", Some(&g2.id));
    for (i, st) in ["completed", "failed", "cancelled", "lost"].iter().enumerate() {
        db.job(&format!("end-{i}"), st, Some(&g2.id));
    }
    db.job("refunded", "draft", None);
    db.job("user-running", "running", None);
    assert_eq!(in_flight_conn(&db.conn).unwrap(), 4, "old-running, step-b, queued, cancelling");
    assert_eq!(grant_snapshot_conn(&db.conn, LATER).unwrap().unwrap().snapshot.in_flight, 4);
}

#[test]
fn utc_text_is_the_datetime_now_form_only() {
    assert!(check_utc_text(NOW).is_ok());
    for bad in ["2026-11-19T12:00:00", "2026-11-19 12:00", "2026-13-01 00:00:00", "2026-11-19 24:00:00", "２026-11-19 12:00:00", ""] {
        assert!(check_utc_text(bad).is_err(), "{bad}");
    }
}
