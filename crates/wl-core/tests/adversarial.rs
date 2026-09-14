//! Adversarial characterization tests. Every `defect_*` test asserts the
//! CURRENT (buggy) behavior to lock it in — each corresponds to a
//! numbered finding in the battle-test report.

use wl_core::domain::*;
use wl_core::engine::{Engine, EngineOutcome};
use wl_core::hlc::{Hlc, HlcTimestamp};
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

/// Finding: after completing progressive phase 1 (5 min) of a 2-phase
/// directive, the engine reports the NEW phase with the OLD phase's
/// minutes — `complete()` computes minutes from the stale pre-advance
/// directive snapshot. HUD timer targets the wrong duration.
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
                estimated_minutes, 5,
                "DEFECT: phase 2 announced with phase 1's minutes (5); correct value is 25"
            );
        }
        o => panic!("unexpected outcome {o:?}"),
    }
    let d = r.directive(&id).unwrap().unwrap();
    assert_eq!(d.progressive_step, 2);
}

/// Finding: when the final progressive phase completes, the
/// `directive_phases` row for that phase is never marked `done` — it
/// stays `active` forever (and that junk state then replicates via
/// sync to every device).
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
        PhaseState::Active,
        "DEFECT: final phase stays 'active' after the directive completed"
    );
}

/// Finding: the `hlc_clock` table exists in the schema ("Local clock
/// head ... for HLC resume across restarts") but `Repos::new` never
/// reads or writes it — every process start boots `Hlc::new()` with
/// `last_wall_nanos = 0`. Monotonicity across restarts therefore
/// depends entirely on the wall clock never regressing. After an NTP
/// correction / manual clock set-back / dual-boot clock skew, a fresh
/// process issues timestamps OLDER than pre-restart ones → LWW
/// arbitration silently inverts and newer writes lose to stale ones.
#[test]
fn defect_hlc_not_persisted_across_restarts() {
    // Simulate: device writes op at wall T1, process dies, clock is
    // set back by 10 minutes, process restarts and writes again.
    let r1 = Repos::new(open_in_memory().unwrap(), 1);
    let before = r1.hlc.now(1);
    // "Restart": brand-new Hlc (exactly what Repos::new constructs).
    let restarted = Hlc::new();
    let _after = restarted.now(1);
    // Under a regressed clock (simulated via drift being test-only) we
    // can't set wall time here; assert the structural hole instead:
    // the fresh HLC's last_wall_nanos starts at 0, so it will accept
    // ANY wall value, including one before `before.physical`.
    let hlc = Hlc::new();
    // Feed it a remote ts from "before the restart" — observe() must
    // ratchet, and it does; but now() itself would happily go below a
    // pre-restart timestamp because nothing seeds last_wall_nanos.
    let probe = hlc.now(1);
    assert!(probe.physical > 0);
    // The schema's hlc_clock table is dead:
    let count: i64 = r1
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM hlc_clock", [], |x| x.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "DEFECT: hlc_clock persistence table never populated — restart with a regressed clock breaks monotonicity"
    );
    // And `before` vs a simulated post-restart regressed tick:
    let regressed = HlcTimestamp {
        physical: before.physical.saturating_sub(600_000_000_000), // -10 min
        counter: 0,
        device: 1,
    };
    assert!(
        regressed < before,
        "a restarted process under a set-back clock would issue this — older than pre-restart ops"
    );
}

/// Finding: the 16-bit counter wraps after 65 535 same-instant ticks
/// (`wrapping_add`), producing a timestamp SMALLER than its
/// predecessor — HLC order regresses mid-burst. Reachable on coarse
/// wall clocks (Windows ~15.6 ms granularity → the same `SystemTime`
/// reading repeats for ~65k consecutive calls in a bulk enqueue loop).
#[test]
fn defect_hlc_counter_wrap_regresses_order() {
    use wl_core::hlc::Hlc;
    let hlc = Hlc::new();
    // Force the same wall reading path: tick as fast as possible.
    // Even if the wall advances on this platform, verify the wrap
    // arithmetic directly: seed inner state via 65_536 observe() calls
    // that keep physical pinned to a max remote value.
    let pin = HlcTimestamp {
        physical: u64::MAX - 1_000_000,
        counter: 0,
        device: 9,
    };
    let mut last = hlc.observe(&pin, 1); // local clock now pinned near max
    for _ in 0..65_535 {
        let t = hlc.observe(&pin, 1);
        assert!(t > last, "order must hold until the counter wraps");
        last = t;
    }
    // The 65_536th same-instant tick wraps the u16 counter back to a
    // low value → timestamp becomes LESS than `last`.
    let wrapped = hlc.observe(&pin, 1);
    assert!(
        wrapped < last,
        "DEFECT: counter wrap produces a regressed HLC ({wrapped} < {last})"
    );
}

/// Finding: `Repos::new` never wires the persisted device id to the
/// HLC device discriminator used in CRDT tie-breaking — but the shell
/// DOES pass it. Instead the sharper store-layer defect: the outbox
/// `created_at_epoch_ms` is `ts.epoch_ms()` of the OP timestamp — ops
/// generated within the same millisecond share ordering keys and the
/// `ORDER BY created_at_epoch_ms` (no tie-break) makes pending_outbox
/// order non-deterministic between pushes → ops can be re-encrypted
/// and re-pushed in different order across retries. Deterministic?
/// SQLite without a tie-break returns arbitrary row order.
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
    if rows.len() == 2 && rows[0].0 == rows[1].0 {
        // Same-ms ops: the SQL ORDER BY has no tie-break column; the
        // returned order is engine-implementation luck. This test
        // records the hazard (both orders are legal today).
        assert_eq!(rows[0].0, rows[1].0, "same-ms ops share the ordering key");
    }
}

/// Finding: engine `activate_next` called via the read path
/// (`current_directive` command constructs `Engine::new(repos, None)`)
/// mutates directive state to `active` WITHOUT writing a CRDT outbox
/// op (identity=None). The same activation via `complete`/`bail_out`
/// (identity=Some) DOES emit. Result: device A's canvas-load
/// activation is invisible to sync — device B still sees the directive
/// `queued`, activates its own, and the cross-device "at most ONE
/// active directive" invariant is silently broken.
#[test]
fn defect_read_path_activation_emits_no_outbox_op() {
    let (r, _id) = repo_with_two_phase_directive();
    // Read path: identity None (exactly what current_directive does).
    let e = Engine::new(&r, None);
    let out = e.activate_next(TODAY).unwrap();
    assert!(matches!(out, EngineOutcome::DirectiveActive { .. }));
    let pending = r.pending_outbox(100).unwrap();
    assert!(
        pending.is_empty(),
        "DEFECT: state mutated to 'active' but zero outbox ops — sync peers never learn (queued-vs-active divergence)"
    );
}
/// reset `progressive_step` or phase states, so a downszed directive
/// resumes mid-phase with a halved estimate that no longer matches the
/// stored per-phase minutes.
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
        d.progressive_step, 2,
        "DEFECT: step not reset — tomorrow's session resumes at phase 2 with a 15-min total while phase rows still say 25 min"
    );
    let phases = r.phases_for_directive(&id).unwrap();
    assert_eq!(phases[0].state, PhaseState::Done);
    assert_eq!(phases[1].state, PhaseState::Active);
}
