use crate::state::{Activity, FlashMode, HookPayload, SessionInfo, State};

fn rainbow_mode_observed_at(payload: &HookPayload) -> u64 {
    payload
        .rainbow_mode_ts_ms
        .or(payload.ts_ms)
        .unwrap_or_else(crate::state::unix_now_ms)
}

fn update_session_identity(session: &mut SessionInfo, payload: &HookPayload) -> bool {
    if payload.is_subagent {
        return false;
    }

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
    if payload.is_subagent {
        return;
    }

    if reset_for_new_session {
        session.rainbow_name = payload.rainbow_name.unwrap_or(false);
        session.rainbow_name_known = payload.rainbow_name.is_some();
        session.rainbow_mode_ts_ms = if payload.rainbow_name.is_some() {
            rainbow_mode_observed_at(payload)
        } else {
            0
        };
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
    }

    let observed_at = rainbow_mode_observed_at(payload);
    let effective_marker = payload
        .rainbow_mode_marker
        .as_deref()
        .or(session.rainbow_mode_marker.as_deref())
        .unwrap_or("");
    if session.rainbow_name_known
        && (
            observed_at,
            !rainbow_name,
            effective_marker,
        ) <= (
            session.rainbow_mode_ts_ms,
            !session.rainbow_name,
            session.rainbow_mode_marker.as_deref().unwrap_or(""),
        )
    {
        return;
    }

    if let Some(marker) = payload.rainbow_mode_marker.as_deref() {
        session.rainbow_mode_marker = Some(marker.to_string());
    }
    session.rainbow_name = rainbow_name;
    session.rainbow_name_known = true;
    session.rainbow_mode_ts_ms = observed_at;
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
    let mut tombstone_to_clear = None;

    // SessionEnd → remove this session. Ignore a late end event from a process
    // that has already been replaced by a newer agent session in the pane.
    if event == "SessionEnd" {
        if payload.is_subagent {
            return;
        }
        if let Some(session_id) = payload
            .session_id
            .as_deref()
            .filter(|session_id| !session_id.is_empty())
        {
            let ts_ms = payload.ts_ms.unwrap_or_else(crate::state::unix_now_ms);
            state
                .session_end_tombstones
                .entry((payload.pane_id, session_id.to_string()))
                .and_modify(|ended_at| *ended_at = (*ended_at).max(ts_ms))
                .or_insert(ts_ms);
        }
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

    // Child agents share their root's pane, so they may enrich an existing
    // session's activity but must never create pane ownership. This also lets
    // a freshly loaded plugin accept synchronized root state without a
    // child-created placeholder winning on an equal timestamp.
    if payload.is_subagent && !state.sessions.contains_key(&payload.pane_id) {
        return;
    }

    if !payload.is_subagent {
        if let Some(session_id) = payload
            .session_id
            .as_deref()
            .filter(|session_id| !session_id.is_empty())
        {
            let key = (payload.pane_id, session_id.to_string());
            if let Some(&ended_at) = state.session_end_tombstones.get(&key) {
                if payload.ts_ms.is_some_and(|ts_ms| ts_ms <= ended_at) {
                    return;
                }
                tombstone_to_clear = Some(key);
            }
        }
    }

    // Drop events that arrive out of order (async hooks can race through
    // parallel subprocesses). Only enforced when the hook supplied ts_ms —
    // an absent field means an old hook script and is treated as fresh.
    if let Some(ts_ms) = payload.ts_ms {
        if let Some(session) = state.sessions.get(&payload.pane_id) {
            let same_restored_owner = session.restored
                && (payload.is_subagent
                    || payload
                        .session_id
                        .as_deref()
                        .filter(|session_id| !session_id.is_empty())
                        .map(|session_id| session_id == session.session_id)
                        .unwrap_or(true));
            if ts_ms < session.last_ts_ms && !same_restored_owner {
                if let Some(key) = tombstone_to_clear.take() {
                    state
                        .session_end_tombstones
                        .entry(key)
                        .and_modify(|blocked_at| *blocked_at = (*blocked_at).max(ts_ms))
                        .or_insert(ts_ms);
                }
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
                    if !(payload.is_subagent && session.restored) {
                        session.last_ts_ms = ts_ms;
                    }
                }
                if !payload.is_subagent {
                    session.restored = false;
                    if let Some(key) = tombstone_to_clear {
                        state.session_end_tombstones.remove(&key);
                    }
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
            rainbow_name_known: payload.rainbow_name.is_some(),
            rainbow_mode_ts_ms: if payload.rainbow_name.is_some() {
                rainbow_mode_observed_at(&payload)
            } else {
                0
            },
            rainbow_mode_marker: payload.rainbow_mode_marker.clone(),
            restored: false,
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
        if !(payload.is_subagent && session.restored) {
            session.last_ts_ms = ts_ms;
        }
    }
    let new_session = update_session_identity(session, &payload);
    update_rainbow_mode(session, &payload, new_session || event == "SessionStart");
    if !payload.is_subagent {
        session.restored = false;
        if let Some(key) = tombstone_to_clear {
            state.session_end_tombstones.remove(&key);
        }
    }
    if let Some(cwd) = payload.cwd {
        session.cwd = Some(cwd);
    }
    if let Some((idx, name)) = tab_index.zip(tab_name) {
        session.tab_index = Some(idx);
        session.tab_name = Some(name);
    }
}

/// Seed state discovered when the plugin attaches to an already-running
/// terminal. Discovery is deliberately lower priority than hooks and peer
/// synchronization: it never changes activity for an owner we already know.
pub fn handle_discovered_session(state: &mut State, mut payload: HookPayload) -> bool {
    if payload.is_subagent {
        return false;
    }

    payload.hook_event = "SessionRestore".to_string();
    payload.tool_name = None;

    let Some(discovered_id) = payload
        .session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
        .map(str::to_owned)
    else {
        return false;
    };
    let discovered_ts = payload.ts_ms.unwrap_or(0);

    if let Some(existing) = state.sessions.get_mut(&payload.pane_id) {
        if discovered_id == existing.session_id {
            let mut changed = false;
            if existing.restored {
                if discovered_ts <= existing.last_ts_ms {
                    return false;
                }
                update_rainbow_mode(existing, &payload, false);
                existing.last_ts_ms = discovered_ts;
                existing.last_event_ts = crate::state::unix_now();
                changed = true;
            } else {
                let previous_mode = (
                    existing.rainbow_name,
                    existing.rainbow_name_known,
                    existing.rainbow_mode_ts_ms,
                    existing.rainbow_mode_marker.clone(),
                );
                update_rainbow_mode(existing, &payload, false);
                changed |= previous_mode
                    != (
                        existing.rainbow_name,
                        existing.rainbow_name_known,
                        existing.rainbow_mode_ts_ms,
                        existing.rainbow_mode_marker.clone(),
                    );
            }
            if existing.cwd.is_none() {
                existing.cwd = payload.cwd;
                changed = existing.cwd.is_some() || changed;
            }
            return changed;
        }

        if discovered_ts == 0 || discovered_ts <= existing.last_ts_ms {
            return false;
        }
    }

    let pane_id = payload.pane_id;
    handle_hook_event(state, payload);
    if let Some(session) = state.sessions.get_mut(&pane_id) {
        if session.session_id == discovered_id {
            session.restored = true;
            return true;
        }
    }
    false
}
