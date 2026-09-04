mod attach;
mod custom_layouts;
mod event_handler;
mod installer;
mod layout_generators;
mod manifest;
mod placeholder;
mod rainbow;
mod render;
mod session_selection;
mod session_templates;
mod split_three;
mod state;
mod tab_pane_map;
mod tool_symbol;
mod theme;

use state::{unix_now, unix_now_ms, HookPayload, MenuAction, SessionInfo, Settings, State, ViewMode};
use std::collections::BTreeMap;
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;

const DONE_TIMEOUT: u64 = 30;
const TIMER_INTERVAL: f64 = 1.0;
const MANIFEST_DEBOUNCE_MS: u64 = 1000;
const FLASH_TICK: f64 = 0.25;
const SPLIT_THREE_ACTION_TIMEOUT_MS: u64 = 5000;
const SAVE_CONFIG_SCRIPT: &str = r#"
set -eu
if [ "$#" -ge 2 ]; then
    config_path=$2
else
    config_path="$HOME/.config/zellij/plugins/zellaude.json"
fi
symlink_hops=0
while [ -L "$config_path" ]; do
    symlink_hops=$((symlink_hops + 1))
    if [ "$symlink_hops" -gt 40 ]; then
        printf '%s\n' 'too many symlinks in zellaude config path' >&2
        exit 1
    fi
    link_target=$(readlink "$config_path")
    case "$link_target" in
        /*) config_path=$link_target ;;
        *) config_path=$(dirname "$config_path")/$link_target ;;
    esac
done
config_dir=$(dirname "$config_path")
mkdir -p "$config_dir"
umask 077
tmp_path=$(mktemp "$config_dir/.zellaude.json.XXXXXX")
trap 'rm -f "$tmp_path"' 0 HUP INT TERM

if [ -s "$config_path" ]; then
    jq --slurp --argjson settings "$1" '
        if length != 1 then
            error("zellaude configuration must contain exactly one JSON value")
        else .[0] |
        if type == "array" then
            $settings + {custom_states: .}
        elif type == "object" and has("id") then
            $settings + {custom_states: [.]}
        elif type == "object" then
            . + $settings
        else
            error("zellaude configuration must be a JSON object or array")
        end
        end
    ' "$config_path" > "$tmp_path"
else
    printf '%s\n' "$1" > "$tmp_path"
fi

mv "$tmp_path" "$config_path"
trap - 0 HUP INT TERM
"#;
/// Print the `CustomStateSources` envelope: the raw zellaude.json text and
/// every generator file, so the plugin's own parsers keep receiving the raw
/// documents. No document travels in argv -- one argument caps at 128 KiB and
/// a custom-state file can exceed that -- so each is read with `--rawfile` into
/// a buffer file that one `jq -s` then wraps. Only the paths are arguments,
/// bounded by `PATH_MAX`. The buffer is a file rather than a pipe so `set -e`
/// still governs the loop: a pipeline reports its last stage, so a file that
/// cannot be read would exit 0 with the envelope silently short, and
/// `apply_custom_state_sources` would take that as the complete current state
/// and drop the generators it does not name. `LC_ALL=C` makes the file order
/// byte order rather than the caller's locale.
const RELOAD_CUSTOM_STATES_SCRIPT: &str = r#"
set -eu
LC_ALL=C
export LC_ALL
config_path="$HOME/.config/zellij/plugins/zellaude.json"
generators_dir="$HOME/.config/zellij/plugins/zellaude/generators"
set -- --rawfile settings "$config_path"
[ -f "$config_path" ] || set -- --arg settings '{}'
stream=$(mktemp)
trap 'rm -f "$stream"' 0 HUP INT TERM
for path in "$generators_dir"/*.kdl; do
    [ -f "$path" ] || continue
    jq -n --arg path "$path" --rawfile content "$path" \
        '{path: $path, content: $content}' >> "$stream"
done
jq -s "$@" '{settings_json: $settings, generator_files: .}' < "$stream"
"#;

fn split_three_focus_matches(tab_id: usize, pane_id: u32) -> bool {
    matches!(
        get_focused_pane_info(),
        Ok((focused_tab_id, PaneId::Terminal(focused_pane_id)))
            if focused_tab_id == tab_id && focused_pane_id == pane_id
    )
}

/// Everything the plugin needs to function at all. Named once so the initial
/// request and the retry cannot drift apart — a retry asking for a smaller set
/// would be granted and still leave the plugin unable to work.
const REQUIRED_PERMISSIONS: [PermissionType; 7] = [
    PermissionType::ReadApplicationState,
    PermissionType::ChangeApplicationState,
    PermissionType::RunCommands,
    PermissionType::ReadCliPipes,
    PermissionType::MessageAndLaunchOtherPlugins,
    PermissionType::Reconfigure,
    PermissionType::RunActionsAsUser,
];

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.plugin_configuration = configuration.clone();
        self.custom_layouts_from_plugin_configuration =
            custom_layouts::has_plugin_configuration(&configuration);
        match custom_layouts::parse_plugin_configuration(&configuration) {
            Ok(Some(layouts)) => {
                self.custom_layouts = custom_layouts::index(layouts);
            }
            Ok(None) => {}
            Err(error) => self.custom_layout_config_error = Some(error),
        }
        self.split_three_uses_legacy_keybinds =
            split_three::uses_legacy_mode_keybinds(&get_zellij_version());
        request_permission(&REQUIRED_PERMISSIONS);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::ModeUpdate,
            EventType::SessionUpdate,
            EventType::Timer,
            EventType::Mouse,
            EventType::Key,
            EventType::PastedText,
            EventType::RunCommandResult,
            EventType::PermissionRequestResult,
            EventType::InitialKeybinds,
            EventType::ActionComplete,
        ]);
        set_timeout(TIMER_INTERVAL);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::TabUpdate(tabs) => {
                // Inactive-tab plugins do not receive tab updates. Reset on
                // every update delivered to the newly active instance so a
                // returning tab always retargets the client-scoped binding.
                self.split_three_bindings_installed = false;
                self.custom_layout_bindings_installed = false;
                let new_active = tabs.iter().find(|t| t.active).map(|t| t.position);
                if new_active != self.active_tab_index {
                    // Tab focus changed — clear persist flashes on the newly focused tab
                    if let Some(idx) = new_active {
                        self.clear_flashes_on_tab(idx);
                    }
                }
                self.active_tab_index = new_active;
                self.tabs = tabs;
                self.rebuild_pane_map();
                self.maybe_cancel_custom_layout_prompt_on_focus_loss();
                self.maybe_install_runtime_bindings();
                self.maybe_start_attach_scan();
                self.maybe_write_manifest();
                true
            }
            Event::PaneUpdate(manifest) => {
                self.pane_manifest = Some(manifest);
                self.rebuild_pane_map();
                self.maybe_cancel_custom_layout_prompt_on_focus_loss();
                self.maybe_recover_pending_split_three_spawn();
                self.maybe_finish_split_three_validation();
                self.maybe_install_runtime_bindings();
                self.maybe_start_attach_scan();
                self.maybe_compile_session_templates();
                self.maybe_write_manifest();
                true
            }
            Event::ModeUpdate(mode_info) => {
                let legacy_keybinds = mode_info.keybinds;
                self.input_mode = mode_info.mode;
                self.zellij_styling = Some(mode_info.style.colors);
                if let Some(name) = mode_info.session_name {
                    self.zellij_session_name = Some(name.clone());
                    self.reported_session_name = Some(name);
                }
                self.maybe_capture_legacy_keybinds(legacy_keybinds);
                self.maybe_start_attach_scan();
                self.maybe_write_manifest();
                true
            }
            // The only client-independent source of the session's current
            // name: ModeUpdate needs an attached client, and hook payloads
            // carry the stale launch-time name after a rename. Without this a
            // detached session never learns its name (or a rename), and the
            // manifest cannot follow.
            Event::SessionUpdate(sessions, _) => {
                if let Some(current) = sessions.iter().find(|session| session.is_current_session) {
                    if self.reported_session_name.as_deref() != Some(current.name.as_str()) {
                        self.reported_session_name = Some(current.name.clone());
                        // Written from the update's own topology snapshot:
                        // this event reaches hidden instances too, whose own
                        // tabs/panes stop updating while another tab is
                        // visible and would republish a stale layout here.
                        if !current.tabs.is_empty() {
                            let body = manifest::body(&current.name, &current.tabs, &current.panes);
                            self.dispatch_manifest(body);
                        }
                    }
                }
                false
            }
            Event::Key(key) if self.custom_layout_prompt.is_some() => {
                self.handle_custom_layout_prompt_key(key)
            }
            Event::PastedText(pasted) if self.custom_layout_prompt.is_some() => {
                self.handle_custom_layout_paste(&pasted)
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
                if let Some(prompt) = self.custom_layout_prompt.as_mut() {
                    prompt.note_input();
                    return true;
                }
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
                                let focus_pane_id = region
                                    .focus_pane_id
                                    .filter(|_| self.settings.smart_focus);
                                if let Some(pane_id) = focus_pane_id {
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
                                            state::SettingKey::SmartFocus => {
                                                self.settings.smart_focus =
                                                    !self.settings.smart_focus;
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
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                match context.get("type").map(|s| s.as_str()) {
                    Some("load_config") if exit_code == Some(0) => {
                        let raw = String::from_utf8_lossy(&stdout);
                        if let Ok(settings) = serde_json::from_str::<Settings>(raw.trim()) {
                            self.settings = settings;
                        }
                        match custom_layouts::parse_config_document(raw.trim()) {
                            Ok(Some(layouts)) if !self.custom_layouts_from_plugin_configuration => {
                                self.custom_layouts = custom_layouts::index(layouts);
                                self.custom_layout_config_error = None;
                            }
                            Ok(Some(_)) | Ok(None) => {}
                            Err(error) if !self.custom_layouts_from_plugin_configuration => {
                                self.custom_layout_config_error = Some(error);
                            }
                            Err(_) => {}
                        }
                        match session_templates::parse_config_document(raw.trim()) {
                            Ok(configured) => {
                                self.session_templates =
                                    Some(session_templates::effective(configured));
                                self.session_template_config_error = None;
                            }
                            Err(error) => {
                                // Keep whatever is already on disk: a typo in
                                // one key must not strand a session template
                                // the user relies on mid-day. `load_config` only
                                // ever runs once per instance, so `session_templates`
                                // never becomes `Some` in the same instance this
                                // error is recorded in — report it here, since the
                                // check inside `maybe_compile_session_templates`
                                // would otherwise never see it.
                                eprintln!("Zellaude could not read session templates: {error}");
                                self.session_template_config_error = Some(error);
                            }
                        }
                        self.config_loaded = true;
                        self.on_command_permissions_granted();
                        self.maybe_compile_session_templates();
                        true
                    }
                    Some("reload_custom_states") => {
                        match (exit_code, serde_json::from_slice(&stdout)) {
                            (Some(0), Ok(sources)) => self.apply_custom_state_sources(&sources),
                            (Some(0), Err(error)) => {
                                eprintln!("Zellaude could not read custom states: {error}")
                            }
                            _ => eprintln!(
                                "Zellaude could not read custom states: {}",
                                String::from_utf8_lossy(&stderr).trim()
                            ),
                        }
                        // Runs on the failure path too: a submit held for this
                        // reload must not be swallowed by a read that failed.
                        self.resolve_pending_submit();
                        true
                    }
                    Some("install_hooks") if exit_code == Some(0) => {
                        self.hooks_installed = true;
                        self.maybe_start_attach_scan();
                        false
                    }
                    Some("save_config") => {
                        if exit_code != Some(0) {
                            eprintln!(
                                "Zellaude could not save settings: {}",
                                String::from_utf8_lossy(&stderr).trim()
                            );
                        }
                        false
                    }
                    Some("write_layout") => {
                        if exit_code != Some(0) {
                            eprintln!(
                                "Zellaude could not write layout {}: {}",
                                context.get("layout").map(String::as_str).unwrap_or("?"),
                                String::from_utf8_lossy(&stderr).trim()
                            );
                        }
                        false
                    }
                    Some("write_manifest") => {
                        if exit_code != Some(0) {
                            eprintln!(
                                "Zellaude could not write the session manifest: {}",
                                String::from_utf8_lossy(&stderr).trim()
                            );
                            // Requeue the failed body so the timer retries it,
                            // instead of skipping it as "already written".
                            self.manifest_pending_body = self.manifest_last_body.take();
                        }
                        false
                    }
                    Some("prune_layouts") => {
                        if exit_code != Some(0) {
                            eprintln!(
                                "Zellaude could not prune generated layouts: {}",
                                String::from_utf8_lossy(&stderr).trim()
                            );
                        }
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
                        let scan_started_ms = context
                            .get("scan_started_ms")
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(0);
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
                if let Some(pending_body) = self.manifest_pending_body.take() {
                    self.dispatch_manifest(pending_body);
                }
                let custom_layout_prompt_changed =
                    self.maybe_cancel_custom_layout_prompt_on_focus_loss();
                self.maybe_recover_pending_split_three_spawn();
                self.maybe_finish_split_three_validation();
                self.recover_stalled_split_three();
                let stale_changed = self.cleanup_stale_sessions();
                let flash_changed = self.cleanup_expired_flashes();
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
                    || custom_layout_prompt_changed
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
            Event::InitialKeybinds(keybinds) => {
                self.initial_keybinds = Some(keybinds);
                self.maybe_install_runtime_bindings();
                false
            }
            Event::ActionComplete(_action, affected_pane_id, context) => {
                self.handle_split_three_action_complete(affected_pane_id, context);
                false
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
            split_three::PIPE_NAME => {
                if let Some(direction) =
                    split_three::SplitDirection::from_payload(pipe_message.payload.as_deref())
                {
                    self.start_split_three(direction);
                }
                false
            }
            custom_layouts::PIPE_NAME => {
                if pipe_message.payload.as_deref() == Some("prompt") {
                    self.start_custom_layout_prompt();
                }
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        render::render_status_bar(self, rows, cols);
    }
}

impl State {
    fn start_custom_layout_prompt(&mut self) {
        if self.custom_layout_prompt.is_some()
            || self.split_three_operation.is_some()
            || !self.command_permissions_granted
            || !self.split_three_instance_is_active()
        {
            return;
        }

        let Ok((_tab_id, PaneId::Terminal(return_pane_id))) = get_focused_pane_info() else {
            return;
        };
        let cwd = get_pane_cwd(PaneId::Terminal(return_pane_id))
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
        let mut prompt = custom_layouts::Prompt::new(return_pane_id, cwd);
        prompt.error = self.custom_layout_prompt_hint();

        self.view_mode = ViewMode::Normal;
        self.custom_layout_prompt = Some(prompt);
        set_selectable(true);
        switch_to_input_mode(&InputMode::Normal);
        let plugin_id = *self
            .plugin_id
            .get_or_insert_with(|| get_plugin_ids().plugin_id);
        focus_plugin_pane(plugin_id, false, false);
        // Do not mark focus as acquired until the host reports it. A pane
        // update queued before this request may still describe the original
        // terminal; the bounded acquisition window ignores that stale frame
        // while still cleaning up a focus request the host never honors.
        if let Some(prompt) = self.custom_layout_prompt.as_mut() {
            prompt.begin_focus_acquisition(unix_now_ms());
        }
        self.reload_custom_states();
    }

    /// What the prompt shows before anything is typed: a configuration error
    /// from either source, else a nudge when nothing is configured at all.
    fn custom_layout_prompt_hint(&self) -> Option<String> {
        if let Some(error) = self
            .custom_layout_config_error
            .as_deref()
            .or(self.layout_generator_config_error.as_deref())
        {
            return Some(format!("Configuration error: {error}"));
        }
        (self.custom_layouts.is_empty() && self.layout_generators.is_empty())
            .then(|| "No custom states configured".to_string())
    }

    fn reload_custom_states(&mut self) {
        let mut ctx = BTreeMap::new();
        ctx.insert("type".into(), "reload_custom_states".into());
        run_command(
            &[
                "sh",
                "-c",
                RELOAD_CUSTOM_STATES_SCRIPT,
                "zellaude-reload-custom-states",
            ],
            ctx,
        );
    }

    /// Apply a reload: whatever parsed replaces what is held; a source that
    /// failed to parse keeps its last good copy and fills its error slot, so a
    /// typo mid-edit cannot strand a state the user is about to open. States
    /// declared in the plugin block outrank the file and ignore it entirely.
    fn apply_custom_state_sources(&mut self, sources: &layout_generators::CustomStateSources) {
        let settings_json = sources.settings_json.trim();
        if !self.custom_layouts_from_plugin_configuration {
            match custom_layouts::parse_config_document(settings_json) {
                Ok(configured) => {
                    self.custom_layouts = configured.map(custom_layouts::index).unwrap_or_default();
                    self.custom_layout_config_error = None;
                }
                Err(error) => self.custom_layout_config_error = Some(error),
            }
        }

        self.layout_generator_config_error = None;
        match layout_generators::parse_generator_files(&sources.generator_files) {
            Ok(generators) => self.layout_generators = generators,
            Err(error) => self.layout_generator_config_error = Some(error),
        }
        match layout_generators::parse_floor_overrides(settings_json) {
            Ok(overrides) => self.pane_floor_overrides = overrides,
            Err(error) => {
                self.layout_generator_config_error.get_or_insert(error);
            }
        }

        let hint = self.custom_layout_prompt_hint();
        if let Some(prompt) = self.custom_layout_prompt.as_mut() {
            if prompt.input.is_empty() {
                prompt.error = hint;
            }
        }
    }

    /// The tab the prompt was opened from: its name binds `{tab}`, and its
    /// display area minus the bars a generated tab carries is what the grid
    /// may use.
    fn prompt_source_tab(
        &self,
        tab_position: usize,
        chrome: &custom_layouts::TabChrome,
        plugin_location: &str,
    ) -> Option<layout_generators::SourceTab> {
        let tab = self.tabs.iter().find(|tab| tab.position == tab_position)?;
        Some(layout_generators::SourceTab {
            name: tab.name.clone(),
            geometry: layout_generators::TabGeometry {
                columns: tab.display_area_columns,
                rows: tab
                    .display_area_rows
                    .saturating_sub(chrome.bar_rows(plugin_location)),
            },
        })
    }

    /// An exact `custom_states` id first, so a literal id such as `2fa` keeps
    /// working, then the generators. Generator refusals already name the file
    /// they came from, so they reach the prompt untouched.
    fn resolve_custom_state(
        &self,
        input: &str,
        source: &layout_generators::SourceTab,
    ) -> Result<Vec<custom_layouts::CustomLayout>, String> {
        if let Some(layout) = self.custom_layouts.get(input) {
            return Ok(vec![layout.clone()]);
        }
        layout_generators::invoke(
            &self.layout_generators,
            input,
            source,
            self.pane_floor_overrides,
        )
    }

    fn resolve_pending_submit(&mut self) {
        let pending = self
            .custom_layout_prompt
            .as_mut()
            .and_then(|prompt| prompt.pending_submit.take());
        if let Some(input) = pending {
            self.launch_custom_state(&input);
        }
    }

    fn handle_custom_layout_prompt_key(&mut self, key: KeyWithModifier) -> bool {
        let action = self.custom_layout_prompt.as_mut().map(|prompt| {
            prompt.note_input();
            custom_layouts::handle_prompt_key(prompt, &key)
        });
        match action {
            Some(custom_layouts::PromptKey::Submit(id)) => self.open_custom_layout(&id),
            Some(custom_layouts::PromptKey::Cancel) => self.cancel_custom_layout_prompt(),
            Some(custom_layouts::PromptKey::Updated) => true,
            Some(custom_layouts::PromptKey::Ignored) | None => false,
        }
    }

    fn handle_custom_layout_paste(&mut self, pasted: &str) -> bool {
        self.custom_layout_prompt.as_mut().is_some_and(|prompt| {
            prompt.note_input();
            custom_layouts::handle_paste(prompt, pasted)
        })
    }

    fn maybe_cancel_custom_layout_prompt_on_focus_loss(&mut self) -> bool {
        let Some(prompt) = self.custom_layout_prompt.as_mut() else {
            return false;
        };
        let plugin_id = *self
            .plugin_id
            .get_or_insert_with(|| get_plugin_ids().plugin_id);
        let is_focused = match get_focused_pane_info() {
            Ok((_tab_id, PaneId::Plugin(focused_plugin_id))) => focused_plugin_id == plugin_id,
            Ok(_) => false,
            // A host-query timeout says nothing about focus. The next targeted
            // timer will retry, including while this plugin's tab is inactive.
            Err(_) => return false,
        };
        if prompt.observe_focus(is_focused, unix_now_ms()) {
            self.custom_layout_prompt = None;
            set_selectable(false);
            true
        } else {
            false
        }
    }

    /// Every Enter re-reads the files first: a refusal leaves the prompt open
    /// with the input intact, so the next Enter has to see an edit made in
    /// between. The launch happens in `resolve_pending_submit` when the read
    /// lands, and `launch_custom_state` never reloads, so the cycle is one
    /// deep.
    fn open_custom_layout(&mut self, id: &str) -> bool {
        if id.is_empty() {
            if let Some(prompt) = self.custom_layout_prompt.as_mut() {
                prompt.error = Some("Enter a custom state id".to_string());
            }
            return true;
        }
        if let Some(prompt) = self.custom_layout_prompt.as_mut() {
            prompt.pending_submit = Some(id.to_string());
        }
        self.reload_custom_states();
        true
    }

    /// Resolve and open, against the files as the reload just read them. Only
    /// `resolve_pending_submit` calls this, and it never reloads itself --
    /// that is what keeps the submit cycle one deep.
    fn launch_custom_state(&mut self, input: &str) -> bool {
        let plugin_id = *self
            .plugin_id
            .get_or_insert_with(|| get_plugin_ids().plugin_id);
        // `.iter()`, not `.values()`: the position key is what TabInfo is
        // looked up by, and the grid is sized from that tab. The same walk
        // reads the bars, which the new tab has to repeat itself -- Zellij
        // parses the generated KDL alone, so the session's tab template, and
        // with it the status bar under the grid, never reaches it.
        let instance_tab = self.pane_manifest.as_ref().and_then(|manifest| {
            manifest.panes.iter().find_map(|(position, panes)| {
                let plugin = panes
                    .iter()
                    .find(|pane| pane.is_plugin && pane.id == plugin_id)?;
                let location = plugin.plugin_url.clone()?;
                Some((*position, location, custom_layouts::tab_chrome(panes)))
            })
        });
        let Some((tab_position, plugin_location, chrome)) = instance_tab else {
            if let Some(prompt) = self.custom_layout_prompt.as_mut() {
                prompt.error = Some("Zellaude plugin location is unavailable".to_string());
            }
            return true;
        };
        let Some(source) = self.prompt_source_tab(tab_position, &chrome, &plugin_location) else {
            if let Some(prompt) = self.custom_layout_prompt.as_mut() {
                prompt.error = Some("Zellaude cannot read this tab's size".to_string());
            }
            return true;
        };
        let tabs = match self.resolve_custom_state(input, &source) {
            Ok(tabs) => tabs,
            Err(error) => {
                if let Some(prompt) = self.custom_layout_prompt.as_mut() {
                    prompt.error = Some(error);
                }
                return true;
            }
        };
        let cwd = self
            .custom_layout_prompt
            .as_ref()
            .and_then(|prompt| prompt.cwd.as_deref());
        let kdl = match custom_layouts::tabs_to_kdl(
            &tabs,
            &plugin_location,
            &self.plugin_configuration,
            cwd,
            &chrome,
        ) {
            Ok(kdl) => kdl,
            Err(error) => {
                if let Some(prompt) = self.custom_layout_prompt.as_mut() {
                    prompt.error = Some(error);
                }
                return true;
            }
        };
        // Zellij 0.44 waits at most one second for tab creation and returns an
        // empty ID list on timeout without cancelling the queued action. The
        // generated KDL has already been validated structurally, so treating
        // an empty response as a retryable failure could launch every command
        // twice when the original tab finishes opening a moment later.
        let _tab_ids = new_tabs_with_layout(&kdl);

        self.custom_layout_prompt = None;
        set_selectable(false);
        switch_to_input_mode(&InputMode::Normal);
        true
    }

    fn cancel_custom_layout_prompt(&mut self) -> bool {
        let Some(prompt) = self.custom_layout_prompt.take() else {
            return false;
        };
        set_selectable(false);
        switch_to_input_mode(&InputMode::Normal);
        if get_pane_info(PaneId::Terminal(prompt.return_pane_id)).is_some() {
            focus_terminal_pane(prompt.return_pane_id, false, false);
        }
        true
    }

    fn split_three_instance_is_active(&mut self) -> bool {
        let plugin_id = *self
            .plugin_id
            .get_or_insert_with(|| get_plugin_ids().plugin_id);
        self.pane_manifest.as_ref().is_some_and(|manifest| {
            split_three::is_active_instance(manifest, &self.tabs, plugin_id)
        })
    }

    fn start_split_three(&mut self, direction: split_three::SplitDirection) {
        if self.split_three_operation.is_some()
            || !self.command_permissions_granted
            || !self.split_three_instance_is_active()
        {
            return;
        }

        let Ok((tab_id, PaneId::Terminal(original_pane_id))) = get_focused_pane_info() else {
            return;
        };
        let Some(original_pane) = get_pane_info(PaneId::Terminal(original_pane_id)) else {
            return;
        };
        if !split_three::target_is_supported(&original_pane, direction) {
            return;
        }

        self.split_three_next_operation_id = self.split_three_next_operation_id.wrapping_add(1);
        if self.split_three_next_operation_id == 0 {
            self.split_three_next_operation_id = 1;
        }
        let mut operation = split_three::Operation::new(
            self.split_three_next_operation_id,
            direction,
            tab_id,
            original_pane_id,
            (&original_pane).into(),
        );
        if let Some(panes) = self.pane_manifest.as_ref().and_then(|manifest| {
            manifest.panes.values().find(|panes| {
                panes
                    .iter()
                    .any(|pane| !pane.is_plugin && pane.id == original_pane_id)
            })
        }) {
            operation.initial_terminal_pane_count = panes
                .iter()
                .filter(|pane| !pane.is_plugin && !pane.is_suppressed)
                .count();
            operation.known_terminal_pane_ids = panes
                .iter()
                .filter(|pane| !pane.is_plugin)
                .map(|pane| pane.id)
                .collect();
        }
        operation.known_terminal_pane_ids.insert(original_pane_id);
        self.dispatch_split_three(operation, split_three::focus_pane_action(original_pane_id));
    }

    fn handle_split_three_action_complete(
        &mut self,
        affected_pane_id: Option<PaneId>,
        context: BTreeMap<String, String>,
    ) {
        let Some(mut operation) = self.split_three_operation.take() else {
            return;
        };
        if !operation.matches_context(&context) {
            let late_first = operation.stage == split_three::OperationStage::RecoverFirstSplit
                && operation
                    .matches_context_for_stage(&context, split_three::OperationStage::FirstSplit);
            let late_second = operation.stage == split_three::OperationStage::RecoverSecondSplit
                && operation
                    .matches_context_for_stage(&context, split_three::OperationStage::SecondSplit);
            if let Some(PaneId::Terminal(pane_id)) = affected_pane_id {
                if late_first
                    && pane_id != operation.original_pane_id
                    && !operation.known_terminal_pane_ids.contains(&pane_id)
                {
                    operation.first_new_pane_id = Some(pane_id);
                    rename_terminal_pane(pane_id, "");
                    self.begin_split_three_rollback(operation);
                    return;
                }
                if late_second
                    && pane_id != operation.original_pane_id
                    && Some(pane_id) != operation.first_new_pane_id
                    && !operation.known_terminal_pane_ids.contains(&pane_id)
                {
                    operation.second_new_pane_id = Some(pane_id);
                    self.wait_for_split_three_validation(operation);
                    return;
                }
            }
            self.split_three_operation = Some(operation);
            return;
        }

        match operation.stage {
            split_three::OperationStage::FocusOriginal => {
                if !split_three_focus_matches(operation.tab_id, operation.original_pane_id) {
                    self.finish_split_three_operation();
                    return;
                }
                let Some(original_pane) =
                    get_pane_info(PaneId::Terminal(operation.original_pane_id))
                else {
                    self.finish_split_three_operation();
                    return;
                };
                if split_three::PaneRect::from(&original_pane) != operation.original_rect
                    || !split_three::target_is_supported(&original_pane, operation.direction)
                {
                    self.finish_split_three_operation();
                    return;
                }

                operation.stage = split_three::OperationStage::FirstSplit;
                let direction = operation.direction;
                let operation_id = operation.id;
                let pane_index = operation.initial_terminal_pane_count + 1;
                self.dispatch_split_three(
                    operation,
                    split_three::new_pane_action(direction, operation_id, 1, pane_index),
                );
            }
            split_three::OperationStage::FirstSplit => {
                let Some(PaneId::Terminal(first_new_pane_id)) = affected_pane_id else {
                    operation.stage = split_three::OperationStage::RecoverFirstSplit;
                    self.wait_for_split_three_spawn(operation);
                    return;
                };
                if first_new_pane_id == operation.original_pane_id {
                    operation.stage = split_three::OperationStage::RecoverFirstSplit;
                    self.wait_for_split_three_spawn(operation);
                    return;
                }
                operation.first_new_pane_id = Some(first_new_pane_id);
                rename_terminal_pane(first_new_pane_id, "");
                if !split_three_focus_matches(operation.tab_id, first_new_pane_id) {
                    self.begin_split_three_rollback(operation);
                    return;
                }
                self.prepare_split_three_drag(operation);
            }
            split_three::OperationStage::FocusForDrag => {
                let Some(first_new_pane_id) = operation.first_new_pane_id else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                if !split_three_focus_matches(operation.tab_id, first_new_pane_id) {
                    self.begin_split_three_rollback(operation);
                    return;
                }
                let Some(original_pane) =
                    get_pane_info(PaneId::Terminal(operation.original_pane_id))
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                let Some(first_new_pane) = get_pane_info(PaneId::Terminal(first_new_pane_id))
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                let Some(drag) = split_three::plan_first_boundary_drag(
                    operation.original_rect,
                    &original_pane,
                    &first_new_pane,
                    operation.direction,
                ) else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                operation.drag = Some(drag);
                // Mark before dispatch: if completion is lost, a timeout will
                // still send a harmless release before closing any pane.
                operation.mouse_maybe_down = true;
                operation.stage = split_three::OperationStage::DragPress;
                self.dispatch_split_three(operation, split_three::drag_press_action(drag));
            }
            split_three::OperationStage::DragPress => {
                let (Some(first_new_pane_id), Some(_drag)) =
                    (operation.first_new_pane_id, operation.drag)
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                // Re-focus by pane ID after the press. This returns the client
                // to the intended tab before the release if focus changed
                // during the short asynchronous action sequence.
                operation.stage = split_three::OperationStage::FocusForRelease;
                self.dispatch_split_three(
                    operation,
                    split_three::focus_pane_action(first_new_pane_id),
                );
            }
            split_three::OperationStage::FocusForRelease => {
                let (Some(first_new_pane_id), Some(drag)) =
                    (operation.first_new_pane_id, operation.drag)
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                if !split_three_focus_matches(operation.tab_id, first_new_pane_id) {
                    self.begin_split_three_rollback(operation);
                    return;
                }
                // A release applies the outstanding cell delta even without a
                // separate motion event, and immediately clears Zellij's
                // mouse-resize state.
                operation.stage = split_three::OperationStage::DragRelease;
                self.dispatch_split_three(operation, split_three::drag_release_action(drag));
            }
            split_three::OperationStage::DragRelease => {
                operation.mouse_maybe_down = false;
                let (Some(first_new_pane_id), Some(drag)) =
                    (operation.first_new_pane_id, operation.drag)
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                if !split_three_focus_matches(operation.tab_id, first_new_pane_id) {
                    self.begin_split_three_rollback(operation);
                    return;
                }
                let Some(original_pane) =
                    get_pane_info(PaneId::Terminal(operation.original_pane_id))
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                let Some(first_new_pane) = get_pane_info(PaneId::Terminal(first_new_pane_id))
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                if !split_three::ready_for_second_split(
                    operation.original_rect,
                    &original_pane,
                    &first_new_pane,
                    operation.direction,
                    drag.first_span,
                ) {
                    self.begin_split_three_rollback(operation);
                    return;
                }

                operation.stage = split_three::OperationStage::FocusForSecondSplit;
                self.dispatch_split_three(
                    operation,
                    split_three::focus_pane_action(first_new_pane_id),
                );
            }
            split_three::OperationStage::FocusForSecondSplit => {
                let (Some(first_new_pane_id), Some(drag)) =
                    (operation.first_new_pane_id, operation.drag)
                else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                if !split_three_focus_matches(operation.tab_id, first_new_pane_id) {
                    self.begin_split_three_rollback(operation);
                    return;
                }
                let (Some(original_pane), Some(first_new_pane)) = (
                    get_pane_info(PaneId::Terminal(operation.original_pane_id)),
                    get_pane_info(PaneId::Terminal(first_new_pane_id)),
                ) else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                if !split_three::ready_for_second_split(
                    operation.original_rect,
                    &original_pane,
                    &first_new_pane,
                    operation.direction,
                    drag.first_span,
                ) {
                    self.begin_split_three_rollback(operation);
                    return;
                }

                operation.stage = split_three::OperationStage::SecondSplit;
                let direction = operation.direction;
                let operation_id = operation.id;
                let pane_index = operation.initial_terminal_pane_count + 2;
                self.dispatch_split_three(
                    operation,
                    split_three::new_pane_action(direction, operation_id, 2, pane_index),
                );
            }
            split_three::OperationStage::SecondSplit => {
                let Some(PaneId::Terminal(second_new_pane_id)) = affected_pane_id else {
                    operation.stage = split_three::OperationStage::RecoverSecondSplit;
                    self.wait_for_split_three_spawn(operation);
                    return;
                };
                let Some(first_new_pane_id) = operation.first_new_pane_id else {
                    self.begin_split_three_rollback(operation);
                    return;
                };
                if second_new_pane_id == operation.original_pane_id
                    || second_new_pane_id == first_new_pane_id
                {
                    operation.stage = split_three::OperationStage::RecoverSecondSplit;
                    self.wait_for_split_three_spawn(operation);
                    return;
                }
                operation.second_new_pane_id = Some(second_new_pane_id);
                if !split_three_focus_matches(operation.tab_id, second_new_pane_id) {
                    self.begin_split_three_rollback(operation);
                    return;
                }
                self.wait_for_split_three_validation(operation);
            }
            split_three::OperationStage::RollbackFocusForRelease => {
                let Some(drag) = operation.drag else {
                    operation.mouse_maybe_down = false;
                    self.continue_split_three_rollback(operation);
                    return;
                };
                // Advance even if focus did not land: a release must always
                // follow a dispatched press, and is harmless on another pane.
                operation.stage = split_three::OperationStage::RollbackRelease;
                self.dispatch_split_three(operation, split_three::drag_cancel_action(drag));
            }
            split_three::OperationStage::RollbackRelease => {
                operation.mouse_maybe_down = false;
                self.continue_split_three_rollback(operation);
            }
            split_three::OperationStage::RollbackSecond => {
                operation.second_new_pane_id = None;
                self.continue_split_three_rollback(operation);
            }
            split_three::OperationStage::RollbackFirst => {
                operation.first_new_pane_id = None;
                self.continue_split_three_rollback(operation);
            }
            split_three::OperationStage::RollbackFocus => {
                self.finish_split_three_operation();
            }
            split_three::OperationStage::RecoverFirstSplit
            | split_three::OperationStage::RecoverSecondSplit
            | split_three::OperationStage::ValidateFinal => {
                // These stages wait for PaneUpdate or the recovery timer and
                // do not dispatch actions that can complete normally.
                self.split_three_operation = Some(operation);
            }
        }
    }

    fn prepare_split_three_drag(&mut self, mut operation: split_three::Operation) {
        let Some(first_new_pane_id) = operation.first_new_pane_id else {
            self.begin_split_three_rollback(operation);
            return;
        };
        let Some(original_pane) = get_pane_info(PaneId::Terminal(operation.original_pane_id))
        else {
            self.begin_split_three_rollback(operation);
            return;
        };
        let Some(first_new_pane) = get_pane_info(PaneId::Terminal(first_new_pane_id)) else {
            self.begin_split_three_rollback(operation);
            return;
        };

        if let Some(drag) = split_three::plan_first_boundary_drag(
            operation.original_rect,
            &original_pane,
            &first_new_pane,
            operation.direction,
        ) {
            operation.drag = Some(drag);
            operation.stage = split_three::OperationStage::FocusForDrag;
            self.dispatch_split_three(operation, split_three::focus_pane_action(first_new_pane_id));
        } else {
            self.begin_split_three_rollback(operation);
        }
    }

    fn wait_for_split_three_spawn(&mut self, mut operation: split_three::Operation) {
        operation.recovery_attempts = 0;
        self.split_three_operation = Some(operation);
        self.split_three_action_started_ms = unix_now_ms();
        self.maybe_recover_pending_split_three_spawn();
    }

    fn wait_for_split_three_validation(&mut self, mut operation: split_three::Operation) {
        operation.stage = split_three::OperationStage::ValidateFinal;
        operation.recovery_attempts = 0;
        self.split_three_operation = Some(operation);
        self.split_three_action_started_ms = unix_now_ms();
        self.maybe_finish_split_three_validation();
    }

    fn maybe_finish_split_three_validation(&mut self) {
        let Some(operation) = self.split_three_operation.take() else {
            return;
        };
        if operation.stage != split_three::OperationStage::ValidateFinal {
            self.split_three_operation = Some(operation);
            return;
        }

        let (Some(first_new_pane_id), Some(second_new_pane_id)) =
            (operation.first_new_pane_id, operation.second_new_pane_id)
        else {
            self.begin_split_three_rollback(operation);
            return;
        };
        let geometry_status = self
            .pane_manifest
            .as_ref()
            .map(|manifest| {
                split_three::final_geometry_status_in_manifest(
                    manifest,
                    operation.original_rect,
                    operation.original_pane_id,
                    first_new_pane_id,
                    second_new_pane_id,
                    operation.direction,
                )
            })
            .unwrap_or(split_three::FinalGeometryStatus::StillSettling);
        match geometry_status {
            split_three::FinalGeometryStatus::Settled => {
                rename_terminal_pane(second_new_pane_id, "");
                self.finish_split_three_operation();
            }
            split_three::FinalGeometryStatus::StillSettling => {
                self.split_three_operation = Some(operation);
            }
            split_three::FinalGeometryStatus::Invalid => {
                eprintln!(
                    "Split Three rolled back invalid settled geometry in tab {}",
                    operation.tab_id
                );
                self.begin_split_three_rollback(operation);
            }
        }
    }

    fn maybe_recover_pending_split_three_spawn(&mut self) {
        let Some(mut operation) = self.split_three_operation.take() else {
            return;
        };
        let recovering_first = match operation.stage {
            split_three::OperationStage::RecoverFirstSplit => true,
            split_three::OperationStage::RecoverSecondSplit => false,
            _ => {
                self.split_three_operation = Some(operation);
                return;
            }
        };

        let ordinal = if recovering_first { 1 } else { 2 };
        let pane_index = operation.initial_terminal_pane_count + usize::from(ordinal);
        let expected_marker = split_three::pane_marker(operation.id, ordinal, pane_index);
        let mut candidates = self
            .pane_manifest
            .as_ref()
            .and_then(|manifest| {
                manifest.panes.values().find(|panes| {
                    panes
                        .iter()
                        .any(|pane| !pane.is_plugin && pane.id == operation.original_pane_id)
                })
            })
            .into_iter()
            .flatten()
            .filter(|pane| {
                !pane.is_plugin
                    && pane.title == expected_marker
                    && !operation.known_terminal_pane_ids.contains(&pane.id)
                    && Some(pane.id) != operation.first_new_pane_id
                    && Some(pane.id) != operation.second_new_pane_id
                    && split_three::pane_is_within(
                        operation.original_rect,
                        pane,
                        operation.direction,
                    )
            })
            .map(|pane| pane.id);
        let recovered_pane_id = candidates.next().filter(|_| candidates.next().is_none());

        if let Some(pane_id) = recovered_pane_id {
            if recovering_first {
                operation.first_new_pane_id = Some(pane_id);
                rename_terminal_pane(pane_id, "");
                self.begin_split_three_rollback(operation);
            } else {
                operation.second_new_pane_id = Some(pane_id);
                self.wait_for_split_three_validation(operation);
            }
        } else {
            self.split_three_operation = Some(operation);
        }
    }

    fn begin_split_three_rollback(&mut self, mut operation: split_three::Operation) {
        operation.recovery_attempts = 0;
        if operation.mouse_maybe_down {
            let focus_anchor = operation
                .first_new_pane_id
                .into_iter()
                .chain(std::iter::once(operation.original_pane_id))
                .chain(operation.second_new_pane_id)
                .find(|pane_id| get_pane_info(PaneId::Terminal(*pane_id)).is_some());
            if let Some(pane_id) = focus_anchor {
                operation.stage = split_three::OperationStage::RollbackFocusForRelease;
                self.dispatch_split_three(operation, split_three::focus_pane_action(pane_id));
                return;
            }
            if let Some(drag) = operation.drag {
                operation.stage = split_three::OperationStage::RollbackRelease;
                self.dispatch_split_three(operation, split_three::drag_cancel_action(drag));
                return;
            }
            operation.mouse_maybe_down = false;
        }
        self.continue_split_three_rollback(operation);
    }

    fn continue_split_three_rollback(&mut self, mut operation: split_three::Operation) {
        operation.recovery_attempts = 0;
        loop {
            if let Some(second_new_pane_id) = operation.second_new_pane_id {
                if get_pane_info(PaneId::Terminal(second_new_pane_id)).is_some() {
                    operation.stage = split_three::OperationStage::RollbackSecond;
                    self.dispatch_split_three(
                        operation,
                        split_three::close_pane_action(second_new_pane_id),
                    );
                    return;
                }
                operation.second_new_pane_id = None;
                continue;
            }
            if let Some(first_new_pane_id) = operation.first_new_pane_id {
                if get_pane_info(PaneId::Terminal(first_new_pane_id)).is_some() {
                    operation.stage = split_three::OperationStage::RollbackFirst;
                    self.dispatch_split_three(
                        operation,
                        split_three::close_pane_action(first_new_pane_id),
                    );
                    return;
                }
                operation.first_new_pane_id = None;
                continue;
            }
            if get_pane_info(PaneId::Terminal(operation.original_pane_id)).is_some() {
                operation.stage = split_three::OperationStage::RollbackFocus;
                let original_pane_id = operation.original_pane_id;
                self.dispatch_split_three(
                    operation,
                    split_three::focus_pane_action(original_pane_id),
                );
            } else {
                self.finish_split_three_operation();
            }
            return;
        }
    }

    fn finish_split_three_operation(&mut self) {
        self.split_three_operation = None;
        self.split_three_action_started_ms = 0;
    }

    fn recover_stalled_split_three(&mut self) {
        let Some(mut operation) = self.split_three_operation.take() else {
            return;
        };
        if unix_now_ms().saturating_sub(self.split_three_action_started_ms)
            < SPLIT_THREE_ACTION_TIMEOUT_MS
        {
            self.split_three_operation = Some(operation);
            return;
        }

        operation.recovery_attempts = operation.recovery_attempts.saturating_add(1);
        match operation.stage {
            split_three::OperationStage::FirstSplit => {
                operation.stage = split_three::OperationStage::RecoverFirstSplit;
                self.wait_for_split_three_spawn(operation);
            }
            split_three::OperationStage::SecondSplit => {
                operation.stage = split_three::OperationStage::RecoverSecondSplit;
                self.wait_for_split_three_spawn(operation);
            }
            split_three::OperationStage::RecoverFirstSplit => {
                if operation.recovery_attempts < 3 {
                    self.split_three_action_started_ms = unix_now_ms();
                    self.split_three_operation = Some(operation);
                } else {
                    self.finish_split_three_operation();
                }
            }
            split_three::OperationStage::RecoverSecondSplit => {
                if operation.recovery_attempts < 3 {
                    self.split_three_action_started_ms = unix_now_ms();
                    self.split_three_operation = Some(operation);
                } else {
                    operation.recovery_attempts = 0;
                    self.begin_split_three_rollback(operation);
                }
            }
            split_three::OperationStage::ValidateFinal => {
                eprintln!(
                    "Split Three rolled back geometry that did not settle in tab {}",
                    operation.tab_id
                );
                operation.recovery_attempts = 0;
                self.begin_split_three_rollback(operation);
            }
            split_three::OperationStage::RollbackFocusForRelease => {
                operation.recovery_attempts = 0;
                if let Some(drag) = operation.drag {
                    operation.stage = split_three::OperationStage::RollbackRelease;
                    self.dispatch_split_three(operation, split_three::drag_cancel_action(drag));
                } else {
                    operation.mouse_maybe_down = false;
                    self.continue_split_three_rollback(operation);
                }
            }
            split_three::OperationStage::RollbackRelease => {
                if operation.recovery_attempts < 2 {
                    let Some(drag) = operation.drag else {
                        operation.mouse_maybe_down = false;
                        operation.recovery_attempts = 0;
                        self.continue_split_three_rollback(operation);
                        return;
                    };
                    self.dispatch_split_three(operation, split_three::drag_cancel_action(drag));
                } else {
                    operation.mouse_maybe_down = false;
                    operation.recovery_attempts = 0;
                    self.continue_split_three_rollback(operation);
                }
            }
            split_three::OperationStage::RollbackSecond => {
                if operation.recovery_attempts < 2 {
                    let Some(pane_id) = operation.second_new_pane_id else {
                        operation.recovery_attempts = 0;
                        self.continue_split_three_rollback(operation);
                        return;
                    };
                    self.dispatch_split_three(operation, split_three::close_pane_action(pane_id));
                } else {
                    operation.second_new_pane_id = None;
                    operation.recovery_attempts = 0;
                    self.continue_split_three_rollback(operation);
                }
            }
            split_three::OperationStage::RollbackFirst => {
                if operation.recovery_attempts < 2 {
                    let Some(pane_id) = operation.first_new_pane_id else {
                        operation.recovery_attempts = 0;
                        self.continue_split_three_rollback(operation);
                        return;
                    };
                    self.dispatch_split_three(operation, split_three::close_pane_action(pane_id));
                } else {
                    operation.first_new_pane_id = None;
                    operation.recovery_attempts = 0;
                    self.continue_split_three_rollback(operation);
                }
            }
            split_three::OperationStage::RollbackFocus => {
                self.finish_split_three_operation();
            }
            _ => {
                operation.recovery_attempts = 0;
                self.begin_split_three_rollback(operation);
            }
        }
    }

    fn dispatch_split_three(&mut self, operation: split_three::Operation, action: Action) {
        let context = operation.context();
        self.split_three_operation = Some(operation);
        self.split_three_action_started_ms = unix_now_ms();
        run_action(action, context);
    }

    fn on_command_permissions_granted(&mut self) {
        let newly_granted = !self.command_permissions_granted;
        self.command_permissions_granted = true;
        self.permissions_denied = false;

        // Keep the plugin visible during fullscreen once application-state
        // changes are allowed.
        set_selectable(false);
        self.maybe_install_runtime_bindings();

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

    fn maybe_install_runtime_bindings(&mut self) {
        self.maybe_install_split_three_bindings();
        self.maybe_install_custom_layout_bindings();
    }

    fn maybe_install_split_three_bindings(&mut self) {
        if !self.command_permissions_granted {
            return;
        }
        if !self.split_three_instance_is_active() {
            // Each tab owns a plugin instance. Reset while inactive so this
            // instance retargets the client binding when its tab is revisited.
            self.split_three_bindings_installed = false;
            return;
        }
        if self.split_three_bindings_installed {
            return;
        }
        let plugin_id = self.plugin_id.unwrap_or_else(|| get_plugin_ids().plugin_id);
        let Some(keybinds) = self.initial_keybinds.as_ref() else {
            return;
        };

        // Mark first: reconfiguration emits another InitialKeybinds event.
        // Becoming inactive resets this flag so tab switches retarget it.
        self.split_three_bindings_installed = true;
        split_three::install(keybinds, plugin_id);
    }

    fn maybe_install_custom_layout_bindings(&mut self) {
        if !self.command_permissions_granted {
            return;
        }
        if !self.split_three_instance_is_active() {
            self.custom_layout_bindings_installed = false;
            return;
        }
        if self.custom_layout_bindings_installed {
            return;
        }
        let plugin_id = self.plugin_id.unwrap_or_else(|| get_plugin_ids().plugin_id);
        let Some(keybinds) = self.initial_keybinds.as_ref() else {
            return;
        };

        // Reconfiguration emits another InitialKeybinds snapshot. Mark first
        // and let a later tab activation reset this client-scoped target.
        self.custom_layout_bindings_installed = true;
        custom_layouts::install(keybinds, plugin_id);
    }

    fn maybe_capture_legacy_keybinds(&mut self, keybinds: KeybindsVec) {
        if !self.split_three_uses_legacy_keybinds {
            return;
        }

        // Zellij 0.44.0 includes the full snapshot in every ModeUpdate. Keep
        // refreshing it until permission arrives so late user rebinds win.
        self.initial_keybinds = Some(keybinds);
        self.maybe_install_runtime_bindings();
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
        let Some(session_name) = self.zellij_session_name.as_deref() else {
            return;
        };

        if attach::run(session_name, &self.pane_to_tab) {
            self.attach_scan_requested = true;
        }
    }

    /// Export this session's pane manifest to the runtime cache so external
    /// tools can map layout slots to pane ids, built from this instance's own
    /// pane and tab state. No permission gate: every trigger sits behind a
    /// subscribed event, and Zellij withholds those until the grant exists —
    /// while a cached grant authorizes each host call even before the
    /// PermissionRequestResult event arrives (that event needs an attached
    /// client, which a background session may never have).
    fn maybe_write_manifest(&mut self) {
        let Some(session_name) = self.reported_session_name.clone() else {
            return;
        };
        if self.tabs.is_empty() {
            return;
        }
        let Some(pane_manifest) = self.pane_manifest.as_ref() else {
            return;
        };
        let body = manifest::body(&session_name, &self.tabs, pane_manifest);
        self.dispatch_manifest(body);
    }

    /// Debounced to one write per second and skipped while the content is
    /// unchanged, which also keeps per-tab instances from churning over the
    /// one file: they all derive the same body from the same events. After a
    /// rename the previous name's manifest is removed; only this plugin ever
    /// learns a rename happened.
    fn dispatch_manifest(&mut self, body: manifest::ManifestBody) {
        if self.manifest_last_body.as_ref() == Some(&body) {
            self.manifest_pending_body = None;
            return;
        }
        let now = unix_now_ms();
        if now.saturating_sub(self.manifest_last_write_ms) < MANIFEST_DEBOUNCE_MS {
            self.manifest_pending_body = Some(body);
            return;
        }
        let payload = manifest::payload_json(&body, now);
        let previous_session_name = self
            .manifest_last_body
            .take()
            .map(|last| last.zellij_session)
            .filter(|last_name| *last_name != body.zellij_session);
        manifest::write(
            &payload,
            &body.zellij_session,
            previous_session_name.as_deref(),
        );
        self.manifest_last_body = Some(body);
        self.manifest_last_write_ms = now;
        self.manifest_pending_body = None;
    }

    /// Compile every template to `~/.config/zellij/layouts/`. Runs once per
    /// plugin instance, as soon as both the settings file and this plugin's own
    /// URL are known — the URL only becomes available once a pane manifest
    /// describing this pane arrives.
    fn maybe_compile_session_templates(&mut self) {
        if self.session_templates_compiled || !self.command_permissions_granted {
            return;
        }
        let Some(templates) = self.session_templates.clone() else {
            return;
        };
        let plugin_id = *self
            .plugin_id
            .get_or_insert_with(|| get_plugin_ids().plugin_id);
        let plugin_location = self
            .pane_manifest
            .as_ref()
            .into_iter()
            .flat_map(|manifest| manifest.panes.values())
            .flatten()
            .find(|pane| pane.is_plugin && pane.id == plugin_id)
            .and_then(|pane| pane.plugin_url.clone());
        let Some(plugin_location) = plugin_location else {
            return;
        };
        let home = std::env::var("HOME").unwrap_or_default();

        self.session_templates_compiled = true;
        if let Some(error) = self.session_template_config_error.as_deref() {
            eprintln!("Zellaude could not read session templates: {error}");
        }

        // Built from the templates, not from the compile results below, so a
        // template that fails to compile keeps whatever it generated last time.
        let keep = session_templates::keep_list(&templates);

        for template in &templates {
            let basename = session_templates::layout_basename(&template.name);
            let kdl = match template.to_kdl(&plugin_location, &self.plugin_configuration, &home) {
                Ok(kdl) => kdl,
                Err(error) => {
                    eprintln!(
                        "Zellaude could not compile session template {:?}: {error}",
                        template.name
                    );
                    continue;
                }
            };
            let mut ctx = BTreeMap::new();
            ctx.insert("type".into(), "write_layout".into());
            ctx.insert("layout".into(), basename.clone());
            run_command(
                &[
                    "sh",
                    "-c",
                    session_templates::WRITE_LAYOUT_SCRIPT,
                    "zellaude-write-layout",
                    &basename,
                    &kdl,
                ],
                ctx,
            );
        }

        let mut ctx = BTreeMap::new();
        ctx.insert("type".into(), "prune_layouts".into());
        run_command(
            &[
                "sh",
                "-c",
                session_templates::PRUNE_LAYOUTS_SCRIPT,
                "zellaude-prune-layouts",
                &keep,
            ],
            ctx,
        );
    }

    /// Only the instance whose tab is visible should spend host calls on
    /// discovery.
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
        // Merge the small settings object into the existing file on the host.
        // Custom states can legitimately contain far more data than a single
        // OS argument permits, and plugin-block states are higher-precedence
        // runtime input that must never replace states owned by this file.
        let json = self.serialized_settings();
        let mut ctx = BTreeMap::new();
        ctx.insert("type".into(), "save_config".into());
        run_command(
            &[
                "sh",
                "-c",
                SAVE_CONFIG_SCRIPT,
                "zellaude-save-config",
                &json,
            ],
            ctx,
        );
    }

    fn serialized_settings(&self) -> String {
        serde_json::to_string(&self.settings).unwrap_or_default()
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

    fn run_reload_custom_states_script(home: &std::path::Path) -> std::process::Output {
        std::process::Command::new("sh")
            .args([
                "-c",
                RELOAD_CUSTOM_STATES_SCRIPT,
                "zellaude-reload-custom-states-test",
            ])
            .env("HOME", home)
            .output()
            .unwrap()
    }

    fn temp_home(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "zellaude-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(home.join(".config/zellij/plugins/zellaude/generators")).unwrap();
        home
    }

    fn generator_sources(
        settings_json: &str,
        generator: &str,
    ) -> layout_generators::CustomStateSources {
        layout_generators::CustomStateSources {
            settings_json: settings_json.to_string(),
            generator_files: vec![layout_generators::GeneratorFile {
                path: "/generators/g.kdl".to_string(),
                content: generator.to_string(),
            }],
        }
    }

    fn run_save_config_script(
        config_path: &std::path::Path,
        settings_json: &str,
    ) -> std::process::Output {
        std::process::Command::new("sh")
            .args([
                "-c",
                SAVE_CONFIG_SCRIPT,
                "zellaude-save-config-test",
                settings_json,
            ])
            .arg(config_path)
            .output()
            .unwrap()
    }

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
    fn settings_save_payload_does_not_copy_effective_custom_states() {
        let layout = custom_layouts::CustomLayout {
            id: "claude6".to_string(),
            width: Some(1),
            height: Some(1),
            commands: custom_layouts::CommandGrid::Flat(vec!["claude".to_string()]),
        };
        let mut state = State::default();
        state.custom_layouts.insert(layout.id.clone(), layout);

        let document: serde_json::Value =
            serde_json::from_str(&state.serialized_settings()).unwrap();

        assert!(document.get("custom_states").is_none());
        assert!(document.get("notifications").is_some());
    }

    #[test]
    fn settings_save_merges_large_file_owned_states_atomically() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let test_dir = std::env::temp_dir().join(format!(
            "zellaude-save-config-{}-{unique}",
            std::process::id()
        ));
        let config_path = test_dir.join("nested/zellaude.json");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();

        let file_layout = custom_layouts::CustomLayout {
            id: "large-file-state".to_string(),
            width: Some(3),
            height: Some(1),
            commands: custom_layouts::CommandGrid::Flat(vec!["x".repeat(50_000); 3]),
        };
        std::fs::write(&config_path, serde_json::to_vec(&file_layout).unwrap()).unwrap();
        let state = State::default();
        let settings_json = state.serialized_settings();
        let output = run_save_config_script(&config_path, &settings_json);
        assert!(
            output.status.success(),
            "save failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(saved["custom_states"][0]["id"], "large-file-state");
        assert_eq!(
            saved["custom_states"][0]["commands"][2]
                .as_str()
                .unwrap()
                .len(),
            50_000
        );
        assert!(saved.get("notifications").is_some());

        let wrapped = serde_json::json!({
            "unrelated": {"preserve": true},
            "custom_states": [file_layout],
        });
        std::fs::write(&config_path, serde_json::to_vec(&wrapped).unwrap()).unwrap();
        assert!(run_save_config_script(&config_path, &settings_json)
            .status
            .success());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(saved["unrelated"]["preserve"], true);
        assert_eq!(saved["custom_states"][0]["id"], "large-file-state");

        let array_document = serde_json::json!([{
            "id": "array-state",
            "width": 1,
            "height": 1,
            "commands": ["true"]
        }]);
        std::fs::write(
            &config_path,
            serde_json::to_vec(&array_document).unwrap(),
        )
        .unwrap();
        assert!(run_save_config_script(&config_path, &settings_json)
            .status
            .success());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(saved["custom_states"][0]["id"], "array-state");

        std::fs::remove_file(&config_path).unwrap();
        assert!(run_save_config_script(&config_path, &settings_json)
            .status
            .success());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert!(saved.get("notifications").is_some());

        #[cfg(unix)]
        {
            let real_path = test_dir.join("real/zellaude.json");
            let link_path = test_dir.join("linked/zellaude.json");
            std::fs::create_dir_all(real_path.parent().unwrap()).unwrap();
            std::fs::create_dir_all(link_path.parent().unwrap()).unwrap();
            std::fs::write(&real_path, serde_json::to_vec(&wrapped).unwrap()).unwrap();
            std::os::unix::fs::symlink("../real/zellaude.json", &link_path).unwrap();

            assert!(run_save_config_script(&link_path, &settings_json)
                .status
                .success());
            assert!(std::fs::symlink_metadata(&link_path)
                .unwrap()
                .file_type()
                .is_symlink());
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&real_path).unwrap()).unwrap();
            assert_eq!(saved["unrelated"]["preserve"], true);
            assert!(saved.get("notifications").is_some());
        }

        for invalid in [
            b"{ this is not json".as_slice(),
            b" \n\t".as_slice(),
            b"{}\n{}".as_slice(),
        ] {
            std::fs::write(&config_path, invalid).unwrap();
            assert!(!run_save_config_script(&config_path, &settings_json)
                .status
                .success());
            assert_eq!(std::fs::read(&config_path).unwrap(), invalid);
        }

        std::fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn custom_state_reload_reads_zellaude_json_and_every_generator_file() {
        let home = temp_home("reload-custom-states");
        let generators = home.join(".config/zellij/plugins/zellaude/generators");
        // "B" before "a" is byte order; a collating locale inverts it, so this
        // pair proves LC_ALL=C rather than mere determinism.
        std::fs::write(generators.join("B.kdl"), "command \"B\"\n").unwrap();
        std::fs::write(generators.join("a.kdl"), "command \"a\"\n").unwrap();
        std::fs::write(generators.join("notes.txt"), "not a generator").unwrap();
        std::fs::create_dir(generators.join("nested.kdl")).unwrap();

        let output = run_reload_custom_states_script(&home);
        assert!(
            output.status.success(),
            "reload failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let sources: layout_generators::CustomStateSources =
            serde_json::from_slice(&output.stdout).unwrap();

        assert_eq!(sources.settings_json, "{}");
        assert_eq!(
            sources
                .generator_files
                .iter()
                .map(|file| file.content.as_str())
                .collect::<Vec<_>>(),
            vec!["command \"B\"\n", "command \"a\"\n"]
        );

        let config_path = home.join(".config/zellij/plugins/zellaude.json");
        std::fs::write(&config_path, r#"{"min_pane_width": 100}"#).unwrap();
        let sources: layout_generators::CustomStateSources =
            serde_json::from_slice(&run_reload_custom_states_script(&home).stdout).unwrap();
        assert_eq!(sources.settings_json, r#"{"min_pane_width": 100}"#);
        assert_eq!(sources.generator_files.len(), 2);

        // Neither document may travel in argv: one argument caps at 128 KiB,
        // and a custom-state file legitimately runs past that.
        let bulk = "x".repeat(200_000);
        let big_generator = format!("command \"a\"\n// {bulk}\n");
        let big_settings = format!("{{\"pad\": \"{bulk}\"}}");
        std::fs::write(generators.join("a.kdl"), &big_generator).unwrap();
        std::fs::write(&config_path, &big_settings).unwrap();
        let output = run_reload_custom_states_script(&home);
        assert!(
            output.status.success(),
            "reload failed on large documents: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let sources: layout_generators::CustomStateSources =
            serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(sources.settings_json, big_settings);
        let restored = sources
            .generator_files
            .iter()
            .find(|file| file.path.ends_with("a.kdl"))
            .expect("the large generator file must survive the round trip");
        assert_eq!(restored.content, big_generator);

        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A short envelope is worse than no envelope: `apply_custom_state_sources`
    /// takes a successful read as the complete current state, so a truncated
    /// one deletes working generators instead of keeping the last good set.
    #[cfg(unix)]
    #[test]
    fn custom_state_reload_refuses_rather_than_truncating_on_an_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;

        let home = temp_home("reload-unreadable");
        let generators = home.join(".config/zellij/plugins/zellaude/generators");
        for name in ["a.kdl", "b.kdl", "c.kdl"] {
            std::fs::write(generators.join(name), format!("command \"{name}\"\n")).unwrap();
        }
        let blocked = generators.join("b.kdl");
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&blocked).is_ok() {
            std::fs::remove_dir_all(&home).unwrap(); // running as root; the mode proves nothing
            return;
        }

        let output = run_reload_custom_states_script(&home);
        assert!(
            !output.status.success(),
            "an unreadable file must refuse the whole envelope"
        );
        assert!(
            serde_json::from_slice::<layout_generators::CustomStateSources>(&output.stdout)
                .is_err(),
            "a failed read must not print an envelope at all"
        );

        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn custom_state_reload_survives_many_modest_generator_files() {
        // The accumulation, not any single file, is what overflowed argv: the
        // whole envelope was passed back in on every iteration, so forty 4 KB
        // files broke it with nothing large in sight. A per-file test cannot
        // see that -- after the rewrite each file goes to the pipe alone.
        let home = temp_home("reload-many-generators");
        let generators = home.join(".config/zellij/plugins/zellaude/generators");
        let expected: Vec<String> = (0..40)
            .map(|index| format!("command \"m{index:02}\"\n// {}\n", "x".repeat(4096)))
            .collect();
        for (index, content) in expected.iter().enumerate() {
            std::fs::write(generators.join(format!("m{index:02}.kdl")), content).unwrap();
        }

        let output = run_reload_custom_states_script(&home);
        assert!(
            output.status.success(),
            "reload failed on 40 modest files: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let sources: layout_generators::CustomStateSources =
            serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            sources
                .generator_files
                .iter()
                .map(|file| file.content.clone())
                .collect::<Vec<_>>(),
            expected
        );

        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_reload_applies_what_parsed_and_keeps_the_last_good_copy_of_what_did_not() {
        let generator = "command \"g\"\narg \"n\"\ntab {\n    pane \"claude\"\n}\n";
        let configured =
            r#"{"custom_states":[{"id":"a","width":1,"height":1,"commands":["true"]}]}"#;
        let mut state = State::default();

        state.apply_custom_state_sources(&generator_sources(configured, generator));
        assert!(state.custom_layouts.contains_key("a"));
        assert_eq!(state.layout_generators.len(), 1);
        assert_eq!(state.pane_floor_overrides, Default::default());

        // The key is gone from the file, so the state goes with it.
        state
            .apply_custom_state_sources(&generator_sources(r#"{"min_pane_width": 80}"#, generator));
        assert!(state.custom_layouts.is_empty());
        assert!(state.custom_layout_config_error.is_none());
        assert_eq!(state.pane_floor_overrides.min_pane_width, Some(80));

        // A document that does not parse leaves the last good states alone.
        state.apply_custom_state_sources(&generator_sources(configured, generator));
        state.apply_custom_state_sources(&generator_sources("{ broken", generator));
        assert!(state.custom_layouts.contains_key("a"));
        assert!(state.custom_layout_config_error.is_some());

        // So does a generator file that does not parse.
        state.apply_custom_state_sources(&generator_sources(configured, "command \"g\"\nbogus"));
        assert_eq!(state.layout_generators.len(), 1);
        assert!(state.layout_generator_config_error.is_some());
        state.apply_custom_state_sources(&generator_sources(configured, generator));
        assert!(state.layout_generator_config_error.is_none());

        // States from the plugin block ignore the file entirely, so a document
        // without custom_states must not clear them.
        state.custom_layouts_from_plugin_configuration = true;
        state.apply_custom_state_sources(&generator_sources("{}", generator));
        assert!(state.custom_layouts.contains_key("a"));
    }

    #[test]
    fn a_refused_input_resolves_on_the_next_enter_once_the_file_is_fixed() {
        // The same floors the E2E stages, so both tell one story: a 200x49 tab
        // fits one column of three panes at 14 rows each.
        let floors = r#"{"min_pane_width": 100, "min_pane_height": 5}"#;
        let generator = |extra: &str| {
            format!(
                "command \"hot\"\narg \"n\"\n{extra}\ntab \"{{tab}}-hot\" {{\n    \
                 each for=\"i\" in=\"1..=n\" {{\n        pane \"printf hot-p{{i}}\"\n    }}\n}}\n"
            )
        };
        let source = layout_generators::SourceTab {
            name: "e2e".to_string(),
            geometry: layout_generators::TabGeometry {
                columns: 200,
                rows: 49,
            },
        };
        let mut state = State::default();

        state.apply_custom_state_sources(&generator_sources(
            floors,
            &generator("min_pane_height 40\n"),
        ));
        let refusal = state.resolve_custom_state("hot 3", &source).unwrap_err();
        assert!(
            refusal.contains("does not fit") && refusal.contains("g.kdl"),
            "want a prefixed floor refusal, got {refusal:?}"
        );

        // The user edits the file with the prompt still open; the next Enter
        // reloads first, so the same input now resolves.
        state.apply_custom_state_sources(&generator_sources(floors, &generator("")));
        let tabs = state.resolve_custom_state("hot 3", &source).unwrap();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, "e2e-hot");
        assert_eq!(
            tabs[0].commands,
            custom_layouts::CommandGrid::Rows(vec![
                vec!["printf hot-p1".to_string()],
                vec!["printf hot-p2".to_string()],
                vec!["printf hot-p3".to_string()],
            ])
        );
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
    fn split_three_install_waits_for_permission_and_keybind_snapshot() {
        let mut state = active_split_three_state();
        state.initial_keybinds = Some(vec![]);

        state.maybe_install_split_three_bindings();
        assert!(!state.split_three_bindings_installed);

        state.command_permissions_granted = true;
        state.maybe_install_split_three_bindings();
        assert!(state.split_three_bindings_installed);
    }

    #[test]
    fn split_three_accepts_an_empty_legacy_mode_update_snapshot() {
        let mut state = active_split_three_state();
        state.command_permissions_granted = true;
        state.split_three_uses_legacy_keybinds = true;

        state.maybe_capture_legacy_keybinds(vec![]);

        assert_eq!(state.initial_keybinds, Some(vec![]));
        assert!(state.split_three_bindings_installed);
    }

    #[test]
    fn split_three_refreshes_legacy_keybinds_until_permission_arrives() {
        let mut state = active_split_three_state();
        state.split_three_uses_legacy_keybinds = true;
        state.maybe_capture_legacy_keybinds(vec![]);

        let custom_right: KeybindsVec = vec![(
            InputMode::Pane,
            vec![(
                KeyWithModifier::new(BareKey::Char(split_three::SPLIT_THREE_RIGHT_KEY))
                    .with_shift_modifier(),
                vec![],
            )],
        )];
        state.maybe_capture_legacy_keybinds(custom_right.clone());

        assert_eq!(state.initial_keybinds, Some(custom_right));
        assert!(!state.split_three_bindings_installed);
        assert_eq!(
            split_three::available_bindings(state.initial_keybinds.as_ref().unwrap(), 42).len(),
            1
        );

        state.command_permissions_granted = true;
        state.maybe_install_split_three_bindings();
        assert!(state.split_three_bindings_installed);
    }

    fn active_split_three_state() -> State {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            0,
            vec![PaneInfo {
                id: 42,
                is_plugin: true,
                ..PaneInfo::default()
            }],
        );
        State {
            plugin_id: Some(42),
            pane_manifest: Some(manifest),
            tabs: vec![TabInfo {
                position: 0,
                active: true,
                ..TabInfo::default()
            }],
            ..State::default()
        }
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
