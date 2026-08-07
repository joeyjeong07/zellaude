use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};
use zellij_tile::prelude::*;

use crate::{custom_layouts, session_templates, split_three};

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub const FLASH_DURATION_MS: u64 = 2000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Activity {
    Init,
    Thinking,
    Tool(String),
    Prompting,
    Waiting,
    Notification,
    Done,
    AgentDone,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub pane_id: u32,
    pub activity: Activity,
    pub tab_name: Option<String>,
    pub tab_index: Option<usize>,
    pub last_event_ts: u64,
    pub cwd: Option<String>,
    #[serde(default)]
    pub last_ts_ms: u64,
    #[serde(default)]
    pub rainbow_name: bool,
    #[serde(default = "default_rainbow_name_known")]
    pub rainbow_name_known: bool,
    /// Timestamp of the observation that set `rainbow_name`. This is kept
    /// separate from activity recency so attach recovery can correct stale
    /// mode state without making an idle session look newly active.
    #[serde(default)]
    pub rainbow_mode_ts_ms: u64,
    #[serde(default)]
    pub rainbow_mode_marker: Option<String>,
    /// Attach-time recovery is provisional and must never outrank state
    /// observed from a real hook or synchronized from a live peer.
    #[serde(default)]
    pub restored: bool,
    /// Derived locally from pane introspection rather than observed from an
    /// agent. An empty `session_id` cannot stand in for this: hook payloads
    /// legitimately omit the id, and those sessions are real.
    #[serde(default)]
    pub placeholder: bool,
}

fn default_rainbow_name_known() -> bool {
    // Older instances used false for both a detected standard mode and an
    // unknown mode. Keep rendering the stored bool, but let exact discovery
    // enrich it instead of treating that ambiguity as authoritative.
    false
}

#[derive(Debug, Deserialize)]
pub struct HookPayload {
    pub session_id: Option<String>,
    pub pane_id: u32,
    pub hook_event: String,
    pub tool_name: Option<String>,
    pub cwd: Option<String>,
    pub zellij_session: Option<String>,
    pub term_program: Option<String>,
    pub ts_ms: Option<u64>,
    #[serde(default)]
    pub is_subagent: bool,
    #[serde(default)]
    pub rainbow_name: Option<bool>,
    #[serde(default)]
    pub rainbow_mode_ts_ms: Option<u64>,
    #[serde(default)]
    pub rainbow_mode_marker: Option<String>,
}

pub struct ClickRegion {
    pub start_col: usize,
    pub end_col: usize,
    pub tab_index: usize,
    pub focus_pane_id: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum NotifyMode {
    Never,
    Unfocused,
    #[default]
    Always,
}

impl NotifyMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Always => Self::Unfocused,
            Self::Unfocused => Self::Never,
            Self::Never => Self::Always,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum FlashMode {
    Off,
    #[default]
    Once,
    Persist,
}

impl FlashMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Once => Self::Persist,
            Self::Persist => Self::Off,
            Self::Off => Self::Once,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub notifications: NotifyMode,
    pub flash: FlashMode,
    pub elapsed_time: bool,
    pub mode_indicator: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            notifications: NotifyMode::Always,
            flash: FlashMode::Once,
            elapsed_time: true,
            mode_indicator: true,
        }
    }
}

#[derive(Default, PartialEq)]
pub enum ViewMode {
    #[default]
    Normal,
    Settings,
}

#[derive(Clone, Copy)]
pub enum SettingKey {
    Notifications,
    Flash,
    ElapsedTime,
    ModeIndicator,
}

pub enum MenuAction {
    ToggleSetting(SettingKey),
    CloseMenu,
}

pub struct MenuClickRegion {
    pub start_col: usize,
    pub end_col: usize,
    pub action: MenuAction,
}

#[derive(Default)]
pub struct State {
    pub sessions: BTreeMap<u32, SessionInfo>,
    pub session_end_tombstones: BTreeMap<(u32, String), u64>,
    pub pane_to_tab: HashMap<u32, (usize, String)>,
    pub tabs: Vec<TabInfo>,
    pub pane_manifest: Option<PaneManifest>,
    pub zellij_styling: Option<Styling>,
    pub active_tab_index: Option<usize>,
    pub click_regions: Vec<ClickRegion>,
    /// pane_id -> flash deadline in ms (for waiting animation)
    pub flash_deadlines: HashMap<u32, u64>,
    pub zellij_session_name: Option<String>,
    pub term_program: Option<String>,
    pub input_mode: InputMode,
    pub settings: Settings,
    pub view_mode: ViewMode,
    pub prefix_click_region: Option<(usize, usize)>,
    pub menu_click_regions: Vec<MenuClickRegion>,
    pub config_loaded: bool,
    pub command_permissions_granted: bool,
    /// The user dismissed or refused Zellij's permission prompt. Zellij draws
    /// that prompt itself and does not offer it again, so without surfacing this
    /// the plugin is left permanently inert and indistinguishable from a working
    /// one — no hooks installed, no config, no explanation.
    pub permissions_denied: bool,
    pub split_three_bindings_installed: bool,
    pub custom_layout_bindings_installed: bool,
    pub split_three_uses_legacy_keybinds: bool,
    pub initial_keybinds: Option<KeybindsVec>,
    pub split_three_operation: Option<split_three::Operation>,
    pub split_three_next_operation_id: u64,
    pub split_three_action_started_ms: u64,
    pub custom_layouts: BTreeMap<String, custom_layouts::CustomLayout>,
    pub custom_layouts_from_plugin_configuration: bool,
    pub custom_layout_config_error: Option<String>,
    pub custom_layout_prompt: Option<custom_layouts::Prompt>,
    /// `None` until the settings file has been read; `Some` afterwards, even
    /// when the key is absent, so the built-in still compiles.
    pub session_templates: Option<Vec<session_templates::SessionTemplate>>,
    pub session_templates_compiled: bool,
    pub session_template_config_error: Option<String>,
    pub hooks_installed: bool,
    pub attach_scan_requested: bool,
    pub last_agent_poll_ms: u64,
    pub pane_introspection_supported: Option<bool>,
    pub plugin_id: Option<u32>,
    pub plugin_configuration: BTreeMap<String, String>,
    /// pane_id -> when its agent session last ended. Kept apart from
    /// `session_end_tombstones`, which also records merely blocked events.
    pub pane_session_ended_ms: HashMap<u32, u64>,
    /// Where the next bounded introspection sweep resumes.
    pub agent_poll_cursor: u32,
}
