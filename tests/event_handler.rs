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
        pub rainbow_name_known: bool,
        pub rainbow_mode_ts_ms: u64,
        pub rainbow_mode_marker: Option<String>,
        pub restored: bool,
        pub placeholder: bool,
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
        pub rainbow_mode_ts_ms: Option<u64>,
        pub rainbow_mode_marker: Option<String>,
    }

    pub struct State {
        pub sessions: BTreeMap<u32, SessionInfo>,
        pub session_end_tombstones: BTreeMap<(u32, String), u64>,
        pub pane_session_ended_ms: HashMap<u32, u64>,
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
                session_end_tombstones: BTreeMap::new(),
                pane_session_ended_ms: HashMap::new(),
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

#[path = "../src/placeholder.rs"]
mod placeholder;

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
        rainbow_mode_ts_ms: rainbow_name.map(|_| ts_ms),
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

#[test]
fn newer_attach_discovery_corrects_mode_without_overwriting_live_activity() {
    let mut state = State::default();
    event_handler::handle_hook_event(
        &mut state,
        payload("root-known", Some(true), None, 20),
    );

    let mut same_session = payload("root-known", Some(false), None, 30);
    same_session.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, same_session);

    let session = state.sessions.get(&7).unwrap();
    assert!(!session.rainbow_name);
    assert!(session.rainbow_name_known);
    assert_eq!(session.activity, Activity::Thinking);
    assert_eq!(session.last_ts_ms, 20);
    assert_eq!(session.rainbow_mode_ts_ms, 30);

    let mut late_mode = payload("root-known", Some(true), None, 25);
    late_mode.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, late_mode);
    assert!(
        !state.sessions.get(&7).unwrap().rainbow_name,
        "an older discovery must not undo a newer exact mode observation"
    );

    let late_hook = payload("root-known", Some(true), None, 25);
    event_handler::handle_hook_event(&mut state, late_hook);
    assert!(
        !state.sessions.get(&7).unwrap().rainbow_name,
        "an older hook must not undo a newer exact mode observation"
    );

    let mut older_owner = payload("stale-owner", Some(false), None, 10);
    older_owner.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, older_owner);
    assert_eq!(state.sessions.get(&7).unwrap().session_id, "root-known");

    let mut newer_owner = payload("new-owner", Some(false), None, 40);
    newer_owner.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, newer_owner);
    let session = state.sessions.get(&7).unwrap();
    assert_eq!(session.session_id, "new-owner");
    assert!(!session.rainbow_name);
    assert!(session.rainbow_name_known);
    assert_eq!(session.activity, Activity::Idle);
    assert!(session.restored);
}

#[test]
fn attach_discovery_can_fill_an_unknown_mode_without_changing_activity() {
    let mut state = State::default();
    event_handler::handle_hook_event(
        &mut state,
        payload("root-unknown", None, None, 20),
    );

    let mut discovered = payload("root-unknown", Some(true), Some("ultra"), 10);
    discovered.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, discovered);

    let session = state.sessions.get(&7).unwrap();
    assert!(session.rainbow_name);
    assert!(session.rainbow_name_known);
    assert_eq!(session.rainbow_mode_marker.as_deref(), Some("ultra"));
    assert_eq!(session.activity, Activity::Thinking);
    assert_eq!(session.last_ts_ms, 20);
}

#[test]
fn cached_mode_keeps_its_original_observation_time_during_attach() {
    let mut state = State::default();
    event_handler::handle_hook_event(
        &mut state,
        payload("root-known", Some(true), None, 20),
    );

    let mut cached = payload("root-known", Some(false), None, 100);
    cached.hook_event = "SessionRestore".to_string();
    cached.rainbow_mode_ts_ms = Some(10);
    event_handler::handle_discovered_session(&mut state, cached);

    let session = state.sessions.get(&7).unwrap();
    assert!(session.rainbow_name);
    assert_eq!(session.rainbow_mode_ts_ms, 20);
    assert_eq!(session.last_ts_ms, 20);
}

#[test]
fn a_scan_started_before_session_end_cannot_resurrect_the_closed_owner() {
    let mut state = State::default();
    event_handler::handle_hook_event(
        &mut state,
        payload("closing-owner", Some(true), None, 20),
    );

    let mut ended = payload("closing-owner", None, None, 30);
    ended.hook_event = "SessionEnd".to_string();
    event_handler::handle_hook_event(&mut state, ended);
    assert!(state.sessions.is_empty());

    let mut late_scan = payload("closing-owner", Some(true), None, 10);
    late_scan.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, late_scan);

    assert!(
        state.sessions.is_empty(),
        "an attach result older than SessionEnd must not recreate the closed session"
    );
}

#[test]
fn live_reused_pane_state_wins_over_a_late_different_owner_restore() {
    let mut state = State::default();

    let mut restored = payload("restored-owner", Some(true), None, 10);
    restored.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, restored);
    assert_eq!(state.sessions.get(&7).unwrap().activity, Activity::Idle);
    assert!(state.sessions.get(&7).unwrap().restored);

    let mut child = payload("", None, None, 15);
    child.hook_event = "PreToolUse".to_string();
    child.tool_name = Some("Read".to_string());
    child.is_subagent = true;
    event_handler::handle_hook_event(&mut state, child);
    assert!(
        state.sessions.get(&7).unwrap().restored,
        "a child event cannot validate provisional root ownership"
    );
    assert_eq!(
        state.sessions.get(&7).unwrap().last_ts_ms,
        10,
        "a child event cannot change provisional root recency"
    );

    event_handler::handle_hook_event(
        &mut state,
        payload("live-owner", Some(false), None, 20),
    );

    let mut late_restore = payload("restored-owner", Some(true), None, 10);
    late_restore.hook_event = "SessionRestore".to_string();
    event_handler::handle_discovered_session(&mut state, late_restore);

    let session = state.sessions.get(&7).unwrap();
    assert_eq!(session.session_id, "live-owner");
    assert!(!session.rainbow_name);
    assert!(!session.restored);
    assert_eq!(session.activity, Activity::Thinking);
    assert_eq!(session.last_ts_ms, 20);
}

#[test]
fn rejected_newer_event_does_not_clear_an_ended_owner_tombstone() {
    let mut state = State::default();
    state
        .session_end_tombstones
        .insert((7, "ended-owner".to_string()), 30);
    event_handler::handle_hook_event(
        &mut state,
        payload("current-owner", Some(false), None, 100),
    );

    event_handler::handle_hook_event(
        &mut state,
        payload("ended-owner", Some(true), None, 40),
    );
    assert_eq!(state.sessions.get(&7).unwrap().session_id, "current-owner");
    assert_eq!(
        state
            .session_end_tombstones
            .get(&(7, "ended-owner".to_string())),
        Some(&40)
    );
}


#[test]
fn subagent_events_cannot_claim_a_placeholder_pane() {
    let mut state = State::default();
    state.sessions.insert(7, placeholder::placeholder_session(7));

    let mut subagent = payload("child", None, None, 1_000);
    subagent.hook_event = "PermissionRequest".to_string();
    subagent.is_subagent = true;
    event_handler::handle_hook_event(&mut state, subagent);

    let session = state.sessions.get(&7).unwrap();
    assert_eq!(session.activity, Activity::Idle);
    assert!(state.flash_deadlines.is_empty());
}

#[test]
fn a_hook_event_without_a_session_id_creates_a_real_session() {
    let mut state = State::default();

    event_handler::handle_hook_event(&mut state, payload("", None, None, 1_000));

    assert!(!state.sessions.get(&7).unwrap().placeholder);
}

#[test]
fn a_stale_session_end_cannot_retire_a_running_agents_placeholder() {
    let mut state = State::default();
    state.sessions.insert(7, placeholder::placeholder_session(7));

    let mut ended = payload("previous-agent", None, None, 2_000);
    ended.hook_event = "SessionEnd".to_string();
    event_handler::handle_hook_event(&mut state, ended);

    assert!(state.sessions.contains_key(&7));
}

#[test]
fn a_session_end_without_an_id_still_records_when_the_pane_ended() {
    let mut state = State::default();

    let mut ended = payload("", None, None, 2_000);
    ended.hook_event = "SessionEnd".to_string();
    event_handler::handle_hook_event(&mut state, ended);

    assert_eq!(state.pane_session_ended_ms.get(&7), Some(&2_000));
}

#[test]
fn a_late_event_cannot_resurrect_a_pane_whose_end_carried_no_session_id() {
    // A hook script that predates the session_id field — or one whose jq
    // extraction came back empty — sends SessionEnd with no id. Both the
    // tombstone write and the tombstone lookup were gated on a non-empty id, so
    // the empty-id path recorded nothing and consulted nothing: a tool event
    // that raced the end recreated the pane as a *real* session, and the
    // introspection poll only ever retires placeholders, so it stayed on the bar
    // for the life of the session.
    let mut state = State::default();
    event_handler::handle_hook_event(&mut state, payload("s1", None, None, 100));
    assert!(state.sessions.contains_key(&7));

    let mut end = payload("s1", None, None, 200);
    end.hook_event = "SessionEnd".to_string();
    end.session_id = None;
    event_handler::handle_hook_event(&mut state, end);
    assert!(!state.sessions.contains_key(&7), "the end should evict it");

    // A tool event from before the end, delivered afterwards.
    let mut late = payload("s1", None, None, 150);
    late.hook_event = "PostToolUse".to_string();
    late.session_id = None;
    event_handler::handle_hook_event(&mut state, late);

    assert!(
        !state.sessions.contains_key(&7),
        "a pre-end event resurrected the pane"
    );
}
