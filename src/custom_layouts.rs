use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;

pub const PIPE_NAME: &str = "zellaude:custom-layout";
pub const CUSTOM_LAYOUT_KEY: char = 'n';
pub const MAX_ID_CHARACTERS: usize = 128;
pub const MAX_PANES: usize = 64;
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
pub const MAX_TOTAL_COMMAND_BYTES: usize = 1024 * 1024;
pub const FOCUS_ACQUISITION_TIMEOUT_MS: u64 = 2_000;

/// Runtime-only Tab-mode binding installed by the plugin.
pub type TabBinding = (InputMode, KeyWithModifier, Vec<Action>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomLayout {
    pub id: String,
    #[serde(deserialize_with = "deserialize_dimension")]
    pub width: usize,
    #[serde(deserialize_with = "deserialize_dimension")]
    pub height: usize,
    pub commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Dimension {
    Number(usize),
    String(String),
}

fn deserialize_dimension<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    match Dimension::deserialize(deserializer)? {
        Dimension::Number(value) => Ok(value),
        Dimension::String(value) => value
            .parse::<usize>()
            .map_err(|_| de::Error::custom("dimension must be a positive integer")),
    }
}

impl CustomLayout {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("custom state id must not be empty".to_string());
        }
        if self.id.trim() != self.id {
            return Err(format!(
                "custom state id {:?} must not start or end with whitespace",
                self.id
            ));
        }
        if self.id.chars().count() > MAX_ID_CHARACTERS {
            return Err(format!(
                "custom state id {:?} exceeds {MAX_ID_CHARACTERS} characters",
                self.id
            ));
        }
        if self.id.chars().any(char::is_control) {
            return Err(format!(
                "custom state id {:?} must not contain control characters",
                self.id
            ));
        }
        if self.width == 0 || self.height == 0 {
            return Err(format!(
                "custom state {:?} must have non-zero width and height",
                self.id
            ));
        }
        let expected_commands = self.width.checked_mul(self.height).ok_or_else(|| {
            format!(
                "custom state {:?} has dimensions that are too large",
                self.id
            )
        })?;
        if expected_commands > MAX_PANES {
            return Err(format!(
                "custom state {:?} requests {} panes; the maximum is {MAX_PANES}",
                self.id, expected_commands
            ));
        }
        if self.commands.is_empty() {
            return Err(format!(
                "custom state {:?} must contain at least one command",
                self.id
            ));
        }
        if self.commands.len() > expected_commands {
            return Err(format!(
                "custom state {:?} is {}x{} and has room for {} commands, but has {}",
                self.id,
                self.width,
                self.height,
                expected_commands,
                self.commands.len()
            ));
        }
        let mut total_command_bytes = 0usize;
        for (index, command) in self.commands.iter().enumerate() {
            if command.contains('\0') {
                return Err(format!(
                    "command {} in custom state {:?} contains a NUL byte",
                    index + 1,
                    self.id
                ));
            }
            if command.len() > MAX_COMMAND_BYTES {
                return Err(format!(
                    "command {} in custom state {:?} exceeds {MAX_COMMAND_BYTES} bytes",
                    index + 1,
                    self.id
                ));
            }
            total_command_bytes = total_command_bytes.saturating_add(command.len());
        }
        if total_command_bytes > MAX_TOTAL_COMMAND_BYTES {
            return Err(format!(
                "commands in custom state {:?} exceed {MAX_TOTAL_COMMAND_BYTES} bytes in total",
                self.id
            ));
        }
        Ok(())
    }

    /// Build a complete tab with the source tab's bars and the configured
    /// command grid. Explicit plugin-created layouts do not inherit the
    /// session's default tab template, so every bar the new tab should keep --
    /// Zellaude's own and Zellij's status bar below the grid -- has to be
    /// written into the layout, along with the caller's plugin URL and
    /// complete configuration.
    ///
    /// The command array is mapped left-to-right, top-to-bottom. Columns are
    /// the first-level grid children and each column owns its rows, producing
    /// equal column widths while `row * width + column` assigns each command
    /// to its visual reading-order position.
    pub fn to_kdl(
        &self,
        plugin_location: &str,
        plugin_configuration: &BTreeMap<String, String>,
        cwd: Option<&str>,
        chrome: &TabChrome,
    ) -> Result<String, String> {
        self.validate()?;
        if plugin_location.is_empty() {
            return Err("Zellaude plugin location is unavailable".to_string());
        }

        // The pane manifest may not describe this instance yet, and a tab
        // without the Zellaude bar is not worth generating.
        let mut top = chrome.top.clone();
        if !top.iter().any(|location| location == plugin_location) {
            top.insert(0, plugin_location.to_string());
        }

        let mut kdl = String::from("layout {\n");
        let _ = write!(kdl, "    tab name={} focus=true", kdl_string(&self.id));
        if let Some(cwd) = cwd {
            let _ = write!(kdl, " cwd={}", kdl_string(cwd));
        }
        kdl.push_str(" {\n");
        for location in &top {
            write_bar(&mut kdl, location, plugin_location, plugin_configuration);
        }
        kdl.push_str("        pane split_direction=\"vertical\" {\n");

        for column in 0..self.width {
            kdl.push_str("            pane split_direction=\"horizontal\" {\n");
            for row in 0..self.height {
                let command_index = row * self.width + column;
                if let Some(command) = self.commands.get(command_index) {
                    let focus = if command_index == 0 {
                        " focus=true"
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        kdl,
                        "                pane command=\"sh\"{focus} {{\n                    args \"-lc\" {}\n                }}",
                        kdl_string(command)
                    );
                } else {
                    // Keep a rectangular grid for balanced non-factor counts
                    // (eg. seven commands in a 4x2 grid). Unfilled cells are
                    // ordinary shell panes and naturally land at bottom-right
                    // because command indices use visual reading order.
                    kdl.push_str("                pane\n");
                }
            }
            kdl.push_str("            }\n");
        }
        kdl.push_str("        }\n");
        for location in &chrome.bottom {
            write_bar(&mut kdl, location, plugin_location, plugin_configuration);
        }
        kdl.push_str("    }\n}\n");
        Ok(kdl)
    }
}

/// The one-row plugin panes that frame a tab: Zellaude's bar above the
/// content, Zellij's status bar below it, and anything else the session's tab
/// template puts there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabChrome {
    pub top: Vec<String>,
    pub bottom: Vec<String>,
}

/// Read the bars of the tab a custom state is opened from, so the generated
/// tab can repeat them.
///
/// A bar is a plugin pane one row tall that spans the tab. Width is what makes
/// the test reliable: a collapsed member of a stack is also one row tall, so
/// height alone would pull a plugin pane sitting inside the grid into the
/// chrome. Which side a bar belongs to follows from the content it frames.
pub fn tab_chrome(panes: &[PaneInfo]) -> TabChrome {
    let tiled = || {
        panes
            .iter()
            .filter(|pane| !pane.is_floating && !pane.is_suppressed)
    };
    let width = tiled().map(|pane| pane.pane_columns).max().unwrap_or(0);
    let is_bar = |pane: &PaneInfo| {
        width > 0 && pane.is_plugin && pane.pane_rows == 1 && pane.pane_columns == width
    };
    // A tab that is nothing but bars has no content to place them around, so
    // every bar keeps its position above the grid.
    let content_top = tiled()
        .filter(|pane| !is_bar(pane))
        .map(|pane| pane.pane_y)
        .min();

    let mut bars: Vec<&PaneInfo> = tiled().filter(|pane| is_bar(pane)).collect();
    bars.sort_by_key(|pane| pane.pane_y);
    let mut chrome = TabChrome::default();
    for bar in bars {
        let Some(location) = bar.plugin_url.clone().filter(|url| !url.is_empty()) else {
            continue;
        };
        match content_top {
            Some(content_top) if bar.pane_y > content_top => chrome.bottom.push(location),
            _ => chrome.top.push(location),
        }
    }
    chrome
}

/// Write one borderless one-row bar pane. Only Zellaude's own bar carries the
/// plugin configuration; the other bars are identified by location alone.
fn write_bar(
    kdl: &mut String,
    location: &str,
    plugin_location: &str,
    plugin_configuration: &BTreeMap<String, String>,
) {
    kdl.push_str("        pane size=1 borderless=true {\n");
    let _ = writeln!(
        kdl,
        "            plugin location={} {{",
        kdl_string(location)
    );
    if location == plugin_location {
        for (key, value) in plugin_configuration {
            let _ = writeln!(
                kdl,
                "                {} {}",
                kdl_string(key),
                kdl_string(value)
            );
        }
    }
    kdl.push_str("            }\n        }\n");
}

pub fn kdl_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' | '"' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            character if character.is_control() => {
                let _ = write!(escaped, "\\u{{{:x}}}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

/// Parse the custom-state portion of zellaude.json. For convenience, the file
/// may be a single state, an array of states, or the regular settings object
/// with a `custom_states` (also `custom_layouts`) property.
pub fn parse_config_document(raw: &str) -> Result<Option<Vec<CustomLayout>>, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    match &value {
        Value::Array(_) => parse_layout_value(value).map(Some),
        Value::Object(object) if object.contains_key("id") => parse_layout_value(value).map(Some),
        Value::Object(object) => {
            let configured = object
                .get("custom_states")
                .or_else(|| object.get("custom_layouts"))
                .or_else(|| object.get("custom_state"));
            configured.cloned().map(parse_layout_value).transpose()
        }
        _ => Err("zellaude configuration must be a JSON object or array".to_string()),
    }
}

pub fn parse_plugin_configuration(
    configuration: &BTreeMap<String, String>,
) -> Result<Option<Vec<CustomLayout>>, String> {
    configuration
        .get("custom_states")
        .or_else(|| configuration.get("custom_layouts"))
        .or_else(|| configuration.get("custom_state"))
        .map(|raw| {
            let value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
            parse_layout_value(value)
        })
        .transpose()
}

pub fn has_plugin_configuration(configuration: &BTreeMap<String, String>) -> bool {
    ["custom_states", "custom_layouts", "custom_state"]
        .iter()
        .any(|key| configuration.contains_key(*key))
}

fn parse_layout_value(value: Value) -> Result<Vec<CustomLayout>, String> {
    let layouts = match value {
        Value::Array(_) => serde_json::from_value::<Vec<CustomLayout>>(value),
        Value::Object(_) => {
            serde_json::from_value::<CustomLayout>(value).map(|layout| vec![layout])
        }
        _ => return Err("custom_states must be a state object or an array of states".to_string()),
    }
    .map_err(|error| error.to_string())?;

    let mut ids = BTreeSet::new();
    for layout in &layouts {
        layout.validate()?;
        if !ids.insert(layout.id.clone()) {
            return Err(format!("duplicate custom state id {:?}", layout.id));
        }
    }
    Ok(layouts)
}

pub fn index(layouts: Vec<CustomLayout>) -> BTreeMap<String, CustomLayout> {
    layouts
        .into_iter()
        .map(|layout| (layout.id.clone(), layout))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub input: String,
    /// Character index rather than byte index, so Unicode IDs edit safely.
    pub cursor: usize,
    pub error: Option<String>,
    pub return_pane_id: u32,
    pub cwd: Option<String>,
    focus_acquired: bool,
    focus_requested_ms: Option<u64>,
}

impl Prompt {
    pub fn new(return_pane_id: u32, cwd: Option<String>) -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            error: None,
            return_pane_id,
            cwd,
            focus_acquired: false,
            focus_requested_ms: None,
        }
    }

    pub fn begin_focus_acquisition(&mut self, now_ms: u64) {
        self.focus_requested_ms = Some(now_ms);
    }

    pub fn note_input(&mut self) {
        self.focus_acquired = true;
    }

    /// Returns true once a prompt that had focus has subsequently lost it, or
    /// when the host did not honor its focus request within the acquisition
    /// timeout. Early false observations are ignored because making the status
    /// bar selectable and focusing it can produce separate pane updates.
    pub fn observe_focus(&mut self, is_focused: bool, now_ms: u64) -> bool {
        if is_focused {
            self.focus_acquired = true;
            false
        } else if self.focus_acquired {
            true
        } else {
            self.focus_requested_ms.is_some_and(|requested_ms| {
                now_ms.saturating_sub(requested_ms) >= FOCUS_ACQUISITION_TIMEOUT_MS
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKey {
    Updated,
    Submit(String),
    Cancel,
    Ignored,
}

pub fn handle_prompt_key(prompt: &mut Prompt, key: &KeyWithModifier) -> PromptKey {
    if key.bare_key == BareKey::Esc
        || (key.bare_key == BareKey::Char('c')
            && key.key_modifiers.len() == 1
            && key.key_modifiers.contains(&KeyModifier::Ctrl))
    {
        return PromptKey::Cancel;
    }
    if key.bare_key == BareKey::Enter && key.key_modifiers.is_empty() {
        return PromptKey::Submit(prompt.input.trim().to_string());
    }
    if key.key_modifiers.is_empty() {
        match key.bare_key {
            BareKey::Left => {
                prompt.cursor = prompt.cursor.saturating_sub(1);
                return PromptKey::Updated;
            }
            BareKey::Right => {
                prompt.cursor = (prompt.cursor + 1).min(prompt.input.chars().count());
                return PromptKey::Updated;
            }
            BareKey::Home => {
                prompt.cursor = 0;
                return PromptKey::Updated;
            }
            BareKey::End => {
                prompt.cursor = prompt.input.chars().count();
                return PromptKey::Updated;
            }
            BareKey::Backspace if prompt.cursor > 0 => {
                let remove_at = prompt.cursor - 1;
                prompt.input.remove(byte_index(&prompt.input, remove_at));
                prompt.cursor = remove_at;
                prompt.error = None;
                return PromptKey::Updated;
            }
            BareKey::Delete if prompt.cursor < prompt.input.chars().count() => {
                prompt
                    .input
                    .remove(byte_index(&prompt.input, prompt.cursor));
                prompt.error = None;
                return PromptKey::Updated;
            }
            BareKey::Backspace | BareKey::Delete => return PromptKey::Updated,
            _ => {}
        }
    }

    let BareKey::Char(mut character) = key.bare_key else {
        return PromptKey::Ignored;
    };
    if key
        .key_modifiers
        .iter()
        .any(|modifier| *modifier != KeyModifier::Shift)
    {
        return PromptKey::Ignored;
    }
    if prompt.input.chars().count() >= MAX_ID_CHARACTERS {
        return PromptKey::Ignored;
    }
    if key.key_modifiers.contains(&KeyModifier::Shift) && character.is_ascii_lowercase() {
        character = character.to_ascii_uppercase();
    }
    if character.is_control() {
        return PromptKey::Ignored;
    }

    prompt
        .input
        .insert(byte_index(&prompt.input, prompt.cursor), character);
    prompt.cursor += 1;
    prompt.error = None;
    PromptKey::Updated
}

pub fn handle_paste(prompt: &mut Prompt, pasted: &str) -> bool {
    let room = MAX_ID_CHARACTERS.saturating_sub(prompt.input.chars().count());
    let insert: String = pasted
        .chars()
        .filter(|character| !character.is_control())
        .take(room)
        .collect();
    if insert.is_empty() {
        return false;
    }
    let inserted_characters = insert.chars().count();
    prompt
        .input
        .insert_str(byte_index(&prompt.input, prompt.cursor), &insert);
    prompt.cursor += inserted_characters;
    prompt.error = None;
    true
}

fn byte_index(value: &str, character_index: usize) -> usize {
    value
        .char_indices()
        .nth(character_index)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(value.len())
}

pub fn bindings(plugin_id: u32) -> Vec<TabBinding> {
    vec![binding(plugin_id)]
}

/// Return the shortcut when it is unused or wholly owned by Zellaude. Even an
/// apparently exact target is refreshed: Zellij can deliver a stale keybind
/// snapshot after another tab has already retargeted the live client binding.
/// A user binding always wins.
pub fn available_bindings(existing: &KeybindsVec, plugin_id: u32) -> Vec<TabBinding> {
    let desired = bindings(plugin_id)
        .into_iter()
        .next()
        .expect("custom layout always has one binding");
    let matches: Vec<&Vec<Action>> = existing
        .iter()
        .filter(|(mode, _)| *mode == InputMode::Tab)
        .flat_map(|(_, bindings)| bindings)
        .filter_map(|(key, actions)| (key == &desired.1).then_some(actions))
        .collect();
    if matches.is_empty() || matches.iter().all(|actions| is_zellaude_binding(actions)) {
        vec![desired]
    } else {
        vec![]
    }
}

pub fn install(existing: &KeybindsVec, plugin_id: u32) {
    let available = available_bindings(existing, plugin_id);
    if !available.is_empty() {
        reconfigure(reconfigure_snippet(&available, plugin_id), false);
    }
}

pub fn reconfigure_snippet(bindings: &[TabBinding], plugin_id: u32) -> String {
    if bindings.is_empty() {
        return String::new();
    }
    format!(
        "keybinds {{\n    tab {{\n        bind \"N\" {{\n            MessagePluginId {plugin_id} {{\n                name \"{PIPE_NAME}\"\n                payload \"prompt\"\n            }}\n            SwitchToMode \"Normal\"\n            SwitchToMode \"Normal\"\n            SwitchToMode \"Normal\"\n        }}\n    }}\n}}\n"
    )
}

fn binding(plugin_id: u32) -> TabBinding {
    (
        InputMode::Tab,
        KeyWithModifier::new(BareKey::Char(CUSTOM_LAYOUT_KEY)).with_shift_modifier(),
        vec![
            Action::KeybindPipe {
                name: Some(PIPE_NAME.to_string()),
                payload: Some("prompt".to_string()),
                args: None,
                plugin: None,
                plugin_id: Some(plugin_id),
                configuration: None,
                launch_new: false,
                skip_cache: false,
                floating: None,
                in_place: None,
                cwd: None,
                pane_title: None,
            },
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

fn is_zellaude_binding(actions: &[Action]) -> bool {
    match actions {
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
            (name.as_deref() == Some(PIPE_NAME) && payload.as_deref() == Some("prompt"))
                || (name.is_none() && payload.is_none())
        }
        _ => false,
    }
}
