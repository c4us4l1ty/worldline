//! Entropy Log — the escape-hatch ledger, read back.
//!
//! The escape hatch (skill §5 "Do") demands a reason before a directive
//! can be abandoned, and that reason was previously write-only: it was
//! recorded, counted into velocity, and never shown to anyone again. This
//! page is where it comes back. The point is not a scoreboard — this
//! product bans failure states and streaks — it is the only place the
//! three reasons can be compared against each other, which is the only
//! way "I keep bailing on the same kind of thing" becomes visible before
//! it becomes a habit.
//!
//! Grouped by goal, because a bailout only means something next to the
//! thing it derailed: three bails spread over three goals is noise, three
//! on one goal is a pattern.
//!
//! Read-only by decision (2026-09-27). A blocked directive has no unblock
//! path in the engine yet (PRD delta 33), so offering a tap target here
//! would be a control that cannot do what it says. The page reports;
//! changing anything still happens on the canvas.

use dioxus::prelude::*;

use crate::app::{invoke, AppCtx, EntropyView, Screen};
use crate::icons::IconBack;

/// Human-readable name for a stored `bailouts.reason`.
///
/// The three strings are the escape modal's own titles (skill §5), reused
/// verbatim so the reason a user picked at 23:00 and the reason they read
/// here at 08:00 are the same words. An unrecognised key renders as
/// "Unclassified" rather than disappearing: a future shell adding a
/// fourth reason must not silently shrink this page.
pub fn reason_label(key: &str) -> &'static str {
    match key {
        "external_dependency" => "Dependency blocked",
        "miscalculated_scope" => "Scope miscalculation",
        "energy_depletion" => "Cognitive / energy depletion",
        _ => "Unclassified",
    }
}

/// Short form for the summary line, where three full titles would wrap.
fn reason_short(key: &str) -> &'static str {
    match key {
        "external_dependency" => "external",
        "miscalculated_scope" => "scope",
        "energy_depletion" => "energy",
        _ => "other",
    }
}

/// The three reasons in the order the escape modal presents them, so the
/// summary reads the same way every time regardless of which is most
/// common. Sorting by count would make the line jump around daily and
/// stop being scannable.
const REASON_ORDER: [&str; 3] = [
    "energy_depletion",
    "miscalculated_scope",
    "external_dependency",
];

/// One line of pattern: how many events, split by reason, plus how many
/// are still stuck.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntropySummary {
    pub total: usize,
    /// `(short label, count)`, in [`REASON_ORDER`], zeros dropped.
    pub by_reason: Vec<(&'static str, usize)>,
    pub still_blocked: usize,
}

/// Count the ledger.
///
/// Only non-zero reasons are reported. A "0 scope" on the summary is
/// noise that costs horizontal space in a 420px drawer-width page and
/// teaches the eye to skip the line.
pub fn summarize(entries: &[EntropyView]) -> EntropySummary {
    let mut s = EntropySummary {
        total: entries.len(),
        ..Default::default()
    };
    let mut counted = 0;
    for key in REASON_ORDER {
        let n = entries.iter().filter(|e| e.reason == key).count();
        if n > 0 {
            s.by_reason.push((reason_short(key), n));
            counted += n;
        }
    }
    // Anything this build does not recognise still has to be counted, or
    // the headline says "4 events" while the breakdown adds to 3 and the
    // page quietly under-reports. The remainder is surfaced as "other"
    // rather than dropped.
    let other = s.total.saturating_sub(counted);
    if other > 0 {
        s.by_reason.push(("other", other));
    }
    s.still_blocked = entries.iter().filter(|e| e.still_blocked).count();
    s
}

/// Render the summary as the single mono line under the page title.
pub fn summary_line(s: &EntropySummary) -> String {
    if s.total == 0 {
        return String::new();
    }
    let mut parts = vec![format!("{} event{}", s.total, plural(s.total))];
    for (label, n) in &s.by_reason {
        parts.push(format!("{n} {label}"));
    }
    let mut line = parts.join(" · ");
    if s.still_blocked > 0 {
        line.push_str(&format!(" · {} still blocked", s.still_blocked));
    }
    line
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Group entries under their goal, preserving the order goals first
/// appear in (the shell returns newest-first, so the most recently active
/// goal leads).
///
/// Borrows the entries rather than cloning them, and indexes the groups by
/// goal name instead of scanning for a match. The ledger is append-only
/// and nothing prunes it, so this used to be a linear `find` per entry
/// against a `Vec` of up-to-now clones — quadratic in goals, and one full
/// copy of every `EntropyView` (six `String`s each) on every render of the
/// screen. At 200 bailouts over 20 goals that is ~4 000 string comparisons
/// and 1 200 allocations per render, for a list that is only ever written
/// once per visit.
pub fn group_by_goal(entries: &[EntropyView]) -> Vec<(&str, Vec<&EntropyView>)> {
    let mut groups: Vec<(&str, Vec<&EntropyView>)> = Vec::new();
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for e in entries {
        match index.get(e.goal_title.as_str()) {
            Some(&i) => groups[i].1.push(e),
            None => {
                index.insert(e.goal_title.as_str(), groups.len());
                groups.push((e.goal_title.as_str(), vec![e]));
            }
        }
    }
    groups
}

/// Render "21 Sep" from the shell's `YYYY-MM-DD`.
///
/// A date the shell could not derive (empty, or a shape we do not
/// recognise) renders as the raw string rather than as a blank gap, so a
/// malformed row is visible instead of silently unlabelled.
fn short_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = iso.split('-');
    let (Some(_y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return iso.to_string();
    };
    let Ok(m) = m.parse::<usize>() else {
        return iso.to_string();
    };
    if !(1..=12).contains(&m) || d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
        return iso.to_string();
    }
    format!("{} {}", d.trim_start_matches('0'), MONTHS[m - 1])
}

pub fn EntropyLogScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut entries = use_signal::<Option<Vec<EntropyView>>>(|| None);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        spawn(async move {
            match invoke::<Vec<EntropyView>>("entropy_log", ()).await {
                Ok(v) => {
                    error.set(None);
                    entries.set(Some(v));
                }
                Err(e) => {
                    // A shell without the command (an older build) answers
                    // with a missing-command error, which is a real state
                    // and not a bug to hide: say so on the page.
                    *error.write() = Some(e);
                }
            }
        });
    });

    // Recomputed per render — which is NOT the once-per-toast the audit
    // assumed. `EntropyLogScreen` takes no props, so a parent re-render
    // memoizes them and never re-renders this scope at all
    // (`VNode::diff_vcomponent` returns early when `old_props.memoize`
    // says the props are unchanged). This runs when `entries` or `error`
    // changes, which is at most twice per visit. The cost that *did* scale
    // with the ledger was the grouping itself — see `group_by_goal`.
    let entries = entries.read();
    let summary = entries.as_ref().map(|v| summarize(v));
    let groups = entries.as_deref().map(group_by_goal);

    rsx! {
        div { class: "wl-page",
            div { class: "wl-page-head",
                button {
                    class: "wl-circle-btn wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    IconBack {}
                }
                h1 { class: "wl-page-title", "Entropy Log" }
            }

            div { class: "wl-scroll-region",
                if let Some(err) = error.read().clone() {
                    p { class: "wl-form-error", "{err}" }
                }

                if let Some(s) = summary {
                    if s.total > 0 {
                        p { class: "wl-entropy-summary wl-mono", "{summary_line(&s)}" }
                    }
                }

                if let Some(gs) = groups {
                    if gs.is_empty() {
                        div { class: "wl-directive-card",
                            h2 { class: "wl-entropy-empty-title", "Nothing has been abandoned." }
                            p { class: "wl-body-muted",
                                "Every directive so far ran to completion. This page fills in the first time the escape hatch is used — it exists to make the reason visible, not to be visited."
                            }
                        }
                    } else {
                        for (goal, items) in gs {
                            div { key: "{goal}", class: "wl-entropy-group",
                                h2 { class: "wl-entropy-goal", "{goal}" }
                                for e in items {
                                    div { key: "{e.id}", class: "wl-entropy-entry",
                                        div { class: "wl-entropy-row",
                                            span { class: "wl-entropy-date wl-mono", "{short_date(&e.date)}" }
                                            span { class: "wl-entropy-reason", "{reason_label(&e.reason)}" }
                                            if e.still_blocked {
                                                span { class: "wl-chip", "blocked" }
                                            }
                                        }
                                        p { class: "wl-entropy-directive", "{e.directive_title}" }
                                        if let Some(note) = e.note.as_ref() {
                                            if !note.trim().is_empty() {
                                                p { class: "wl-entropy-note", "\"{note}\"" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    p { class: "wl-body-muted", "Reading the ledger…" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(reason: &str, blocked: bool, goal: &str) -> EntropyView {
        EntropyView {
            id: format!("{goal}-{reason}"),
            goal_title: goal.into(),
            directive_id: "d".into(),
            directive_title: "Draft Section 2.1".into(),
            reason: reason.into(),
            note: None,
            date: "2026-09-21".into(),
            still_blocked: blocked,
        }
    }

    #[test]
    fn every_stored_reason_has_a_label() {
        for key in REASON_ORDER {
            assert_ne!(reason_label(key), "Unclassified", "{key} lost its label");
        }
    }

    #[test]
    fn an_unknown_reason_is_reported_not_swallowed() {
        // A future shell adding a fourth reason must not shrink the page.
        let s = summarize(&[entry("weather_depression", false, "G")]);
        assert_eq!(s.total, 1);
        assert_eq!(s.by_reason, vec![("other", 1)]);
    }

    #[test]
    fn the_breakdown_always_accounts_for_every_event() {
        // The invariant that makes the headline trustworthy: "4 events"
        // must mean the parts add up to 4, including reasons this build
        // does not know.
        let mixed = vec![
            entry("energy_depletion", false, "G"),
            entry("external_dependency", true, "G"),
            entry("weather_depression", false, "G"),
            entry("weather_depression", false, "G"),
        ];
        let s = summarize(&mixed);
        assert_eq!(s.total, 4);
        assert_eq!(s.by_reason.iter().map(|(_, n)| n).sum::<usize>(), s.total);
        assert_eq!(
            s.by_reason,
            vec![("energy", 1), ("external", 1), ("other", 2)]
        );
    }

    #[test]
    fn summary_omits_zero_categories_and_counts_blocked() {
        let s = summarize(&[
            entry("energy_depletion", false, "G"),
            entry("energy_depletion", false, "G"),
            entry("miscalculated_scope", false, "G"),
            entry("external_dependency", true, "G"),
        ]);
        assert_eq!(s.total, 4);
        assert_eq!(s.still_blocked, 1);
        // Energy first, and no zero rows.
        assert_eq!(
            s.by_reason,
            vec![("energy", 2), ("scope", 1), ("external", 1)]
        );
        assert_eq!(
            summary_line(&s),
            "4 events · 2 energy · 1 scope · 1 external · 1 still blocked"
        );
    }

    #[test]
    fn a_single_event_is_not_pluralised() {
        let s = summarize(&[entry("energy_depletion", false, "G")]);
        assert_eq!(summary_line(&s), "1 event · 1 energy");
    }

    #[test]
    fn an_empty_ledger_has_no_summary_line() {
        let s = summarize(&[]);
        assert_eq!(s.total, 0);
        assert_eq!(summary_line(&s), "");
    }

    /// The grouping is what makes a single bailout legible: three bails
    /// over three goals is noise, three on one goal is a pattern. So the
    /// first-seen order and the per-goal collection are both load-bearing
    /// — the shell returns newest-first, and the most recently active goal
    /// has to lead.
    #[test]
    fn grouping_keeps_first_seen_goal_order_and_collects_its_entries() {
        let ledger = [
            entry("energy_depletion", false, "Ship"),
            entry("miscalculated_scope", false, "Fleet"),
            entry("external_dependency", false, "Ship"),
        ];
        let g = group_by_goal(&ledger);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "Ship");
        assert_eq!(g[0].1.len(), 2);
        assert_eq!(g[1].0, "Fleet");
        assert_eq!(g[1].1.len(), 1);
        // Borrowed, not copied: the rows are the ledger's own entries, so
        // rendering this page costs no allocation per bailout. The ledger
        // is append-only for the life of the install, so a per-render copy
        // of every entry was a copy that grew forever.
        let mut seen: Vec<&str> = g
            .iter()
            .flat_map(|(_, v)| v.iter())
            .map(|e| e.id.as_str())
            .collect();
        seen.sort_unstable();
        let mut expected: Vec<&str> = ledger.iter().map(|e| e.id.as_str()).collect();
        expected.sort_unstable();
        assert_eq!(seen, expected, "every entry appears exactly once");
    }

    /// A goal whose title is empty is still a group, and two of them are
    /// the same group — the old linear scan compared the same way, so this
    /// is a property the index must not change.
    #[test]
    fn entries_with_no_goal_title_still_group_together() {
        let ledger = [
            entry("energy_depletion", false, ""),
            entry("external_dependency", false, ""),
            entry("energy_depletion", false, "Ship"),
        ];
        let g = group_by_goal(&ledger);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "");
        assert_eq!(g[0].1.len(), 2);
        assert_eq!(g[1].0, "Ship");
    }

    #[test]
    fn a_malformed_date_shows_itself_instead_of_a_blank_gap() {
        assert_eq!(short_date("2026-09-01"), "1 Sep");
        assert_eq!(short_date("2026-12-25"), "25 Dec");
        // Degenerate clock (HLC physical 0) and junk both pass through.
        assert_eq!(short_date(""), "");
        assert_eq!(short_date("not-a-date"), "not-a-date");
        assert_eq!(short_date("2026-13-01"), "2026-13-01");
    }
}
