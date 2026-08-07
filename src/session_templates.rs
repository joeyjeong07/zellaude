use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::custom_layouts::{kdl_string, MAX_COMMAND_BYTES, MAX_PANES, MAX_TOTAL_COMMAND_BYTES};

pub const MAX_NAME_CHARACTERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTemplate {
    pub name: String,
    #[serde(default)]
    pub cwd: Option<String>,
    pub tabs: Vec<TemplateTab>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateTab {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub width: Option<usize>,
    #[serde(default)]
    pub height: Option<usize>,
    #[serde(default)]
    pub focus: bool,
}

/// A template name becomes a filename and an argument to `zellij -n`, so it is
/// held to far stricter rules than a custom-state id, which is neither.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("session template name must not be empty".to_string());
    }
    if name.chars().count() > MAX_NAME_CHARACTERS {
        return Err(format!(
            "session template name {name:?} exceeds {MAX_NAME_CHARACTERS} characters"
        ));
    }
    if name == "." || name == ".." {
        return Err(format!("session template name {name:?} is reserved"));
    }
    if name.starts_with('-') {
        return Err(format!(
            "session template name {name:?} must not start with '-'; zellij would read it as a flag"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!(
            "session template name {name:?} may only contain letters, digits, '.', '_' and '-'"
        ));
    }
    Ok(())
}

impl TemplateTab {
    /// Columns × rows. A tab with no commands has no grid; one with commands
    /// and no explicit dimensions lays them out in a single row.
    pub fn grid(&self) -> (usize, usize) {
        if self.commands.is_empty() {
            return (0, 0);
        }
        match (self.width, self.height) {
            (Some(width), Some(height)) => (width, height),
            (Some(width), None) => (width, self.commands.len().div_ceil(width.max(1))),
            (None, Some(height)) => (self.commands.len().div_ceil(height.max(1)), height),
            (None, None) => (self.commands.len(), 1),
        }
    }

    fn validate(&self, template: &str, index: usize) -> Result<usize, String> {
        let position = index + 1;
        if self.commands.is_empty() {
            if self.width.is_some() || self.height.is_some() {
                return Err(format!(
                    "tab {position} in session template {template:?} sets width or height without commands"
                ));
            }
            return Ok(0);
        }
        let (width, height) = self.grid();
        if width == 0 || height == 0 {
            return Err(format!(
                "tab {position} in session template {template:?} must have non-zero width and height"
            ));
        }
        let cells = width.checked_mul(height).ok_or_else(|| {
            format!("tab {position} in session template {template:?} has dimensions that are too large")
        })?;
        if cells > MAX_PANES {
            return Err(format!(
                "tab {position} in session template {template:?} requests {cells} panes; the maximum is {MAX_PANES}"
            ));
        }
        if self.commands.len() > cells {
            return Err(format!(
                "tab {position} in session template {template:?} is {width}x{height} and has room for {cells} commands, but has {}",
                self.commands.len()
            ));
        }
        let mut bytes = 0usize;
        for (command_index, command) in self.commands.iter().enumerate() {
            if command.contains('\0') {
                return Err(format!(
                    "command {} in tab {position} of session template {template:?} contains a NUL byte",
                    command_index + 1
                ));
            }
            if command.len() > MAX_COMMAND_BYTES {
                return Err(format!(
                    "command {} in tab {position} of session template {template:?} exceeds {MAX_COMMAND_BYTES} bytes",
                    command_index + 1
                ));
            }
            bytes = bytes.saturating_add(command.len());
        }
        Ok(bytes)
    }
}

impl SessionTemplate {
    pub fn validate(&self) -> Result<(), String> {
        validate_name(&self.name)?;
        if self.tabs.is_empty() {
            return Err(format!(
                "session template {:?} must contain at least one tab",
                self.name
            ));
        }
        let focused_tabs = self.tabs.iter().filter(|tab| tab.focus).count();
        if focused_tabs > 1 {
            return Err(format!(
                "session template {:?} marks {focused_tabs} tabs as focus; at most one is allowed",
                self.name
            ));
        }
        let mut bytes = 0usize;
        for (index, tab) in self.tabs.iter().enumerate() {
            bytes = bytes.saturating_add(tab.validate(&self.name, index)?);
        }
        if bytes > MAX_TOTAL_COMMAND_BYTES {
            return Err(format!(
                "commands in session template {:?} exceed {MAX_TOTAL_COMMAND_BYTES} bytes in total",
                self.name
            ));
        }
        Ok(())
    }
}

/// Read the `session_templates` key out of the settings document. Absent key
/// yields `Ok(None)`, which is how the caller knows to keep only the built-in.
pub fn parse_config_document(raw: &str) -> Result<Option<Vec<SessionTemplate>>, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let Value::Object(object) = value else {
        return Ok(None);
    };
    let Some(configured) = object.get("session_templates") else {
        return Ok(None);
    };
    let templates: Vec<SessionTemplate> =
        serde_json::from_value(configured.clone()).map_err(|error| error.to_string())?;

    let mut names = BTreeSet::new();
    for template in &templates {
        template.validate()?;
        if !names.insert(template.name.clone()) {
            return Err(format!("duplicate session template name {:?}", template.name));
        }
    }
    Ok(Some(templates))
}

pub const BUILT_IN_NAME: &str = "zellaude";
const MARKER_PREFIX: &str = "// zellaude-generated";

/// First line of every generated file. The host script writes only over files
/// that already begin with `MARKER_PREFIX`, so a hand-written layout that
/// happens to share a name survives untouched.
pub fn marker(name: &str) -> String {
    format!("{MARKER_PREFIX} v{} {name}", env!("CARGO_PKG_VERSION"))
}

/// The four-tab session Zellaude ships with. Always compiled, so a fresh
/// install has something to start; a user template of the same name replaces
/// it wholesale rather than merging.
pub fn built_in() -> SessionTemplate {
    let grid = |name: &str, commands: &[&str]| TemplateTab {
        name: Some(name.to_string()),
        cwd: None,
        commands: commands.iter().map(|c| c.to_string()).collect(),
        width: None,
        height: None,
        focus: false,
    };
    SessionTemplate {
        name: BUILT_IN_NAME.to_string(),
        cwd: None,
        tabs: vec![
            grid("git", &["lazygit", "btop"]),
            grid("claude", &["claude"]),
            grid("editor", &["nvim"]),
            grid("shell", &[]),
        ],
    }
}

/// The built-in first, then whatever the user configured, with a user template
/// named `zellaude` displacing the built-in.
pub fn effective(configured: Option<Vec<SessionTemplate>>) -> Vec<SessionTemplate> {
    let configured = configured.unwrap_or_default();
    let mut templates = Vec::with_capacity(configured.len() + 1);
    if !configured.iter().any(|t| t.name == BUILT_IN_NAME) {
        templates.push(built_in());
    }
    templates.extend(configured);
    templates
}

/// Zellij resolves a layout's relative directories against the directory the
/// session was started from, which is the behaviour an omitted `cwd` relies on.
/// It does not expand `~`, so that is done here.
fn expand_home(path: &str, home: &str) -> String {
    if path == "~" {
        home.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        path.to_string()
    }
}

impl SessionTemplate {
    pub fn to_kdl(
        &self,
        plugin_location: &str,
        plugin_configuration: &BTreeMap<String, String>,
        home: &str,
    ) -> Result<String, String> {
        self.validate()?;
        if plugin_location.is_empty() {
            return Err("Zellaude plugin location is unavailable".to_string());
        }

        let mut kdl = String::new();
        let _ = writeln!(kdl, "{}", marker(&self.name));
        kdl.push_str("layout {\n");
        kdl.push_str("    default_tab_template {\n");
        kdl.push_str("        pane size=1 borderless=true {\n");
        let _ = writeln!(
            kdl,
            "            plugin location={} {{",
            kdl_string(plugin_location)
        );
        for (key, value) in plugin_configuration {
            let _ = writeln!(
                kdl,
                "                {} {}",
                kdl_string(key),
                kdl_string(value)
            );
        }
        kdl.push_str("            }\n        }\n        children\n    }\n");

        let mut focus_used = false;
        for tab in &self.tabs {
            kdl.push_str("    tab");
            if let Some(name) = &tab.name {
                let _ = write!(kdl, " name={}", kdl_string(name));
            }
            if let Some(cwd) = tab.cwd.as_deref().or(self.cwd.as_deref()) {
                let _ = write!(kdl, " cwd={}", kdl_string(&expand_home(cwd, home)));
            }
            if tab.focus && !focus_used {
                focus_used = true;
                kdl.push_str(" focus=true");
            }
            if tab.commands.is_empty() {
                kdl.push('\n');
                continue;
            }
            kdl.push_str(" {\n");
            kdl.push_str("        pane split_direction=\"vertical\" {\n");
            let (width, height) = tab.grid();
            for column in 0..width {
                kdl.push_str("            pane split_direction=\"horizontal\" {\n");
                for row in 0..height {
                    match tab.commands.get(row * width + column) {
                        Some(command) => {
                            let _ = writeln!(
                                kdl,
                                "                pane command=\"sh\" {{\n                    args \"-lc\" {}\n                }}",
                                kdl_string(command)
                            );
                        }
                        // Keep the grid rectangular for non-factor counts; the
                        // spare cells land bottom-right because the command
                        // index follows visual reading order.
                        None => kdl.push_str("                pane\n"),
                    }
                }
                kdl.push_str("            }\n");
            }
            kdl.push_str("        }\n    }\n");
        }
        kdl.push_str("}\n");
        Ok(kdl)
    }
}
