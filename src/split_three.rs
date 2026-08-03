use std::collections::{BTreeMap, BTreeSet};
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;
use zellij_utils::input::mouse::MouseEvent;
use zellij_utils::position::Position;

/// Runtime-only Pane-mode bindings installed by the plugin.
pub type PaneBinding = (InputMode, KeyWithModifier, Vec<Action>);

pub const SPLIT_THREE_RIGHT_KEY: char = 'r';
pub const SPLIT_THREE_DOWN_KEY: char = 'd';
pub const PIPE_NAME: &str = "zellaude:split-three";
const PANE_MARKER_DIGITS: [char; 4] = ['\u{2061}', '\u{2062}', '\u{2063}', '\u{2064}'];

const CONTEXT_KIND: &str = "zellaude_split_three";
const CONTEXT_KIND_KEY: &str = "type";
const CONTEXT_OPERATION_KEY: &str = "operation_id";
const CONTEXT_STAGE_KEY: &str = "stage";

/// Zellij will not create or resize a terminal pane below five cells. Three
/// equal panes therefore need at least fifteen cells along the split axis.
pub const MIN_SPLIT_SPAN: usize = 15;
const MAX_LAYOUT_ROUNDING_DRIFT: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Right,
    Down,
}

impl SplitDirection {
    pub fn from_payload(payload: Option<&str>) -> Option<Self> {
        match payload {
            Some("right") => Some(Self::Right),
            Some("down") => Some(Self::Down),
            _ => None,
        }
    }

    fn payload(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }

    pub fn zellij_direction(self) -> Direction {
        match self {
            Self::Right => Direction::Right,
            Self::Down => Direction::Down,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRect {
    pub x: usize,
    pub y: usize,
    pub columns: usize,
    pub rows: usize,
}

impl From<&PaneInfo> for PaneRect {
    fn from(pane: &PaneInfo) -> Self {
        Self {
            x: pane.pane_x,
            y: pane.pane_y,
            columns: pane.pane_columns,
            rows: pane.pane_rows,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragPlan {
    pub press_line: usize,
    pub press_column: usize,
    pub release_line: usize,
    pub release_column: usize,
    pub first_span: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStage {
    FocusOriginal,
    FirstSplit,
    RecoverFirstSplit,
    FocusForDrag,
    DragPress,
    FocusForRelease,
    DragRelease,
    FocusForSecondSplit,
    SecondSplit,
    RecoverSecondSplit,
    ValidateFinal,
    RollbackFocusForRelease,
    RollbackRelease,
    RollbackSecond,
    RollbackFirst,
    RollbackFocus,
}

impl OperationStage {
    fn context_value(self) -> &'static str {
        match self {
            Self::FocusOriginal => "focus_original",
            Self::FirstSplit => "first_split",
            Self::RecoverFirstSplit => "recover_first_split",
            Self::FocusForDrag => "focus_for_drag",
            Self::DragPress => "drag_press",
            Self::FocusForRelease => "focus_for_release",
            Self::DragRelease => "drag_release",
            Self::FocusForSecondSplit => "focus_for_second_split",
            Self::SecondSplit => "second_split",
            Self::RecoverSecondSplit => "recover_second_split",
            Self::ValidateFinal => "validate_final",
            Self::RollbackFocusForRelease => "rollback_focus_for_release",
            Self::RollbackRelease => "rollback_release",
            Self::RollbackSecond => "rollback_second",
            Self::RollbackFirst => "rollback_first",
            Self::RollbackFocus => "rollback_focus",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalGeometryStatus {
    Settled,
    StillSettling,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub id: u64,
    pub direction: SplitDirection,
    pub tab_id: usize,
    pub original_pane_id: u32,
    pub original_rect: PaneRect,
    pub initial_terminal_pane_count: usize,
    pub known_terminal_pane_ids: BTreeSet<u32>,
    pub first_new_pane_id: Option<u32>,
    pub second_new_pane_id: Option<u32>,
    pub drag: Option<DragPlan>,
    pub mouse_maybe_down: bool,
    pub recovery_attempts: u8,
    pub stage: OperationStage,
}

impl Operation {
    pub fn new(
        id: u64,
        direction: SplitDirection,
        tab_id: usize,
        original_pane_id: u32,
        original_rect: PaneRect,
    ) -> Self {
        Self {
            id,
            direction,
            tab_id,
            original_pane_id,
            original_rect,
            initial_terminal_pane_count: 1,
            known_terminal_pane_ids: BTreeSet::new(),
            first_new_pane_id: None,
            second_new_pane_id: None,
            drag: None,
            mouse_maybe_down: false,
            recovery_attempts: 0,
            stage: OperationStage::FocusOriginal,
        }
    }

    pub fn context(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (CONTEXT_KIND_KEY.to_string(), CONTEXT_KIND.to_string()),
            (CONTEXT_OPERATION_KEY.to_string(), self.id.to_string()),
            (
                CONTEXT_STAGE_KEY.to_string(),
                self.stage.context_value().to_string(),
            ),
        ])
    }

    pub fn matches_context(&self, context: &BTreeMap<String, String>) -> bool {
        self.matches_context_for_stage(context, self.stage)
    }

    pub fn matches_context_for_stage(
        &self,
        context: &BTreeMap<String, String>,
        stage: OperationStage,
    ) -> bool {
        context.get(CONTEXT_KIND_KEY).map(String::as_str) == Some(CONTEXT_KIND)
            && context
                .get(CONTEXT_OPERATION_KEY)
                .and_then(|value| value.parse::<u64>().ok())
                == Some(self.id)
            && context.get(CONTEXT_STAGE_KEY).map(String::as_str) == Some(stage.context_value())
    }
}

pub fn uses_legacy_mode_keybinds(version: &str) -> bool {
    version.trim_start_matches('v') == "0.44.0"
}

pub fn bindings(plugin_id: u32) -> Vec<PaneBinding> {
    vec![
        binding(SPLIT_THREE_RIGHT_KEY, SplitDirection::Right, plugin_id),
        binding(SPLIT_THREE_DOWN_KEY, SplitDirection::Down, plugin_id),
    ]
}

/// Return bindings whose keys are unused, belong to an older Split Three, or
/// currently target another tab's zellaude instance. User bindings win.
pub fn available_bindings(existing: &KeybindsVec, plugin_id: u32) -> Vec<PaneBinding> {
    bindings(plugin_id)
        .into_iter()
        .filter(|(mode, key, desired_actions)| {
            let matches: Vec<&Vec<Action>> = existing
                .iter()
                .filter(|(existing_mode, _)| existing_mode == mode)
                .flat_map(|(_, bindings)| bindings)
                .filter_map(|(existing_key, actions)| (existing_key == key).then_some(actions))
                .collect();
            matches.is_empty()
                || (matches
                    .iter()
                    .all(|actions| is_zellaude_binding(key, actions))
                    && matches.iter().any(|actions| *actions != desired_actions))
        })
        .collect()
}

pub fn install(existing: &KeybindsVec, plugin_id: u32) {
    let available = available_bindings(existing, plugin_id);
    if !available.is_empty() {
        // zellij 0.44's RebindKeys protobuf drops every KeybindPipe field.
        // Runtime reconfiguration parses MessagePluginId losslessly and still
        // belongs only to this client. Never write the user's config.kdl.
        reconfigure(reconfigure_snippet(&available, plugin_id), false);
    }
}

pub fn reconfigure_snippet(bindings: &[PaneBinding], plugin_id: u32) -> String {
    let mut config = String::from("keybinds {\n    pane {\n");
    for (_, key, _) in bindings {
        let (key_name, payload) = match key.bare_key {
            BareKey::Char(SPLIT_THREE_RIGHT_KEY) => ("R", "right"),
            BareKey::Char(SPLIT_THREE_DOWN_KEY) => ("D", "down"),
            _ => continue,
        };
        config.push_str(&format!(
            "        bind \"{key_name}\" {{\n            MessagePluginId {plugin_id} {{\n                name \"{PIPE_NAME}\"\n                payload \"{payload}\"\n            }}\n            SwitchToMode \"Normal\"\n            SwitchToMode \"Normal\"\n            SwitchToMode \"Normal\"\n        }}\n"
        ));
    }
    config.push_str("    }\n}\n");
    config
}

fn is_zellaude_binding(key: &KeyWithModifier, actions: &[Action]) -> bool {
    is_legacy_binding(key, actions) || is_pipe_binding(key, actions)
}

fn is_pipe_binding(key: &KeyWithModifier, actions: &[Action]) -> bool {
    let expected_payload = match key.bare_key {
        BareKey::Char(SPLIT_THREE_RIGHT_KEY) => "right",
        BareKey::Char(SPLIT_THREE_DOWN_KEY) => "down",
        _ => return false,
    };
    match actions {
        // Current binding. InitialKeybinds strips every KeybindPipe field, so
        // three idempotent mode switches are the protobuf-stable marker.
        [Action::KeybindPipe {
            name,
            payload,
            launch_new: false,
            ..
        }, Action::SwitchToMode {
            input_mode: InputMode::Normal,
        }, Action::SwitchToMode {
            input_mode: InputMode::Normal,
        }, Action::SwitchToMode {
            input_mode: InputMode::Normal,
        }] => {
            (name.as_deref() == Some(PIPE_NAME) && payload.as_deref() == Some(expected_payload))
                || (name.is_none() && payload.is_none())
        }
        // Recognize the pre-sentinel development binding while its metadata
        // is still available, but never claim an ambiguous stripped pipe.
        [Action::KeybindPipe {
            name: Some(name),
            payload: Some(payload),
            launch_new: false,
            ..
        }, Action::SwitchToMode {
            input_mode: InputMode::Normal,
        }] => name == PIPE_NAME && payload == expected_payload,
        _ => false,
    }
}

fn is_legacy_binding(key: &KeyWithModifier, actions: &[Action]) -> bool {
    let direction = match key.bare_key {
        BareKey::Char(SPLIT_THREE_RIGHT_KEY) => SplitDirection::Right,
        BareKey::Char(SPLIT_THREE_DOWN_KEY) => SplitDirection::Down,
        _ => return false,
    };
    actions
        == [
            legacy_new_pane_action(direction),
            legacy_new_pane_action(direction),
            Action::SwitchToMode {
                input_mode: InputMode::Normal,
            },
        ]
}

fn binding(key: char, direction: SplitDirection, plugin_id: u32) -> PaneBinding {
    (
        InputMode::Pane,
        KeyWithModifier::new(BareKey::Char(key)).with_shift_modifier(),
        vec![
            Action::KeybindPipe {
                name: Some(PIPE_NAME.to_string()),
                payload: Some(direction.payload().to_string()),
                args: None,
                plugin: None,
                // Zellij pairs this ID with the client that pressed the key,
                // so one client's shortcut cannot fan out to other clients.
                plugin_id: Some(plugin_id),
                configuration: None,
                launch_new: false,
                skip_cache: false,
                floating: None,
                in_place: None,
                cwd: None,
                pane_title: None,
            },
            // InitialKeybinds serializes KeybindPipe without its identifying
            // fields. Three idempotent switches are a distinctive marker that
            // lets another tab or reloaded plugin safely retarget the binding.
            Action::SwitchToMode {
                input_mode: InputMode::Normal,
            },
            Action::SwitchToMode {
                input_mode: InputMode::Normal,
            },
            Action::SwitchToMode {
                input_mode: InputMode::Normal,
            },
        ],
    )
}

fn legacy_new_pane_action(direction: SplitDirection) -> Action {
    Action::NewPane {
        direction: Some(direction.zellij_direction()),
        pane_name: None,
        start_suppressed: false,
    }
}

pub fn pane_marker(operation_id: u64, ordinal: u8, pane_index: usize) -> String {
    let mut marker = format!("Pane #{pane_index}");
    push_base_four(&mut marker, operation_id, 1);
    // Four digits encode the complete u8 and make the boundary between the
    // variable-length operation ID and pane ordinal unambiguous.
    push_base_four(&mut marker, u64::from(ordinal), 4);
    marker
}

fn push_base_four(output: &mut String, value: u64, minimum_digits: u32) {
    let significant_digits = if value == 0 {
        1
    } else {
        (u64::BITS - value.leading_zeros()).div_ceil(2)
    };
    for digit in (0..significant_digits.max(minimum_digits)).rev() {
        let index = ((value >> (digit * 2)) & 0b11) as usize;
        output.push(PANE_MARKER_DIGITS[index]);
    }
}

pub fn new_pane_action(
    direction: SplitDirection,
    operation_id: u64,
    ordinal: u8,
    pane_index: usize,
) -> Action {
    Action::NewPane {
        direction: Some(direction.zellij_direction()),
        // Zellij keeps an initial pane title as an immutable fallback. Make
        // that fallback look exactly like its native "Pane #N" title while a
        // zero-width token lets timeout recovery identify only our pane. Once
        // the custom name is cleared, later OSC titles still take precedence.
        pane_name: Some(pane_marker(operation_id, ordinal, pane_index)),
        start_suppressed: false,
    }
}

pub fn focus_pane_action(pane_id: u32) -> Action {
    Action::FocusTerminalPaneWithId {
        pane_id,
        should_float_if_hidden: false,
        should_be_in_place_if_hidden: false,
    }
}

pub fn close_pane_action(pane_id: u32) -> Action {
    Action::CloseTerminalPane { pane_id }
}

pub fn drag_press_action(plan: DragPlan) -> Action {
    Action::MouseEvent {
        event: MouseEvent::new_left_press_event(Position::new(
            plan.press_line as i32,
            plan.press_column as u16,
        )),
    }
}

pub fn drag_release_action(plan: DragPlan) -> Action {
    Action::MouseEvent {
        event: MouseEvent::new_left_release_event(Position::new(
            plan.release_line as i32,
            plan.release_column as u16,
        )),
    }
}

pub fn drag_cancel_action(plan: DragPlan) -> Action {
    Action::MouseEvent {
        event: MouseEvent::new_left_release_event(Position::new(
            plan.press_line as i32,
            plan.press_column as u16,
        )),
    }
}

pub fn target_is_supported(pane: &PaneInfo, direction: SplitDirection) -> bool {
    // `get_focused_pane_info()` is authoritative. PaneInfo::is_focused can be
    // false when queried by an unselectable status-bar plugin, even for the
    // terminal that Zellij just reported as focused.
    if pane.is_plugin
        || !pane.is_selectable
        || pane.is_floating
        || pane.is_suppressed
        || pane.is_fullscreen
    {
        return false;
    }

    match direction {
        SplitDirection::Right => pane.pane_columns >= MIN_SPLIT_SPAN,
        SplitDirection::Down => pane.pane_rows >= MIN_SPLIT_SPAN,
    }
}

/// Only the zellaude instance in the active tab may install or answer its
/// targeted keybind pipe. Missing state fails closed so two instances can
/// never split the same client concurrently.
pub fn is_active_instance(manifest: &PaneManifest, tabs: &[TabInfo], plugin_id: u32) -> bool {
    let plugin_tab = manifest.panes.iter().find_map(|(tab_position, panes)| {
        panes
            .iter()
            .any(|pane| pane.is_plugin && pane.id == plugin_id)
            .then_some(*tab_position)
    });
    let active_tab = tabs.iter().find(|tab| tab.active).map(|tab| tab.position);
    matches!((plugin_tab, active_tab), (Some(plugin), Some(active)) if plugin == active)
}

/// Plan the exact cell drag after the first native split. The first pane is
/// shrunk to one third and the focused second pane receives the remaining two
/// thirds; one more native split then halves that remainder.
pub fn plan_first_boundary_drag(
    original: PaneRect,
    first: &PaneInfo,
    second: &PaneInfo,
    direction: SplitDirection,
) -> Option<DragPlan> {
    let first_rect = PaneRect::from(first);
    let second_rect = PaneRect::from(second);
    if !two_panes_cover(original, first_rect, second_rect, direction) {
        return None;
    }

    let total_span = split_span(original, direction);
    let first_span = total_span.div_ceil(3);
    if first_span < 5 || total_span.saturating_sub(first_span) < 10 {
        return None;
    }

    match direction {
        SplitDirection::Right => {
            let line = original.y.checked_add(original.rows / 2)?;
            let target_boundary = first_rect.x.checked_add(first_span)?;
            if has_left_frame(second) {
                Some(DragPlan {
                    press_line: line,
                    press_column: second.pane_x,
                    release_line: line,
                    release_column: target_boundary,
                    first_span,
                })
            } else if has_right_frame(first) {
                Some(DragPlan {
                    press_line: line,
                    press_column: second.pane_x.checked_sub(1)?,
                    release_line: line,
                    release_column: target_boundary.checked_sub(1)?,
                    first_span,
                })
            } else {
                None
            }
        }
        SplitDirection::Down => {
            let column = original.x.checked_add(original.columns / 2)?;
            let target_boundary = first_rect.y.checked_add(first_span)?;
            if has_top_frame(second) {
                Some(DragPlan {
                    press_line: second.pane_y,
                    press_column: column,
                    release_line: target_boundary,
                    release_column: column,
                    first_span,
                })
            } else if has_bottom_frame(first) {
                Some(DragPlan {
                    press_line: second.pane_y.checked_sub(1)?,
                    press_column: column,
                    release_line: target_boundary.checked_sub(1)?,
                    release_column: column,
                    first_span,
                })
            } else {
                None
            }
        }
    }
}

pub fn ready_for_second_split(
    original: PaneRect,
    first: &PaneInfo,
    second: &PaneInfo,
    direction: SplitDirection,
    expected_first_span: usize,
) -> bool {
    let first_rect = PaneRect::from(first);
    let second_rect = PaneRect::from(second);
    two_panes_cover(original, first_rect, second_rect, direction)
        && split_span(first_rect, direction) == expected_first_span
        && split_span(second_rect, direction) >= 10
}

pub fn are_equal_thirds(
    original: PaneRect,
    first: &PaneInfo,
    second: &PaneInfo,
    third: &PaneInfo,
    direction: SplitDirection,
) -> bool {
    let panes = [
        PaneRect::from(first),
        PaneRect::from(second),
        PaneRect::from(third),
    ];
    if !three_panes_cover(original, panes, direction) {
        return false;
    }
    let spans = panes.map(|pane| split_span(pane, direction));
    spans
        .iter()
        .max()
        .unwrap_or(&0)
        .saturating_sub(*spans.iter().min().unwrap_or(&0))
        <= 1
}

/// Classify one atomic final-layout snapshot. A missing pane means the
/// manifest has not caught up yet; once all three panes are present, hidden or
/// non-tiled panes and malformed geometry are terminal failures.
pub fn final_geometry_status(
    original: PaneRect,
    first: Option<&PaneInfo>,
    second: Option<&PaneInfo>,
    third: Option<&PaneInfo>,
    direction: SplitDirection,
) -> FinalGeometryStatus {
    let (Some(first), Some(second), Some(third)) = (first, second, third) else {
        return FinalGeometryStatus::StillSettling;
    };

    if [first, second, third]
        .into_iter()
        .all(final_pane_is_supported)
        && are_equal_thirds(original, first, second, third, direction)
    {
        FinalGeometryStatus::Settled
    } else {
        FinalGeometryStatus::Invalid
    }
}

fn final_pane_is_supported(pane: &PaneInfo) -> bool {
    !pane.is_plugin
        && pane.is_selectable
        && !pane.is_floating
        && !pane.is_suppressed
        && !pane.is_fullscreen
}

/// Read all three rectangles from one manifest generation so validation never
/// combines pane data from different points in Zellij's relayout.
pub fn final_geometry_status_in_manifest(
    manifest: &PaneManifest,
    original: PaneRect,
    original_pane_id: u32,
    first_new_pane_id: u32,
    second_new_pane_id: u32,
    direction: SplitDirection,
) -> FinalGeometryStatus {
    let Some(panes) = manifest.panes.values().find(|panes| {
        panes
            .iter()
            .any(|pane| !pane.is_plugin && pane.id == original_pane_id)
    }) else {
        return FinalGeometryStatus::StillSettling;
    };
    let find_terminal = |pane_id| {
        panes
            .iter()
            .find(|pane| !pane.is_plugin && pane.id == pane_id)
    };

    final_geometry_status(
        original,
        find_terminal(original_pane_id),
        find_terminal(first_new_pane_id),
        find_terminal(second_new_pane_id),
        direction,
    )
}

pub fn pane_is_within(original: PaneRect, pane: &PaneInfo, direction: SplitDirection) -> bool {
    let pane = PaneRect::from(pane);
    let (Some(pane_right), Some(original_right), Some(pane_bottom), Some(original_bottom)) = (
        pane.x.checked_add(pane.columns),
        original.x.checked_add(original.columns),
        pane.y.checked_add(pane.rows),
        original.y.checked_add(original.rows),
    ) else {
        return false;
    };
    match direction {
        SplitDirection::Right => {
            pane.x >= original.x.saturating_sub(MAX_LAYOUT_ROUNDING_DRIFT)
                && pane_right <= original_right.saturating_add(MAX_LAYOUT_ROUNDING_DRIFT)
                && pane.y >= original.y
                && pane_bottom <= original_bottom
        }
        SplitDirection::Down => {
            pane.x >= original.x
                && pane_right <= original_right
                && pane.y >= original.y.saturating_sub(MAX_LAYOUT_ROUNDING_DRIFT)
                && pane_bottom <= original_bottom.saturating_add(MAX_LAYOUT_ROUNDING_DRIFT)
        }
    }
}

fn split_span(rect: PaneRect, direction: SplitDirection) -> usize {
    match direction {
        SplitDirection::Right => rect.columns,
        SplitDirection::Down => rect.rows,
    }
}

fn two_panes_cover(
    original: PaneRect,
    first: PaneRect,
    second: PaneRect,
    direction: SplitDirection,
) -> bool {
    match direction {
        SplitDirection::Right => {
            let Some(second_end) = second.x.checked_add(second.columns) else {
                return false;
            };
            let Some(original_end) = original.x.checked_add(original.columns) else {
                return false;
            };
            let Some(combined_span) = second_end.checked_sub(first.x) else {
                return false;
            };
            within_layout_rounding(first.x, original.x)
                && within_layout_rounding(second_end, original_end)
                && within_layout_rounding(combined_span, original.columns)
                && first.y == original.y
                && second.y == original.y
                && first.rows == original.rows
                && second.rows == original.rows
                && first.x.checked_add(first.columns) == Some(second.x)
        }
        SplitDirection::Down => {
            let Some(second_end) = second.y.checked_add(second.rows) else {
                return false;
            };
            let Some(original_end) = original.y.checked_add(original.rows) else {
                return false;
            };
            let Some(combined_span) = second_end.checked_sub(first.y) else {
                return false;
            };
            first.x == original.x
                && within_layout_rounding(first.y, original.y)
                && within_layout_rounding(second_end, original_end)
                && within_layout_rounding(combined_span, original.rows)
                && second.x == original.x
                && first.columns == original.columns
                && second.columns == original.columns
                && first.y.checked_add(first.rows) == Some(second.y)
        }
    }
}

fn three_panes_cover(original: PaneRect, panes: [PaneRect; 3], direction: SplitDirection) -> bool {
    let [first, second, third] = panes;
    match direction {
        SplitDirection::Right => {
            let Some(third_end) = third.x.checked_add(third.columns) else {
                return false;
            };
            let Some(original_end) = original.x.checked_add(original.columns) else {
                return false;
            };
            let Some(combined_span) = third_end.checked_sub(first.x) else {
                return false;
            };
            within_layout_rounding(first.x, original.x)
                && within_layout_rounding(third_end, original_end)
                && within_layout_rounding(combined_span, original.columns)
                && first.y == original.y
                && second.y == original.y
                && third.y == original.y
                && first.rows == original.rows
                && second.rows == original.rows
                && third.rows == original.rows
                && first.x.checked_add(first.columns) == Some(second.x)
                && second.x.checked_add(second.columns) == Some(third.x)
        }
        SplitDirection::Down => {
            let Some(third_end) = third.y.checked_add(third.rows) else {
                return false;
            };
            let Some(original_end) = original.y.checked_add(original.rows) else {
                return false;
            };
            let Some(combined_span) = third_end.checked_sub(first.y) else {
                return false;
            };
            first.x == original.x
                && within_layout_rounding(first.y, original.y)
                && within_layout_rounding(third_end, original_end)
                && within_layout_rounding(combined_span, original.rows)
                && second.x == original.x
                && third.x == original.x
                && first.columns == original.columns
                && second.columns == original.columns
                && third.columns == original.columns
                && first.y.checked_add(first.rows) == Some(second.y)
                && second.y.checked_add(second.rows) == Some(third.y)
        }
    }
}

fn within_layout_rounding(actual: usize, expected: usize) -> bool {
    actual.abs_diff(expected) <= MAX_LAYOUT_ROUNDING_DRIFT
}

fn has_left_frame(pane: &PaneInfo) -> bool {
    pane.pane_content_x > pane.pane_x
}

fn has_right_frame(pane: &PaneInfo) -> bool {
    pane.pane_content_x
        .checked_add(pane.pane_content_columns)
        .is_some_and(|end| end < pane.pane_x.saturating_add(pane.pane_columns))
}

fn has_top_frame(pane: &PaneInfo) -> bool {
    pane.pane_content_y > pane.pane_y
}

fn has_bottom_frame(pane: &PaneInfo) -> bool {
    pane.pane_content_y
        .checked_add(pane.pane_content_rows)
        .is_some_and(|end| end < pane.pane_y.saturating_add(pane.pane_rows))
}
