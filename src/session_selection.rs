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
    if candidate.restored != current.restored {
        return !candidate.restored;
    }
    recency_key(candidate) > recency_key(current)
}

/// Preserve a concrete mode observation while two instances reconcile the
/// same root session. Activity provenance and recency are selected separately.
pub fn reconcile_rainbow_mode(candidate: &mut SessionInfo, current: &mut SessionInfo) {
    if candidate.session_id != current.session_id {
        return;
    }

    let candidate_wins = match (
        candidate.rainbow_name_known,
        current.rainbow_name_known,
    ) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => return,
        (true, true) => {
            let candidate_key = (
                candidate.rainbow_mode_ts_ms,
                !candidate.rainbow_name,
                candidate.rainbow_mode_marker.as_deref().unwrap_or(""),
            );
            let current_key = (
                current.rainbow_mode_ts_ms,
                !current.rainbow_name,
                current.rainbow_mode_marker.as_deref().unwrap_or(""),
            );
            candidate_key > current_key
        }
    };

    if candidate_wins {
        copy_rainbow_mode(current, candidate);
    } else {
        copy_rainbow_mode(candidate, current);
    }
}

fn copy_rainbow_mode(target: &mut SessionInfo, source: &SessionInfo) {
    target.rainbow_name = source.rainbow_name;
    target.rainbow_name_known = source.rainbow_name_known;
    target.rainbow_mode_ts_ms = source.rainbow_mode_ts_ms;
    target.rainbow_mode_marker = source.rainbow_mode_marker.clone();
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
            rainbow_name_known: true,
            rainbow_mode_ts_ms: last_ts_ms,
            rainbow_mode_marker: None,
            restored: false,
            placeholder: false,
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

    #[test]
    fn reconciliation_carries_a_known_mode_across_provenance_selection() {
        let mut restored = session(1, Activity::Idle, 10, 10_000);
        restored.restored = true;
        restored.rainbow_name = true;
        restored.rainbow_mode_marker = Some("ultra".to_string());

        let mut live = session(1, Activity::Thinking, 20, 20_000);
        live.rainbow_name_known = false;

        reconcile_rainbow_mode(&mut live, &mut restored);

        assert!(live.rainbow_name);
        assert!(live.rainbow_name_known);
        assert_eq!(live.rainbow_mode_marker.as_deref(), Some("ultra"));
        assert!(is_newer_than(&live, &restored));
    }

    #[test]
    fn reconciliation_uses_the_newest_mode_observation_independently_of_activity() {
        let mut newer_activity = session(1, Activity::Thinking, 20, 20_000);
        newer_activity.rainbow_mode_ts_ms = 10_000;

        let mut newer_mode = session(1, Activity::Idle, 10, 10_000);
        newer_mode.rainbow_name = true;
        newer_mode.rainbow_mode_ts_ms = 30_000;
        newer_mode.rainbow_mode_marker = Some("ultra".to_string());

        reconcile_rainbow_mode(&mut newer_mode, &mut newer_activity);

        assert!(newer_activity.rainbow_name);
        assert_eq!(newer_activity.rainbow_mode_ts_ms, 30_000);
        assert_eq!(
            newer_activity.rainbow_mode_marker.as_deref(),
            Some("ultra")
        );
        assert!(is_newer_than(&newer_activity, &newer_mode));
    }

    #[test]
    fn reconciliation_fails_closed_on_an_exact_timestamp_conflict() {
        let mut rainbow = session(1, Activity::Idle, 10, 10_000);
        rainbow.rainbow_name = true;
        rainbow.rainbow_mode_marker = Some("z-marker".to_string());

        let mut standard = session(1, Activity::Idle, 10, 10_000);
        standard.rainbow_mode_marker = Some("a-marker".to_string());

        reconcile_rainbow_mode(&mut rainbow, &mut standard);

        assert!(!rainbow.rainbow_name);
        assert!(!standard.rainbow_name);
        assert_eq!(rainbow.rainbow_mode_marker.as_deref(), Some("a-marker"));
    }
}
