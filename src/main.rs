mod attach;
mod event_handler;
mod installer;
mod placeholder;
mod rainbow;
mod render;
mod session_selection;
mod state;
mod tab_pane_map;
mod tool_symbol;
mod theme;

use state::{unix_now, unix_now_ms, HookPayload, MenuAction, SessionInfo, Settings, State, ViewMode};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

const DONE_TIMEOUT: u64 = 30;
const TIMER_INTERVAL: f64 = 1.0;
const FLASH_TICK: f64 = 0.25;
/// Foreground-command polling for pre-prompt agent TUIs; keeps host queries
/// bounded when the timer runs at flash/rainbow cadence.
const AGENT_POLL_INTERVAL_MS: u64 = 2000;

/// Everything the plugin needs to function at all. Named once so the initial
/// request and the retry cannot drift apart — a retry asking for a smaller set
/// would be granted and still leave the plugin unable to work.
const REQUIRED_PERMISSIONS: [PermissionType; 5] = [
    PermissionType::ReadApplicationState,
    PermissionType::ChangeApplicationState,
    PermissionType::RunCommands,
    PermissionType::ReadCliPipes,
    PermissionType::MessageAndLaunchOtherPlugins,
];

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        request_permission(&REQUIRED_PERMISSIONS);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::ModeUpdate,
            EventType::Timer,
            EventType::Mouse,
            EventType::Key,
            EventType::RunCommandResult,
            EventType::PermissionRequestResult,
        ]);
        set_timeout(TIMER_INTERVAL);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::TabUpdate(tabs) => {
                let new_active = tabs.iter().find(|t| t.active).map(|t| t.position);
                if new_active != self.active_tab_index {
                    // Tab focus changed — clear persist flashes on the newly focused tab
                    if let Some(idx) = new_active {
                        self.clear_flashes_on_tab(idx);
                    }
                    // This instance stopped polling while its tab was hidden,
                    // so its placeholders may be stale. Poll on the next tick.
                    self.last_agent_poll_ms = 0;
                }
                self.active_tab_index = new_active;
                self.tabs = tabs;
                self.rebuild_pane_map();
                self.maybe_start_attach_scan();
                true
            }
            Event::PaneUpdate(manifest) => {
                self.pane_manifest = Some(manifest);
                self.rebuild_pane_map();
                self.maybe_start_attach_scan();
                true
            }
            Event::ModeUpdate(mode_info) => {
                self.input_mode = mode_info.mode;
                self.zellij_styling = Some(mode_info.style.colors);
                if let Some(name) = mode_info.session_name {
                    self.zellij_session_name = Some(name);
                }
                self.maybe_start_attach_scan();
                true
            }
            // The notice tells the user to press `y`. Zellij's own prompt is
            // gone by the time this state exists, so nothing else would act on
            // that keystroke — the instruction has to be honoured here or it is
            // a lie. Only reachable when the pane has focus, which is exactly
            // the case the click path cannot cover.
            Event::Key(key) if self.permissions_denied => {
                if key.bare_key == BareKey::Char('y') && key.key_modifiers.is_empty() {
                    request_permission(&REQUIRED_PERMISSIONS);
                }
                false
            }
            Event::Mouse(Mouse::LeftClick(_, col)) => {
                let col = col as usize;

                // While denied, the whole bar is a retry button. Answering
                // Zellij's prompt otherwise means focusing a one-row borderless
                // pane by keyboard before y does anything, which is the part
                // people get stuck on; a click re-raises the prompt instead.
                //
                // The flag stays set until a grant actually arrives. Clearing it
                // here would spend the affordance on a single click: a second
                // click would fall through to the prefix region below and open
                // the settings menu, and a prompt dismissed without a Denied
                // event would leave the bar looking healthy while inert.
                if self.permissions_denied {
                    request_permission(&REQUIRED_PERMISSIONS);
                    return true;
                }

                // Check prefix click region first → toggle ViewMode
                if let Some((start, end)) = self.prefix_click_region {
                    if col >= start && col < end {
                        self.view_mode = match self.view_mode {
                            ViewMode::Normal => ViewMode::Settings,
                            ViewMode::Settings => ViewMode::Normal,
                        };
                        return true;
                    }
                }

                match self.view_mode {
                    ViewMode::Normal => {
                        for region in &self.click_regions {
                            if col >= region.start_col && col < region.end_col {
                                if let Some(pane_id) = region.focus_pane_id {
                                    focus_terminal_pane(pane_id, false, false);
                                } else {
                                    switch_tab_to(region.tab_index as u32 + 1);
                                }
                                return false;
                            }
                        }
                        false
                    }
                    ViewMode::Settings => {
                        for region in &self.menu_click_regions {
                            if col >= region.start_col && col < region.end_col {
                                match &region.action {
                                    MenuAction::ToggleSetting(key) => {
                                        match key {
                                            state::SettingKey::Notifications => {
                                                self.settings.notifications =
                                                    self.settings.notifications.cycle();
                                            }
                                            state::SettingKey::Flash => {
                                                self.settings.flash =
                                                    self.settings.flash.cycle();
                                            }
                                            state::SettingKey::ElapsedTime => {
                                                self.settings.elapsed_time =
                                                    !self.settings.elapsed_time;
                                            }
                                            state::SettingKey::ModeIndicator => {
                                                self.settings.mode_indicator =
                                                    !self.settings.mode_indicator;
                                            }
                                        }
                                        self.save_config();
                                    }
                                    MenuAction::CloseMenu => {
                                        self.view_mode = ViewMode::Normal;
                                    }
                                }
                                return true;
                            }
                        }
                        false
                    }
                }
            }
            Event::RunCommandResult(exit_code, stdout, _stderr, context) => {
                match context.get("type").map(|s| s.as_str()) {
                    Some("load_config") if exit_code == Some(0) => {
                        let raw = String::from_utf8_lossy(&stdout);
                        if let Ok(settings) = serde_json::from_str::<Settings>(raw.trim()) {
                            self.settings = settings;
                        }
                        self.config_loaded = true;
                        self.on_command_permissions_granted();
                        true
                    }
                    Some("install_hooks") if exit_code == Some(0) => {
                        self.hooks_installed = true;
                        self.maybe_start_attach_scan();
                        false
                    }
                    Some("attach_scan") => {
                        if exit_code != Some(0) {
                            self.attach_scan_requested = false;
                            return false;
                        }

                        let allowed_panes: Vec<u32> = context
                            .get("pane_ids")
                            .into_iter()
                            .flat_map(|pane_ids| pane_ids.split(','))
                            .filter_map(|pane_id| pane_id.parse().ok())
                            .collect();
                        let pane_leaders: BTreeMap<u32, i32> = context
                            .get("pane_leaders")
                            .into_iter()
                            .flat_map(|pane_leaders| pane_leaders.split(','))
                            .filter_map(|record| {
                                let (pane_id, leader_pid) = record.split_once(':')?;
                                Some((pane_id.parse().ok()?, leader_pid.parse().ok()?))
                            })
                            .collect();
                        let scan_started_ms = context
                            .get("scan_started_ms")
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(0);
                        let introspection_supported = context
                            .get("introspection_supported")
                            .is_some_and(|value| value == "true");
                        let expected_session = self.zellij_session_name.clone();
                        let raw = String::from_utf8_lossy(&stdout);
                        let mut discovered_by_pane: BTreeMap<u32, HookPayload> =
                            BTreeMap::new();
                        for line in raw.lines() {
                            let Ok(mut payload) = serde_json::from_str::<HookPayload>(line) else {
                                continue;
                            };
                            if !allowed_panes.contains(&payload.pane_id)
                                || payload.zellij_session.as_ref() != expected_session.as_ref()
                            {
                                continue;
                            }
                            payload.hook_event = "SessionRestore".to_string();
                            payload.tool_name = None;
                            if payload.is_subagent {
                                continue;
                            }
                            if let Some(previous) = discovered_by_pane.get(&payload.pane_id) {
                                if payload.session_id == previous.session_id
                                    && payload.rainbow_name.is_none()
                                {
                                    payload.rainbow_name = previous.rainbow_name;
                                    payload.rainbow_mode_ts_ms = previous
                                        .rainbow_mode_ts_ms
                                        .or(previous.ts_ms);
                                    payload.rainbow_mode_marker =
                                        previous.rainbow_mode_marker.clone();
                                }
                            }
                            discovered_by_pane.insert(payload.pane_id, payload);
                        }

                        let mut changed = false;
                        for (pane_id, payload) in discovered_by_pane {
                            if !self.pane_to_tab.contains_key(&pane_id) {
                                continue;
                            }
                            if introspection_supported {
                                let Some(expected_leader) = pane_leaders.get(&pane_id)
                                else {
                                    continue;
                                };
                                if get_pane_pid(PaneId::Terminal(pane_id)).ok()
                                    != Some(*expected_leader)
                                {
                                    continue;
                                }
                            }

                            let discovered_id = payload
                                .session_id
                                .as_deref()
                                .filter(|session_id| !session_id.is_empty());
                            if let Some(existing) = self.sessions.get(&pane_id) {
                                let different_owner =
                                    discovered_id != Some(existing.session_id.as_str());
                                let existing_ts_ms = if existing.last_ts_ms > 0 {
                                    existing.last_ts_ms
                                } else {
                                    existing.last_event_ts.saturating_mul(1000)
                                };
                                if different_owner
                                    && !existing.restored
                                    && (scan_started_ms == 0 || existing_ts_ms >= scan_started_ms)
                                {
                                    continue;
                                }
                            }

                            changed |= event_handler::handle_discovered_session(self, payload);
                        }
                        if changed {
                            self.broadcast_sessions();
                        }
                        changed
                    }
                    _ => false,
                }
            }
            Event::Timer(_) => {
                let stale_changed = self.cleanup_stale_sessions();
                let flash_changed = self.cleanup_expired_flashes();
                let placeholder_changed = self.poll_agent_panes();
                let has_flashes = self.has_active_flashes();
                let has_rainbows = self.has_rainbow_sessions();
                if has_rainbows {
                    set_timeout(rainbow::ANIMATION_TICK_SECONDS);
                } else if has_flashes {
                    set_timeout(FLASH_TICK);
                } else {
                    set_timeout(TIMER_INTERVAL);
                }
                has_rainbows
                    || has_flashes
                    || stale_changed
                    || flash_changed
                    || placeholder_changed
                    || self.has_elapsed_display()
            }
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.on_command_permissions_granted();
                // The grant clears the denial notice, so it has to repaint.
                // Relying on load_config's RunCommandResult to do it leaves the
                // banner painted over the tabs whenever that path is skipped —
                // config_loaded already set, or the command never round-trips.
                true
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.command_permissions_granted = false;
                // Zellij will not offer its prompt again on its own, and every
                // path that makes this plugin useful — config, hook install,
                // pane scanning — is behind these permissions. Record it so the
                // bar can say so, and re-render immediately: a silent inert bar
                // is indistinguishable from a working one.
                self.permissions_denied = true;
                true
            }
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        match pipe_message.name.as_str() {
            "zellaude" => {
                // Hook event from CLI
                let payload_str = match pipe_message.payload {
                    Some(ref s) => s,
                    None => return false,
                };
                let payload: HookPayload = match serde_json::from_str(payload_str) {
                    Ok(p) => p,
                    Err(_) => return false,
                };
                event_handler::handle_hook_event(self, payload);
                true
            }
            "zellaude:focus" => {
                // Notification click — focus the requested pane
                if let Some(ref payload) = pipe_message.payload {
                    if let Ok(pane_id) = payload.trim().parse::<u32>() {
                        focus_terminal_pane(pane_id, false, false);
                    }
                }
                false
            }
            "zellaude:request" => {
                // Another instance asking for state — respond with ours
                self.broadcast_sessions();
                false
            }
            "zellaude:settings" => {
                // Another instance broadcast new settings
                if let Some(ref payload) = pipe_message.payload {
                    if let Ok(settings) = serde_json::from_str::<Settings>(payload) {
                        self.settings = settings;
                        return true;
                    }
                }
                false
            }
            "zellaude:sync" => {
                // Another instance sharing state — merge it
                if let Some(ref payload) = pipe_message.payload {
                    if let Ok(sessions) =
                        serde_json::from_str::<BTreeMap<u32, SessionInfo>>(payload)
                    {
                        self.merge_sessions(sessions);
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        render::render_status_bar(self, rows, cols);
    }
}

impl State {
    fn on_command_permissions_granted(&mut self) {
        let newly_granted = !self.command_permissions_granted;
        self.command_permissions_granted = true;
        self.permissions_denied = false;

        // Keep the plugin visible during fullscreen once application-state
        // changes are allowed.
        set_selectable(false);

        if newly_granted {
            self.request_sync();
            if !self.hooks_installed {
                installer::run_install();
            }
        }
        if !self.config_loaded {
            self.load_config();
        }
    }

    fn maybe_start_attach_scan(&mut self) {
        if self.attach_scan_requested
            || !self.command_permissions_granted
            || !self.hooks_installed
        {
            return;
        }
        if self.pane_to_tab.is_empty() || !self.is_on_active_tab() {
            return;
        }
        let supports_introspection = self.introspection_supported();
        let Some(session_name) = self.zellij_session_name.as_deref() else {
            return;
        };

        if attach::run(session_name, &self.pane_to_tab, supports_introspection) {
            self.attach_scan_requested = true;
        }
    }

    /// Only the instance whose tab is visible should spend host calls on
    /// discovery or polling.
    fn is_on_active_tab(&mut self) -> bool {
        let plugin_id = *self
            .plugin_id
            .get_or_insert_with(|| get_plugin_ids().plugin_id);
        let tabs = &self.tabs;
        self.pane_manifest
            .as_ref()
            .is_some_and(|manifest| {
                !tabs.is_empty() && attach::is_active_instance(manifest, tabs, plugin_id)
            })
    }

    fn introspection_supported(&mut self) -> bool {
        *self
            .pane_introspection_supported
            .get_or_insert_with(|| attach::supports_pane_introspection(&get_zellij_version()))
    }

    /// Bounds how often the pane-introspection poll runs. Runs after the cheap
    /// gates so an instance that cannot poll does not consume its own window,
    /// and treats a backward clock step as due so polling cannot latch off.
    fn agent_poll_due(&mut self, now_ms: u64) -> bool {
        if now_ms >= self.last_agent_poll_ms
            && now_ms - self.last_agent_poll_ms < AGENT_POLL_INTERVAL_MS
        {
            return false;
        }
        self.last_agent_poll_ms = now_ms;
        true
    }

    /// Recognize agent TUIs that have not produced a hook event yet (Codex
    /// starts its session lazily on the first prompt) by classifying each
    /// unclaimed pane's current foreground command. Only the instance on the
    /// active tab polls: inactive instances are not visible and re-derive
    /// placeholders when their tab regains focus.
    fn poll_agent_panes(&mut self) -> bool {
        if !self.command_permissions_granted || !self.hooks_installed {
            return false;
        }
        if !self.is_on_active_tab() || !self.introspection_supported() {
            return false;
        }
        let now = unix_now_ms();
        if !self.agent_poll_due(now) {
            return false;
        }

        let candidates: Vec<u32> = self
            .pane_to_tab
            .keys()
            .copied()
            .filter(|pane_id| {
                self.sessions
                    .get(pane_id)
                    .is_none_or(placeholder::is_placeholder)
            })
            .collect();
        let (to_poll, next_cursor) = placeholder::panes_to_poll(
            &self.sessions,
            candidates,
            self.agent_poll_cursor,
            placeholder::AGENT_POLL_BUDGET,
        );
        self.agent_poll_cursor = next_cursor;

        let observed: Vec<(u32, placeholder::PaneAgent)> = to_poll
            .into_iter()
            .map(|pane_id| {
                let observation = if placeholder::ended_recently(
                    &self.pane_session_ended_ms,
                    pane_id,
                    now,
                ) {
                    placeholder::PaneAgent::Unknown
                } else {
                    match get_pane_running_command(PaneId::Terminal(pane_id)) {
                        Ok(command) if attach::client_for_command(&command).is_some() => {
                            placeholder::PaneAgent::Running
                        }
                        Ok(_) => placeholder::PaneAgent::Absent,
                        // A failed query says nothing about the pane.
                        Err(_) => placeholder::PaneAgent::Unknown,
                    }
                };
                (pane_id, observation)
            })
            .collect();
        let changed = placeholder::reconcile_agent_panes(&mut self.sessions, observed);
        if changed {
            self.refresh_session_tab_names();
        }
        changed
    }

    fn rebuild_pane_map(&mut self) {
        if let Some(ref manifest) = self.pane_manifest {
            self.pane_to_tab = tab_pane_map::build_pane_to_tab_map(&self.tabs, manifest);
            self.refresh_session_tab_names();
            self.remove_dead_panes();
        }
    }

    fn refresh_session_tab_names(&mut self) {
        for session in self.sessions.values_mut() {
            if let Some((idx, name)) = self.pane_to_tab.get(&session.pane_id) {
                session.tab_index = Some(*idx);
                session.tab_name = Some(name.clone());
            }
        }
    }

    fn remove_dead_panes(&mut self) {
        self.sessions
            .retain(|pane_id, _| self.pane_to_tab.contains_key(pane_id));
    }

    fn cleanup_stale_sessions(&mut self) -> bool {
        let now = unix_now();
        let mut changed = false;
        for session in self.sessions.values_mut() {
            match session.activity {
                state::Activity::Done | state::Activity::AgentDone => {
                    if now.saturating_sub(session.last_event_ts) >= DONE_TIMEOUT {
                        session.activity = state::Activity::Idle;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        changed
    }

    fn clear_flashes_on_tab(&mut self, tab_idx: usize) {
        let pane_ids: Vec<u32> = self
            .sessions
            .values()
            .filter(|s| s.tab_index == Some(tab_idx))
            .map(|s| s.pane_id)
            .collect();
        for pane_id in pane_ids {
            self.flash_deadlines.remove(&pane_id);
        }
    }

    fn has_active_flashes(&self) -> bool {
        let now = unix_now_ms();
        self.flash_deadlines.values().any(|&deadline| now < deadline)
    }

    fn has_rainbow_sessions(&self) -> bool {
        self.sessions.values().any(|session| session.rainbow_name)
    }

    fn cleanup_expired_flashes(&mut self) -> bool {
        let before = self.flash_deadlines.len();
        let now = unix_now_ms();
        self.flash_deadlines.retain(|_, deadline| now < *deadline);
        self.flash_deadlines.len() != before
    }

    fn has_elapsed_display(&self) -> bool {
        if !self.settings.elapsed_time {
            return false;
        }
        let now = unix_now();
        self.sessions.values().any(|s| {
            !matches!(s.activity, state::Activity::Idle)
                && now.saturating_sub(s.last_event_ts) >= DONE_TIMEOUT
        })
    }

    fn request_sync(&self) {
        pipe_message_to_plugin(MessageToPlugin::new("zellaude:request"));
    }

    fn broadcast_sessions(&self) {
        // Placeholders are derived locally from pane introspection; syncing
        // them could resurrect one an instance already removed.
        let shared: BTreeMap<u32, &SessionInfo> = self
            .sessions
            .iter()
            .filter(|(_, session)| !placeholder::is_placeholder(session))
            .map(|(pane_id, session)| (*pane_id, session))
            .collect();
        let mut msg = MessageToPlugin::new("zellaude:sync");
        msg.message_payload = Some(serde_json::to_string(&shared).unwrap_or_default());
        pipe_message_to_plugin(msg);
    }

    fn broadcast_settings(&self) {
        let mut msg = MessageToPlugin::new("zellaude:settings");
        msg.message_payload =
            Some(serde_json::to_string(&self.settings).unwrap_or_default());
        pipe_message_to_plugin(msg);
    }

    fn load_config(&self) {
        let mut ctx = BTreeMap::new();
        ctx.insert("type".into(), "load_config".into());
        run_command(
            &[
                "sh",
                "-c",
                "cat \"$HOME/.config/zellij/plugins/zellaude.json\" 2>/dev/null || echo '{}'",
            ],
            ctx,
        );
    }

    fn save_config(&self) {
        if !self.config_loaded {
            return;
        }
        self.broadcast_settings();
        let json = serde_json::to_string(&self.settings).unwrap_or_default();
        let json_esc = json.replace('\'', "'\\''");
        let cmd = format!(
            "mkdir -p \"$HOME/.config/zellij/plugins\" && printf '%s' '{json_esc}' > \"$HOME/.config/zellij/plugins/zellaude.json\""
        );
        let mut ctx = BTreeMap::new();
        ctx.insert("type".into(), "save_config".into());
        run_command(&["sh", "-c", &cmd], ctx);
    }

    fn merge_sessions(&mut self, incoming: BTreeMap<u32, SessionInfo>) {
        for (pane_id, mut session) in incoming {
            if placeholder::is_placeholder(&session) {
                continue;
            }
            let incoming_ts_ms = if session.last_ts_ms > 0 {
                session.last_ts_ms
            } else {
                session.last_event_ts.saturating_mul(1000)
            };
            let tombstone_key = (pane_id, session.session_id.clone());
            let newer_than_tombstone =
                match self.session_end_tombstones.get(&tombstone_key).copied() {
                    Some(ended_at) if incoming_ts_ms <= ended_at => continue,
                    Some(_) => true,
                    None => false,
                };

            let mut same_current_owner = false;
            let dominated = if let Some(existing) = self.sessions.get_mut(&pane_id) {
                same_current_owner = existing.session_id == session.session_id;
                session_selection::reconcile_rainbow_mode(&mut session, existing);
                session_selection::is_newer_than(&session, existing)
            } else {
                true
            };
            if dominated {
                // Refresh tab name from our local pane map
                if let Some((idx, name)) = self.pane_to_tab.get(&pane_id) {
                    session.tab_index = Some(*idx);
                    session.tab_name = Some(name.clone());
                }
                self.sessions.insert(pane_id, session);
            }
            if newer_than_tombstone {
                if dominated || same_current_owner {
                    self.session_end_tombstones.remove(&tombstone_key);
                } else {
                    self.session_end_tombstones
                        .entry(tombstone_key)
                        .and_modify(|blocked_at| {
                            *blocked_at = (*blocked_at).max(incoming_ts_ms)
                        })
                        .or_insert(incoming_ts_ms);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use state::Activity;

    fn session(session_id: &str, ts_ms: u64, restored: bool) -> SessionInfo {
        SessionInfo {
            session_id: session_id.to_string(),
            pane_id: 7,
            activity: Activity::Idle,
            tab_name: None,
            tab_index: None,
            last_event_ts: ts_ms / 1000,
            cwd: None,
            last_ts_ms: ts_ms,
            rainbow_name: false,
            rainbow_name_known: true,
            rainbow_mode_ts_ms: ts_ms,
            rainbow_mode_marker: None,
            restored,
            placeholder: false,
        }
    }

    #[test]
    fn peer_sync_cannot_resurrect_a_session_older_than_its_end() {
        let mut state = State::default();
        state
            .session_end_tombstones
            .insert((7, "ended".to_string()), 30);

        state.merge_sessions(BTreeMap::from([(7, session("ended", 20, false))]));
        assert!(state.sessions.is_empty());

        state.merge_sessions(BTreeMap::from([(7, session("ended", 40, false))]));
        assert_eq!(state.sessions.get(&7).unwrap().last_ts_ms, 40);
        assert!(state.session_end_tombstones.is_empty());
    }

    #[test]
    fn rejected_peer_does_not_clear_its_ended_owner_tombstone() {
        let mut state = State::default();
        state
            .session_end_tombstones
            .insert((7, "ended".to_string()), 30);
        state
            .sessions
            .insert(7, session("current", 100, false));

        state.merge_sessions(BTreeMap::from([(7, session("ended", 40, false))]));

        assert_eq!(state.sessions.get(&7).unwrap().session_id, "current");
        assert_eq!(
            state
                .session_end_tombstones
                .get(&(7, "ended".to_string())),
            Some(&40)
        );
    }

    #[test]
    fn peer_sync_never_imports_placeholders() {
        let mut state = State::default();

        state.merge_sessions(BTreeMap::from([(
            7,
            placeholder::placeholder_session(7),
        )]));

        assert!(state.sessions.is_empty());
    }

    #[test]
    fn a_placeholder_is_promoted_in_place_by_the_first_hook_event() {
        let mut state = State::default();
        state
            .sessions
            .insert(7, placeholder::placeholder_session(7));

        event_handler::handle_hook_event(
            &mut state,
            HookPayload {
                session_id: Some("real".to_string()),
                pane_id: 7,
                hook_event: "UserPromptSubmit".to_string(),
                tool_name: None,
                cwd: None,
                zellij_session: None,
                term_program: None,
                ts_ms: Some(1000),
                is_subagent: false,
                rainbow_name: Some(false),
                rainbow_mode_ts_ms: None,
                rainbow_mode_marker: None,
            },
        );

        let session = state.sessions.get(&7).unwrap();
        assert_eq!(session.session_id, "real");
        assert_eq!(session.activity, Activity::Thinking);
        assert!(!placeholder::is_placeholder(session));
    }

    #[test]
    fn the_agent_poll_throttle_consumes_a_window_it_has_stamped() {
        let mut state = State::default();

        assert!(state.agent_poll_due(10_000));
        assert!(!state.agent_poll_due(10_001));
        assert!(state.agent_poll_due(10_000 + AGENT_POLL_INTERVAL_MS));
    }

    #[test]
    fn a_backward_clock_step_cannot_latch_the_agent_poll_off() {
        let mut state = State::default();
        assert!(state.agent_poll_due(100_000));

        // The clock steps back; the stale future stamp must not block polling.
        assert!(state.agent_poll_due(40_000));
        assert!(!state.agent_poll_due(40_001));
    }

    #[test]
    fn peer_sync_imports_a_real_session_whose_id_is_empty() {
        let mut state = State::default();

        state.merge_sessions(BTreeMap::from([(7, session("", 1000, false))]));

        assert_eq!(state.sessions.get(&7).map(|s| s.last_ts_ms), Some(1000));
    }

    #[test]
    fn legacy_synced_mode_is_treated_as_unknown_but_keeps_its_rendered_value() {
        let legacy: SessionInfo = serde_json::from_str(
            r#"{
                "session_id":"legacy",
                "pane_id":7,
                "activity":"Idle",
                "tab_name":null,
                "tab_index":null,
                "last_event_ts":1,
                "cwd":null,
                "last_ts_ms":1000,
                "rainbow_name":true,
                "rainbow_mode_marker":null
            }"#,
        )
        .unwrap();

        assert!(legacy.rainbow_name);
        assert!(!legacy.rainbow_name_known);
    }
}

/// zellij-tile links against a wasm host import that does not exist when the
/// crate is built for the host triple, so `cargo test` could not link the binary
/// at all and the test suite was unrunnable. Stub it for non-wasm builds —
/// `#[cfg(test)]` is not enough, because the presence of a `tests/` directory
/// makes cargo build the plain binary too. Never reached: the pure logic under
/// test does not call into the host.
#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
extern "C" fn host_run_plugin_command() {}
