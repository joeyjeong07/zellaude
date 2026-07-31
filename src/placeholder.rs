use crate::state::{Activity, SessionInfo};
use std::collections::BTreeMap;

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

pub fn ended_recently(
    tombstones: &BTreeMap<(u32, String), u64>,
    pane_id: u32,
    now_ms: u64,
) -> bool {
    tombstones
        .range((pane_id, String::new())..)
        .take_while(|((tombstone_pane, _), _)| *tombstone_pane == pane_id)
        .any(|(_, &ended_at)| now_ms.saturating_sub(ended_at) < PLACEHOLDER_GRACE_MS)
}

/// What pane introspection saw running in a pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaneAgent {
    /// The foreground command is a known agent client.
    Running,
    /// The foreground command is something else.
    Absent,
    /// The command could not be read, or is not trustworthy yet.
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
}

#[cfg(test)]
mod unreadable_tests {
    use super::*;

    #[test]
    fn an_unreadable_pane_command_keeps_the_placeholder() {
        let mut sessions = BTreeMap::from([(7, placeholder_session(7))]);

        assert!(!reconcile_agent_panes(&mut sessions, [(7, PaneAgent::Unknown)]));

        assert!(sessions.contains_key(&7));
    }
}

#[cfg(test)]
mod grace_tests {
    use super::*;

    #[test]
    fn a_pane_whose_session_just_ended_is_still_within_the_grace_window() {
        let tombstones = BTreeMap::from([((7, "gone".to_string()), 10_000)]);

        assert!(ended_recently(&tombstones, 7, 11_000));
    }

    #[test]
    fn the_grace_window_expires_and_other_panes_are_unaffected() {
        let tombstones = BTreeMap::from([((7, "gone".to_string()), 10_000)]);

        assert!(!ended_recently(&tombstones, 7, 10_000 + PLACEHOLDER_GRACE_MS));
        assert!(!ended_recently(&tombstones, 8, 11_000));
    }
}
