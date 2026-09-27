//! Engine integration tests — Stackelberg invariants.

use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::engine::{Engine, EngineOutcome};
use crate::store::open_in_memory;
use crate::store::repo::Repos;

/// Shared fixture: repo + goal + 2 milestones, first with directives.
pub(crate) fn setup_full() -> Repos {
    let conn = open_in_memory().unwrap();
    let r = Repos::new(conn, 1);
    let g = r
        .create_goal(
            "Ship Worldline v0.1",
            None,
            Some("2026-10-01"),
            crate::domain::COMPLEXITY_DEFAULT,
            None,
        )
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
        None,
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
        None,
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

/// Walks the real user loop: load the canvas (`current`, which activates
/// whatever is runnable), then press ⌘+Enter (`complete`).
///
/// The loop used to call `complete` alone and lean on it activating the
/// next directive when none was active. That was the stray-keystroke bug
/// — ⌘+Enter on an idle canvas started a directive nobody asked for —
/// and this test had encoded the bug as the contract. Activating is the
/// canvas load's job, so that is what the test does now.
#[test]
fn completion_unlocks_next_and_completes_milestones() {
    let r = setup_full();
    let e = engine(&r);
    let mut saw_completed = false;
    for _ in 0..8 {
        match e.current(TODAY).unwrap() {
            EngineOutcome::DirectiveActive { .. } => {}
            EngineOutcome::Idle => break,
            o => panic!("unexpected {o:?}"),
        }
        match e.complete(TODAY).unwrap() {
            EngineOutcome::DirectiveCompleted { .. } => saw_completed = true,
            EngineOutcome::DirectiveActive { .. } | EngineOutcome::Idle => {}
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

// ---------------------------------------------------------------------------
// mark_complete — the ledger's only write (2026-09-27)
// ---------------------------------------------------------------------------

/// A tick closes the WHOLE task, not one phase of it.
///
/// The ledger is task-level and a tick is a statement about the task, so
/// four taps to say "done" would be a different control wearing the same
/// clothes. The loop is what makes `phase` come out at `total` rather than
/// wherever it happened to be.
#[test]
fn marking_a_progressive_task_done_closes_every_phase() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let active_id = r.active_directive().unwrap().unwrap().id;
    // Make the active directive progressive so the loop has work to do.
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE directives SET estimated_minutes = 30, progressive_total = 3 WHERE id = ?1",
            [&active_id],
        )
        .unwrap();

    let out = e.mark_complete(TODAY, &active_id).unwrap();
    assert!(matches!(out, EngineOutcome::DirectiveCompleted { .. }));
    let d = r.directive(&active_id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Completed);
    assert_eq!(
        d.progressive_step, d.progressive_total,
        "every phase must be closed, not just the current one"
    );
    let closed: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM directive_phases WHERE directive_id = ?1 AND state = 'done'",
            [&active_id],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(closed, 3, "all three phase rows are done");
}

/// The ledger can tick a task that is NOT the active one, and doing so
/// leaves the canvas alone. This is the case the canvas-only completion
/// path could not express at all.
#[test]
fn a_queued_task_can_be_marked_done_without_disturbing_the_canvas() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let active_id = r.active_directive().unwrap().unwrap().id;
    let other = r
        .runnable_directives(TODAY)
        .unwrap()
        .into_iter()
        .find(|d| d.id != active_id)
        .expect("setup_full seeds a second directive")
        .id;

    e.mark_complete(TODAY, &other).unwrap();
    assert_eq!(
        r.directive(&other).unwrap().unwrap().state,
        DirectiveState::Completed
    );
    // The active directive is untouched — the canvas still has its task.
    assert_eq!(
        r.active_directive().unwrap().unwrap().id,
        active_id,
        "ticking a queued task must not steal the canvas"
    );
}

/// A tick on a task with NO phases — the most common thing the ledger can
/// be asked to do, and the case the first version of `mark_complete` got
/// wrong: its phase-closing loop sat outside the progressive branch, so
/// every 20-minute task was sent through `advance_progressive_step` and
/// failed with "current or next phase missing".
#[test]
fn a_monolithic_task_can_be_ticked() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let id = r.active_directive().unwrap().unwrap().id;
    let d = r.directive(&id).unwrap().unwrap();
    assert_eq!(
        d.progressive_total, 1,
        "this test is only meaningful on a task with no phase rows"
    );
    assert!(r.phases_for_directive(&id).unwrap().is_empty());
    e.mark_complete(TODAY, &id).unwrap();
    assert_eq!(
        r.directive(&id).unwrap().unwrap().state,
        DirectiveState::Completed
    );
}

/// A tick is a checkbox. A second tap on a ticked row is a person
/// confirming, not a mistake, and must not produce an error the UI would
/// have to render as a failure.
#[test]
fn marking_an_already_done_task_twice_is_a_no_op() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let id = r.active_directive().unwrap().unwrap().id;
    e.mark_complete(TODAY, &id).unwrap();
    let out = e.mark_complete(TODAY, &id).unwrap();
    assert!(matches!(out, EngineOutcome::DirectiveCompleted { .. }));
    assert_eq!(
        r.directive(&id).unwrap().unwrap().state,
        DirectiveState::Completed
    );
}

/// The milestone rule from delta 200 survives the new writer: a milestone
/// is not reported done while work it contains is still unreachable.
#[test]
fn a_milestone_with_blocked_work_is_not_reported_done() {
    let r = setup_full();
    let e = engine(&r);
    e.activate_next(TODAY).unwrap();
    let ids: Vec<String> = r
        .runnable_directives(TODAY)
        .unwrap()
        .into_iter()
        .chain(std::iter::once(r.active_directive().unwrap().unwrap()))
        .map(|d| d.id)
        .collect();
    let milestone_id = r.directive(&ids[0]).unwrap().unwrap().milestone_id;
    // One finishes; the other is parked.
    r.set_directive_state(&ids[0], DirectiveState::Blocked, None)
        .unwrap();
    e.mark_complete(TODAY, &ids[1]).unwrap();
    let m = r.milestone(&milestone_id).unwrap().unwrap();
    assert_ne!(
        m.status,
        MilestoneStatus::Completed,
        "a milestone with a blocked task inside it is not done"
    );
}

#[test]
fn marking_a_missing_task_is_an_error_that_names_it() {
    let r = setup_full();
    let e = engine(&r);
    let err = e.mark_complete(TODAY, "dir-nope").unwrap_err();
    assert!(
        err.to_string().contains("dir-nope"),
        "the error must name the id the caller sent: {err}"
    );
    // A malformed date is refused before anything is written.
    assert!(e.mark_complete("not-a-date", "dir-nope").is_err());
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
    let g = r
        .create_goal("G", None, None, crate::domain::COMPLEXITY_DEFAULT, None)
        .unwrap();
    let m = r.create_milestone(&g.id, "M", None, 0, None).unwrap();
    r.create_directive(
        &m.id,
        "Tomorrow task",
        None,
        20,
        1,
        "2026-09-14",
        None,
        &[],
        None,
    )
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
    r.create_directive(
        &m2.id,
        "Overdue task",
        None,
        10,
        1,
        "2026-09-10",
        None,
        &[],
        None,
    )
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
    assert!(e
        .check_in("09/18/2026", CheckInOutcome::Done, None)
        .is_err());
    assert!(e
        .check_in("2026-02-30", CheckInOutcome::Done, None)
        .is_err());
    assert!(r.check_in_for_date("09/18/2026").unwrap().is_none());
    // The ledger's writer refuses a malformed date before touching a row.
    // It is checked at all because the date is not stored on the directive
    // — it is used to decide what runs next — so a bad one could otherwise
    // slip past the write boundary entirely.
    let id = r.runnable_directives(TODAY).unwrap()[0].id.clone();
    assert!(e.mark_complete("not-a-date", &id).is_err());
    assert_eq!(
        r.directive(&id).unwrap().unwrap().state,
        DirectiveState::Queued,
        "a refused date must not have written anything"
    );
}

// ---------------------------------------------------------------------------
// CORE-6: a progressive directive with missing phase rows self-heals
// ---------------------------------------------------------------------------

/// Builds a 2-phase, 60-minute progressive directive.
fn progressive_directive(r: &Repos, identity: Option<&Identity>) -> Directive {
    let g = r
        .create_goal(
            "Long haul",
            None,
            None,
            crate::domain::COMPLEXITY_DEFAULT,
            identity,
        )
        .unwrap();
    let m = r
        .create_milestone(&g.id, "Phase work", None, 0, identity)
        .unwrap();
    r.create_directive(
        &m.id,
        "Sixty minutes of work",
        None,
        60,
        2,
        "2099-01-01",
        None,
        &[("Warm up".into(), None, 20), ("Push".into(), None, 40)],
        identity,
    )
    .unwrap()
}

#[test]
fn missing_phase_rows_are_rebuilt_from_the_estimate() {
    let r = setup_full();
    let d = progressive_directive(&r, None);
    // Simulate the corruption: the phase rows vanish (truncated pull,
    // partial write, hand-edited dir) but the directive keeps
    // progressive_total = 2.
    r.conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM directive_phases WHERE directive_id = ?1",
            [&d.id],
        )
        .unwrap();
    assert!(r.phases_for_directive(&d.id).unwrap().is_empty());

    // The engine must now be able to read the current phase again
    // instead of failing closed with "current phase missing".
    let reparsed = r.directive(&d.id).unwrap().unwrap();
    assert!(reparsed.uses_progressive_activation());
    let e = Engine::new(&r, None);
    e.activate_next("2099-01-01")
        .expect("activate must self-heal phases");
    let now = r.active_directive().unwrap().unwrap();
    assert_eq!(now.progressive_step, 1);
    e.complete(&now.id)
        .expect("complete must survive the repair");
}

#[test]
fn rebuilt_phases_sum_to_the_estimate() {
    let r = setup_full();
    let d = progressive_directive(&r, None);
    r.conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM directive_phases WHERE directive_id = ?1",
            [&d.id],
        )
        .unwrap();

    let rebuilt = r.ensure_phases(&d.id, None).unwrap();
    assert_eq!(rebuilt.len(), 2);
    let sum: i64 = rebuilt.iter().map(|p| p.minutes).sum();
    assert_eq!(sum, d.estimated_minutes, "repair must not lose minutes");
    assert!(rebuilt.iter().all(|p| p.minutes > 0));
    // First step active, rest pending — same convention as create.
    assert_eq!(rebuilt[0].state, PhaseState::Active);
    assert_eq!(rebuilt[1].state, PhaseState::Pending);
}

#[test]
fn ensure_phases_is_idempotent_and_preserves_good_rows() {
    let r = setup_full();
    let d = progressive_directive(&r, None);
    let before = r.phases_for_directive(&d.id).unwrap();
    // Complete rows must be returned untouched — no rewrite, no churn.
    let after = r.ensure_phases(&d.id, None).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(
        before.iter().map(|p| p.minutes).collect::<Vec<_>>(),
        after.iter().map(|p| p.minutes).collect::<Vec<_>>(),
        "a healthy directive must not be rewritten"
    );
}

#[test]
fn a_directive_too_short_to_split_fails_with_an_actionable_error() {
    let r = setup_full();
    let g = r
        .create_goal(
            "Impossible",
            None,
            None,
            crate::domain::COMPLEXITY_DEFAULT,
            None,
        )
        .unwrap();
    let m = r
        .create_milestone(&g.id, "Too many steps", None, 0, None)
        .unwrap();
    // 5 minutes across 10 phases: the repair cannot invent time.
    let d = r
        .create_directive(
            &m.id,
            "Absurd",
            None,
            5,
            10,
            "2099-01-01",
            None,
            &vec![("s".into(), None, 1); 10],
            None,
        )
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM directive_phases WHERE directive_id = ?1",
            [&d.id],
        )
        .unwrap();
    let err = r.ensure_phases(&d.id, None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("corrupt") && msg.contains("reschedule"),
        "error must name the remedy, got: {msg}"
    );
}
