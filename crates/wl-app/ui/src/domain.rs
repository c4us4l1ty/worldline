//! The write bounds the UI checks before sending.
//!
//! **These duplicate `wl-core::domain`.** The UI crate is standalone (its
//! own `[workspace]`, wasm-only, no tokio) and cannot depend on the core
//! crate without dragging rusqlite and a SQLite build into a wasm bundle
//! that never opens a database. So the two numbers the compose screen and
//! the plan preview have to agree on are stated here as well.
//!
//! That is a real cost and it is bounded deliberately: only the two
//! constants a text field or an estimate input can violate are mirrored, and
//! the shell re-checks both at the write boundary. The UI copy exists to
//! give an answer immediately; the shell copy is the one that is
//! authoritative. `the_ui_bounds_match_the_store` in `app.rs` pins the
//! values so the pair cannot drift silently.

/// A single directive longer than a day is a planning error, not a
/// directive. Mirrors `wl_core::domain::MAX_MINUTES`.
pub const MAX_MINUTES: i64 = 1440;

/// A task title's budget. Mirrors `wl_core::domain::MAX_TITLE_CHARS`.
///
/// Longer is not *rejected* by the store — it is clipped — so this is a
/// `maxlength` on the input rather than an error message.
pub const MAX_TITLE_CHARS: usize = 500;

/// Rejects an estimate the store would refuse, with the store's own wording.
///
/// Called from the plan preview's inline edit so a bad number is refused
/// where the user typed it, rather than arriving as a failed commit after
/// they had approved the plan.
pub fn check_minutes(field: &str, minutes: i64) -> Result<(), String> {
    if minutes <= 0 || minutes > MAX_MINUTES {
        return Err(format!("{field} must be within 1–{MAX_MINUTES} minutes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bounds_are_the_stores_bounds() {
        // The two crates cannot share a constant, so these are pinned here
        // against the values in `wl-core::domain`. If one moves, this test
        // is the thing that has to be updated deliberately.
        assert_eq!(MAX_MINUTES, 1440);
        assert_eq!(MAX_TITLE_CHARS, 500);
    }

    #[test]
    fn an_estimate_outside_the_band_is_refused_where_it_is_typed() {
        assert!(check_minutes("estimate", 25).is_ok());
        assert!(check_minutes("estimate", MAX_MINUTES).is_ok());
        for bad in [0, -1, MAX_MINUTES + 1] {
            assert!(check_minutes("estimate", bad).is_err(), "{bad}");
        }
    }
}
