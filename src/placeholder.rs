#[cfg(test)]
use crate::state::Activity;
use crate::state::SessionInfo;

/// A provisional bar entry for an agent pane discovered by pane introspection
/// rather than a hook event. Nothing creates these since the introspection
/// poll was removed; the flag and its guards remain for compatibility with
/// entries from older plugin builds. An empty session id and zero timestamps
/// make a placeholder lose every ordering comparison, so any real hook event
/// replaces it in place.
#[cfg(test)]
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
