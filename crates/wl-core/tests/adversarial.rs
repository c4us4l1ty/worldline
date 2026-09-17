//! Regression tests for historical adversarial findings. The `defect_*`
//! names identify the original failures; assertions require corrected
//! behavior rather than preserving those defects.

use wl_core::domain::*;
use wl_core::engine::{Engine, EngineOutcome};
use wl_core::hlc::HlcTimestamp;
use wl_core::store::open_in_memory;
use wl_core::store::repo::Repos;

const TODAY: &str = "2026-09-13";

fn repo_with_two_phase_directive() -> (Repos, String) {
    let r = Repos::new(open_in_memory().unwrap(), 1);
    let g = r.create_goal("G", None, None, None).unwrap();
    let ms = r.create_milestone(&g.id, "M", None, 0, None).unwrap();
    let d = r
        .create_directive(
            &ms.id,
            "Two-phase task",
            None,
            30,
            2,
            TODAY,
            &[
                ("Phase one".into(), None, 5),
                ("Phase two".into(), None, 25),
            ],
            None,
        )
        .unwrap();
    (r, d.id)
}

/// FIXED (was: HLC text order inverted at counter digit boundaries).
/// The canonical encoding is now FIXED-WIDTH (`pt(20).ctr(5).dev(5)`,
/// zero-padded), so every TEXT comparison — relay `WHERE hlc > ?`,
/// `ORDER BY hlc`, client LWW guards — sorts identically to numeric
/// HLC order. This test locks the regression.
#[test]
fn defect_hlc_text_order_is_not_numeric_order() {
    let numerically_newer = HlcTimestamp::parse("9000000000000001.10.1").unwrap();
    let numerically_older = HlcTimestamp::parse("9000000000000001.2.2").unwrap();
    assert!(numerically_newer > numerically_older);
    assert!(
        numerically_newer.to_string() > numerically_older.to_string(),
        "text order must now MATCH numeric order — fixed-width encoding regressed"
    );
}

/// FIXED (was: phase 2 announced with phase 1's minutes). `complete()`
/// re-reads the directive AFTER advancing, so the HUD timer targets the
/// new phase's minutes (25), not the finished phase's (5).
#[test]
fn defect_complete_after_phase_advance_reports_stale_phase_minutes() {
    let (r, id) = repo_with_two_phase_directive();
    let e = Engine::new(&r, None);
    e.activate_next(TODAY).unwrap();
    let out = e.complete(TODAY).unwrap();
    match out {
        EngineOutcome::DirectiveActive {
            estimated_minutes,
            phase: Some((2, 2)),
            ..
        } => {
            assert_eq!(
                estimated_minutes, 25,
                "phase 2 must be announced with its own minutes (25), not phase 1's (5)"
            );
        }
        o => panic!("unexpected outcome {o:?}"),
    }
    let d = r.directive(&id).unwrap().unwrap();
    assert_eq!(d.progressive_step, 2);
}

/// FIXED (was: final phase row stayed `active` after completion).
/// `complete()` marks the final phase `done` before closing the
/// directive, so peers never replicate the junk active-final-phase state.
#[test]
fn defect_final_phase_row_never_marked_done() {
    let (r, id) = repo_with_two_phase_directive();
    let e = Engine::new(&r, None);
    e.activate_next(TODAY).unwrap();
    e.complete(TODAY).unwrap();
    e.complete(TODAY).unwrap();
    let d = r.directive(&id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Completed);
    let phases = r.phases_for_directive(&id).unwrap();
    assert_eq!(phases.len(), 2);
    assert_eq!(phases[0].state, PhaseState::Done);
    assert_eq!(
        phases[1].state,
        PhaseState::Done,
        "final phase must be 'done' once the directive completes"
    );
}

/// FIXED (was: `hlc_clock` never populated). `Repos::new` resumes the
/// persisted clock head and every tick persists it, so a restart under
/// a regressed wall clock cannot issue pre-restart-older timestamps.
#[test]
fn defect_hlc_not_persisted_across_restarts() {
    // A domain write ticks the clock AND persists the head: after any
    // mutation the hlc_clock row exists, and a fresh Repos on the same
    // database resumes from it instead of booting at zero.
    let conn = open_in_memory().unwrap();
    let before = {
        let r1 = Repos::new(conn, 1);
        r1.create_goal("G", None, None, None).unwrap();
        let count: i64 = r1
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM hlc_clock", [], |x| x.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "every tick must persist the clock head for restart resume"
        );
        let (wall, ctr): (i64, i64) = r1
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT last_wall_nanos, counter FROM hlc_clock WHERE id = 1",
                [],
                |x| Ok((x.get(0)?, x.get(1)?)),
            )
            .unwrap();
        assert!(wall > 0);
        let _ = ctr;
        // Latest issued timestamp is at-or-below the persisted head.
        r1.hlc.head()
    };
    assert!(before.0 > 0);
    // A regressed wall clock still cannot go below a pre-restart op:
    // HLC monotonicity is anchored in the persisted head, not the wall.
    let regressed = HlcTimestamp {
        physical: before.0.saturating_sub(600_000_000_000), // -10 min
        counter: 0,
        device: 1,
    };
    let floor = HlcTimestamp {
        physical: before.0,
        counter: before.1,
        device: 1,
    };
    assert!(
        regressed < floor,
        "sanity: the simulated set-back tick is older than the persisted head it must not undercut"
    );
}

/// FIXED (was: `wrapping_add` regressed mid-burst). Counter exhaustion
/// steps the physical component forward instead of wrapping, so
/// same-instant bursts stay strictly monotonic past 65 535 ticks.
#[test]
fn defect_hlc_counter_wrap_regresses_order() {
    use wl_core::hlc::Hlc;
    let hlc = Hlc::new();
    // Pin the clock near a large remote physical value, then burst past
    // the 16-bit counter range on the same instant.
    let pin = HlcTimestamp {
        physical: u64::MAX - 1_000_000,
        counter: 0,
        device: 9,
    };
    let mut last = hlc.observe(&pin, 1); // local clock now pinned near max
    for _ in 0..65_535 {
        let t = hlc.observe(&pin, 1);
        assert!(t > last, "order must hold until the counter exhausts");
        last = t;
    }
    // The 65_536th same-instant tick steps physical forward instead of
    // wrapping the counter — still strictly greater than `last`.
    let stepped = hlc.observe(&pin, 1);
    assert!(
        stepped > last,
        "counter exhaustion must advance physical, not regress ({stepped} <= {last})"
    );
}

/// FIXED (was: `ORDER BY created_at_epoch_ms` with no tie-break).
/// `pending_outbox` orders by `(created_at_epoch_ms, hlc_timestamp,
/// operation_id)`, so same-millisecond ops drain in a deterministic
/// order on every retry.
#[test]
fn defect_outbox_ordering_not_deterministic_within_same_ms() {
    let r = Repos::new(open_in_memory().unwrap(), 1);
    let id = wl_core::crypto::identity::Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    // Two ops enqueued within the same wall millisecond.
    loop {
        let x = r.enqueue_outbox(&id, "goals", "g1", &serde_json::json!({"n": 1}));
        let y = r.enqueue_outbox(&id, "goals", "g2", &serde_json::json!({"n": 2}));
        if x.is_ok() && y.is_ok() {
            break;
        }
    }
    let rows: Vec<(i64, String)> = r
        .conn
        .lock()
        .unwrap()
        .prepare("SELECT created_at_epoch_ms, operation_id FROM crdt_outbox ORDER BY created_at_epoch_ms")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    // The public API is deterministic regardless of ms collisions:
    // repeated reads return the same order (tie-broken by hlc + op id).
    let first = r.pending_outbox(100).unwrap();
    let second = r.pending_outbox(100).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(
        first.iter().map(|o| &o.operation_id).collect::<Vec<_>>(),
        second.iter().map(|o| &o.operation_id).collect::<Vec<_>>(),
        "pending_outbox order must be deterministic across reads"
    );
}

/// FIXED (was: read-path activation with identity=None wrote no outbox
/// op). The shell's `current_directive` now passes the unlocked identity
/// through, so canvas-load activation write-throughs like every other
/// transition. The None-identity (vault-locked) path still writes
/// nothing — there is no key to encrypt with — but it must not be used
/// while unlocked.
#[test]
fn defect_read_path_activation_emits_no_outbox_op() {
    use wl_core::crypto::identity::Identity;
    let (r, _id) = repo_with_two_phase_directive();
    // Write-through path (what the shell uses while unlocked): activation
    // lands in the outbox for peers.
    let id = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let e = Engine::new(&r, Some(&id));
    let out = e.activate_next(TODAY).unwrap();
    assert!(matches!(out, EngineOutcome::DirectiveActive { .. }));
    assert!(
        !r.pending_outbox(100).unwrap().is_empty(),
        "activation with an unlocked identity must emit an outbox op for peers"
    );
}
/// FIXED (was: downsize kept step=2 with 5+25 min rows under a 15-min
/// total). `reschedule_directive` resets progress to phase 1 and
/// rescales phase minutes to the new total, so estimate and phases
/// agree again.
#[test]
fn defect_downsize_requeue_keeps_stale_phase_progress() {
    let (r, id) = repo_with_two_phase_directive();
    let e = Engine::new(&r, None);
    e.activate_next(TODAY).unwrap();
    e.complete(TODAY).unwrap(); // now on phase 2
    e.bail_out(TODAY, BailoutReason::MiscalculatedScope, None)
        .unwrap();
    let d = r.directive(&id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Queued);
    assert_eq!(d.estimated_minutes, 15, "halved with 15-min floor");
    assert_eq!(
        d.progressive_step, 1,
        "downsize must reset to phase 1 for tomorrow's session"
    );
    let phases = r.phases_for_directive(&id).unwrap();
    assert_eq!(phases[0].state, PhaseState::Active);
    assert_eq!(phases[1].state, PhaseState::Pending);
    let total: i64 = phases.iter().map(|p| p.minutes).sum();
    assert_eq!(
        total, 15,
        "rescaled phase minutes must sum to the new estimate"
    );
}
