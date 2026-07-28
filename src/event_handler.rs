use crate::state::{Activity, FlashMode, HookPayload, SessionInfo, State};

/// True when a `Notification` says Claude is sitting idle at the prompt, as
/// opposed to asking for permission. Matched loosely because the wording is
/// upstream text we don't control.
fn is_idle_prompt_notification(message: Option<&str>) -> bool {
    let Some(message) = message else {
        return false;
    };
    let message = message.to_lowercase();
    message.contains("waiting for") && !message.contains("permission")
}

pub fn handle_hook_event(state: &mut State, payload: HookPayload) {
    // Capture env info for use in notifications
    if let Some(ref name) = payload.zellij_session {
        state.zellij_session_name = Some(name.clone());
    }
    if let Some(ref tp) = payload.term_program {
        state.term_program = Some(tp.clone());
    }

    let event = payload.hook_event.as_str();

    // SessionEnd → remove session (never drop: terminal cleanup)
    if event == "SessionEnd" {
        state.sessions.remove(&payload.pane_id);
        return;
    }

    // Drop events that arrive out of order (async hooks can race through
    // parallel subprocesses). Only enforced when the hook supplied ts_ms —
    // an absent field means an old hook script and is treated as fresh.
    if let Some(ts_ms) = payload.ts_ms {
        if let Some(session) = state.sessions.get(&payload.pane_id) {
            if ts_ms < session.last_ts_ms {
                return;
            }
        }
    }

    let activity = match event {
        "SessionStart" => Activity::Init,
        "PreToolUse" => {
            Activity::Tool(payload.tool_name.clone().unwrap_or_default())
        }
        "PostToolUse" | "PostToolUseFailure" => Activity::Thinking,
        "UserPromptSubmit" => Activity::Thinking,
        "PermissionRequest" => Activity::Waiting,
        // Claude Code emits a Notification when the prompt has sat idle — that's
        // the only signal we get that a turn ended without a `Stop` hook (user
        // interrupt, lost hook). Other notifications (permission requests) are
        // informational: refresh the timestamp and keep the current activity.
        "Notification" if is_idle_prompt_notification(payload.message.as_deref()) => {
            Activity::Prompting
        }
        "Notification" => {
            if let Some(session) = state.sessions.get_mut(&payload.pane_id) {
                session.last_event_ts = crate::state::unix_now();
                if let Some(ts_ms) = payload.ts_ms {
                    session.last_ts_ms = ts_ms;
                }
            }
            return;
        }
        "Stop" => Activity::Done,
        "SubagentStop" => Activity::AgentDone,
        _ => Activity::Idle,
    };

    let (tab_index, tab_name) = state
        .pane_to_tab
        .get(&payload.pane_id)
        .cloned()
        .unzip();

    let session = state
        .sessions
        .entry(payload.pane_id)
        .or_insert_with(|| SessionInfo {
            session_id: payload.session_id.clone().unwrap_or_default(),
            pane_id: payload.pane_id,
            activity: Activity::Init,
            tab_name: None,
            tab_index: None,
            last_event_ts: 0,
            cwd: None,
            last_ts_ms: 0,
        });

    if matches!(activity, Activity::Waiting) {
        match state.settings.flash {
            FlashMode::Once => {
                state.flash_deadlines.insert(
                    payload.pane_id,
                    crate::state::unix_now_ms() + crate::state::FLASH_DURATION_MS,
                );
            }
            FlashMode::Persist => {
                state.flash_deadlines.insert(payload.pane_id, u64::MAX);
            }
            FlashMode::Off => {}
        }
        // Desktop notification is handled by the hook script to avoid
        // duplicates from multiple plugin instances.
    } else {
        state.flash_deadlines.remove(&payload.pane_id);
    }

    session.activity = activity;
    session.last_event_ts = crate::state::unix_now();
    if let Some(ts_ms) = payload.ts_ms {
        session.last_ts_ms = ts_ms;
    }
    if let Some(sid) = &payload.session_id {
        session.session_id = sid.clone();
    }
    if let Some(cwd) = payload.cwd {
        session.cwd = Some(cwd);
    }
    if let Some((idx, name)) = tab_index.zip(tab_name) {
        session.tab_index = Some(idx);
        session.tab_name = Some(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(event: &str, ts_ms: u64) -> HookPayload {
        HookPayload {
            session_id: Some("s1".into()),
            pane_id: 1,
            hook_event: event.into(),
            tool_name: None,
            cwd: None,
            zellij_session: None,
            term_program: None,
            ts_ms: Some(ts_ms),
            message: None,
        }
    }

    fn activity(state: &State) -> Activity {
        state.sessions.get(&1).unwrap().activity.clone()
    }

    #[test]
    fn idle_notification_marks_session_as_prompting() {
        let mut state = State::default();
        handle_hook_event(&mut state, payload("UserPromptSubmit", 100));
        assert_eq!(activity(&state), Activity::Thinking);

        let mut p = payload("Notification", 200);
        p.message = Some("Claude is waiting for your input".into());
        handle_hook_event(&mut state, p);

        assert_eq!(activity(&state), Activity::Prompting);
    }

    #[test]
    fn permission_notification_does_not_clear_running_activity() {
        let mut state = State::default();
        let mut p = payload("PreToolUse", 100);
        p.tool_name = Some("Bash".into());
        handle_hook_event(&mut state, p);

        let mut p = payload("Notification", 200);
        p.message = Some("Claude needs your permission to use Bash".into());
        handle_hook_event(&mut state, p);

        assert_eq!(activity(&state), Activity::Tool("Bash".into()));
    }

    #[test]
    fn untagged_notification_keeps_current_activity() {
        let mut state = State::default();
        handle_hook_event(&mut state, payload("UserPromptSubmit", 100));
        handle_hook_event(&mut state, payload("Notification", 200));

        assert_eq!(activity(&state), Activity::Thinking);
    }

    #[test]
    fn idle_notification_creates_session_when_none_exists() {
        let mut state = State::default();
        let mut p = payload("Notification", 100);
        p.message = Some("Claude is waiting for your input".into());
        handle_hook_event(&mut state, p);

        assert_eq!(activity(&state), Activity::Prompting);
    }
}
