#![allow(dead_code)]

mod state {
    use std::collections::{BTreeMap, HashMap};

    pub const FLASH_DURATION_MS: u64 = 2000;

    pub fn unix_now() -> u64 {
        1
    }

    pub fn unix_now_ms() -> u64 {
        1000
    }

    #[derive(Debug, Clone, PartialEq)]
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

    #[derive(Clone, Copy)]
    pub enum FlashMode {
        Off,
        Once,
        Persist,
    }

    pub struct Settings {
        pub flash: FlashMode,
    }

    #[derive(Debug)]
    pub struct SessionInfo {
        pub session_id: String,
        pub pane_id: u32,
        pub activity: Activity,
        pub tab_name: Option<String>,
        pub tab_index: Option<usize>,
        pub last_event_ts: u64,
        pub cwd: Option<String>,
        pub last_ts_ms: u64,
        pub rainbow_name: bool,
        pub rainbow_mode_marker: Option<String>,
    }

    pub struct HookPayload {
        pub session_id: Option<String>,
        pub pane_id: u32,
        pub hook_event: String,
        pub tool_name: Option<String>,
        pub cwd: Option<String>,
        pub zellij_session: Option<String>,
        pub term_program: Option<String>,
        pub ts_ms: Option<u64>,
        pub is_subagent: bool,
        pub rainbow_name: Option<bool>,
        pub rainbow_mode_marker: Option<String>,
    }

    pub struct State {
        pub sessions: BTreeMap<u32, SessionInfo>,
        pub pane_to_tab: HashMap<u32, (usize, String)>,
        pub flash_deadlines: HashMap<u32, u64>,
        pub zellij_session_name: Option<String>,
        pub term_program: Option<String>,
        pub settings: Settings,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                sessions: BTreeMap::new(),
                pane_to_tab: HashMap::new(),
                flash_deadlines: HashMap::new(),
                zellij_session_name: None,
                term_program: None,
                settings: Settings {
                    flash: FlashMode::Off,
                },
            }
        }
    }
}

#[path = "../src/event_handler.rs"]
mod event_handler;

use state::{Activity, HookPayload, State};

fn payload(
    session_id: &str,
    rainbow_name: Option<bool>,
    marker: Option<&str>,
    ts_ms: u64,
) -> HookPayload {
    HookPayload {
        session_id: Some(session_id.to_string()),
        pane_id: 7,
        hook_event: "UserPromptSubmit".to_string(),
        tool_name: None,
        cwd: None,
        zellij_session: None,
        term_program: None,
        ts_ms: Some(ts_ms),
        is_subagent: false,
        rainbow_name,
        rainbow_mode_marker: marker.map(str::to_string),
    }
}

#[test]
fn transcript_markers_apply_once_and_direct_signals_can_supersede_them() {
    let mut state = State::default();

    event_handler::handle_hook_event(
        &mut state,
        payload("session-a", Some(true), Some("old-command"), 1),
    );
    let session = state.sessions.get(&7).unwrap();
    assert!(session.rainbow_name);
    assert_eq!(session.rainbow_mode_marker.as_deref(), Some("old-command"));

    // A launch override can carry the historical command as its baseline.
    // Replaying that same transcript command must not undo the override.
    event_handler::handle_hook_event(
        &mut state,
        payload("session-a", Some(false), Some("old-command"), 2),
    );
    assert!(state.sessions.get(&7).unwrap().rainbow_name);

    event_handler::handle_hook_event(
        &mut state,
        payload("session-a", Some(false), Some("new-command"), 3),
    );
    let session = state.sessions.get(&7).unwrap();
    assert!(!session.rainbow_name);
    assert_eq!(session.rainbow_mode_marker.as_deref(), Some("new-command"));

    // A direct/current signal supersedes transcript state but retains its
    // baseline marker so stale history cannot replay on the next hook.
    event_handler::handle_hook_event(&mut state, payload("session-a", Some(true), None, 4));
    let session = state.sessions.get(&7).unwrap();
    assert!(session.rainbow_name);
    assert_eq!(session.rainbow_mode_marker.as_deref(), Some("new-command"));

    event_handler::handle_hook_event(
        &mut state,
        payload("session-a", Some(false), Some("new-command"), 5),
    );
    assert!(state.sessions.get(&7).unwrap().rainbow_name);

    event_handler::handle_hook_event(
        &mut state,
        payload("session-a", Some(false), Some("later-command"), 6),
    );
    assert!(!state.sessions.get(&7).unwrap().rainbow_name);
}

#[test]
fn a_new_session_never_inherits_the_previous_pane_highlight() {
    let mut state = State::default();
    event_handler::handle_hook_event(
        &mut state,
        payload("session-a", Some(true), Some("ultra-command"), 1),
    );

    event_handler::handle_hook_event(&mut state, payload("session-b", None, None, 2));

    let session = state.sessions.get(&7).unwrap();
    assert_eq!(session.session_id, "session-b");
    assert!(!session.rainbow_name);
    assert_eq!(session.rainbow_mode_marker, None);
}

#[test]
fn session_start_resets_state_even_when_a_resumed_session_reuses_its_id() {
    let mut state = State::default();
    event_handler::handle_hook_event(
        &mut state,
        payload("session-a", Some(false), Some("historical-command"), 1),
    );

    let mut resumed = payload("session-a", Some(true), Some("historical-command"), 2);
    resumed.hook_event = "SessionStart".to_string();
    event_handler::handle_hook_event(&mut state, resumed);

    assert!(state.sessions.get(&7).unwrap().rainbow_name);
}

#[test]
fn child_agents_never_replace_the_root_session_or_its_rainbow_mode() {
    let mut state = State::default();

    let mut early_child = payload("child-before-sync", Some(false), None, 1);
    early_child.hook_event = "PreToolUse".to_string();
    early_child.is_subagent = true;
    event_handler::handle_hook_event(&mut state, early_child);
    assert!(state.sessions.is_empty());

    event_handler::handle_hook_event(
        &mut state,
        payload("root-ultra", Some(true), Some("ultra"), 2),
    );

    let mut child_tool = payload("child-xhigh", Some(false), None, 3);
    child_tool.hook_event = "PreToolUse".to_string();
    child_tool.tool_name = Some("Read".to_string());
    child_tool.is_subagent = true;
    event_handler::handle_hook_event(&mut state, child_tool);

    let session = state.sessions.get(&7).unwrap();
    assert_eq!(session.session_id, "root-ultra");
    assert!(session.rainbow_name);
    assert_eq!(session.activity, Activity::Tool("Read".to_string()));

    let mut child_stop = payload("child-xhigh", None, None, 4);
    child_stop.hook_event = "SubagentStop".to_string();
    child_stop.is_subagent = true;
    event_handler::handle_hook_event(&mut state, child_stop);

    let session = state.sessions.get(&7).unwrap();
    assert_eq!(session.session_id, "root-ultra");
    assert!(session.rainbow_name);
    assert_eq!(session.activity, Activity::AgentDone);

    event_handler::handle_hook_event(&mut state, payload("root-ultra", Some(false), None, 5));
    assert!(!state.sessions.get(&7).unwrap().rainbow_name);
}
