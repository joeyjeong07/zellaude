#![allow(dead_code)]

extern crate self as zellij_tile;

pub mod prelude {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PaletteColor {
        Rgb((u8, u8, u8)),
        EightBit(u8),
    }

    impl Default for PaletteColor {
        fn default() -> Self {
            Self::EightBit(0)
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct StyleDeclaration {
        pub base: PaletteColor,
        pub background: PaletteColor,
        pub emphasis_0: PaletteColor,
        pub emphasis_1: PaletteColor,
        pub emphasis_2: PaletteColor,
        pub emphasis_3: PaletteColor,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Styling {
        pub text_unselected: StyleDeclaration,
        pub text_selected: StyleDeclaration,
        pub ribbon_unselected: StyleDeclaration,
        pub ribbon_selected: StyleDeclaration,
        pub frame_selected: StyleDeclaration,
        pub frame_highlight: StyleDeclaration,
        pub exit_code_success: StyleDeclaration,
        pub exit_code_error: StyleDeclaration,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum InputMode {
        #[default]
        Normal,
        Locked,
        Pane,
        Tab,
        Resize,
        Move,
        Scroll,
        EnterSearch,
        Search,
        RenameTab,
        RenamePane,
        Session,
        Prompt,
        Tmux,
    }

    #[derive(Clone, Debug)]
    pub struct TabInfo {
        pub position: usize,
        pub name: String,
        pub active: bool,
        pub is_fullscreen_active: bool,
    }
}

#[path = "../src/rainbow.rs"]
mod rainbow;

#[path = "../src/theme.rs"]
mod theme;

#[path = "../src/tool_symbol.rs"]
mod tool_symbol;

mod state {
    use crate::prelude::{InputMode, Styling, TabInfo};
    use std::collections::{BTreeMap, HashMap};

    pub fn unix_now() -> u64 {
        1
    }

    pub fn unix_now_ms() -> u64 {
        1_000
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
    pub enum NotifyMode {
        Never,
        Unfocused,
        Always,
    }

    #[derive(Clone, Copy)]
    pub enum FlashMode {
        Off,
        Once,
        Persist,
    }

    pub struct Settings {
        pub notifications: NotifyMode,
        pub flash: FlashMode,
        pub elapsed_time: bool,
        pub mode_indicator: bool,
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

    pub struct ClickRegion {
        pub start_col: usize,
        pub end_col: usize,
        pub tab_index: usize,
        pub focus_pane_id: Option<u32>,
    }

    pub struct MenuClickRegion {
        pub start_col: usize,
        pub end_col: usize,
        pub action: MenuAction,
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
        pub rainbow_mode_marker: Option<String>,
    }

    pub struct State {
        pub sessions: BTreeMap<u32, SessionInfo>,
        pub tabs: Vec<TabInfo>,
        pub zellij_styling: Option<Styling>,
        pub click_regions: Vec<ClickRegion>,
        pub flash_deadlines: HashMap<u32, u64>,
        pub zellij_session_name: Option<String>,
        pub input_mode: InputMode,
        pub settings: Settings,
        pub view_mode: ViewMode,
        pub prefix_click_region: Option<(usize, usize)>,
        pub menu_click_regions: Vec<MenuClickRegion>,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                sessions: BTreeMap::new(),
                tabs: Vec::new(),
                zellij_styling: None,
                click_regions: Vec::new(),
                flash_deadlines: HashMap::new(),
                zellij_session_name: None,
                input_mode: InputMode::Normal,
                settings: Settings {
                    notifications: NotifyMode::Always,
                    flash: FlashMode::Once,
                    elapsed_time: true,
                    mode_indicator: true,
                },
                view_mode: ViewMode::Normal,
                prefix_click_region: None,
                menu_click_regions: Vec::new(),
            }
        }
    }
}

#[path = "../src/session_selection.rs"]
mod session_selection;

#[path = "../src/render.rs"]
mod render;

use prelude::{PaletteColor, StyleDeclaration, Styling, TabInfo};
use state::{Activity, SessionInfo, State, ViewMode};

fn rgb(r: u8, g: u8, b: u8) -> PaletteColor {
    PaletteColor::Rgb((r, g, b))
}

fn declaration(colors: [PaletteColor; 6]) -> StyleDeclaration {
    StyleDeclaration {
        base: colors[0],
        background: colors[1],
        emphasis_0: colors[2],
        emphasis_1: colors[3],
        emphasis_2: colors[4],
        emphasis_3: colors[5],
    }
}

fn gruvbox_dark() -> Styling {
    let text_unselected = declaration([
        rgb(251, 241, 199),
        rgb(60, 56, 54),
        rgb(214, 93, 14),
        rgb(104, 157, 106),
        rgb(152, 151, 26),
        rgb(177, 98, 134),
    ]);

    Styling {
        text_unselected,
        text_selected: StyleDeclaration {
            background: rgb(80, 73, 69),
            ..text_unselected
        },
        ribbon_unselected: declaration([
            rgb(60, 56, 54),
            rgb(235, 219, 178),
            rgb(204, 36, 29),
            rgb(251, 241, 199),
            rgb(69, 133, 136),
            rgb(177, 98, 134),
        ]),
        ribbon_selected: declaration([
            rgb(60, 56, 54),
            rgb(152, 151, 26),
            rgb(204, 36, 29),
            rgb(214, 93, 14),
            rgb(177, 98, 134),
            rgb(69, 133, 136),
        ]),
        frame_selected: declaration([
            rgb(152, 151, 26),
            PaletteColor::EightBit(0),
            rgb(214, 93, 14),
            rgb(104, 157, 106),
            rgb(177, 98, 134),
            PaletteColor::EightBit(0),
        ]),
        frame_highlight: declaration([
            rgb(214, 93, 14),
            PaletteColor::EightBit(0),
            rgb(177, 98, 134),
            rgb(214, 93, 14),
            rgb(214, 93, 14),
            rgb(214, 93, 14),
        ]),
        exit_code_success: declaration([
            rgb(152, 151, 26),
            PaletteColor::EightBit(0),
            rgb(104, 157, 106),
            rgb(60, 56, 54),
            rgb(177, 98, 134),
            rgb(69, 133, 136),
        ]),
        exit_code_error: declaration([
            rgb(204, 36, 29),
            PaletteColor::EightBit(0),
            rgb(215, 153, 33),
            PaletteColor::EightBit(0),
            PaletteColor::EightBit(0),
            PaletteColor::EightBit(0),
        ]),
    }
}

fn tab(position: usize, name: &str, active: bool) -> TabInfo {
    TabInfo {
        position,
        name: name.to_string(),
        active,
        is_fullscreen_active: false,
    }
}

#[test]
fn normal_bar_uses_gruvbox_prefix_mode_tab_and_surface_colors() {
    let mut state = State {
        zellij_styling: Some(gruvbox_dark()),
        tabs: vec![tab(0, "active", true), tab(1, "idle", false)],
        ..State::default()
    };

    let output = render::build_status_bar(&mut state, 1, 100);

    assert!(output.contains("\x1b[48;2;235;219;178m\x1b[38;2;60;56;54m\x1b[1m Zellaude "));
    assert!(output.contains("\x1b[48;2;152;151;26m\x1b[38;2;60;56;54m\x1b[1m NORMAL "));
    assert!(output.contains("\x1b[48;2;80;73;69m \x1b[1m\x1b[38;2;251;241;199mactive"));
    assert!(output.contains("\x1b[48;2;60;56;54m \x1b[38;2;251;241;199midle"));
    assert!(output.contains("\x1b[48;2;60;56;54m"));
}

#[test]
fn replacing_live_styling_changes_the_next_render() {
    let mut state = State {
        zellij_styling: Some(gruvbox_dark()),
        tabs: vec![tab(0, "tab", true)],
        ..State::default()
    };
    let before = render::build_status_bar(&mut state, 1, 80);

    let mut replacement = gruvbox_dark();
    replacement.ribbon_unselected.background = rgb(1, 2, 3);
    state.zellij_styling = Some(replacement);
    let after = render::build_status_bar(&mut state, 1, 80);

    assert!(before.contains("\x1b[48;2;235;219;178m"));
    assert!(!after.contains("\x1b[48;2;235;219;178m"));
    assert!(after.contains("\x1b[48;2;1;2;3m"));
}

#[test]
fn rainbow_letters_are_contrast_checked_against_the_gruvbox_tab_pair() {
    let mut state = State {
        zellij_styling: Some(gruvbox_dark()),
        tabs: vec![tab(0, "agent", true)],
        ..State::default()
    };
    state.sessions.insert(
        7,
        SessionInfo {
            session_id: "session".to_string(),
            pane_id: 7,
            activity: Activity::Thinking,
            tab_name: Some("agent".to_string()),
            tab_index: Some(0),
            last_event_ts: 1,
            cwd: None,
            last_ts_ms: 1_000,
            rainbow_name: true,
            rainbow_mode_marker: Some("ultra".to_string()),
        },
    );

    let output = render::build_status_bar(&mut state, 1, 80);
    let color = rainbow::ensure_contrast(
        rainbow::rainbow_rgb(1_000, 0, 0, false),
        (80, 73, 69),
        (251, 241, 199),
    );
    let expected = format!("\x1b[38;2;{};{};{}ma", color.0, color.1, color.2);

    assert!(rainbow::contrast_ratio(color, (80, 73, 69)) >= 4.5);
    assert!(output.contains(&expected));
}

#[test]
fn permission_flash_uses_gruvbox_error_background_and_preserves_rainbow() {
    let mut state = State {
        zellij_styling: Some(gruvbox_dark()),
        tabs: vec![tab(0, "agent", true)],
        ..State::default()
    };
    state.sessions.insert(
        7,
        SessionInfo {
            session_id: "session".to_string(),
            pane_id: 7,
            activity: Activity::Waiting,
            tab_name: Some("agent".to_string()),
            tab_index: Some(0),
            last_event_ts: 1,
            cwd: None,
            last_ts_ms: 1_000,
            rainbow_name: true,
            rainbow_mode_marker: Some("ultra".to_string()),
        },
    );
    state.flash_deadlines.insert(7, 2_000);

    let output = render::build_status_bar(&mut state, 1, 80);
    let color = rainbow::ensure_contrast(
        rainbow::rainbow_rgb(1_000, 0, 0, false),
        (204, 36, 29),
        (251, 241, 199),
    );
    let mut expected = String::new();
    rainbow::write_rainbow(
        &mut expected,
        "agent",
        1_000,
        0,
        false,
        Some((204, 36, 29)),
        Some((251, 241, 199)),
    )
    .unwrap();

    assert!(output.contains("\x1b[48;2;204;36;29m"));
    assert!(output.contains("\x1b[48;2;204;36;29m \x1b[38;2;251;241;199m⚠"));
    assert!(output.contains(&expected));
    assert!(rainbow::contrast_ratio(color, (204, 36, 29)) >= 4.5);
}

#[test]
fn settings_surface_uses_gruvbox_text_and_accent_colors() {
    let mut state = State {
        zellij_styling: Some(gruvbox_dark()),
        view_mode: ViewMode::Settings,
        ..State::default()
    };

    let output = render::build_status_bar(&mut state, 1, 120);

    assert!(output.contains("\x1b[48;2;152;151;26m\x1b[38;2;60;56;54m\x1b[1m Zellaude "));
    assert!(output.contains("\x1b[48;2;60;56;54m"));
    assert!(output.contains("\x1b[38;2;104;157;106m●"));
    assert!(output.contains("\x1b[38;2;214;93;14m◐"));
    assert!(output.contains("\x1b[38;2;204;36;29m×"));
}
