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
    }
}

pub fn is_placeholder(session: &SessionInfo) -> bool {
    session.session_id.is_empty()
}

/// Reconcile the session map with the observed foreground commands of panes
/// that have no real session. `observed` maps pane id to the classified agent
/// client (`client_for_command`) of its currently running command. Returns
/// true when an entry was added or removed.
pub fn reconcile_agent_panes(
    sessions: &mut BTreeMap<u32, SessionInfo>,
    observed: impl IntoIterator<Item = (u32, Option<&'static str>)>,
) -> bool {
    let mut changed = false;
    for (pane_id, client) in observed {
        match (sessions.get(&pane_id), client) {
            (None, Some(_)) => {
                sessions.insert(pane_id, placeholder_session(pane_id));
                changed = true;
            }
            (Some(session), None) if is_placeholder(session) => {
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
            ..placeholder_session(pane_id)
        }
    }

    #[test]
    fn agent_pane_without_session_gains_a_placeholder() {
        let mut sessions = BTreeMap::new();

        assert!(reconcile_agent_panes(&mut sessions, [(7, Some("codex"))]));

        let session = sessions.get(&7).unwrap();
        assert!(is_placeholder(session));
        assert_eq!(session.activity, Activity::Idle);
        assert_eq!(session.last_ts_ms, 0);
    }

    #[test]
    fn placeholder_is_removed_when_the_agent_exits() {
        let mut sessions = BTreeMap::from([(7, placeholder_session(7))]);

        assert!(reconcile_agent_panes(&mut sessions, [(7, None)]));

        assert!(sessions.is_empty());
    }

    #[test]
    fn shell_panes_and_stable_placeholders_change_nothing() {
        let mut sessions = BTreeMap::from([(7, placeholder_session(7))]);

        assert!(!reconcile_agent_panes(
            &mut sessions,
            [(7, Some("codex")), (8, None)]
        ));

        assert!(sessions.contains_key(&7));
        assert!(!sessions.contains_key(&8));
    }

    #[test]
    fn a_real_session_is_never_touched() {
        let mut sessions = BTreeMap::from([(7, real_session(7))]);

        assert!(!reconcile_agent_panes(&mut sessions, [(7, None)]));

        assert_eq!(sessions.get(&7).unwrap().session_id, "real");
    }
}
