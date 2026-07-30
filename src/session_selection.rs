use crate::state::{Activity, SessionInfo};

fn activity_priority(activity: &Activity) -> u8 {
    match activity {
        Activity::Waiting => 8,
        Activity::Tool(_) => 7,
        Activity::Thinking => 6,
        Activity::Prompting => 5,
        Activity::Notification => 4,
        Activity::Init => 3,
        Activity::Done => 2,
        Activity::AgentDone => 1,
        Activity::Idle => 0,
    }
}

fn recency_key(session: &SessionInfo) -> (u64, u32) {
    let timestamp_ms = if session.last_ts_ms > 0 {
        session.last_ts_ms
    } else {
        session.last_event_ts.saturating_mul(1000)
    };
    (timestamp_ms, session.pane_id)
}

pub fn is_newer_than(candidate: &SessionInfo, current: &SessionInfo) -> bool {
    recency_key(candidate) > recency_key(current)
}

/// Choose the session whose activity should represent a tab in the status bar.
/// Activity urgency wins first, then the most recent event breaks ties.
pub fn session_to_display<'a>(
    sessions: impl IntoIterator<Item = &'a SessionInfo>,
) -> Option<&'a SessionInfo> {
    sessions
        .into_iter()
        .max_by_key(|session| (activity_priority(&session.activity), recency_key(session)))
}

/// Choose the pane to reveal when an agent-aware tab is clicked.
/// Permission requests retain their existing precedence; otherwise the most
/// recently active Claude Code or Codex session wins.
pub fn session_to_focus<'a>(
    sessions: impl IntoIterator<Item = &'a SessionInfo>,
) -> Option<&'a SessionInfo> {
    let mut newest = None;
    let mut newest_waiting = None;

    for session in sessions {
        if newest
            .map(|current| recency_key(session) > recency_key(current))
            .unwrap_or(true)
        {
            newest = Some(session);
        }

        if matches!(session.activity, Activity::Waiting)
            && newest_waiting
                .map(|current| recency_key(session) > recency_key(current))
                .unwrap_or(true)
        {
            newest_waiting = Some(session);
        }
    }

    newest_waiting.or(newest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(
        pane_id: u32,
        activity: Activity,
        last_event_ts: u64,
        last_ts_ms: u64,
    ) -> SessionInfo {
        SessionInfo {
            session_id: format!("session-{pane_id}"),
            pane_id,
            activity,
            tab_name: Some("tab".to_string()),
            tab_index: Some(0),
            last_event_ts,
            cwd: None,
            last_ts_ms,
            rainbow_name: false,
            rainbow_mode_marker: None,
        }
    }

    #[test]
    fn display_prefers_activity_priority_then_recency() {
        let older_tool = session(1, Activity::Tool("Read".to_string()), 1, 1000);
        let newer_tool = session(2, Activity::Tool("Edit".to_string()), 2, 2000);
        let newest_thinking = session(3, Activity::Thinking, 3, 3000);

        let selected = session_to_display([&older_tool, &newer_tool, &newest_thinking]).unwrap();

        assert_eq!(selected.pane_id, 2);
    }

    #[test]
    fn focus_prefers_the_most_recent_session() {
        let older_high_priority = session(1, Activity::Tool("Bash".to_string()), 1, 1000);
        let newer_thinking = session(2, Activity::Thinking, 2, 2000);

        let selected = session_to_focus([&older_high_priority, &newer_thinking]).unwrap();

        assert_eq!(selected.pane_id, 2);
    }

    #[test]
    fn focus_keeps_permission_waiting_precedence() {
        let older_waiting = session(1, Activity::Waiting, 1, 1000);
        let newer_tool = session(2, Activity::Tool("Bash".to_string()), 2, 2000);

        let selected = session_to_focus([&older_waiting, &newer_tool]).unwrap();

        assert_eq!(selected.pane_id, 1);
    }

    #[test]
    fn focus_chooses_the_newest_waiting_session() {
        let older_waiting = session(1, Activity::Waiting, 1, 1000);
        let newer_waiting = session(2, Activity::Waiting, 2, 2000);

        let selected = session_to_focus([&older_waiting, &newer_waiting]).unwrap();

        assert_eq!(selected.pane_id, 2);
    }

    #[test]
    fn focus_supports_legacy_second_timestamps_and_pane_zero() {
        let older = session(7, Activity::Thinking, 10, 0);
        let newer = session(0, Activity::Thinking, 20, 0);

        let selected = session_to_focus([&older, &newer]).unwrap();

        assert_eq!(selected.pane_id, 0);
    }

    #[test]
    fn newer_comparison_uses_milliseconds_within_the_same_second() {
        let current = session(1, Activity::Thinking, 10, 10_001);
        let candidate = session(1, Activity::Tool("Read".to_string()), 10, 10_002);

        assert!(is_newer_than(&candidate, &current));
        assert!(!is_newer_than(&current, &candidate));
    }
}
