//! Engine integration tests — Stackelberg invariants.

use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::engine::{Engine, EngineOutcome, RecoveryAction};
use crate::store::open_in_memory;
use crate::store::repo::Repos;

/// Shared fixture: repo + goal + 2 milestones, first with directives.
pub(crate) fn setup_full() -> Repos {
    let conn = open_in_memory().unwrap();
    let r = Repos::new(conn, 1);
    let g = r
        .create_goal("Ship Worldline v0.1", None, Some("2026-10-01"), None)
        .unwrap();
    let m1 = r
        .create_milestone(&g.id, "Crypto core", None, 0, None)
        .unwrap();
    let m2 = r
        .create_milestone(&g.id, "Directive canvas", None, 1, None)
        .unwrap();
    let _ = m2;

    // Monolithic 20-min directive today.
    r.create_directive(
        &m1.id,
        "Review crypto test vectors",
        None,
        20,
        1,
        "2026-09-13",
        &[],
        None,
    )
    .unwrap();
    // Progressive 60-min directive today (2 phases).
    r.create_directive(
        &m1.id,
        "Write 300 words on Section 2.1",
        Some("Draft prose, no editing"),
        60,
        2,
        "2026-09-13",
        &[
            (
                "Open IDE and write the function signature".into(),
                Some("5 minutes, just the signature".into()),
                5,
            ),
            ("Implement core loop logic".into(), None, 25),
        ],
        None,
    )
    .unwrap();
    r
}

const TODAY: &str = "2026-09-13";

fn engine<'a>(r: &'a Repos) -> Engine<'a> {
    Engine::new(r, None)
}

#[test]
fn single_active_directive_invariant() {
    let r = setup_full();
    let e = engine(&r);
    // First activation picks the earliest-queued directive.
    let out = e.activate_next(TODAY).unwrap();
    let EngineOutcome::DirectiveActive { directive_id, .. } = out else {
        panic!("expected activation");
    };
    // Re-activating never spawns a second active directive.
    for _ in 0..5 {
        let again = e.activate_next(TODAY).unwrap();
        assert!(
            matches!(&again, EngineOutcome::DirectiveActive { directive_id: id, .. } if *id == directive_id)
        );
    }
    let active = r.active_directive().unwrap().unwrap();
    assert_eq!(active.id, directive_id);
}

#[test]
fn completion_unlocks_next_and_completes_milestones() {
    let r = setup_full();
    let e = engine(&r);
    // Drive the engine until it goes idle: each `complete` either
    // activates, advances a progressive phase, or completes a
    // directive. Two directives today (one monolithic, one with 2
    // phases) ⇒ 1 activate + 1 complete + 1 activate + 2 phase
    // completions = 5 calls, +1 safety margin.
    let mut saw_completed = false;
    for _ in 0..8 {
        match e.complete(TODAY).unwrap() {
            EngineOutcome::DirectiveCompleted { .. } => saw_completed = true,
            EngineOutcome::DirectiveActive { .. } => {}
            EngineOutcome::Idle => break,
            o => panic!("unexpected {o:?}"),
        }
    }
    assert!(saw_completed, "no directive was ever completed");
    // Milestone has no remaining directives → completed.
    let completed: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM milestones WHERE status = 'completed'",
            [],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(completed, 1);
    assert!(r.active_directive().unwrap().is_none());
}

#[test]
fn progressive_phases_advance_then_complete() {
    let r = setup_full();
    let e = engine(&r);

    // Advance past monolithic first.
    let _ = e.activate_next(TODAY).unwrap();
    e.complete(TODAY).unwrap(); // monolithic done

    // Now progressive directive activates.
    let out = e.activate_next(TODAY).unwrap();
    let EngineOutcome::DirectiveActive {
        directive_id,
        phase: Some((1, 2)),
        estimated_minutes,
        ..
    } = out
    else {
        panic!("expected progressive phase 1/2, got {out:?}");
    };
    assert_eq!(estimated_minutes, 5); // phase 1 minutes, not the 60-min total

    // Phase 1 completion → phase 2, NOT directive completion.
    let out = e.complete(TODAY).unwrap();
    assert!(
        matches!(
            out,
            EngineOutcome::DirectiveActive {
                phase: Some((2, 2)),
                ..
            }
        ),
        "got {out:?}"
    );
    let d = r.directive(&directive_id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Active);
    assert_eq!(d.progressive_step, 2);

    // Phase 2 completion closes the directive.
    let out = e.complete(TODAY).unwrap();
    assert!(
        matches!(out, EngineOutcome::DirectiveCompleted { .. }),
        "got {out:?}"
    );
    let d = r.directive(&directive_id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Completed);
}

#[test]
fn escape_hatch_requires_reason_and_downsizes() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let active_id = r.active_directive().unwrap().unwrap().id;

    // Scope miscalculation → estimate halved (floor 15), requeued tomorrow.
    let out = e
        .bail_out(
            TODAY,
            BailoutReason::MiscalculatedScope,
            Some("way bigger than planned"),
        )
        .unwrap();
    let EngineOutcome::BailedOut {
        directive_id,
        reason,
        recovery,
    } = out
    else {
        panic!("expected bailout outcome");
    };
    assert_eq!(directive_id, active_id);
    assert_eq!(reason, BailoutReason::MiscalculatedScope);
    let Some(RecoveryAction::DownsizeAndRequeue { new_minutes, .. }) = recovery else {
        panic!("expected downsize");
    };
    assert_eq!(new_minutes, 15); // 20/2 = 10, floored at 15

    let d = r.directive(&active_id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Queued);
    assert_eq!(d.scheduled_for_date, "2026-09-14");
    assert_eq!(d.estimated_minutes, 15);

    // A bailout ledger row exists with the categorized reason.
    let conn = r.conn.lock().unwrap();
    let reasons: Vec<String> = conn
        .prepare("SELECT reason FROM bailouts WHERE directive_id = ?1")
        .unwrap()
        .query_map([&active_id], |x| x.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(reasons, vec!["miscalculated_scope".to_string()]);
}

#[test]
fn bailout_external_dependency_blocks_and_advances() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let active_id = r.active_directive().unwrap().unwrap().id;

    let out = e
        .bail_out(
            TODAY,
            BailoutReason::ExternalDependency,
            Some("waiting on client"),
        )
        .unwrap();
    let EngineOutcome::BailedOut {
        recovery: Some(RecoveryAction::AdvanceUnblocked { next_directive_id }),
        ..
    } = out
    else {
        panic!("expected advance-unblocked");
    };
    // The other directive exists to advance to.
    assert!(!next_directive_id.is_empty());
    assert_ne!(next_directive_id, active_id);
    let d = r.directive(&active_id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Blocked);

    // Engine advances to the unblocked thread.
    let out = e.activate_next(TODAY).unwrap();
    assert!(
        matches!(out, EngineOutcome::DirectiveActive { directive_id, .. } if directive_id == next_directive_id)
    );
}

#[test]
fn bailout_energy_offers_low_cognitive_recovery() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let active_id = r.active_directive().unwrap().unwrap().id;

    let out = e
        .bail_out(TODAY, BailoutReason::EnergyDepletion, None)
        .unwrap();
    let EngineOutcome::BailedOut {
        recovery: Some(RecoveryAction::LowCognitiveTask { .. }),
        ..
    } = out
    else {
        panic!("expected low-cognitive recovery");
    };
    let d = r.directive(&active_id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Skipped); // velocity event, not failure
}

#[test]
fn check_in_records_objectively() {
    let r = setup_full();
    let e = engine(&r);
    let out = e
        .check_in(TODAY, CheckInOutcome::Partial, Some("half done"))
        .unwrap();
    assert!(matches!(
        out,
        EngineOutcome::CheckedIn {
            outcome: CheckInOutcome::Partial,
            ..
        }
    ));
    let c = r.check_in_for_date(TODAY).unwrap().unwrap();
    assert_eq!(c.outcome, CheckInOutcome::Partial);
}

#[test]
fn idle_when_queue_empty() {
    let conn = open_in_memory().unwrap();
    let r = Repos::new(conn, 1);
    let e = Engine::new(&r, None);
    assert_eq!(e.activate_next(TODAY).unwrap(), EngineOutcome::Idle);
    // Future-scheduled directives do not activate early.
    let g = r.create_goal("G", None, None, None).unwrap();
    let m = r.create_milestone(&g.id, "M", None, 0, None).unwrap();
    r.create_directive(&m.id, "Tomorrow task", None, 20, 1, "2026-09-14", &[], None)
        .unwrap();
    assert_eq!(e.activate_next(TODAY).unwrap(), EngineOutcome::Idle);
    assert!(matches!(
        e.activate_next("2026-09-14").unwrap(),
        EngineOutcome::DirectiveActive { .. }
    ));
}

#[test]
fn overdue_directives_surface_first() {
    let r = setup_full();
    let g = r.active_goal().unwrap().unwrap();
    let m2 = r
        .milestones_for_goal(&g.id)
        .unwrap()
        .into_iter()
        .nth(1)
        .unwrap();
    r.create_directive(&m2.id, "Overdue task", None, 10, 1, "2026-09-10", &[], None)
        .unwrap();
    let e = engine(&r);
    let out = e.activate_next(TODAY).unwrap();
    assert!(
        matches!(out, EngineOutcome::DirectiveActive { ref directive_id, .. } if directive_id.starts_with("dir-"))
    );
    // The overdue one wins: check its scheduled date.
    let d = r.active_directive().unwrap().unwrap();
    assert_eq!(d.scheduled_for_date, "2026-09-10");
}

#[test]
fn outbox_write_through_on_domain_writes() {
    // Simulate the shell layer: after engine transitions, enqueue ops.
    let r = setup_full();
    let id = Identity::generate().unwrap();
    let e = engine(&r);
    let out = e.activate_next(TODAY).unwrap();
    if let EngineOutcome::DirectiveActive { directive_id, .. } = out {
        let d = r.directive(&directive_id).unwrap().unwrap();
        r.enqueue_outbox(
            &id,
            "directives",
            &d.id,
            &serde_json::json!({"state": "active"}),
        )
        .unwrap();
    }
    let pending = r.pending_outbox(100).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].table_name, "directives");
    // Fully offline flow: nothing pushed, pending stays durable.
    assert_eq!(r.pending_outbox(100).unwrap().len(), 1);
}

#[test]
fn engine_rejects_malformed_dates_before_any_write() {
    let r = setup_full();
    let e = engine(&r);
    assert!(e.check_in("09/18/2026", CheckInOutcome::Done, None).is_err());
    assert!(e.check_in("2026-02-30", CheckInOutcome::Done, None).is_err());
    assert!(r.check_in_for_date("09/18/2026").unwrap().is_none());
    assert!(e
        .bail_out("not-a-date", BailoutReason::EnergyDepletion, None)
        .is_err());
}
