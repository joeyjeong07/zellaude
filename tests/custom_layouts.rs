#![allow(dead_code)]

#[path = "../src/custom_layouts.rs"]
mod custom_layouts;

use custom_layouts::{CustomLayout, Prompt, PromptKey};
use std::collections::BTreeMap;
use std::path::PathBuf;
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;
use zellij_utils::input::config::Config;
use zellij_utils::input::layout::{Layout, Run, SplitDirection, SplitSize};
use zellij_utils::pane_size::PaneGeom;

const TEST_PLUGIN_ID: u32 = 42;
const EXAMPLE: &str = r#"{
    "id": "claude6",
    "width": "3",
    "height": "2",
    "commands": ["A1", "A2", "A3", "A4", "A5", "A6"]
}"#;

#[test]
fn example_json_and_supported_configuration_wrappers_are_accepted() {
    let direct = custom_layouts::parse_config_document(EXAMPLE)
        .unwrap()
        .unwrap();
    assert_eq!(direct, vec![example_layout()]);

    let wrapped = format!(r#"{{"notifications":"Always","custom_states":[{EXAMPLE}]}}"#);
    assert_eq!(
        custom_layouts::parse_config_document(&wrapped)
            .unwrap()
            .unwrap(),
        vec![example_layout()]
    );

    let plugin_configuration = BTreeMap::from([("custom_state".to_string(), EXAMPLE.to_string())]);
    assert_eq!(
        custom_layouts::parse_plugin_configuration(&plugin_configuration)
            .unwrap()
            .unwrap(),
        vec![example_layout()]
    );

    let numeric = EXAMPLE.replace(r#""3""#, "3").replace(r#""2""#, "2");
    assert_eq!(
        custom_layouts::parse_config_document(&numeric)
            .unwrap()
            .unwrap(),
        vec![example_layout()]
    );
}

#[test]
fn validation_rejects_invalid_ids_dimensions_and_command_counts() {
    let invalid = [
        (
            CustomLayout {
                id: String::new(),
                width: 1,
                height: 1,
                commands: vec!["A1".to_string()],
            },
            "must not be empty",
        ),
        (
            CustomLayout {
                id: " padded".to_string(),
                width: 1,
                height: 1,
                commands: vec!["A1".to_string()],
            },
            "must not start or end with whitespace",
        ),
        (
            CustomLayout {
                id: "bad\nid".to_string(),
                width: 1,
                height: 1,
                commands: vec!["A1".to_string()],
            },
            "must not contain control characters",
        ),
        (
            CustomLayout {
                id: "zero".to_string(),
                width: 0,
                height: 1,
                commands: vec![],
            },
            "non-zero width and height",
        ),
        (
            CustomLayout {
                id: "overflow".to_string(),
                width: usize::MAX,
                height: 2,
                commands: vec![],
            },
            "dimensions that are too large",
        ),
        (
            CustomLayout {
                id: "too-many".to_string(),
                width: 9,
                height: 8,
                commands: vec!["x".to_string(); 72],
            },
            "the maximum is 64",
        ),
        (
            CustomLayout {
                id: "empty".to_string(),
                width: 3,
                height: 2,
                commands: vec![],
            },
            "must contain at least one command",
        ),
        (
            CustomLayout {
                id: "too-many-commands".to_string(),
                width: 3,
                height: 2,
                commands: vec!["x".to_string(); 7],
            },
            "has room for 6 commands, but has 7",
        ),
        (
            CustomLayout {
                id: "command-too-large".to_string(),
                width: 1,
                height: 1,
                commands: vec!["x".repeat(custom_layouts::MAX_COMMAND_BYTES + 1)],
            },
            "exceeds 65536 bytes",
        ),
        (
            CustomLayout {
                id: "nul-command".to_string(),
                width: 1,
                height: 1,
                commands: vec!["before\0after".to_string()],
            },
            "contains a NUL byte",
        ),
        (
            CustomLayout {
                id: "commands-too-large".to_string(),
                width: 17,
                height: 1,
                commands: vec!["x".repeat(custom_layouts::MAX_COMMAND_BYTES); 17],
            },
            "exceed 1048576 bytes in total",
        ),
    ];

    for (layout, expected_error) in invalid {
        let error = layout.validate().unwrap_err();
        assert!(
            error.contains(expected_error),
            "expected {error:?} to contain {expected_error:?}"
        );
    }

    let long_id = CustomLayout {
        id: "x".repeat(custom_layouts::MAX_ID_CHARACTERS + 1),
        width: 1,
        height: 1,
        commands: vec!["A1".to_string()],
    };
    assert!(long_id.validate().unwrap_err().contains("exceeds 128"));
}

#[test]
fn configuration_rejects_bad_dimensions_and_duplicate_ids() {
    let bad_dimension = EXAMPLE.replace(r#""3""#, r#""wide""#);
    assert!(custom_layouts::parse_config_document(&bad_dimension).is_err());

    let duplicate = format!("[{EXAMPLE},{EXAMPLE}]");
    assert!(custom_layouts::parse_config_document(&duplicate)
        .unwrap_err()
        .contains("duplicate custom state id"));

    assert!(
        custom_layouts::parse_config_document(r#"{"custom_states":"claude6"}"#)
            .unwrap_err()
            .contains("must be a state object or an array")
    );
}

#[test]
fn an_inline_configuration_key_is_authoritative_even_when_empty_or_invalid() {
    let absent = BTreeMap::from([("unrelated".to_string(), "value".to_string())]);
    assert!(!custom_layouts::has_plugin_configuration(&absent));
    assert_eq!(
        custom_layouts::parse_plugin_configuration(&absent).unwrap(),
        None
    );

    let empty = BTreeMap::from([("custom_states".to_string(), "[]".to_string())]);
    assert!(custom_layouts::has_plugin_configuration(&empty));
    assert_eq!(
        custom_layouts::parse_plugin_configuration(&empty).unwrap(),
        Some(vec![])
    );

    let invalid = BTreeMap::from([("custom_state".to_string(), "not json".to_string())]);
    assert!(custom_layouts::has_plugin_configuration(&invalid));
    assert!(custom_layouts::parse_plugin_configuration(&invalid).is_err());
}

#[test]
fn generated_three_by_two_layout_has_reading_order_geometry_and_startup_order() {
    let plugin_configuration = plugin_configuration();
    let kdl = example_layout()
        .to_kdl(
            "file:/tmp/zellaude.wasm",
            &plugin_configuration,
            Some("/work/tree"),
        )
        .unwrap();
    let parsed = Layout::from_kdl(&kdl, Some("generated.kdl".to_string()), None, None).unwrap();
    assert_eq!(parsed.tabs.len(), 1);
    let (tab_name, tiled, floating) = &parsed.tabs[0];
    assert_eq!(tab_name.as_deref(), Some("claude6"));
    assert!(floating.is_empty());

    assert_eq!(tiled.pane_count(), 7);
    assert_eq!(tiled.children_split_direction, SplitDirection::Horizontal);
    assert_eq!(tiled.children.len(), 2);
    let bar = &tiled.children[0];
    assert_eq!(bar.split_size, Some(SplitSize::Fixed(1)));
    assert_eq!(bar.borderless, Some(true));
    let Some(Run::Plugin(plugin)) = &bar.run else {
        panic!("the first row must run the Zellaude plugin");
    };
    assert_eq!(plugin.location_string(), "file:/tmp/zellaude.wasm");
    assert_eq!(
        plugin.get_configuration().unwrap().inner(),
        &plugin_configuration
    );

    let grid = &tiled.children[1];
    assert_eq!(grid.children_split_direction, SplitDirection::Vertical);
    assert_eq!(grid.children.len(), 3);
    assert!(grid.children.iter().all(|column| {
        column.children_split_direction == SplitDirection::Horizontal && column.children.len() == 2
    }));
    assert_eq!(grid.children[0].children[0].focus, Some(true));
    assert!(grid
        .children
        .iter()
        .flat_map(|column| column.children.iter())
        .skip(1)
        .all(|pane| pane.focus != Some(true)));

    let startup_order: Vec<String> = tiled
        .extract_run_instructions()
        .iter()
        .filter_map(maybe_shell_command)
        .collect();
    assert_eq!(startup_order, command_names());

    let mut space = PaneGeom::default();
    space.cols.set_inner(90);
    space.rows.set_inner(60);
    let positioned = tiled
        .position_panes_in_space(&space, None, false, false)
        .unwrap();
    assert!(matches!(positioned[0].0.run, Some(Run::Plugin(_))));
    assert_eq!(positioned[0].1.x, 0);
    assert_eq!(positioned[0].1.y, 0);
    assert_eq!(positioned[0].1.cols.as_usize(), 90);
    assert_eq!(positioned[0].1.rows.as_usize(), 1);
    let actual: Vec<(String, usize, usize, usize, usize)> = positioned
        .iter()
        .filter_map(|(pane, geometry)| {
            maybe_shell_command(&pane.run).map(|command| {
                (
                    command,
                    geometry.x,
                    geometry.y,
                    geometry.cols.as_usize(),
                    geometry.rows.as_usize(),
                )
            })
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            ("A1".to_string(), 0, 1, 30, 30),
            ("A2".to_string(), 30, 1, 30, 30),
            ("A3".to_string(), 60, 1, 30, 30),
            ("A4".to_string(), 0, 31, 30, 29),
            ("A5".to_string(), 30, 31, 30, 29),
            ("A6".to_string(), 60, 31, 30, 29),
        ]
    );

    for run in tiled.extract_run_instructions() {
        let Some(Run::Command(command)) = run else {
            continue;
        };
        assert_eq!(command.command, PathBuf::from("sh"));
        assert_eq!(command.args.first().map(String::as_str), Some("-lc"));
        assert_eq!(command.cwd, Some(PathBuf::from("/work/tree")));
    }
}

#[test]
fn one_dimensional_and_maximum_sized_grids_generate_valid_layouts() {
    for (width, height) in [(1, 1), (1, 4), (4, 1), (8, 8)] {
        let pane_count = width * height;
        let commands: Vec<String> = (0..pane_count)
            .map(|index| format!("command-{index}"))
            .collect();
        let layout = CustomLayout {
            id: format!("grid-{width}x{height}"),
            width,
            height,
            commands: commands.clone(),
        };
        let kdl = layout
            .to_kdl("file:/tmp/zellaude.wasm", &BTreeMap::new(), None)
            .unwrap();
        let parsed = Layout::from_kdl(&kdl, Some("boundary.kdl".to_string()), None, None)
            .unwrap_or_else(|error| panic!("{width}x{height} did not parse: {error}"));
        let tiled = &parsed.tabs[0].1;

        assert_eq!(tiled.pane_count(), pane_count + 1);
        let mut space = PaneGeom::default();
        space.cols.set_inner(160);
        space.rows.set_inner(100);
        let mut visual_order: Vec<(usize, usize, String)> = tiled
            .position_panes_in_space(&space, None, false, false)
            .unwrap()
            .into_iter()
            .filter_map(|(pane, geometry)| {
                maybe_shell_command(&pane.run).map(|command| (geometry.y, geometry.x, command))
            })
            .collect();
        visual_order.sort_by_key(|(y, x, _)| (*y, *x));
        assert_eq!(
            visual_order
                .into_iter()
                .map(|(_, _, command)| command)
                .collect::<Vec<_>>(),
            commands
        );
    }
}

#[test]
fn a_partially_filled_grid_keeps_blank_cells_at_the_visual_bottom_right() {
    let commands: Vec<String> = (1..=7).map(|index| format!("A{index}")).collect();
    let layout = CustomLayout {
        id: "claude7".to_string(),
        width: 4,
        height: 2,
        commands: commands.clone(),
    };
    let kdl = layout
        .to_kdl("file:/tmp/zellaude.wasm", &BTreeMap::new(), None)
        .unwrap();
    let parsed = Layout::from_kdl(&kdl, Some("partial.kdl".to_string()), None, None).unwrap();
    let tiled = &parsed.tabs[0].1;

    assert_eq!(tiled.pane_count(), 9);
    assert_eq!(
        tiled
            .extract_run_instructions()
            .iter()
            .filter_map(maybe_shell_command)
            .collect::<Vec<_>>(),
        commands
    );

    let mut space = PaneGeom::default();
    space.cols.set_inner(80);
    space.rows.set_inner(41);
    let positioned = tiled
        .position_panes_in_space(&space, None, false, false)
        .unwrap();
    let blank = positioned
        .iter()
        .find(|(pane, _)| !matches!(pane.run, Some(Run::Plugin(_) | Run::Command(_))))
        .expect("the eighth grid cell should be an ordinary shell pane");
    assert_eq!(blank.1.x, 60);
    assert_eq!(blank.1.y, 21);
    assert_eq!(blank.1.cols.as_usize(), 20);
    assert_eq!(blank.1.rows.as_usize(), 20);
}

#[test]
fn generated_kdl_round_trips_quotes_newlines_backslashes_and_unicode_controls() {
    let original = "quote \" slash \\ newline\nnext \u{1} end";
    let layout = CustomLayout {
        id: "escaped".to_string(),
        width: 1,
        height: 1,
        commands: vec![original.to_string()],
    };

    let plugin_configuration = BTreeMap::from([
        (
            "custom_states".to_string(),
            "[\n  {\"id\":\"quoted\"}\n]".to_string(),
        ),
        ("quoted\"key".to_string(), "slash\\line\nnext".to_string()),
    ]);
    let kdl = layout
        .to_kdl("file:/tmp/zellaude.wasm", &plugin_configuration, None)
        .unwrap();
    assert!(kdl.contains(r#"\u{1}"#));
    assert!(!kdl.contains(r#"\u0001"#));

    let parsed = Layout::from_kdl(&kdl, Some("escaped.kdl".to_string()), None, None).unwrap();
    let tiled = &parsed.tabs[0].1;
    let Some(Run::Plugin(plugin)) = &tiled.children[0].run else {
        panic!("the first row must run the Zellaude plugin");
    };
    assert_eq!(
        plugin.get_configuration().unwrap().inner(),
        &plugin_configuration
    );
    assert_eq!(
        tiled
            .extract_run_instructions()
            .iter()
            .filter_map(maybe_shell_command)
            .collect::<Vec<_>>(),
        vec![original.to_string()]
    );

    assert!(layout.to_kdl("", &plugin_configuration, None).is_err());
}

#[test]
fn runtime_reconfigure_preserves_the_targeted_tab_binding() {
    let desired = custom_layouts::bindings(TEST_PLUGIN_ID);
    let parsed = Config::from_kdl(
        &custom_layouts::reconfigure_snippet(&desired, TEST_PLUGIN_ID),
        None,
    )
    .unwrap()
    .keybinds
    .to_keybinds_vec();
    let tab_bindings = parsed
        .iter()
        .find(|(mode, _)| mode == &InputMode::Tab)
        .map(|(_, bindings)| bindings)
        .unwrap();

    assert_eq!(desired.len(), 1);
    assert_eq!(desired[0].0, InputMode::Tab);
    assert!(desired[0].1.has_only_modifiers(&[KeyModifier::Shift]));
    assert_eq!(
        tab_bindings,
        &vec![(desired[0].1.clone(), desired[0].2.clone())]
    );
    assert_prompt_pipe(&desired[0], TEST_PLUGIN_ID);
}

#[test]
fn user_binding_wins_while_an_unrelated_mode_does_not_collide() {
    let key = shifted_n();
    let user_binding = vec![(InputMode::Tab, vec![(key.clone(), vec![Action::NoOp])])];
    assert!(custom_layouts::available_bindings(&user_binding, TEST_PLUGIN_ID).is_empty());

    let unrelated_mode = vec![(InputMode::Pane, vec![(key, vec![Action::NoOp])])];
    assert_eq!(
        custom_layouts::available_bindings(&unrelated_mode, TEST_PLUGIN_ID),
        custom_layouts::bindings(TEST_PLUGIN_ID)
    );
}

#[test]
fn exact_and_old_plugin_targets_are_refreshed_to_defeat_stale_snapshots() {
    let desired = custom_layouts::bindings(TEST_PLUGIN_ID);
    let exact = keybind_snapshot(desired.clone());
    assert_eq!(
        custom_layouts::available_bindings(&exact, TEST_PLUGIN_ID),
        desired
    );

    let old = keybind_snapshot(custom_layouts::bindings(7));
    assert_eq!(
        custom_layouts::available_bindings(&old, TEST_PLUGIN_ID),
        desired
    );
}

#[test]
fn protobuf_stripped_binding_keeps_the_sentinel_needed_for_retargeting() {
    let old = keybind_snapshot(custom_layouts::bindings(7));
    let encoded =
        zellij_utils::plugin_api::event::ProtobufEvent::try_from(Event::InitialKeybinds(old))
            .unwrap();
    let Event::InitialKeybinds(round_tripped) = Event::try_from(encoded).unwrap() else {
        panic!("expected InitialKeybinds");
    };

    let actions = &round_tripped[0].1[0].1;
    assert!(matches!(
        actions.as_slice(),
        [
            Action::KeybindPipe {
                name: None,
                payload: None,
                ..
            },
            Action::SwitchToMode {
                input_mode: InputMode::Normal
            },
            Action::SwitchToMode {
                input_mode: InputMode::Normal
            },
            Action::SwitchToMode {
                input_mode: InputMode::Normal
            }
        ]
    ));
    assert_eq!(
        custom_layouts::available_bindings(&round_tripped, TEST_PLUGIN_ID),
        custom_layouts::bindings(TEST_PLUGIN_ID)
    );
}

#[test]
fn an_ambiguous_stripped_pipe_without_the_full_sentinel_is_preserved() {
    let desired = custom_layouts::bindings(7);
    let actions = vec![desired[0].2[0].clone(), desired[0].2[1].clone()];
    let existing = vec![(InputMode::Tab, vec![(shifted_n(), actions)])];
    let encoded =
        zellij_utils::plugin_api::event::ProtobufEvent::try_from(Event::InitialKeybinds(existing))
            .unwrap();
    let Event::InitialKeybinds(round_tripped) = Event::try_from(encoded).unwrap() else {
        panic!("expected InitialKeybinds");
    };

    assert!(custom_layouts::available_bindings(&round_tripped, TEST_PLUGIN_ID).is_empty());
}

#[test]
fn prompt_edits_at_a_unicode_character_cursor() {
    let mut prompt = Prompt::new(9, Some("/work".to_string()));
    assert!(custom_layouts::handle_paste(&mut prompt, "a🦀b"));
    assert_eq!(prompt.cursor, 3);

    assert_eq!(press(&mut prompt, BareKey::Left), PromptKey::Updated);
    assert_eq!(press(&mut prompt, BareKey::Left), PromptKey::Updated);
    assert_eq!(press(&mut prompt, BareKey::Delete), PromptKey::Updated);
    assert_eq!(prompt.input, "ab");
    assert_eq!(prompt.cursor, 1);

    assert_eq!(type_character(&mut prompt, 'é'), PromptKey::Updated);
    assert_eq!(prompt.input, "aéb");
    assert_eq!(prompt.cursor, 2);
    assert_eq!(press(&mut prompt, BareKey::Backspace), PromptKey::Updated);
    assert_eq!(prompt.input, "ab");
    assert_eq!(prompt.cursor, 1);

    press(&mut prompt, BareKey::End);
    type_character(&mut prompt, 'c');
    press(&mut prompt, BareKey::Home);
    type_character(&mut prompt, 'z');
    assert_eq!(prompt.input, "zabc");
}

#[test]
fn paste_filters_controls_inserts_at_the_cursor_and_honors_the_length_limit() {
    let mut prompt = Prompt::new(1, None);
    prompt.input = "ab".to_string();
    prompt.cursor = 1;
    prompt.error = Some("unknown state".to_string());

    assert!(custom_layouts::handle_paste(&mut prompt, "C\n\tD"));
    assert_eq!(prompt.input, "aCDb");
    assert_eq!(prompt.cursor, 3);
    assert_eq!(prompt.error, None);

    prompt.input = "x".repeat(custom_layouts::MAX_ID_CHARACTERS - 1);
    prompt.cursor = prompt.input.chars().count();
    assert!(custom_layouts::handle_paste(&mut prompt, "yz"));
    assert_eq!(
        prompt.input.chars().count(),
        custom_layouts::MAX_ID_CHARACTERS
    );
    assert!(prompt.input.ends_with('y'));
    assert!(!custom_layouts::handle_paste(&mut prompt, "z"));
}

#[test]
fn prompt_submits_trims_and_cancels_without_accepting_control_modified_text() {
    let mut prompt = Prompt::new(1, None);
    custom_layouts::handle_paste(&mut prompt, " claude6 ");
    assert_eq!(
        press(&mut prompt, BareKey::Enter),
        PromptKey::Submit("claude6".to_string())
    );

    assert_eq!(press(&mut prompt, BareKey::Esc), PromptKey::Cancel);
    assert_eq!(
        custom_layouts::handle_prompt_key(
            &mut prompt,
            &KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(),
        ),
        PromptKey::Cancel
    );
    let before = prompt.clone();
    assert_eq!(
        custom_layouts::handle_prompt_key(
            &mut prompt,
            &KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier(),
        ),
        PromptKey::Ignored
    );
    assert_eq!(prompt, before);

    let mut shifted = Prompt::new(1, None);
    assert_eq!(
        custom_layouts::handle_prompt_key(
            &mut shifted,
            &KeyWithModifier::new(BareKey::Char('n')).with_shift_modifier(),
        ),
        PromptKey::Updated
    );
    assert_eq!(shifted.input, "N");
}

#[test]
fn prompt_ignores_stale_focus_before_acquisition_then_closes_after_real_focus_loss() {
    let mut prompt = Prompt::new(1, None);
    let requested_ms = 1_000;
    prompt.begin_focus_acquisition(requested_ms);

    assert!(!prompt.observe_focus(false, requested_ms));
    assert!(!prompt.observe_focus(false, requested_ms + 1));
    assert!(!prompt.observe_focus(true, requested_ms + 10));
    assert!(prompt.observe_focus(false, requested_ms + 11));

    let mut prompt = Prompt::new(1, None);
    prompt.note_input();
    assert!(prompt.observe_focus(false, requested_ms));
}

#[test]
fn prompt_closes_when_the_host_never_honors_its_focus_request() {
    let mut prompt = Prompt::new(1, None);
    let requested_ms = 5_000;
    prompt.begin_focus_acquisition(requested_ms);

    assert!(!prompt.observe_focus(
        false,
        requested_ms + custom_layouts::FOCUS_ACQUISITION_TIMEOUT_MS - 1
    ));
    assert!(prompt.observe_focus(
        false,
        requested_ms + custom_layouts::FOCUS_ACQUISITION_TIMEOUT_MS
    ));
}

fn example_layout() -> CustomLayout {
    CustomLayout {
        id: "claude6".to_string(),
        width: 3,
        height: 2,
        commands: command_names(),
    }
}

fn command_names() -> Vec<String> {
    (1..=6).map(|index| format!("A{index}")).collect()
}

fn shell_command(run: &Option<Run>) -> String {
    let Some(Run::Command(command)) = run else {
        panic!("expected a command pane, got {run:?}");
    };
    assert_eq!(command.command, PathBuf::from("sh"));
    assert_eq!(command.args.first().map(String::as_str), Some("-lc"));
    command
        .args
        .get(1)
        .cloned()
        .expect("the shell command must be the second argument")
}

fn maybe_shell_command(run: &Option<Run>) -> Option<String> {
    matches!(run, Some(Run::Command(_))).then(|| shell_command(run))
}

fn plugin_configuration() -> BTreeMap<String, String> {
    BTreeMap::from([("custom_states".to_string(), "[]".to_string())])
}

fn shifted_n() -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char(custom_layouts::CUSTOM_LAYOUT_KEY)).with_shift_modifier()
}

fn keybind_snapshot(bindings: Vec<custom_layouts::TabBinding>) -> KeybindsVec {
    vec![(
        InputMode::Tab,
        bindings
            .into_iter()
            .map(|(_, key, actions)| (key, actions))
            .collect(),
    )]
}

fn assert_prompt_pipe(binding: &custom_layouts::TabBinding, plugin_id: u32) {
    assert_eq!(binding.2.len(), 4);
    match &binding.2[0] {
        Action::KeybindPipe {
            name,
            payload,
            plugin,
            plugin_id: target_plugin_id,
            launch_new,
            ..
        } => {
            assert_eq!(name.as_deref(), Some(custom_layouts::PIPE_NAME));
            assert_eq!(payload.as_deref(), Some("prompt"));
            assert_eq!(plugin, &None);
            assert_eq!(target_plugin_id, &Some(plugin_id));
            assert!(!launch_new);
        }
        action => panic!("expected KeybindPipe, got {action:?}"),
    }
}

fn press(prompt: &mut Prompt, key: BareKey) -> PromptKey {
    custom_layouts::handle_prompt_key(prompt, &KeyWithModifier::new(key))
}

fn type_character(prompt: &mut Prompt, character: char) -> PromptKey {
    press(prompt, BareKey::Char(character))
}
