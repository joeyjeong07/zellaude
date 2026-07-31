#![allow(dead_code)]

mod state {
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

    #[derive(Debug, Clone)]
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
    }
}

#[path = "../src/session_selection.rs"]
mod session_selection;

use state::{Activity, SessionInfo};

fn session(session_id: &str, activity: Activity, last_ts_ms: u64) -> SessionInfo {
    SessionInfo {
        session_id: session_id.to_string(),
        pane_id: 7,
        activity,
        tab_name: Some("tab".to_string()),
        tab_index: Some(0),
        last_event_ts: last_ts_ms / 1000,
        cwd: None,
        last_ts_ms,
        rainbow_name: false,
        rainbow_name_known: true,
        rainbow_mode_ts_ms: last_ts_ms,
        rainbow_mode_marker: None,
        restored: false,
    }
}

#[test]
fn restored_state_loses_to_newer_live_peer_state() {
    let mut restored = session("restored-owner", Activity::Idle, 10);
    restored.restored = true;
    let live_peer = session("live-owner", Activity::Thinking, 20);

    assert!(session_selection::is_newer_than(&live_peer, &restored));
    assert!(!session_selection::is_newer_than(&restored, &live_peer));
}

#[test]
fn equal_timestamp_restore_does_not_replace_live_peer_state() {
    let live_peer = session("live-owner", Activity::Thinking, 20);
    let mut restored = session("restored-owner", Activity::Idle, 20);
    restored.restored = true;

    assert!(!session_selection::is_newer_than(&restored, &live_peer));
}
