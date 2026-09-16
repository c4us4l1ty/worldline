//! Store integration tests.

use super::repo::Repos;
use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::store;

fn setup() -> Repos {
    let conn = store::open_in_memory().unwrap();
    Repos::new(conn, 1)
}

fn goal_with_milestones(r: &Repos) -> (Goal, Vec<Milestone>) {
    let g = r
        .create_goal("Ship Worldline v0.1", None, Some("2026-10-01"), None)
        .unwrap();
    let m1 = r
        .create_milestone(&g.id, "Crypto core", None, 0, None)
        .unwrap();
    let m2 = r
        .create_milestone(&g.id, "Directive canvas", None, 1, None)
        .unwrap();
    (g, vec![m1, m2])
}

#[test]
fn migrations_create_all_tables() {
    let conn = store::open_in_memory().unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for expected in [
        "identity_config",
        "goals",
        "milestones",
        "directives",
        "directive_phases",
        "check_ins",
        "bailouts",
        "app_settings",
        "crdt_outbox",
        "crdt_applied",
        "hlc_clock",
        "schema_migrations",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "missing table {expected}"
        );
    }
    // Idempotent.
    store::migrations::run(&conn).unwrap();
}

#[test]
fn wal_mode_file_db() {
    let dir = std::env::temp_dir().join(format!("wl-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wl.sqlite");
    let conn = store::open(&path).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn identity_persists_public_half_only() {
    let r = setup();
    let id = Identity::generate().unwrap();
    r.insert_identity(&id, true, &[2, 6, 10]).unwrap();
    let stored = r.identity().unwrap().unwrap();
    assert_eq!(stored.public_key, id.account_id_hex());
    assert!(stored.bip39_mnemonic_verified);
}

#[test]
fn goal_milestone_directive_roundtrip() {
    let r = setup();
    let (g, ms) = goal_with_milestones(&r);
    assert_eq!(r.milestones_for_goal(&g.id).unwrap().len(), 2);
    assert_eq!(
        r.next_pending_milestone(&g.id).unwrap().unwrap().id,
        ms[0].id
    );

    let d = r
        .create_directive(
            &ms[0].id,
            "Write 300 words on Section 2.1",
            Some("Focus on the CRDT merge semantics"),
            45,
            2,
            "2026-09-13",
            &[
                (
                    "Open IDE, write function signature".into(),
                    Some("Just the signature — 5 minutes".into()),
                    5,
                ),
                ("Implement core loop logic".into(), None, 25),
            ],
            None,
        )
        .unwrap();
    assert_eq!(d.progressive_step, 1);
    assert_eq!(d.progressive_total, 2);
    let phases = r.phases_for_directive(&d.id).unwrap();
    assert_eq!(phases.len(), 2);
    assert_eq!(phases[0].state, PhaseState::Active);
    assert_eq!(phases[1].state, PhaseState::Pending);

    // Progressive advance: phase 1 → done, phase 2 → active.
    assert!(r.advance_progressive_step(&d.id, None).unwrap());
    let d2 = r.directive(&d.id).unwrap().unwrap();
    assert_eq!(d2.progressive_step, 2);
    // Past the final phase: returns false.
    assert!(!r.advance_progressive_step(&d.id, None).unwrap());
}

#[test]
fn directive_state_transitions_and_query() {
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let d1 = r
        .create_directive(&ms[0].id, "Task A", None, 20, 1, "2026-09-13", &[], None)
        .unwrap();
    let d2 = r
        .create_directive(&ms[1].id, "Task B", None, 20, 1, "2026-09-14", &[], None)
        .unwrap();
    let d3 = r
        .create_directive(
            &ms[1].id,
            "Task C (overdue)",
            None,
            20,
            1,
            "2026-09-12",
            &[],
            None,
        )
        .unwrap();

    // runnable = queued AND scheduled_for <= today's probe date,
    // ordered by date → overdue first.
    let runnable = r.runnable_directives("2026-09-13").unwrap();
    assert_eq!(runnable.len(), 2);
    assert_eq!(runnable[0].id, d3.id);
    assert_eq!(runnable[1].id, d1.id);
    // d2 is scheduled for the future: not runnable yet.
    assert!(!runnable.iter().any(|d| d.id == d2.id));

    r.set_directive_state(&d1.id, DirectiveState::Active, None)
        .unwrap();
    assert_eq!(r.active_directive().unwrap().unwrap().id, d1.id);
    r.set_directive_state(&d1.id, DirectiveState::Completed, None)
        .unwrap();
    assert!(r.active_directive().unwrap().is_none());
    assert_eq!(r.completed_count_for_milestone(&ms[0].id).unwrap(), 1);
}

#[test]
fn progressive_total_validation() {
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let err = r.create_directive(&ms[0].id, "Bad", None, 60, 3, "2026-09-13", &[], None);
    assert!(matches!(err, Err(store::StoreError::Invalid(_))));
}

#[test]
fn check_in_upsert_one_per_day() {
    let r = setup();
    let c1 = r
        .upsert_check_in(
            "2026-09-13",
            CheckInOutcome::Done,
            Some("shipped crypto"),
            None,
        )
        .unwrap();
    assert_eq!(c1.outcome, CheckInOutcome::Done);
    let c2 = r
        .upsert_check_in("2026-09-13", CheckInOutcome::Partial, None, None)
        .unwrap();
    // Same day: update in place, no duplicate rows.
    assert_eq!(c1.id, c2.id);
    assert_eq!(c2.outcome, CheckInOutcome::Partial);
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM check_ins", [], |x| x.get(0))
        .unwrap();
    assert_eq!(count, 1);

    r.upsert_check_in("2026-09-12", CheckInOutcome::Skipped, None, None)
        .unwrap();
    let recent = r.recent_check_ins(10).unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].date, "2026-09-13"); // date desc
}

#[test]
fn bailout_ledger() {
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let d = r
        .create_directive(
            &ms[0].id,
            "Blocked task",
            None,
            30,
            1,
            "2026-09-13",
            &[],
            None,
        )
        .unwrap();
    let b = r
        .record_bailout(
            &d.id,
            BailoutReason::ExternalDependency,
            Some("waiting on client"),
            None,
        )
        .unwrap();
    assert_eq!(b.reason, BailoutReason::ExternalDependency);
    let rows: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM bailouts", [], |x| x.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn settings_default_and_save() {
    let r = setup();
    let s = r.settings().unwrap();
    assert_eq!(s.theme, "dark");
    assert_eq!(s.hotkey, "alt+space");
    assert!(!s.always_on_top);

    let mut s2 = s.clone();
    s2.always_on_top = true;
    s2.tier1_model = Some("claude-sonnet".into());
    r.save_settings(&s2, None).unwrap();
    let s3 = r.settings().unwrap();
    assert!(s3.always_on_top);
    assert_eq!(s3.tier1_model.as_deref(), Some("claude-sonnet"));
}

#[test]
fn outbox_enqueue_pending_push_cycle() {
    let r = setup();
    let id = Identity::generate().unwrap();
    r.enqueue_outbox(
        &id,
        "directives",
        "dir-1",
        &serde_json::json!({"state": "completed"}),
    )
    .unwrap();
    r.enqueue_outbox(
        &id,
        "milestones",
        "ms-1",
        &serde_json::json!({"status": "active"}),
    )
    .unwrap();

    let pending = r.pending_outbox(100).unwrap();
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|o| o.encrypted_payload.len() > 12 + 16));

    // Ciphertext is genuinely encrypted: plaintext absent.
    let needle = br#"state"#;
    assert!(pending[0]
        .encrypted_payload
        .windows(needle.len())
        .all(|w| w != needle));

    r.mark_outbox_pushed(&[pending[0].operation_id.clone()])
        .unwrap();
    assert_eq!(r.pending_outbox(100).unwrap().len(), 1);
}

#[test]
fn applied_watermark_idempotent() {
    let r = setup();
    assert!(!r.is_op_applied("op-xyz").unwrap());
    let ts = r.hlc.now(1);
    r.mark_op_applied("op-xyz", ts).unwrap();
    // Re-marking is a no-op (INSERT OR IGNORE).
    r.mark_op_applied("op-xyz", ts).unwrap();
    assert!(r.is_op_applied("op-xyz").unwrap());
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM crdt_applied WHERE operation_id='op-xyz'",
            [],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn goal_status_transitions() {
    let r = setup();
    let g = r.create_goal("Learn Rust", None, None, None).unwrap();
    assert_eq!(g.status, GoalStatus::Active);
    assert!(r.active_goal().unwrap().is_some());
    r.conn
        .lock()
        .unwrap()
        .execute("UPDATE goals SET status='achieved' WHERE id=?1", [&g.id])
        .unwrap();
    assert!(r.active_goal().unwrap().is_none());
}

#[test]
fn mnemonic_verified_roundtrip() {
    // Regression: the UPDATE bound ?2/?3 with only two params, so every
    // verification write failed with InvalidParameterIndex.
    let r = setup();
    let id = Identity::generate().unwrap();
    r.insert_identity(&id, false, &[2, 6, 10]).unwrap();
    assert!(!r.identity().unwrap().unwrap().bip39_mnemonic_verified);
    r.set_mnemonic_verified(true).unwrap();
    assert!(r.identity().unwrap().unwrap().bip39_mnemonic_verified);
    r.set_mnemonic_verified(false).unwrap();
    assert!(!r.identity().unwrap().unwrap().bip39_mnemonic_verified);
}

#[test]
fn corrupt_enum_rows_rejected_at_write_time() {
    // Defense in depth, layer 1: CHECK constraints refuse bogus enum
    // strings, so the row-mapper error path (layer 2, `parse_enum`) is
    // unreachable through SQL — a corrupt row can never be constructed
    // to panic the old `expect()` mappers on.
    let r = setup();
    let (g, ms) = goal_with_milestones(&r);
    let d = r
        .create_directive(&ms[0].id, "T", None, 20, 1, "2026-09-13", &[], None)
        .unwrap();
    assert!(r
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE goals SET status='bogus' WHERE id=?1", [&g.id])
        .is_err());
    assert!(r
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE directives SET state='bogus' WHERE id=?1", [&d.id],)
        .is_err());
    // Legit rows still read fine.
    assert!(r.goal(&g.id).is_ok());
    assert!(r.directive(&d.id).is_ok());
    assert!(r.milestone(&ms[0].id).is_ok());
}

#[test]
fn activating_second_directive_parks_first() {
    // Stackelberg invariant enforced at the write path: activating B
    // while A is active re-queues A (with its own HLC bump), so
    // active_directive() can never hide a second active behind LIMIT 1.
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let a = r
        .create_directive(&ms[0].id, "A", None, 20, 1, "2026-09-13", &[], None)
        .unwrap();
    let b = r
        .create_directive(&ms[0].id, "B", None, 20, 1, "2026-09-13", &[], None)
        .unwrap();
    r.set_directive_state(&a.id, DirectiveState::Active, None)
        .unwrap();
    r.set_directive_state(&b.id, DirectiveState::Active, None)
        .unwrap();
    assert_eq!(r.active_directive().unwrap().unwrap().id, b.id);
    assert_eq!(
        r.directive(&a.id).unwrap().unwrap().state,
        DirectiveState::Queued
    );
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM directives WHERE state='active'",
            [],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn enforce_single_active_keeps_newest() {
    // Simulates a merged remote state with two actives (raw SQL, the
    // way LWW apply can produce it): the newest HLC wins, losers park.
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let a = r
        .create_directive(&ms[0].id, "A", None, 20, 1, "2026-09-13", &[], None)
        .unwrap();
    let b = r
        .create_directive(&ms[0].id, "B", None, 20, 1, "2026-09-13", &[], None)
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE directives SET state='active' WHERE id IN (?1, ?2)",
            [&a.id, &b.id],
        )
        .unwrap();
    let parked = r.enforce_single_active(None).unwrap();
    assert_eq!(parked, 1);
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM directives WHERE state='active'",
            [],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    // Idempotent when already clean.
    assert_eq!(r.enforce_single_active(None).unwrap(), 0);
}

#[test]
fn observe_remote_hlc_advances_local_clock() {
    // Receive-event merge: after observing a fast peer's timestamp,
    // local ticks must exceed it (otherwise LWW inverts).
    let r = setup();
    let remote = crate::hlc::HlcTimestamp {
        physical: 9_999_999_999_999_999_999,
        counter: 0,
        device: 7,
    };
    r.observe_remote_hlc(&remote);
    let local = r.hlc.now(r.device_id());
    assert!(local > remote);
}
