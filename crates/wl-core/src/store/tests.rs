//! Store integration tests.

use super::repo::Repos;

use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::hlc::MAX_REMOTE_DRIFT_NANOS;
use crate::store;

fn setup() -> Repos {
    let conn = store::open_in_memory().unwrap();
    Repos::new(conn, 1)
}

#[test]
fn directive_creation_rolls_back_when_phase_outbox_fails() {
    let r = setup();
    let (goal, milestones) = goal_with_milestones(&r);
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_phase_outbox BEFORE INSERT ON crdt_outbox
         WHEN NEW.table_name = 'directive_phases'
         BEGIN SELECT RAISE(ABORT, 'outbox unavailable'); END;",
        )
        .unwrap();
    let phases = [("Start".into(), None, 5), ("Finish".into(), None, 25)];
    assert!(r
        .create_directive(
            &milestones[0].id,
            "Atomic task",
            None,
            30,
            2,
            "2026-09-13",
            &phases,
            Some(&identity),
        )
        .is_err());
    assert!(r.runnable_directives("2026-09-13").unwrap().is_empty());
    assert!(r.pending_outbox(100).unwrap().is_empty());
    assert_eq!(r.goal(&goal.id).unwrap(), Some(goal));
    r.conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_phase_outbox")
        .unwrap();
    let directive = r
        .create_directive(
            &milestones[0].id,
            "Atomic task",
            None,
            30,
            2,
            "2026-09-13",
            &phases,
            Some(&identity),
        )
        .unwrap();
    let outbox = r.pending_outbox(100).unwrap();
    let directive_op = outbox
        .iter()
        .find(|op| op.table_name == "directives")
        .unwrap();
    assert_eq!(directive_op.hlc_timestamp, directive.hlc_timestamp);
    let stored_phases = r.phases_for_directive(&directive.id).unwrap();
    assert_eq!(stored_phases.len(), 2);
    for phase in stored_phases {
        assert!(outbox.iter().any(
            |op| op.table_name == "directive_phases" && op.hlc_timestamp == phase.hlc_timestamp
        ));
    }
}

#[test]
fn reschedule_directive_rolls_back_when_phase_outbox_fails() {
    let r = setup();
    let (_, milestones) = goal_with_milestones(&r);
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let directive = r
        .create_directive(
            &milestones[0].id,
            "Rescale task",
            None,
            30,
            2,
            "2026-09-13",
            &[("Start".into(), None, 5), ("Finish".into(), None, 25)],
            None,
        )
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_phase_outbox BEFORE INSERT ON crdt_outbox
         WHEN NEW.table_name = 'directive_phases'
         BEGIN SELECT RAISE(ABORT, 'outbox unavailable'); END;",
        )
        .unwrap();
    assert!(r
        .reschedule_directive(&directive.id, 15, "2026-09-14", Some(&identity))
        .is_err());
    let unchanged = r.directive(&directive.id).unwrap().unwrap();
    assert_eq!(unchanged.state, DirectiveState::Queued);
    assert_eq!(unchanged.estimated_minutes, 30);
    assert_eq!(unchanged.scheduled_for_date, "2026-09-13");
    assert_eq!(unchanged.progressive_step, 1);
    let phases = r.phases_for_directive(&directive.id).unwrap();
    assert_eq!((phases[0].minutes, phases[1].minutes), (5, 25));
    assert!(r.pending_outbox(100).unwrap().is_empty());
    r.conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_phase_outbox")
        .unwrap();
    r.reschedule_directive(&directive.id, 15, "2026-09-14", Some(&identity))
        .unwrap();
    let down = r.directive(&directive.id).unwrap().unwrap();
    assert_eq!(down.estimated_minutes, 15);
    assert_eq!(down.scheduled_for_date, "2026-09-14");
    let outbox = r.pending_outbox(100).unwrap();
    let rescaled = r.phases_for_directive(&directive.id).unwrap();
    let total: i64 = rescaled.iter().map(|p| p.minutes).sum();
    assert_eq!(total, 15);
    assert!(outbox
        .iter()
        .any(|op| op.table_name == "directives" && op.hlc_timestamp == down.hlc_timestamp));
    for phase in rescaled {
        assert!(outbox.iter().any(
            |op| op.table_name == "directive_phases" && op.hlc_timestamp == phase.hlc_timestamp
        ));
    }
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
    assert!(s.ai_provider.is_none());
    assert!(s.relay_url.is_none());

    let mut s2 = s.clone();
    s2.theme = "light".into();
    s2.tier1_model = Some("claude-sonnet".into());
    s2.relay_url = Some("http://127.0.0.1:8080".into());
    r.save_settings(&s2, None).unwrap();
    let s3 = r.settings().unwrap();
    assert_eq!(s3.theme, "light");
    assert_eq!(s3.tier1_model.as_deref(), Some("claude-sonnet"));
    assert_eq!(s3.relay_url.as_deref(), Some("http://127.0.0.1:8080"));
}

/// The `hotkey` and `always_on_top` columns were removed as features
/// without a schema migration: each keeps its `NOT NULL DEFAULT` and is
/// simply never named in the INSERT or the SELECT.
///
/// The consequence worth pinning is that a row written by an OLD build —
/// which does still set `always_on_top` — must be readable by this one
/// without error, because a synced install is a mixed-version fleet for as
/// long as any device is un-updated. If the SELECT ever starts naming the
/// column, or starts requiring it, this test is what notices.
#[test]
fn settings_survive_a_row_written_by_a_build_that_still_had_the_pin() {
    let r = setup();
    // Exactly the statement the previous build used, pin column included.
    r.lock_conn()
        .execute(
            "INSERT INTO app_settings (id, theme, always_on_top, ai_provider, tier1_model, tier2_model, relay_url, hlc_timestamp)
             VALUES (1, 'dark', 1, 'openrouter', 'old/model', NULL, 'http://old:8080', '0.0.1:1:1')",
            [],
        )
        .unwrap();

    // Readable, and the retired field is simply not surfaced.
    let s = r.settings().unwrap();
    assert_eq!(s.theme, "dark");
    assert_eq!(s.ai_provider.as_deref(), Some("openrouter"));
    assert_eq!(s.tier1_model.as_deref(), Some("old/model"));
    assert_eq!(s.relay_url.as_deref(), Some("http://old:8080"));

    // And rewriting the row from this build does not fail on the column
    // this build no longer names.
    r.save_settings(&s, None).unwrap();
    assert_eq!(
        r.settings().unwrap().tier1_model.as_deref(),
        Some("old/model")
    );
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
    // Force a genuine tie: both rows share one hlc_timestamp (as a
    // peer merge can produce), so the id tie-break is exercised
    // deterministically instead of depending on per-row ticks.
    let shared_ts = r.hlc.now(1).to_string();
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE directives SET state='active', hlc_timestamp=?3 WHERE id IN (?1, ?2)",
            [&a.id, &b.id, &shared_ts],
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
    // Exactly one active survives, and it is deterministic: max
    // hlc_timestamp, tie-break min id (both rows share one HLC here, so
    // the lexicographically smallest id wins).
    let survivor = r.active_directive().unwrap().unwrap();
    let expected = [&a.id, &b.id].into_iter().min().unwrap().clone();
    assert_eq!(survivor.id, expected);
    assert_eq!(
        r.directive(&expected).unwrap().unwrap().state,
        DirectiveState::Active
    );
    // Idempotent when already clean.
    assert_eq!(r.enforce_single_active(None).unwrap(), 0);
}

#[test]
fn runnable_directives_order_is_deterministic_on_full_tie() {
    // Two queued directives sharing (scheduled_for_date, hlc) — the
    // exact collision a peer merge can produce — must surface in a
    // stable id order, not SQLite scan order.
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let ts = r.hlc.now(1).to_string();
    let conn = r.conn.lock().unwrap();
    for title in ["Twin A", "Twin B"] {
        conn.execute(
            "INSERT INTO directives (id, milestone_id, title, execution_context,
                estimated_minutes, progressive_step, progressive_total, state,
                scheduled_for_date, hlc_timestamp)
             VALUES (?1, ?2, ?3, NULL, 20, 1, 1, 'queued', '2026-09-13', ?4)",
            rusqlite::params![
                format!("dir-{}", uuid::Uuid::new_v4().simple()),
                ms[0].id,
                title,
                ts
            ],
        )
        .unwrap();
    }
    drop(conn);
    let first = r
        .runnable_directives("2026-09-13")
        .unwrap()
        .iter()
        .map(|d| d.id.clone())
        .collect::<Vec<_>>();
    let second = r
        .runnable_directives("2026-09-13")
        .unwrap()
        .iter()
        .map(|d| d.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(first, second, "repeat reads must agree");
    assert_eq!(first.len(), 2);
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(first, sorted, "id tie-break orders lexicographically");
    assert_eq!(
        r.next_runnable_directive("2026-09-13").unwrap().unwrap().id,
        sorted[0]
    );
}

#[test]
fn observe_remote_hlc_advances_local_clock() {
    // Receive-event merge: after observing a fast peer's timestamp,
    // local ticks must exceed it (otherwise LWW inverts).
    let r = setup();
    let ahead = r.hlc.now(r.device_id()).physical + MAX_REMOTE_DRIFT_NANOS / 2;
    let remote = crate::hlc::HlcTimestamp {
        physical: ahead,
        counter: 0,
        device: 7,
    };
    r.observe_remote_hlc(&remote);
    let local = r.hlc.now(r.device_id());
    assert!(local > remote);
}

#[test]
fn observe_cannot_drag_the_persisted_head_out_of_range() {
    // The persisted head (`hlc_clock`) is what `Repos::new` restores on
    // the next boot, so a merge that ran away did not just upset one
    // cycle — it changed what every future timestamp is measured from.
    let r = setup();
    r.observe_remote_hlc(&crate::hlc::HlcTimestamp {
        physical: u64::MAX,
        counter: 7,
        device: 7,
    });
    let (wall, _) = r.hlc.head();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    assert!(
        wall <= now + MAX_REMOTE_DRIFT_NANOS + 1_000_000_000,
        "head {wall} escaped the horizon at {now}"
    );
    // The clock still issues usable, increasing stamps afterwards.
    let a = r.hlc.now(r.device_id());
    let b = r.hlc.now(r.device_id());
    assert!(b > a);
}

#[test]
fn write_paths_reject_blank_oversized_and_malformed_input() {
    let r = setup();
    // Goals.
    assert!(r.create_goal("   ", None, None, None).is_err());
    assert!(r.create_goal(&"x".repeat(501), None, None, None).is_err());
    assert!(r
        .create_goal("Ok", Some(&"y".repeat(4001)), None, None)
        .is_err());
    assert!(r.create_goal("Ok", None, Some("2026-13-40"), None).is_err());
    assert!(r.create_goal("Ok", None, Some("2026-9-8"), None).is_err());
    let g = r
        .create_goal("Ok", Some("fine"), Some("2026-10-01"), None)
        .unwrap();
    // Milestones.
    assert!(r.create_milestone(&g.id, "", None, 0, None).is_err());
    assert!(r
        .create_milestone(&g.id, &"x".repeat(501), None, 0, None)
        .is_err());
    let m = r.create_milestone(&g.id, "M", None, 0, None).unwrap();
    // Directives.
    let no_phases: Vec<(String, Option<String>, i64)> = vec![];
    assert!(r
        .create_directive(&m.id, "", None, 10, 1, "2026-09-13", &no_phases, None)
        .is_err());
    assert!(r
        .create_directive(&m.id, "T", None, 0, 1, "2026-09-13", &no_phases, None)
        .is_err());
    assert!(r
        .create_directive(&m.id, "T", None, 1441, 1, "2026-09-13", &no_phases, None)
        .is_err());
    assert!(r
        .create_directive(&m.id, "T", None, 10, 1, "not-a-date", &no_phases, None)
        .is_err());
    assert!(r
        .create_directive(
            &m.id,
            "T",
            Some(&"z".repeat(4001)),
            10,
            1,
            "2026-09-13",
            &no_phases,
            None
        )
        .is_err());
    assert!(r
        .create_directive(
            &m.id,
            "T",
            None,
            30,
            2,
            "2026-09-13",
            &[("P".into(), None, 0)],
            None
        )
        .is_err());
    // Check-ins.
    assert!(r
        .upsert_check_in("09/13/2026", CheckInOutcome::Done, None, None)
        .is_err());
    assert!(r
        .upsert_check_in(
            "2026-09-13",
            CheckInOutcome::Done,
            Some(&"n".repeat(2001)),
            None
        )
        .is_err());
    // Nothing partial was persisted by the rejections above.
    assert!(r.goal(&g.id).unwrap().is_some());
    assert_eq!(r.milestones_for_goal(&g.id).unwrap().len(), 1);
    assert!(r.pending_outbox(100).unwrap().is_empty());
}

#[test]
fn bailout_note_truncates_to_spec_cap() {
    let r = setup();
    let (_, milestones) = goal_with_milestones(&r);
    let d = r
        .create_directive(&milestones[0].id, "T", None, 10, 1, "2026-09-13", &[], None)
        .unwrap();
    let long = "n".repeat(500);
    let b = r
        .record_bailout(&d.id, BailoutReason::EnergyDepletion, Some(&long), None)
        .unwrap();
    assert_eq!(b.note.as_ref().unwrap().chars().count(), 140);
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT LENGTH(note) FROM bailouts WHERE id = ?1",
            [&b.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 140);
}

#[test]
fn settings_reject_unknown_theme_provider_and_oversized_text() {
    let r = setup();
    let case = |f: &dyn Fn(&mut AppSettings)| {
        let mut s = AppSettings::default();
        f(&mut s);
        s
    };
    assert!(r
        .save_settings(&case(&|s| s.theme = "neon".into()), None)
        .is_err());
    assert!(r
        .save_settings(&case(&|s| s.ai_provider = Some("evil-ai".into())), None)
        .is_err());
    assert!(r
        .save_settings(&case(&|s| s.tier1_model = Some("m".repeat(257))), None)
        .is_err());
    // CORE-7(a): a replicated settings row can carry a relay URL, so the
    // SSRF policy has to hold on this write path and not only in the shell.
    assert!(r
        .save_settings(
            &case(&|s| s.relay_url = Some("file:///etc/passwd".into())),
            None
        )
        .is_err());
    // A valid save still round-trips.
    r.save_settings(&case(&|s| s.theme = "light".into()), None)
        .unwrap();
    assert_eq!(r.settings().unwrap().theme, "light");
}

#[test]
fn reschedule_rejects_out_of_range_estimates_and_dates() {
    let r = setup();
    let (_, milestones) = goal_with_milestones(&r);
    let d = r
        .create_directive(&milestones[0].id, "T", None, 30, 1, "2026-09-13", &[], None)
        .unwrap();
    assert!(r
        .reschedule_directive(&d.id, 0, "2026-09-14", None)
        .is_err());
    assert!(r
        .reschedule_directive(&d.id, 1441, "2026-09-14", None)
        .is_err());
    assert!(r.reschedule_directive(&d.id, 30, "tomorrow", None).is_err());
    let unchanged = r.directive(&d.id).unwrap().unwrap();
    assert_eq!(unchanged.estimated_minutes, 30);
}

#[test]
fn pushed_outbox_rows_are_deleted_not_accumulated() {
    let r = setup();
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    r.create_goal("G", None, None, Some(&identity)).unwrap();
    assert_eq!(r.pending_outbox(100).unwrap().len(), 1);
    let ids: Vec<String> = r
        .pending_outbox(100)
        .unwrap()
        .iter()
        .map(|o| o.operation_id.clone())
        .collect();
    r.mark_outbox_pushed(&ids).unwrap();
    assert!(r.pending_outbox(100).unwrap().is_empty());
    assert_eq!(r.delete_pushed_outbox().unwrap(), 1);
    let left: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM crdt_outbox", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn applied_watermark_prunes_only_at_or_below_cursor() {
    let r = setup();
    for (op, hlc) in [
        ("op-a", "00000000000000000001.00000.00001"),
        ("op-b", "00000000000000000002.00000.00001"),
        ("op-c", "00000000000000000002.00000.00002"),
    ] {
        r.mark_op_applied_str(op, hlc).unwrap();
    }
    assert_eq!(
        r.prune_applied_below("00000000000000000002.00000.00001", "op-b")
            .unwrap(),
        2
    );
    assert!(!r.is_op_applied("op-a").unwrap());
    assert!(r.is_op_applied("op-c").unwrap());
}

#[test]
fn zero_backfill_migration_repairs_legacy_rows() {
    // Fresh chain (v1–v5), then simulate legacy '0' backfill values on
    // real rows, rewind the v4 marker, and re-run: v4 must repair them
    // into parseable canonical-zero timestamps.
    let r = setup();
    let (_, milestones) = goal_with_milestones(&r);
    let d = r
        .create_directive(
            &milestones[0].id,
            "T",
            None,
            30,
            2,
            "2026-09-13",
            &[("A".into(), None, 5), ("B".into(), None, 25)],
            None,
        )
        .unwrap();
    r.save_settings(&AppSettings::default(), None).unwrap();
    {
        let conn = r.conn.lock().unwrap();
        conn.execute("UPDATE directive_phases SET hlc_timestamp = '0'", [])
            .unwrap();
        conn.execute("UPDATE app_settings SET hlc_timestamp = '0'", [])
            .unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 4", [])
            .unwrap();
        super::migrations::run(&conn).unwrap();
    }
    for p in r.phases_for_directive(&d.id).unwrap() {
        assert_eq!((p.hlc_timestamp.physical, p.hlc_timestamp.counter), (0, 0));
    }
    let versions: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    // Derived, never hand-copied: this assertion used to be a literal
    // `5` and went stale the moment a migration was added.
    assert_eq!(versions, super::migrations::MIGRATIONS.len() as i64);
}

#[test]
fn clone_split_boot_jitter_separates_forked_heads() {
    // Two live replicas forked from one data dir share device id and
    // clock head; under a regressed wall clock their first ticks must
    // differ, or identical HLCs on different content fork LWW
    // permanently. 64 fork pairs: at least one must separate
    // (all-64-collide probability is 2^-512).
    const HEAD_WALL: i64 = 9_000_000_000_000_000_000;
    fn forked() -> Repos {
        let conn = store::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO hlc_clock (id, last_wall_nanos, counter, device) VALUES (1, ?1, 0, 7)",
            [HEAD_WALL],
        )
        .unwrap();
        Repos::new(conn, 7)
    }
    let mut identical_pairs = 0;
    for _ in 0..64 {
        let a = forked();
        let b = forked();
        // Wall clock is far below the restored head, so both ticks take
        // the increment path from their (possibly jittered) heads.
        let ta = a.hlc.now(a.device_id());
        let tb = b.hlc.now(b.device_id());
        assert!(ta.physical == HEAD_WALL as u64);
        assert!(tb.physical == HEAD_WALL as u64);
        if ta.counter == tb.counter {
            identical_pairs += 1;
        }
    }
    assert!(
        identical_pairs < 64,
        "boot jitter never separated 64 forked heads"
    );
}

// ---------------------------------------------------------------------------
// CORE-3: goal lifecycle
// ---------------------------------------------------------------------------

#[test]
fn set_goal_status_persists_and_emits_a_crdt_op() {
    let r = setup();
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let g = r.create_goal("Ship v0.1", None, None, None).unwrap();
    assert_eq!(g.status, GoalStatus::Active);

    r.set_goal_status(&g.id, GoalStatus::Achieved, Some(&identity))
        .unwrap();
    assert_eq!(r.goal(&g.id).unwrap().unwrap().status, GoalStatus::Achieved);
    // The status change must replicate, not just land locally.
    let ops: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM crdt_outbox WHERE table_name = 'goals' AND record_id = ?1",
            [&g.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ops, 1, "status change should enqueue exactly one op");
}

#[test]
fn set_goal_status_rejects_unknown_goal() {
    let r = setup();
    assert!(matches!(
        r.set_goal_status("goal-missing", GoalStatus::Archived, None),
        Err(crate::store::StoreError::NotFound(_))
    ));
}

#[test]
fn archive_other_active_goals_leaves_exactly_one_active() {
    let r = setup();
    let a = r.create_goal("A", None, None, None).unwrap();
    let b = r.create_goal("B", None, None, None).unwrap();
    let c = r.create_goal("C", None, None, None).unwrap();
    let active_count = |r: &Repos| -> i64 {
        r.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM goals WHERE status = 'active'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(active_count(&r), 3, "precondition: three active goals");

    let archived = r.archive_other_active_goals(&c.id, None).unwrap();
    assert_eq!(archived.len(), 2, "A and B should be archived");
    assert!(archived.contains(&a.id) && archived.contains(&b.id));
    assert_eq!(active_count(&r), 1);
    // `keep` must never archive itself.
    assert_eq!(r.goal(&c.id).unwrap().unwrap().status, GoalStatus::Active);
    assert_eq!(r.goal(&a.id).unwrap().unwrap().status, GoalStatus::Archived);
}

#[test]
fn active_goal_is_deterministic_when_duplicates_exist() {
    // Even if a legacy data dir still carries two `active` goals,
    // `active_goal()` must not flip between them across calls.
    let r = setup();
    r.create_goal("older", None, None, None).unwrap();
    r.create_goal("newer", None, None, None).unwrap();
    let first = r.active_goal().unwrap().unwrap().id;
    for _ in 0..20 {
        assert_eq!(r.active_goal().unwrap().unwrap().id, first);
    }
}

// ---------------------------------------------------------------------------
// CORE-4: identity_config singleton
// ---------------------------------------------------------------------------

#[test]
fn identity_config_is_a_singleton_across_restore() {
    let r = setup();
    let first = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let second = Identity::from_phrase(
        "all year wave sausage worth useful legal winner thank yellow winner thank",
    )
    .unwrap();
    assert_ne!(first.account_id_hex(), second.account_id_hex());

    r.insert_identity(&first, true, &[0, 3, 7]).unwrap();
    assert_eq!(
        r.identity().unwrap().unwrap().public_key,
        first.account_id_hex()
    );

    // Restoring a DIFFERENT phrase must replace, not append.
    r.insert_identity(&second, false, &[1, 2, 3]).unwrap();
    let rows: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM identity_config", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1, "identity_config must never hold two rows");

    let got = r.identity().unwrap().unwrap();
    assert_eq!(got.public_key, second.account_id_hex());
    assert!(!got.bip39_mnemonic_verified);
    assert_eq!(got.verify_indices, vec![1, 2, 3]);
}

#[test]
fn schema_rejects_a_second_identity_row() {
    let r = setup();
    let id = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    r.insert_identity(&id, true, &[]).unwrap();
    // The `id = 1` CHECK must make a duplicate impossible at the schema
    // level, not merely unused by the writers.
    let err = r.conn.lock().unwrap().execute(
        "INSERT INTO identity_config (id, public_key, bip39_mnemonic_verified, verify_indices, hlc_timestamp)
         VALUES (1, 'deadbeef', 0, '[]', '00000000000000000001.00000.00001')",
        [],
    );
    assert!(err.is_err(), "a second identity row must be rejected");
}

#[test]
fn set_mnemonic_verified_touches_only_the_singleton() {
    let r = setup();
    let id = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    r.insert_identity(&id, false, &[]).unwrap();
    r.set_mnemonic_verified(true).unwrap();
    assert!(r.identity().unwrap().unwrap().bip39_mnemonic_verified);
}

// ---------------------------------------------------------------------------
// AUDIT-3: the tombstone writers had integration coverage only
// ---------------------------------------------------------------------------

/// Decrypts every pending outbox op and returns `(table, record_id, is_tombstone)`.
fn decoded_outbox(r: &Repos, identity: &Identity) -> Vec<(String, String, bool)> {
    r.pending_outbox(1024)
        .unwrap()
        .into_iter()
        .map(|op| {
            let aad = crate::crdt::routing_aad(
                &op.table_name,
                &op.record_id,
                &op.operation_id,
                &op.hlc_timestamp.to_string(),
            );
            let sealed = crate::crypto::aead::Sealed::from_bytes(&op.encrypted_payload).unwrap();
            let plain = crate::crypto::aead::unseal(identity, &sealed, aad.as_bytes()).unwrap();
            let fields: serde_json::Value = serde_json::from_slice(&plain).unwrap();
            let tomb = fields
                .get(crate::crdt::TOMBSTONE_MARKER)
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            (op.table_name, op.record_id, tomb)
        })
        .collect()
}

#[test]
fn delete_writers_emit_decryptable_tombstone_ops() {
    let r = setup();
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let (_goal, milestones) = goal_with_milestones(&r);
    let d = r
        .create_directive(
            &milestones[0].id,
            "T",
            None,
            30,
            2,
            "2026-09-13",
            &[("A".into(), None, 5), ("B".into(), None, 25)],
            Some(&identity),
        )
        .unwrap();

    // Before the delete: the insert ops decrypt as upserts, not tombstones.
    let before = decoded_outbox(&r, &identity);
    assert!(!before.iter().any(|(_, _, tomb)| *tomb));

    // Deletes are leaf-first by contract: FKs are enforced, so a
    // directive that still has phase rows must have them removed first.
    for phase in r.phases_for_directive(&d.id).unwrap() {
        r.delete_directive_phase(&d.id, phase.step, Some(&identity))
            .unwrap();
    }
    r.delete_directive(&d.id, Some(&identity)).unwrap();
    assert!(
        r.directive(&d.id).unwrap().is_none(),
        "row must be gone locally"
    );

    // The deletes must be replicable: sealed tombstone ops naming each
    // deleted record, decryptable with the identity's payload key under
    // the full routing-header AAD.
    let after = decoded_outbox(&r, &identity);
    let tombs: Vec<_> = after.iter().filter(|(_, _, t)| *t).collect();
    assert_eq!(tombs.len(), 3, "two phases + the directive: {after:?}");
    // Each phase tombstone names ONE step, as `{directive_id}:{step}`.
    //
    // The old shape was the bare directive id for every step, which made
    // all steps of a directive a single merge record: the peer's apply
    // ran `DELETE FROM directive_phases WHERE directive_id = ?`, so
    // deleting step 2 on one device erased BOTH phases on every other
    // device. It also made the merge head coarse, so an op for step 1
    // lost to an unrelated step-2 op that merely carried a newer HLC.
    for step in [1i64, 2] {
        assert!(
            tombs.iter().any(|(t, rec, _)| {
                t == "directive_phases" && *rec == crate::crdt::phase_record_id(&d.id, step)
            }),
            "missing a per-step tombstone for step {step}: {after:?}"
        );
        assert!(
            !tombs
                .iter()
                .any(|(t, rec, _)| t == "directive_phases" && *rec == d.id),
            "a whole-directive phase tombstone would erase every step on peers"
        );
    }
    assert!(tombs
        .iter()
        .any(|(t, rec, _)| t == "directives" && *rec == d.id));

    // Local delete memory is recorded too, so a later stale upsert for
    // the same record cannot resurrect the row.
    let head_tomb: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT tombstone FROM record_heads WHERE table_name = 'directives' AND record_id = ?1",
            [&d.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(head_tomb, 1, "record_heads must remember the tombstone");
}

#[test]
fn deleting_a_missing_row_is_not_found_and_enqueues_nothing() {
    let r = setup();
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let before = r.pending_outbox(1024).unwrap().len();
    let err = r.delete_directive("directive-nope", Some(&identity));
    assert!(matches!(err, Err(crate::store::StoreError::NotFound(_))));
    assert_eq!(
        r.pending_outbox(1024).unwrap().len(),
        before,
        "a no-op delete must not enqueue an op"
    );
}

#[test]
fn delete_check_in_and_goal_tombstone_too() {
    let r = setup();
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let (goal, milestones) = goal_with_milestones(&r);
    let directive = r
        .create_directive(
            &milestones[0].id,
            "T",
            None,
            10,
            1,
            "2026-09-13",
            &[],
            Some(&identity),
        )
        .unwrap();
    let ci = r
        .upsert_check_in("2026-09-13", CheckInOutcome::Done, None, Some(&identity))
        .unwrap();
    r.delete_check_in(&ci.id, Some(&identity)).unwrap();
    // Leaf-first again: directives hang off milestones, milestones off
    // the goal. Delete the whole chain, then the goal.
    r.delete_directive(&directive.id, Some(&identity)).unwrap();
    for m in &milestones {
        r.delete_milestone(&m.id, Some(&identity)).unwrap();
    }
    r.delete_goal(&goal.id, Some(&identity)).unwrap();
    assert!(r.goal(&goal.id).unwrap().is_none());

    let tombs: Vec<(String, String)> = decoded_outbox(&r, &identity)
        .into_iter()
        .filter(|(_, _, t)| *t)
        .map(|(t, r, _)| (t, r))
        .collect();
    assert!(
        tombs.contains(&("check_ins".to_string(), ci.id.clone())),
        "check-in tombstone missing: {tombs:?}"
    );
    assert!(
        tombs.contains(&("goals".to_string(), goal.id.clone())),
        "goal tombstone missing: {tombs:?}"
    );
}

// ---------------------------------------------------------------------------
// CORE-7: write-boundary gaps
// ---------------------------------------------------------------------------

#[test]
fn save_settings_rejects_a_dangerous_relay_url() {
    let r = setup();
    // `net::validate_relay_url` is the SSRF policy; save_settings must
    // apply it itself, because pull-apply can write `relay_url` from a
    // peer without passing through the shell.
    for bad in [
        "file:///etc/passwd",
        "http://169.254.169.254/latest/meta-data/",
        "http://user:pw@example.com/",
        "javascript:alert(1)",
    ] {
        let s = AppSettings {
            relay_url: Some(bad.into()),
            ..AppSettings::default()
        };
        assert!(
            matches!(
                r.save_settings(&s, None),
                Err(crate::store::StoreError::Invalid(_))
            ),
            "must reject {bad}"
        );
    }
    // A legitimate URL still persists.
    let ok = AppSettings {
        relay_url: Some("https://relay.example.com".into()),
        ..AppSettings::default()
    };
    r.save_settings(&ok, None).unwrap();
    assert_eq!(
        r.settings().unwrap().relay_url.as_deref(),
        Some("https://relay.example.com")
    );
    // Empty means "no relay configured" and stays allowed.
    let none = AppSettings {
        relay_url: Some("  ".into()),
        ..AppSettings::default()
    };
    r.save_settings(&none, None).unwrap();
}

#[test]
fn an_oversized_write_is_rejected_rather_than_enqueued_undrainable() {
    // The relay 400s any sealed op over 256 KiB, which would leave the op
    // in the outbox forever and wedge every drain. The write must fail
    // (and roll back) instead.
    let r = setup();
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let (_goal, _) = goal_with_milestones(&r);
    let huge = "x".repeat(crate::domain::MAX_DESCRIPTION_CHARS + 1);
    // First line of defence: the write-boundary text budget (#64).
    assert!(
        r.create_goal("Big", Some(&huge), None, Some(&identity))
            .is_err(),
        "an over-budget description must be rejected at the text boundary"
    );

    // Second line of defence: bypass the text budget to prove the
    // sealed-size guard is live code, not an unreachable belt-and-braces.
    let g2 = r
        .create_goal("Sealed cap", None, None, Some(&identity))
        .unwrap();
    let payload = serde_json::json!({
        "title": "x".repeat(400 * 1024),
    });
    let err = r.enqueue_outbox(&identity, "goals", &g2.id, &payload);
    assert!(
        matches!(err, Err(crate::store::StoreError::Invalid(ref m)) if m.contains("undrainable")),
        "expected the sealed-cap rejection, got {err:?}"
    );
    // Nothing undrainable was left behind.
    assert!(
        r.pending_outbox(1024)
            .unwrap()
            .iter()
            .all(|op| op.encrypted_payload.len() <= 256 * 1024),
        "no oversized op may reach the outbox"
    );
}

#[test]
fn active_goals_with_progress_counts_only_completed_milestones() {
    let r = setup();
    let (g, ms) = goal_with_milestones(&r);
    r.set_milestone_status(&ms[0].id, MilestoneStatus::Completed, None)
        .unwrap();
    r.set_milestone_status(&ms[1].id, MilestoneStatus::Active, None)
        .unwrap();
    let rows = r.active_goals_with_progress().unwrap();
    assert_eq!(rows.len(), 1);
    let p = &rows[0];
    assert_eq!(p.id, g.id);
    assert_eq!(p.title, "Ship Worldline v0.1");
    assert_eq!(p.target_date.as_deref(), Some("2026-10-01"));
    // `active` is not `completed`: a half-run milestone is not progress.
    assert_eq!((p.milestones_done, p.milestones_total), (1, 2));
}

#[test]
fn active_goals_with_progress_keeps_a_goal_with_no_milestones() {
    let r = setup();
    let g = r.create_goal("Unplanned", None, None, None).unwrap();
    let rows = r.active_goals_with_progress().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, g.id);
    assert_eq!(rows[0].target_date, None);
    assert_eq!((rows[0].milestones_done, rows[0].milestones_total), (0, 0));
}

#[test]
fn active_goals_with_progress_excludes_achieved_and_archived_goals() {
    let r = setup();
    let kept = r.create_goal("Live", None, None, None).unwrap();
    let achieved = r.create_goal("Shipped", None, None, None).unwrap();
    let archived = r.create_goal("Dropped", None, None, None).unwrap();
    r.set_goal_status(&achieved.id, GoalStatus::Achieved, None)
        .unwrap();
    r.set_goal_status(&archived.id, GoalStatus::Archived, None)
        .unwrap();
    let rows = r.active_goals_with_progress().unwrap();
    assert_eq!(
        rows.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        vec![kept.id.as_str()],
        "only `active` goals belong in the panel's goal list"
    );
}

#[test]
fn active_goals_with_progress_is_newest_first() {
    let r = setup();
    let a = r.create_goal("First", None, None, None).unwrap();
    let b = r.create_goal("Second", None, None, None).unwrap();
    let c = r.create_goal("Third", None, None, None).unwrap();
    let rows = r.active_goals_with_progress().unwrap();
    let order: Vec<&str> = rows.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(order, vec![c.id.as_str(), b.id.as_str(), a.id.as_str()]);
}

#[test]
fn bailout_log_round_trips_every_reason_and_joins_the_goal() {
    let r = setup();
    let (g, ms) = goal_with_milestones(&r);
    let d1 = r
        .create_directive(
            &ms[0].id,
            "Client sign-off",
            None,
            30,
            1,
            "2026-09-13",
            &[],
            None,
        )
        .unwrap();
    let d2 = r
        .create_directive(
            &ms[1].id,
            "Rewrite the parser",
            None,
            30,
            1,
            "2026-09-13",
            &[],
            None,
        )
        .unwrap();
    let d3 = r
        .create_directive(&ms[1].id, "Long day", None, 30, 1, "2026-09-13", &[], None)
        .unwrap();
    let recorded = [
        r.record_bailout(
            &d1.id,
            BailoutReason::ExternalDependency,
            Some("awaiting client"),
            None,
        )
        .unwrap(),
        r.record_bailout(&d2.id, BailoutReason::MiscalculatedScope, None, None)
            .unwrap(),
        r.record_bailout(&d3.id, BailoutReason::EnergyDepletion, None, None)
            .unwrap(),
    ];

    let rows = r.bailout_log(10).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        vec![
            recorded[2].id.as_str(),
            recorded[1].id.as_str(),
            recorded[0].id.as_str()
        ],
        "the ledger must read newest-first"
    );
    let by_id = |id: &str| rows.iter().find(|e| e.id == id).unwrap();
    let mut reasons: Vec<BailoutReason> = rows.iter().map(|e| e.reason).collect();
    reasons.sort_by_key(|reason| reason.as_str());
    assert_eq!(
        reasons,
        vec![
            BailoutReason::EnergyDepletion,
            BailoutReason::ExternalDependency,
            BailoutReason::MiscalculatedScope,
        ],
        "every CHECK-permitted reason must survive the enum mapping"
    );
    for b in &recorded {
        let e = by_id(&b.id);
        assert_eq!(e.reason, b.reason);
        assert_eq!(e.note, b.note);
        assert_eq!(e.goal_id, g.id);
        assert_eq!(e.goal_title, "Ship Worldline v0.1");
    }
    assert_eq!(by_id(&recorded[0].id).directive_id, d1.id);
    assert_eq!(by_id(&recorded[0].id).directive_title, "Client sign-off");
    assert_eq!(by_id(&recorded[1].id).directive_title, "Rewrite the parser");
    assert_eq!(by_id(&recorded[2].id).directive_title, "Long day");
}

#[test]
fn bailout_log_flags_only_the_directives_still_parked() {
    // Engine semantics decide which bailouts are permanent: an external
    // dependency blocks the directive for good, a miscalculated scope
    // is requeued, and an energy bailout is skipped. The panel badges on
    // current state, so the reason alone must not set the flag.
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let parked = r
        .create_directive(
            &ms[0].id,
            "Waiting on vendor",
            None,
            30,
            1,
            "2026-09-13",
            &[],
            None,
        )
        .unwrap();
    let requeued = r
        .create_directive(&ms[0].id, "Resized", None, 30, 1, "2026-09-13", &[], None)
        .unwrap();
    let skipped = r
        .create_directive(&ms[1].id, "Ran dry", None, 30, 1, "2026-09-13", &[], None)
        .unwrap();
    r.set_directive_state(&parked.id, DirectiveState::Blocked, None)
        .unwrap();
    r.set_directive_state(&requeued.id, DirectiveState::Queued, None)
        .unwrap();
    r.set_directive_state(&skipped.id, DirectiveState::Skipped, None)
        .unwrap();
    r.record_bailout(&parked.id, BailoutReason::ExternalDependency, None, None)
        .unwrap();
    r.record_bailout(&requeued.id, BailoutReason::MiscalculatedScope, None, None)
        .unwrap();
    r.record_bailout(&skipped.id, BailoutReason::EnergyDepletion, None, None)
        .unwrap();

    let rows = r.bailout_log(10).unwrap();
    let flag = |id: &str| {
        rows.iter()
            .find(|e| e.directive_id == id)
            .unwrap()
            .still_blocked
    };
    assert!(flag(&parked.id));
    assert!(!flag(&requeued.id));
    assert!(!flag(&skipped.id));
}

#[test]
fn bailout_log_derives_a_well_formed_local_date() {
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let d = r
        .create_directive(&ms[0].id, "T", None, 10, 1, "2026-09-13", &[], None)
        .unwrap();
    let b = r
        .record_bailout(&d.id, BailoutReason::EnergyDepletion, None, None)
        .unwrap();
    let rows = r.bailout_log(10).unwrap();
    let date = rows[0].date.clone();
    assert!(
        chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_ok(),
        "derived date {date:?} is not YYYY-MM-DD"
    );
    // Recorded moments ago, so it is today — allow the midnight rollover.
    let today = today_local();
    let yesterday = date_plus_days(&today, -1).unwrap();
    assert!(
        date == today || date == yesterday,
        "expected {today:?} (or {yesterday:?} across a rollover), got {date:?}"
    );

    // A degenerate clock (physical 0 — what migration 0004 backfills)
    // must degrade to the epoch, not panic and not drop the row.
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE bailouts SET hlc_timestamp = '00000000000000000000.00000.00000' WHERE id = ?1",
            [&b.id],
        )
        .unwrap();
    let rows = r.bailout_log(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        chrono::NaiveDate::parse_from_str(&rows[0].date, "%Y-%m-%d").is_ok(),
        "a zeroed clock must still yield a parseable date, got {:?}",
        rows[0].date
    );
}

#[test]
fn bailout_log_clamps_rather_than_rejects_an_oversized_limit() {
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let d = r
        .create_directive(&ms[0].id, "T", None, 10, 1, "2026-09-13", &[], None)
        .unwrap();
    r.record_bailout(&d.id, BailoutReason::EnergyDepletion, None, None)
        .unwrap();
    assert!(r.bailout_log(0).unwrap().is_empty());
    // A panel passing `usize::MAX` (e.g. "show everything") must get the
    // ledger, not a `StoreError::Invalid` it cannot act on.
    assert_eq!(r.bailout_log(usize::MAX).unwrap().len(), 1);
}

#[test]
fn bailout_log_maps_a_corrupt_reason_to_an_error_not_a_panic() {
    // The CHECK constraint is the first line of defence, but the mapper
    // is what a row that predates it (or arrives via remote LWW apply)
    // hits — it must surface `BadEnum` as a store error, never abort.
    let r = setup();
    let (_, ms) = goal_with_milestones(&r);
    let d = r
        .create_directive(&ms[0].id, "T", None, 10, 1, "2026-09-13", &[], None)
        .unwrap();
    let b = r
        .record_bailout(&d.id, BailoutReason::EnergyDepletion, None, None)
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute_batch(
            "ALTER TABLE bailouts RENAME TO bailouts_guarded;
             CREATE TABLE bailouts (
                 id TEXT PRIMARY KEY NOT NULL,
                 directive_id TEXT NOT NULL,
                 reason TEXT NOT NULL,
                 note TEXT,
                 hlc_timestamp TEXT NOT NULL
             );
             INSERT INTO bailouts SELECT * FROM bailouts_guarded;",
        )
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE bailouts SET reason = 'not_a_reason' WHERE id = ?1",
            [&b.id],
        )
        .unwrap();
    let err = r.bailout_log(10);
    assert!(
        matches!(err, Err(store::StoreError::Sqlite(_))),
        "a corrupt reason must map to a store error, got {err:?}"
    );
}
