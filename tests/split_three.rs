#![allow(dead_code)]

#[path = "../src/split_three.rs"]
mod split_three;

use split_three::{
    bindings, DragPlan, PaneRect, SplitDirection, SPLIT_THREE_DOWN_KEY, SPLIT_THREE_RIGHT_KEY,
};
use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;
use zellij_utils::input::config::Config;

const TEST_PLUGIN_ID: u32 = 42;

#[test]
fn only_zellij_0440_uses_mode_update_for_the_initial_snapshot() {
    assert!(split_three::uses_legacy_mode_keybinds("0.44.0"));
    assert!(split_three::uses_legacy_mode_keybinds("v0.44.0"));
    assert!(!split_three::uses_legacy_mode_keybinds("0.44.0-dev"));
    assert!(!split_three::uses_legacy_mode_keybinds("0.44.1"));
    assert!(!split_three::uses_legacy_mode_keybinds("0.44.3"));
    assert!(!split_three::uses_legacy_mode_keybinds("1.0.0"));
    assert!(!split_three::uses_legacy_mode_keybinds("unknown"));
}

#[test]
fn bindings_send_the_direction_to_zellaude_and_return_to_normal() {
    let bindings = bindings(TEST_PLUGIN_ID);

    assert_eq!(bindings.len(), 2);
    assert_pipe_binding(
        find_binding(&bindings, SPLIT_THREE_RIGHT_KEY).unwrap(),
        "right",
        TEST_PLUGIN_ID,
    );
    assert_pipe_binding(
        find_binding(&bindings, SPLIT_THREE_DOWN_KEY).unwrap(),
        "down",
        TEST_PLUGIN_ID,
    );
}

#[test]
fn bindings_do_not_run_two_native_splits_directly() {
    for (_, _, actions) in bindings(TEST_PLUGIN_ID) {
        assert!(actions
            .iter()
            .all(|action| !matches!(action, Action::NewPane { .. })));
    }
}

#[test]
fn focus_barrier_is_supported_by_the_plugin_action_protocol() {
    let expected = split_three::focus_pane_action(17);
    let encoded = zellij_utils::plugin_api::action::ProtobufAction::try_from(expected.clone())
        .expect("focus action should serialize through the plugin API");
    let decoded = Action::try_from(encoded).expect("focus action should deserialize");

    assert_eq!(decoded, expected);
}

#[test]
fn split_action_preserves_its_delayed_spawn_ownership_marker() {
    let expected = split_three::new_pane_action(SplitDirection::Right, 73, 1, 2);
    let encoded = zellij_utils::plugin_api::action::ProtobufAction::try_from(expected.clone())
        .expect("split action should serialize through the plugin API");
    let decoded = Action::try_from(encoded).expect("split action should deserialize");

    assert_eq!(decoded, expected);
    assert!(matches!(
        decoded,
        Action::NewPane {
            pane_name: Some(name),
            ..
        } if name == split_three::pane_marker(73, 1, 2)
    ));
}

#[test]
fn delayed_spawn_marker_is_unique_but_has_native_title_width() {
    let first = split_three::pane_marker(73, 1, 12);
    let second_operation = split_three::pane_marker(74, 1, 12);
    let second_pane = split_three::pane_marker(73, 2, 13);

    assert!(first.starts_with("Pane #12"));
    assert_eq!(UnicodeWidthStr::width(first.as_str()), "Pane #12".width());
    assert_ne!(first, second_operation);
    assert_ne!(first, second_pane);

    let token = first.strip_prefix("Pane #12").unwrap();
    assert_eq!(token.chars().count(), 8);
    assert!(token
        .chars()
        .all(|character| matches!(character, '\u{2061}' | '\u{2062}' | '\u{2063}' | '\u{2064}')));
}

#[test]
fn rollback_actions_are_supported_by_the_plugin_action_protocol() {
    let drag = DragPlan {
        press_line: 7,
        press_column: 20,
        release_line: 7,
        release_column: 20,
        first_span: 20,
    };
    for expected in [
        split_three::drag_cancel_action(drag),
        split_three::close_pane_action(17),
    ] {
        let encoded = zellij_utils::plugin_api::action::ProtobufAction::try_from(expected.clone())
            .expect("rollback action should serialize through the plugin API");
        let decoded = Action::try_from(encoded).expect("rollback action should deserialize");
        assert_eq!(decoded, expected);
    }
}

#[test]
fn runtime_reconfigure_preserves_the_targeted_pipe_fields() {
    let desired = bindings(TEST_PLUGIN_ID);
    let parsed = Config::from_kdl(
        &split_three::reconfigure_snippet(&desired, TEST_PLUGIN_ID),
        None,
    )
    .unwrap();
    let parsed = parsed.keybinds.to_keybinds_vec();
    let pane_bindings = parsed
        .iter()
        .find(|(mode, _)| mode == &InputMode::Pane)
        .map(|(_, bindings)| bindings)
        .unwrap();

    for expected in desired {
        let actions = pane_bindings
            .iter()
            .find(|(key, _)| key == &expected.1)
            .map(|(_, actions)| actions)
            .unwrap();
        assert_eq!(actions, &expected.2);
    }
}

#[test]
fn existing_custom_bindings_are_never_replaced() {
    let custom_action = vec![Action::NoOp];
    let existing = vec![(
        InputMode::Pane,
        vec![(shifted(SPLIT_THREE_RIGHT_KEY), custom_action.clone())],
    )];

    let available = split_three::available_bindings(&existing, TEST_PLUGIN_ID);

    assert_eq!(available.len(), 1);
    assert_eq!(binding_key(&available[0]), SPLIT_THREE_DOWN_KEY);
    assert_eq!(existing[0].1[0].1, custom_action);
}

#[test]
fn both_bindings_are_available_when_the_keys_are_unused() {
    let existing = vec![(
        InputMode::Pane,
        vec![(KeyWithModifier::new(BareKey::Char('x')), vec![Action::NoOp])],
    )];

    assert_eq!(
        split_three::available_bindings(&existing, TEST_PLUGIN_ID),
        bindings(TEST_PLUGIN_ID)
    );
}

#[test]
fn an_already_installed_binding_is_not_rebound() {
    let desired = bindings(TEST_PLUGIN_ID);
    let right = find_binding(&desired, SPLIT_THREE_RIGHT_KEY).unwrap();
    let existing = vec![(InputMode::Pane, vec![(right.1.clone(), right.2.clone())])];

    let available = split_three::available_bindings(&existing, TEST_PLUGIN_ID);

    assert_eq!(available.len(), 1);
    assert_eq!(binding_key(&available[0]), SPLIT_THREE_DOWN_KEY);
}

#[test]
fn bindings_are_retargeted_when_another_zellaude_tab_becomes_active() {
    let existing = vec![(
        InputMode::Pane,
        bindings(7)
            .into_iter()
            .map(|(_, key, actions)| (key, actions))
            .collect(),
    )];

    assert_eq!(
        split_three::available_bindings(&existing, TEST_PLUGIN_ID),
        bindings(TEST_PLUGIN_ID)
    );
}

#[test]
fn protobuf_round_tripped_binding_keeps_its_ownership_marker() {
    let existing = vec![(
        InputMode::Pane,
        bindings(7)
            .into_iter()
            .map(|(_, key, actions)| (key, actions))
            .collect(),
    )];
    let encoded =
        zellij_utils::plugin_api::event::ProtobufEvent::try_from(Event::InitialKeybinds(existing))
            .unwrap();
    let Event::InitialKeybinds(round_tripped) = Event::try_from(encoded).unwrap() else {
        panic!("expected InitialKeybinds");
    };
    assert_eq!(
        split_three::available_bindings(&round_tripped, TEST_PLUGIN_ID),
        bindings(TEST_PLUGIN_ID)
    );
}

#[test]
fn ambiguous_stripped_pipe_without_the_marker_is_preserved() {
    let desired = bindings(7);
    let right = find_binding(&desired, SPLIT_THREE_RIGHT_KEY).unwrap();
    let unmarked_actions = vec![right.2[0].clone(), right.2[3].clone()];
    let existing = vec![(InputMode::Pane, vec![(right.1.clone(), unmarked_actions)])];
    let encoded =
        zellij_utils::plugin_api::event::ProtobufEvent::try_from(Event::InitialKeybinds(existing))
            .unwrap();
    let Event::InitialKeybinds(round_tripped) = Event::try_from(encoded).unwrap() else {
        panic!("expected InitialKeybinds");
    };

    let available = split_three::available_bindings(&round_tripped, TEST_PLUGIN_ID);
    assert_eq!(available.len(), 1);
    assert_eq!(binding_key(&available[0]), SPLIT_THREE_DOWN_KEY);
}

#[test]
fn the_previous_zellaude_binding_is_upgraded_but_custom_keys_are_not() {
    let legacy = vec![
        new_pane(SplitDirection::Right),
        new_pane(SplitDirection::Right),
        Action::SwitchToMode {
            input_mode: InputMode::Normal,
        },
    ];
    let existing = vec![(
        InputMode::Pane,
        vec![(shifted(SPLIT_THREE_RIGHT_KEY), legacy)],
    )];

    let available = split_three::available_bindings(&existing, TEST_PLUGIN_ID);
    assert_eq!(available.len(), 2);
}

#[test]
fn right_split_moves_the_first_boundary_before_halving_the_remainder() {
    let original = PaneRect {
        x: 0,
        y: 0,
        columns: 80,
        rows: 23,
    };
    let first = pane(1, 0, 0, 40, 23, false, true);
    let second = pane(2, 40, 0, 40, 23, true, true);

    let plan =
        split_three::plan_first_boundary_drag(original, &first, &second, SplitDirection::Right)
            .unwrap();

    assert_eq!(
        plan,
        DragPlan {
            press_line: 11,
            press_column: 40,
            release_line: 11,
            release_column: 27,
            first_span: 27,
        }
    );
    assert!(split_three::ready_for_second_split(
        original,
        &pane(1, 0, 0, 27, 23, false, true),
        &pane(2, 27, 0, 53, 23, true, true),
        SplitDirection::Right,
        27,
    ));
    assert!(split_three::are_equal_thirds(
        original,
        &pane(1, 0, 0, 27, 23, false, true),
        &pane(2, 27, 0, 27, 23, false, true),
        &pane(3, 54, 0, 26, 23, true, true),
        SplitDirection::Right,
    ));
}

#[test]
fn down_split_produces_nearest_possible_equal_heights() {
    let original = PaneRect {
        x: 3,
        y: 2,
        columns: 60,
        rows: 23,
    };
    let first = pane(1, 3, 2, 60, 12, false, true);
    let second = pane(2, 3, 14, 60, 11, true, true);

    let plan =
        split_three::plan_first_boundary_drag(original, &first, &second, SplitDirection::Down)
            .unwrap();

    assert_eq!(plan.press_line, 14);
    assert_eq!(plan.release_line, 10);
    assert_eq!(plan.first_span, 8);
    assert!(split_three::are_equal_thirds(
        original,
        &pane(1, 3, 2, 60, 8, false, true),
        &pane(2, 3, 10, 60, 8, false, true),
        &pane(3, 3, 18, 60, 7, true, true),
        SplitDirection::Down,
    ));
}

#[test]
fn divisible_dimensions_become_exact_thirds() {
    let original = PaneRect {
        x: 10,
        y: 4,
        columns: 60,
        rows: 18,
    };
    assert!(split_three::are_equal_thirds(
        original,
        &pane(1, 10, 4, 20, 18, false, true),
        &pane(2, 30, 4, 20, 18, false, true),
        &pane(3, 50, 4, 20, 18, true, true),
        SplitDirection::Right,
    ));
}

#[test]
fn final_validation_waits_until_the_manifest_contains_all_operation_panes() {
    let original = PaneRect {
        x: 0,
        y: 0,
        columns: 80,
        rows: 23,
    };
    let first = pane(1, 0, 0, 27, 23, false, true);
    // A complete manifest is authoritative: missing IDs may still arrive in a
    // later PaneUpdate, while present but malformed rectangles are invalid.
    let malformed_second = pane(2, 27, 0, 53, 23, false, true);
    let third = pane(3, 54, 0, 26, 23, true, true);

    let mut manifest = PaneManifest::default();
    manifest
        .panes
        .insert(77, vec![first.clone(), malformed_second]);

    assert_eq!(
        split_three::final_geometry_status_in_manifest(
            &manifest,
            original,
            1,
            2,
            3,
            SplitDirection::Right,
        ),
        split_three::FinalGeometryStatus::StillSettling,
    );
    manifest.panes.get_mut(&77).unwrap().push(third.clone());
    assert_eq!(
        split_three::final_geometry_status_in_manifest(
            &manifest,
            original,
            1,
            2,
            3,
            SplitDirection::Right,
        ),
        split_three::FinalGeometryStatus::Invalid,
    );
    manifest.panes.get_mut(&77).unwrap().pop();
    manifest.panes.get_mut(&77).unwrap()[1] = pane(2, 27, 0, 27, 23, false, true);
    manifest.panes.get_mut(&77).unwrap().push(third);
    assert_eq!(
        split_three::final_geometry_status_in_manifest(
            &manifest,
            original,
            1,
            2,
            3,
            SplitDirection::Right,
        ),
        split_three::FinalGeometryStatus::Settled,
    );
}

#[test]
fn final_validation_rejects_unsupported_state_on_any_operation_pane() {
    let original = PaneRect {
        x: 0,
        y: 0,
        columns: 60,
        rows: 18,
    };
    let valid = vec![
        pane(1, 0, 0, 20, 18, false, true),
        pane(2, 20, 0, 20, 18, false, true),
        pane(3, 40, 0, 20, 18, true, true),
    ];
    let mutations = [
        (
            "plugin",
            (|pane: &mut PaneInfo| pane.is_plugin = true) as fn(&mut PaneInfo),
        ),
        ("floating", |pane| pane.is_floating = true),
        ("suppressed", |pane| pane.is_suppressed = true),
        ("non-selectable", |pane| pane.is_selectable = false),
        ("fullscreen", |pane| pane.is_fullscreen = true),
    ];

    for pane_index in 0..valid.len() {
        for (state, mutate) in mutations {
            let mut panes = valid.clone();
            mutate(&mut panes[pane_index]);
            assert_eq!(
                split_three::final_geometry_status(
                    original,
                    Some(&panes[0]),
                    Some(&panes[1]),
                    Some(&panes[2]),
                    SplitDirection::Right,
                ),
                split_three::FinalGeometryStatus::Invalid,
                "{state} pane at operation index {pane_index} must not commit",
            );
        }
    }
}

#[test]
fn layout_wide_rounding_may_lend_one_cell_to_equal_thirds() {
    let right_original = PaneRect {
        x: 0,
        y: 1,
        columns: 188,
        rows: 69,
    };
    assert!(split_three::are_equal_thirds(
        right_original,
        &pane(1, 0, 1, 63, 69, false, true),
        &pane(2, 63, 1, 63, 69, false, true),
        &pane(3, 126, 1, 63, 69, true, true),
        SplitDirection::Right,
    ));

    let down_original = PaneRect {
        x: 20,
        y: 0,
        columns: 80,
        rows: 68,
    };
    assert!(split_three::are_equal_thirds(
        down_original,
        &pane(1, 20, 0, 80, 23, false, true),
        &pane(2, 20, 23, 80, 23, false, true),
        &pane(3, 20, 46, 80, 23, true, true),
        SplitDirection::Down,
    ));

    let contracted_original = PaneRect {
        x: 10,
        y: 1,
        columns: 61,
        rows: 69,
    };
    assert!(split_three::are_equal_thirds(
        contracted_original,
        &pane(1, 10, 1, 20, 69, false, true),
        &pane(2, 30, 1, 20, 69, false, true),
        &pane(3, 50, 1, 20, 69, true, true),
        SplitDirection::Right,
    ));

    let translated_original = PaneRect {
        x: 10,
        y: 1,
        columns: 60,
        rows: 69,
    };
    assert!(split_three::are_equal_thirds(
        translated_original,
        &pane(1, 11, 1, 20, 69, false, true),
        &pane(2, 31, 1, 20, 69, false, true),
        &pane(3, 51, 1, 20, 69, true, true),
        SplitDirection::Right,
    ));

    assert!(!split_three::are_equal_thirds(
        right_original,
        &pane(1, 0, 1, 64, 69, false, true),
        &pane(2, 64, 1, 64, 69, false, true),
        &pane(3, 128, 1, 64, 69, true, true),
        SplitDirection::Right,
    ));
}

#[test]
fn first_boundary_drag_anchors_to_a_one_cell_translated_split() {
    let right_original = PaneRect {
        x: 10,
        y: 4,
        columns: 60,
        rows: 18,
    };
    let right_plan = split_three::plan_first_boundary_drag(
        right_original,
        &pane(1, 11, 4, 30, 18, false, true),
        &pane(2, 41, 4, 30, 18, true, true),
        SplitDirection::Right,
    )
    .unwrap();
    assert_eq!(right_plan.release_column, 31);
    assert_eq!(right_plan.first_span, 20);

    let down_original = PaneRect {
        x: 3,
        y: 10,
        columns: 60,
        rows: 60,
    };
    let down_plan = split_three::plan_first_boundary_drag(
        down_original,
        &pane(1, 3, 11, 60, 30, false, true),
        &pane(2, 3, 41, 60, 30, true, true),
        SplitDirection::Down,
    )
    .unwrap();
    assert_eq!(down_plan.release_line, 31);
    assert_eq!(down_plan.first_span, 20);

    assert!(split_three::plan_first_boundary_drag(
        right_original,
        &pane(1, 12, 4, 30, 18, false, true),
        &pane(2, 42, 4, 30, 18, true, true),
        SplitDirection::Right,
    )
    .is_none());
}

#[test]
fn frameless_separator_is_detected_without_toggling_the_users_frames() {
    let original = PaneRect {
        x: 0,
        y: 0,
        columns: 60,
        rows: 18,
    };
    let mut first = pane(1, 0, 0, 30, 18, false, false);
    // With pane frames disabled, Zellij reserves the first pane's final
    // content cell as the shared mouse-resizable separator.
    first.pane_content_columns = 29;
    let second = pane(2, 30, 0, 30, 18, true, false);

    let plan =
        split_three::plan_first_boundary_drag(original, &first, &second, SplitDirection::Right)
            .unwrap();
    assert_eq!(plan.press_column, 29);
    assert_eq!(plan.release_column, 19);

    let displaced = pane(2, 31, 0, 29, 18, true, false);
    assert!(split_three::plan_first_boundary_drag(
        original,
        &first,
        &displaced,
        SplitDirection::Right,
    )
    .is_none());
}

#[test]
fn a_frame_on_either_side_of_the_boundary_can_drive_the_drag() {
    let original = PaneRect {
        x: 0,
        y: 0,
        columns: 60,
        rows: 18,
    };
    let first = pane(1, 0, 0, 30, 18, false, true);
    let second = pane(2, 30, 0, 30, 18, true, false);

    let plan =
        split_three::plan_first_boundary_drag(original, &first, &second, SplitDirection::Right)
            .unwrap();

    assert_eq!(plan.press_column, 29);
    assert_eq!(plan.release_column, 19);
}

#[test]
fn unsupported_targets_fail_before_any_action_runs() {
    let mut target = pane(1, 0, 0, 14, 30, true, true);
    assert!(!split_three::target_is_supported(
        &target,
        SplitDirection::Right
    ));
    target.pane_columns = 30;
    target.is_floating = true;
    assert!(!split_three::target_is_supported(
        &target,
        SplitDirection::Right
    ));
    target.is_floating = false;
    target.is_fullscreen = true;
    assert!(!split_three::target_is_supported(
        &target,
        SplitDirection::Right
    ));
}

#[test]
fn focused_target_does_not_depend_on_the_pane_info_focus_snapshot() {
    let target = pane(1, 0, 0, 30, 30, false, true);
    assert!(split_three::target_is_supported(
        &target,
        SplitDirection::Right
    ));
    assert!(split_three::target_is_supported(
        &target,
        SplitDirection::Down
    ));
}

#[test]
fn only_the_plugin_instance_on_the_active_tab_handles_the_targeted_pipe() {
    let mut manifest = PaneManifest::default();
    manifest
        .panes
        .insert(0, vec![plugin_pane(10), pane(1, 0, 0, 60, 20, true, true)]);
    manifest
        .panes
        .insert(1, vec![plugin_pane(20), pane(2, 0, 0, 60, 20, true, true)]);
    let tabs = vec![
        TabInfo {
            position: 0,
            active: false,
            ..TabInfo::default()
        },
        TabInfo {
            position: 1,
            active: true,
            ..TabInfo::default()
        },
    ];

    assert!(!split_three::is_active_instance(&manifest, &tabs, 10));
    assert!(split_three::is_active_instance(&manifest, &tabs, 20));
    assert!(!split_three::is_active_instance(&manifest, &tabs, 99));
    assert!(!split_three::is_active_instance(&manifest, &[], 20));
}

#[test]
fn action_completions_are_scoped_to_operation_and_stage() {
    let mut operation = split_three::Operation::new(
        42,
        SplitDirection::Right,
        7,
        11,
        PaneRect {
            x: 0,
            y: 0,
            columns: 60,
            rows: 20,
        },
    );
    let first_context = operation.context();
    assert!(operation.matches_context(&first_context));

    operation.stage = split_three::OperationStage::DragPress;
    assert!(!operation.matches_context(&first_context));
    assert!(operation.matches_context(&operation.context()));

    let mut stale_operation = operation.context();
    stale_operation.insert("operation_id".to_string(), "41".to_string());
    assert!(!operation.matches_context(&stale_operation));

    operation.stage = split_three::OperationStage::FirstSplit;
    let delayed_spawn = operation.context();
    operation.stage = split_three::OperationStage::RecoverFirstSplit;
    assert!(!operation.matches_context(&delayed_spawn));
    assert!(operation
        .matches_context_for_stage(&delayed_spawn, split_three::OperationStage::FirstSplit,));

    operation.stage = split_three::OperationStage::SecondSplit;
    let delayed_second_spawn = operation.context();
    operation.stage = split_three::OperationStage::RecoverSecondSplit;
    assert!(operation.matches_context_for_stage(
        &delayed_second_spawn,
        split_three::OperationStage::SecondSplit,
    ));
}

#[test]
fn delayed_spawn_recovery_only_claims_panes_inside_the_original_rectangle() {
    let original = PaneRect {
        x: 10,
        y: 4,
        columns: 60,
        rows: 18,
    };
    assert!(split_three::pane_is_within(
        original,
        &pane(2, 30, 4, 40, 18, true, true),
        SplitDirection::Right,
    ));
    assert!(split_three::pane_is_within(
        original,
        &pane(2, 54, 4, 17, 18, true, true),
        SplitDirection::Right,
    ));
    assert!(!split_three::pane_is_within(
        original,
        &pane(3, 70, 4, 20, 18, true, true),
        SplitDirection::Right,
    ));
}

fn assert_pipe_binding(binding: &split_three::PaneBinding, payload: &str, plugin_id: u32) {
    assert_eq!(binding.0, InputMode::Pane);
    assert!(binding.1.has_only_modifiers(&[KeyModifier::Shift]));
    assert_eq!(binding.2.len(), 4);
    match &binding.2[0] {
        Action::KeybindPipe {
            name,
            payload: actual_payload,
            plugin,
            plugin_id: target_plugin_id,
            launch_new,
            ..
        } => {
            assert_eq!(name.as_deref(), Some(split_three::PIPE_NAME));
            assert_eq!(actual_payload.as_deref(), Some(payload));
            assert_eq!(plugin, &None);
            assert_eq!(target_plugin_id, &Some(plugin_id));
            assert!(!launch_new);
        }
        action => panic!("expected KeybindPipe, got {action:?}"),
    }
    for action in &binding.2[1..] {
        assert_eq!(
            action,
            &Action::SwitchToMode {
                input_mode: InputMode::Normal,
            }
        );
    }
}

fn binding_key(binding: &split_three::PaneBinding) -> char {
    match binding.1.bare_key {
        BareKey::Char(key) => key,
        _ => panic!("Split Three must use a character key"),
    }
}

fn find_binding(
    bindings: &[split_three::PaneBinding],
    key: char,
) -> Option<&split_three::PaneBinding> {
    let expected = shifted(key);
    bindings.iter().find(|binding| binding.1 == expected)
}

fn new_pane(direction: SplitDirection) -> Action {
    Action::NewPane {
        direction: Some(direction.zellij_direction()),
        pane_name: None,
        start_suppressed: false,
    }
}

fn shifted(key: char) -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char(key)).with_shift_modifier()
}

fn pane(
    id: u32,
    x: usize,
    y: usize,
    columns: usize,
    rows: usize,
    focused: bool,
    framed: bool,
) -> PaneInfo {
    let frame = usize::from(framed);
    PaneInfo {
        id,
        is_focused: focused,
        is_selectable: true,
        pane_x: x,
        pane_y: y,
        pane_columns: columns,
        pane_rows: rows,
        pane_content_x: x + frame,
        pane_content_y: y + frame,
        pane_content_columns: columns.saturating_sub(frame * 2),
        pane_content_rows: rows.saturating_sub(frame * 2),
        ..PaneInfo::default()
    }
}

fn plugin_pane(id: u32) -> PaneInfo {
    PaneInfo {
        id,
        is_plugin: true,
        ..PaneInfo::default()
    }
}

#[no_mangle]
extern "C" fn host_run_plugin_command() {}
