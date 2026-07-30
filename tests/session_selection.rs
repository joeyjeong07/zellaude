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
    }
}

#[path = "../src/session_selection.rs"]
mod session_selection;
