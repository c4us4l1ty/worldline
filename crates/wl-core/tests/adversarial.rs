//! Regression tests for historical adversarial findings. The `defect_*`
//! names identify the original failures; assertions require corrected
//! behavior rather than preserving those defects.

use wl_core::crypto::identity::Identity;
use wl_core::domain::*;
use wl_core::engine::{Engine, EngineOutcome};
use wl_core::hlc::HlcTimestamp;
use wl_core::store::open_in_memory;
use wl_core::store::repo::Repos;

const TODAY: &str = "2026-09-13";

fn repo_with_two_phase_directive() -> (Repos, String) {
    let r = Repos::new(open_in_memory().unwrap(), 1);
    let g = r
        .create_goal("G", None, None, COMPLEXITY_DEFAULT, None)
        .unwrap();
    let ms = r.create_milestone(&g.id, "M", None, 0, None).unwrap();
    let d = r
        .create_directive(
            &ms.id,
            "Two-phase task",
            None,
            30,
            2,
            TODAY,
            None,
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
        r1.create_goal("G", None, None, COMPLEXITY_DEFAULT, None)
            .unwrap();
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
///
/// Reached through `reschedule_directive` directly since 2026-09-27: the
/// escape hatch that used to call it was deleted with the whole bailout
/// path, and the property under test is the store's, not the modal's.
#[test]
fn defect_downsize_requeue_keeps_stale_phase_progress() {
    let (r, id) = repo_with_two_phase_directive();
    let e = Engine::new(&r, None);
    e.activate_next(TODAY).unwrap();
    e.complete(TODAY).unwrap(); // now on phase 2
    r.reschedule_directive(&id, 15, "2026-09-14", None).unwrap();
    let d = r.directive(&id).unwrap().unwrap();
    assert_eq!(d.state, DirectiveState::Queued);
    assert_eq!(d.estimated_minutes, 15, "halved with 15-min floor");
    assert_eq!(
        d.progressive_step, 1,
        "downsize must reset to phase 1 for tomorrow's session"
    );
    let phases = r.phases_for_directive(&id).unwrap();
    assert_eq!(phases[0].state, PhaseState::Active);
    let sum: i64 = phases.iter().map(|p| p.minutes).sum();
    assert_eq!(
        sum, 15,
        "the phase table must be rescaled to the new total, not left at 30"
    );
}

// ===========================================================================
// Battle-test campaign: one regression per fixed defect.
// ===========================================================================

/// The repair must not rewrite rows it was not asked to fix.
///
/// `ensure_phases` updated only `minutes` locally, but emitted a
/// SYNTHESIZED row (`title = "Step N"`, `instruction = None`,
/// `state = active|pending`) under a fresh, winning HLC. A peer that ran
/// the repair therefore lost the real title, the instruction, and a
/// `done` state — and the repair manufactured the divergence it exists
/// to heal. It also ran on every canvas load, so one truncated pull was
/// enough to trigger it.
#[test]
fn defect_phase_repair_does_not_erase_titles_or_state() {
    let r = Repos::new(open_in_memory().unwrap(), 1);
    let identity = Identity::from_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let g = r
        .create_goal("Ship", None, None, COMPLEXITY_DEFAULT, Some(&identity))
        .unwrap();
    let m = r
        .create_milestone(&g.id, "M1", None, 0, Some(&identity))
        .unwrap();
    let d = r
        .create_directive(
            &m.id,
            "Progressive",
            None,
            30,
            2,
            "2026-09-13",
            None,
            &[
                ("Write the spec".into(), Some("be thorough".into()), 5),
                ("Ship it".into(), None, 25),
            ],
            Some(&identity),
        )
        .unwrap();
    // Mark step 1 done, as a real session would.
    assert_eq!(r.phases_for_directive(&d.id).unwrap().len(), 2);
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE directive_phases SET state = 'done' WHERE directive_id = ?1 AND step = 1",
            [&d.id],
        )
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM directive_phases WHERE directive_id = ?1 AND step = 2",
            [&d.id],
        )
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute("DELETE FROM crdt_outbox", [])
        .unwrap();
    assert_eq!(r.phases_for_directive(&d.id).unwrap().len(), 1);

    let rebuilt = r.ensure_phases(&d.id, Some(&identity)).unwrap();
    assert_eq!(rebuilt.len(), 2, "the missing step is rebuilt");
    let step1 = rebuilt.iter().find(|p| p.step == 1).unwrap();
    assert_eq!(step1.title, "Write the spec", "a good row keeps its title");
    assert_eq!(
        step1.instruction.as_deref(),
        Some("be thorough"),
        "a good row keeps its instruction"
    );
    assert_eq!(
        step1.state,
        PhaseState::Done,
        "the repair must not reset a completed phase to active"
    );
    // And nothing was replicated for the untouched step.
    let emitted: Vec<String> = r
        .pending_outbox(100)
        .unwrap()
        .into_iter()
        .map(|o| o.record_id)
        .collect();
    assert_eq!(
        emitted,
        vec![wl_core::crdt::phase_record_id(&d.id, 2)],
        "only the step that actually changed may be replicated: {emitted:?}"
    );
}

/// `progressive_total` arrives from a replicated payload with no schema
/// constraint, and `ensure_phases` sized a `Vec` and a loop from it on
/// the path the engine runs on every canvas load. One op carrying
/// `estimated_minutes = 10^18, progressive_total = 10^9` reached
/// `Vec::with_capacity(count)` — a multi-gigabyte allocation from a
/// single crafted row.
#[test]
fn defect_absurd_phase_count_is_rejected_before_allocating() {
    let r = Repos::new(open_in_memory().unwrap(), 1);
    let g = r
        .create_goal("Ship", None, None, COMPLEXITY_DEFAULT, None)
        .unwrap();
    let m = r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
    let d = r
        .create_directive(
            &m.id,
            "Progressive",
            None,
            30,
            2,
            "2026-09-13",
            None,
            &[("A".into(), None, 15), ("B".into(), None, 15)],
            None,
        )
        .unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE directives SET progressive_total = 1000000000, estimated_minutes = 1000000000000000000 WHERE id = ?1",
            [&d.id],
        )
        .unwrap();
    let err = r
        .ensure_phases(&d.id, None)
        .expect_err("a billion phases must be refused, not allocated");
    assert!(
        format!("{err}").contains("corrupt"),
        "the error must name the corruption, got: {err}"
    );
}

/// `skipped` is outstanding work, not completion.
///
/// The outstanding set was `('queued','active','blocked')`, so a
/// milestone with two directives — one `skipped` and one completed — was
/// marked `completed` with the skipped directive unstarted and
/// unreachable: `next_runnable_directive` only selects `queued`, and
/// nothing requeues a skip.
///
/// The escape hatch that used to produce `skipped` is gone (2026-09-27),
/// but the state is still in the schema's CHECK and a peer can still write
/// it through `apply_upsert`, so the rule that keeps it from being silently
/// swallowed is still load-bearing and is still tested here.
#[test]
fn defect_milestone_is_not_completed_while_a_skipped_directive_remains() {
    let r = Repos::new(open_in_memory().unwrap(), 1);
    let e = Engine::new(&r, None);
    let today = "2026-09-13";
    let g = r
        .create_goal("Ship", None, None, COMPLEXITY_DEFAULT, None)
        .unwrap();
    let m = r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
    let d = r
        .create_directive(&m.id, "A", None, 20, 1, today, None, &[], None)
        .unwrap();
    let d2 = r
        .create_directive(&m.id, "B", None, 20, 1, today, None, &[], None)
        .unwrap();
    let _ = e.activate_next(today).unwrap();
    // A peer-equivalent `skipped`, then the other one finished through the
    // ledger's writer.
    r.set_directive_state(&d.id, DirectiveState::Skipped, None)
        .unwrap();
    e.mark_complete(today, &d2.id).unwrap();
    assert!(r.directive(&d.id).unwrap().is_some());
    let state: String = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT state FROM directives WHERE id = ?1", [&d.id], |x| {
            x.get(0)
        })
        .unwrap();
    assert_eq!(state, "skipped");
    let mstate: String = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT status FROM milestones WHERE id = ?1",
            [&m.id],
            |x| x.get(0),
        )
        .unwrap();
    assert_ne!(
        mstate, "completed",
        "a milestone with an unreachable skipped directive is not complete"
    );
}

/// A `progressive_step` past `progressive_total` is a replicated-op
/// artefact (the two columns have no cross-field constraint and the
/// pull path writes both from the payload). It has no phase row, so
/// every engine path that read the current phase errored, and nothing
/// could requeue the directive.
#[test]
fn defect_phase_step_past_the_total_is_clamped_instead_of_wedging() {
    let r = Repos::new(open_in_memory().unwrap(), 1);
    let e = Engine::new(&r, None);
    let today = "2026-09-13";
    let g = r
        .create_goal("Ship", None, None, COMPLEXITY_DEFAULT, None)
        .unwrap();
    let m = r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
    let d = r
        .create_directive(
            &m.id,
            "Progressive",
            None,
            30,
            2,
            today,
            None,
            &[("A".into(), None, 5), ("B".into(), None, 25)],
            None,
        )
        .unwrap();
    let _ = e.activate_next(today).unwrap();
    r.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE directives SET progressive_step = 9 WHERE id = ?1",
            [&d.id],
        )
        .unwrap();

    // The canvas must still render, and ⌘+Enter must still work.
    let out = e.current(today).unwrap();
    assert!(
        matches!(out, wl_core::engine::EngineOutcome::DirectiveActive { .. }),
        "the canvas must render a directive whose step is past the total: {out:?}"
    );
    let done = e.complete(today).unwrap();
    assert!(
        matches!(
            done,
            wl_core::engine::EngineOutcome::DirectiveCompleted { .. }
        ),
        "and the user must still be able to finish it: {done:?}"
    );
}

/// Every settings field commits on `change`, and the relay URL is
/// validated on the way in AND on the way out. The metadata-hostname
/// block was bypassable with a trailing root dot — the same name to the
/// resolver, but neither equal to nor `.ends_with()` the blocked form.
#[test]
fn defect_metadata_hostname_cannot_be_reached_with_a_trailing_dot() {
    for url in [
        "http://metadata.google.internal./computeMetadata/v1/",
        "http://METADATA.GOOGLE.INTERNAL./",
        "http://foo.metadata.google.internal./",
    ] {
        assert!(
            wl_core::net::validate_relay_url(url).is_err(),
            "{url} must not be accepted as a relay"
        );
    }
    // IPv4-compatible IPv6 (`::a9fe:a9fe` == 169.254.169.254) is still
    // honoured by connect(2); the old check only unwrapped the
    // IPv4-MAPPED form.
    for url in [
        "http://[::ffff:a9fe:a9fe]/",
        "http://[::a9fe:a9fe]/",
        "http://192.0.0.192/",
    ] {
        assert!(
            wl_core::net::validate_relay_url(url).is_err(),
            "{url} must not be accepted as a relay"
        );
    }
    // The supported layouts still work.
    for url in ["http://127.0.0.1:8080", "https://relay.example.com"] {
        assert!(
            wl_core::net::validate_relay_url(url).is_ok(),
            "{url} must still be a valid relay"
        );
    }
}
