use crate::state::{Activity, SessionInfo};
use std::collections::{BTreeMap, HashMap};

/// Codex-family TUIs start their session lazily on the first prompt, so no
/// hook fires at launch and the pane would stay invisible until the user
/// submits something. A placeholder marks such a pane as an idle agent from
/// pane introspection alone. Its empty session id and zero timestamps make it
/// lose every ordering comparison, so any real hook event, peer sync, or
/// attach discovery replaces it in place.
pub fn placeholder_session(pane_id: u32) -> SessionInfo {
    SessionInfo {
        session_id: String::new(),
        pane_id,
        activity: Activity::Idle,
        tab_name: None,
        tab_index: None,
        last_event_ts: 0,
        cwd: None,
        last_ts_ms: 0,
        rainbow_name: false,
        rainbow_name_known: false,
        rainbow_mode_ts_ms: 0,
        rainbow_mode_marker: None,
        restored: true,
        placeholder: true,
    }
}

pub fn is_placeholder(session: &SessionInfo) -> bool {
    session.placeholder
}

/// A pane whose agent session just ended may still report the exiting binary
/// as its foreground command. Ignore introspection for a moment so a dying
/// agent is not immediately resurrected as an idle placeholder.
pub const PLACEHOLDER_GRACE_MS: u64 = 5_000;

/// Querying every unclaimed pane on every cycle scales with session size, so
/// each cycle spends at most this many host calls. Sessions smaller than the
/// budget are unaffected; larger ones sweep across consecutive cycles.
pub const AGENT_POLL_BUDGET: usize = 12;

pub fn ended_recently(ended_ms: &HashMap<u32, u64>, pane_id: u32, now_ms: u64) -> bool {
    ended_ms.get(&pane_id).is_some_and(|&ended_at| {
        // A backward clock step has to expire the window, not hold it open.
        // `saturating_sub` returns zero for every `now_ms` before `ended_at`,
        // which kept the grace active until the clock caught up — potentially
        // hours, blocking both placeholder removal and discovery.
        now_ms >= ended_at && now_ms - ended_at < PLACEHOLDER_GRACE_MS
    })
}

/// Pick the panes to query this cycle, and the cursor the next cycle resumes
/// from: one rotation over every candidate, bounded by the budget.
///
/// An earlier version polled panes already holding a placeholder
/// unconditionally and gave the remainder to the rest. Once the placeholders
/// alone reached the budget there was no remainder and the cursor never
/// advanced, so a pane that had just started an agent was never discovered —
/// and with more placeholders than the budget the cycle also made more host
/// calls than the budget it documents. A single rotation keeps both bounds by
/// construction, at the cost of an exited agent's placeholder surviving a few
/// extra cycles in a session large enough to need sweeping.
pub fn panes_to_poll(mut candidates: Vec<u32>, cursor: u32, budget: usize) -> (Vec<u32>, u32) {
    candidates.sort_unstable();
    if candidates.is_empty() || budget == 0 {
        return (Vec::new(), cursor);
    }

    let start = candidates
        .iter()
        .position(|&pane_id| pane_id >= cursor)
        .unwrap_or(0);
    let taken: Vec<u32> = candidates
        .iter()
        .cycle()
        .skip(start)
        .take(budget.min(candidates.len()))
        .copied()
        .collect();
    let next_cursor = taken
        .last()
        .and_then(|last| last.checked_add(1))
        .unwrap_or(0);
    (taken, next_cursor)
}

/// What pane introspection saw running in a pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaneAgent {
    Running,
    Absent,
    /// Not readable, or too soon after a session end to trust.
    Unknown,
}

/// Reconcile the session map with the observed foreground commands of panes
/// that have no real session. Returns true when an entry was added or removed.
pub fn reconcile_agent_panes(
    sessions: &mut BTreeMap<u32, SessionInfo>,
    observed: impl IntoIterator<Item = (u32, PaneAgent)>,
) -> bool {
    let mut changed = false;
    for (pane_id, observation) in observed {
        match (sessions.get(&pane_id), observation) {
            (None, PaneAgent::Running) => {
                sessions.insert(pane_id, placeholder_session(pane_id));
                changed = true;
            }
            (Some(session), PaneAgent::Absent) if is_placeholder(session) => {
                sessions.remove(&pane_id);
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_session(pane_id: u32) -> SessionInfo {
        SessionInfo {
            session_id: "real".to_string(),
            last_ts_ms: 1000,
            last_event_ts: 1,
            placeholder: false,
            ..placeholder_session(pane_id)
        }
    }

    #[test]
    fn agent_pane_without_session_gains_a_placeholder() {
        let mut sessions = BTreeMap::new();

        assert!(reconcile_agent_panes(&mut sessions, [(7, PaneAgent::Running)]));

        let session = sessions.get(&7).unwrap();
        assert!(is_placeholder(session));
        assert_eq!(session.activity, Activity::Idle);
        assert_eq!(session.last_ts_ms, 0);
    }

    #[test]
    fn placeholder_is_removed_when_the_agent_exits() {
        let mut sessions = BTreeMap::from([(7, placeholder_session(7))]);

        assert!(reconcile_agent_panes(&mut sessions, [(7, PaneAgent::Absent)]));

        assert!(sessions.is_empty());
    }

    #[test]
    fn shell_panes_and_stable_placeholders_change_nothing() {
        let mut sessions = BTreeMap::from([(7, placeholder_session(7))]);

        assert!(!reconcile_agent_panes(
            &mut sessions,
            [(7, PaneAgent::Running), (8, PaneAgent::Absent)]
        ));

        assert!(sessions.contains_key(&7));
        assert!(!sessions.contains_key(&8));
    }

    #[test]
    fn a_real_session_is_never_touched() {
        let mut sessions = BTreeMap::from([(7, real_session(7))]);

        assert!(!reconcile_agent_panes(&mut sessions, [(7, PaneAgent::Absent)]));

        assert_eq!(sessions.get(&7).unwrap().session_id, "real");
    }

    #[test]
    fn an_unreadable_pane_command_keeps_the_placeholder() {
        let mut sessions = BTreeMap::from([(7, placeholder_session(7))]);

        assert!(!reconcile_agent_panes(&mut sessions, [(7, PaneAgent::Unknown)]));

        assert!(sessions.contains_key(&7));
    }

    #[test]
    fn a_pane_whose_session_just_ended_is_still_within_the_grace_window() {
        let ended = HashMap::from([(7, 10_000)]);

        assert!(ended_recently(&ended, 7, 11_000));
    }

    #[test]
    fn the_grace_window_expires_and_other_panes_are_unaffected() {
        let ended = HashMap::from([(7, 10_000)]);

        assert!(!ended_recently(&ended, 7, 10_000 + PLACEHOLDER_GRACE_MS));
        assert!(!ended_recently(&ended, 8, 11_000));
    }

    #[test]
    fn every_pane_is_polled_when_the_session_fits_the_budget() {
        let (selected, cursor) = panes_to_poll(vec![3, 1, 2], 0, 12);

        assert_eq!(selected, vec![1, 2, 3]);
        assert_eq!(cursor, 4);
    }

    #[test]
    fn a_large_session_sweeps_across_cycles_without_missing_a_pane() {
        let mut seen = Vec::new();
        let mut cursor = 0;
        for _ in 0..5 {
            let (selected, next) =
                panes_to_poll((1..=10).collect(), cursor, 2);
            assert_eq!(selected.len(), 2);
            seen.extend(selected);
            cursor = next;
        }

        assert_eq!(seen, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn discovery_is_not_starved_when_placeholders_fill_the_budget() {
        // Placeholders used to be polled unconditionally and the rest given the
        // remainder. Once the placeholders alone reached the budget there was no
        // remainder and the cursor never advanced, so a pane that had just
        // started an agent was never looked at again.
        let mut seen = Vec::new();
        let mut cursor = 0;

        for _ in 0..2 {
            let (selected, next) = panes_to_poll(vec![1, 2, 8, 9], cursor, 2);
            seen.extend(selected);
            cursor = next;
        }

        assert!(seen.contains(&1), "pane 1 was never polled: {seen:?}");
        assert!(seen.contains(&2), "pane 2 was never polled: {seen:?}");
    }

    #[test]
    fn a_cycle_never_exceeds_its_host_call_budget() {
        let (selected, _) = panes_to_poll((1..=20).collect(), 0, 12);

        assert_eq!(selected.len(), 12);
    }

    #[test]
    fn a_backward_clock_step_expires_the_grace_window() {
        // saturating_sub returns 0 while now < ended_at, which held the window
        // open until the clock caught up — hours, for a real backward step.
        let mut ended = HashMap::new();
        ended.insert(4, 100_000);

        assert!(ended_recently(&ended, 4, 100_001));
        assert!(!ended_recently(&ended, 4, 40_000));
    }
}
