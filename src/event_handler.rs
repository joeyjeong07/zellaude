use crate::state::{Activity, FlashMode, HookPayload, SessionInfo, State};

fn update_session_identity(session: &mut SessionInfo, payload: &HookPayload) -> bool {
    let Some(session_id) = payload
        .session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
    else {
        return false;
    };

    if session.session_id == session_id {
        return false;
    }

    session.session_id = session_id.to_string();
    true
}

fn update_rainbow_mode(
    session: &mut SessionInfo,
    payload: &HookPayload,
    reset_for_new_session: bool,
) {
    if reset_for_new_session {
        session.rainbow_name = payload.rainbow_name.unwrap_or(false);
        session.rainbow_mode_marker = payload.rainbow_mode_marker.clone();
        return;
    }

    let Some(rainbow_name) = payload.rainbow_name else {
        return;
    };

    if let Some(marker) = payload.rainbow_mode_marker.as_deref() {
        if session.rainbow_mode_marker.as_deref() == Some(marker) {
            return;
        }
        session.rainbow_mode_marker = Some(marker.to_string());
    }
    session.rainbow_name = rainbow_name;
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

    // SessionEnd → remove this session. Ignore a late end event from a process
    // that has already been replaced by a newer agent session in the pane.
    if event == "SessionEnd" {
        let should_remove = state
            .sessions
            .get(&payload.pane_id)
            .map(|session| {
                payload.session_id.as_deref().unwrap_or_default().is_empty()
                    || session.session_id.is_empty()
                    || payload.session_id.as_deref() == Some(session.session_id.as_str())
            })
            .unwrap_or(false);
        if should_remove {
            state.sessions.remove(&payload.pane_id);
        }
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
        "SubagentStart" => Activity::Tool("Task".to_string()),
        "PreToolUse" => {
            Activity::Tool(payload.tool_name.clone().unwrap_or_default())
        }
        "PostToolUse" | "PostToolUseFailure" => Activity::Thinking,
        "UserPromptSubmit" => Activity::Thinking,
        "PermissionRequest" => Activity::Waiting,
        // Notification is informational — just refresh the timestamp, keep current activity.
        "Notification" => {
            if let Some(session) = state.sessions.get_mut(&payload.pane_id) {
                let new_session = update_session_identity(session, &payload);
                update_rainbow_mode(session, &payload, new_session);
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
            rainbow_name: payload.rainbow_name.unwrap_or(false),
            rainbow_mode_marker: payload.rainbow_mode_marker.clone(),
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
    let new_session = update_session_identity(session, &payload);
    update_rainbow_mode(session, &payload, new_session || event == "SessionStart");
    if let Some(cwd) = payload.cwd {
        session.cwd = Some(cwd);
    }
    if let Some((idx, name)) = tab_index.zip(tab_name) {
        session.tab_index = Some(idx);
        session.tab_name = Some(name);
    }
}
