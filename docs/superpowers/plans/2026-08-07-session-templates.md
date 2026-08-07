# Session Templates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user describe a multi-tab Zellij session in `zellaude.json` and start it with `zellij -s work -n <name>`.

**Architecture:** A new module `src/session_templates.rs` parses `session_templates` from the settings file, validates it, and compiles each template to a Zellij layout KDL document. The status bar writes those documents to `~/.config/zellij/layouts/<name>.kdl` by shelling out, once per plugin instance, after `load_config` succeeds and the plugin's own URL is known. Zellij then reads them natively — no new binary, no shell wrapper.

**Tech Stack:** Rust 2021 targeting `wasm32-wasip1`, `zellij-tile` 0.44.3, `serde`/`serde_json`, POSIX `sh` for host-side file writes. Tests are plain `cargo test` on the host target and parse generated KDL back through `zellij_utils::input::layout::Layout`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-07-session-templates-design.md`
- Template `name` charset: `[A-Za-z0-9._-]`, 1–64 characters, never `.` or `..`, never a leading `-`
- Ownership marker, first line of every generated file: `// zellaude-generated v<CARGO_PKG_VERSION> <name>`
- A layout file whose first line lacks `// zellaude-generated ` is never written and never deleted
- Grid limits reuse `custom_layouts`: at most `MAX_PANES` (64) panes per tab, `commands.len() <= width * height`, `MAX_COMMAND_BYTES` (64 KiB) per command, `MAX_TOTAL_COMMAND_BYTES` (1 MiB) per template
- Commands run through `sh -lc`, matching custom states
- `~` in any `cwd` is expanded to `$HOME` at compile time; relative and absolute paths are emitted unchanged
- Built-in template name is `zellaude` — never `default`, which is Zellij's own file
- Writes are atomic: `mktemp` in the destination directory then `mv`
- Failures are reported with `eprintln!`, not on the bar
- Every commit message uses the repo's existing style (`feat:`, `fix:`, `docs:`, `test:`); no Claude attribution trailer
- Run `cargo test` (host target) for tests; `cargo build --release` targets wasm via `.cargo/config.toml`

## File Structure

| File | Responsibility |
|------|----------------|
| `src/session_templates.rs` (create) | Schema, validation, name safety, KDL generation, built-in default, host script text |
| `src/state.rs` (modify) | Three new `State` fields |
| `src/main.rs` (modify) | Wire parsing into `load_config`, trigger compilation, handle its `RunCommandResult` |
| `tests/session_templates.rs` (create) | All unit tests for the new module, plus real-`sh` tests of the host scripts |
| `README.md` (modify) | User documentation |

---

### Task 1: Schema and validation

Parse `session_templates` and reject everything unsafe before any file is touched.

**Files:**
- Create: `src/session_templates.rs`
- Modify: `src/main.rs` (add `mod session_templates;` next to the other module declarations near the top)
- Test: `tests/session_templates.rs`

**Interfaces:**
- Consumes: `custom_layouts::{MAX_PANES, MAX_COMMAND_BYTES, MAX_TOTAL_COMMAND_BYTES}` — already `pub`
- Produces:
  - `pub struct SessionTemplate { pub name: String, pub cwd: Option<String>, pub tabs: Vec<TemplateTab> }`
  - `pub struct TemplateTab { pub name: Option<String>, pub cwd: Option<String>, pub commands: Vec<String>, pub width: Option<usize>, pub height: Option<usize>, pub focus: bool }`
  - `pub fn validate_name(name: &str) -> Result<(), String>`
  - `impl SessionTemplate { pub fn validate(&self) -> Result<(), String> }`
  - `impl TemplateTab { pub fn grid(&self) -> (usize, usize) }`
  - `pub fn parse_config_document(raw: &str) -> Result<Option<Vec<SessionTemplate>>, String>`

- [ ] **Step 1: Write the failing test**

Create `tests/session_templates.rs`:

```rust
#![allow(dead_code)]

#[path = "../src/custom_layouts.rs"]
mod custom_layouts;
#[path = "../src/session_templates.rs"]
mod session_templates;

use session_templates::{parse_config_document, validate_name, SessionTemplate, TemplateTab};

fn tab(commands: &[&str]) -> TemplateTab {
    TemplateTab {
        name: None,
        cwd: None,
        commands: commands.iter().map(|c| c.to_string()).collect(),
        width: None,
        height: None,
        focus: false,
    }
}

fn template(name: &str, tabs: Vec<TemplateTab>) -> SessionTemplate {
    SessionTemplate { name: name.to_string(), cwd: None, tabs }
}

#[test]
fn names_that_could_escape_the_layout_directory_are_rejected() {
    for name in ["work", "my-setup", "a.b_c", "X9"] {
        assert!(validate_name(name).is_ok(), "{name} should be accepted");
    }
    for name in ["", ".", "..", "-x", "a/b", "a\\b", "a b", "héllo", &"x".repeat(65)] {
        assert!(validate_name(name).is_err(), "{name:?} should be rejected");
    }
}

#[test]
fn a_tab_grid_defaults_to_one_row_and_rejects_dimensions_without_commands() {
    assert_eq!(tab(&["a", "b", "c"]).grid(), (3, 1));
    assert_eq!(tab(&[]).grid(), (0, 0));

    let mut sized = tab(&[]);
    sized.width = Some(2);
    let error = template("t", vec![sized]).validate().unwrap_err();
    assert!(error.contains("without commands"), "{error}");

    let mut grid = tab(&["a", "b", "c"]);
    grid.width = Some(2);
    grid.height = Some(2);
    assert_eq!(grid.grid(), (2, 2));
    assert!(template("t", vec![grid]).validate().is_ok());
}

#[test]
fn validation_rejects_empty_tabs_oversized_grids_and_command_overflow() {
    let error = template("t", vec![]).validate().unwrap_err();
    assert!(error.contains("at least one tab"), "{error}");

    let mut overflow = tab(&["a", "b", "c"]);
    overflow.width = Some(1);
    overflow.height = Some(2);
    assert!(template("t", vec![overflow]).validate().is_err());

    let mut huge = tab(&["a"]);
    huge.width = Some(9);
    huge.height = Some(9);
    assert!(template("t", vec![huge]).validate().is_err());

    let mut nul = tab(&["ok\0bad"]);
    nul.width = Some(1);
    assert!(template("t", vec![nul]).validate().is_err());
}

#[test]
fn the_config_document_accepts_the_key_and_rejects_duplicates() {
    let raw = r#"{
        "notifications": "always",
        "session_templates": [
            { "name": "work", "tabs": [
                { "name": "git", "commands": ["lazygit", "btop"] },
                { "name": "shell" }
            ] }
        ]
    }"#;
    let parsed = parse_config_document(raw).unwrap().unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].name, "work");
    assert_eq!(parsed[0].tabs.len(), 2);
    assert_eq!(parsed[0].tabs[0].commands, vec!["lazygit", "btop"]);
    assert!(parsed[0].tabs[1].commands.is_empty());

    assert!(parse_config_document(r#"{"custom_states": []}"#).unwrap().is_none());

    let dupes = r#"{"session_templates":[
        {"name":"a","tabs":[{}]}, {"name":"a","tabs":[{}]}
    ]}"#;
    let error = parse_config_document(dupes).unwrap_err();
    assert!(error.contains("duplicate"), "{error}");

    let bad_name = r#"{"session_templates":[{"name":"../evil","tabs":[{}]}]}"#;
    assert!(parse_config_document(bad_name).is_err());

    assert!(parse_config_document(r#"{"session_templates": 3}"#).is_err());
    assert!(parse_config_document("not json").is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test session_templates`
Expected: FAIL — `couldn't read src/session_templates.rs` (the file does not exist yet)

- [ ] **Step 3: Write minimal implementation**

Create `src/session_templates.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

use crate::custom_layouts::{MAX_COMMAND_BYTES, MAX_PANES, MAX_TOTAL_COMMAND_BYTES};

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
```

Add to `src/main.rs`, alongside the existing `mod` declarations:

```rust
mod session_templates;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test session_templates`
Expected: PASS, 4 tests

Run: `cargo build --release`
Expected: builds clean (the module is declared but not yet called; `dead_code` warnings are acceptable here and disappear in Task 3)

- [ ] **Step 5: Commit**

```bash
git add src/session_templates.rs src/main.rs tests/session_templates.rs
git commit -m "feat: parse and validate session templates"
```

---

### Task 2: KDL generation and the built-in default

Turn a validated template into a layout document Zellij accepts, and ship the four-tab default.

**Files:**
- Modify: `src/session_templates.rs`
- Test: `tests/session_templates.rs`

**Interfaces:**
- Consumes: `SessionTemplate`, `TemplateTab`, `TemplateTab::grid` from Task 1
- Produces:
  - `pub const BUILT_IN_NAME: &str = "zellaude";`
  - `pub fn built_in() -> SessionTemplate`
  - `pub fn effective(configured: Option<Vec<SessionTemplate>>) -> Vec<SessionTemplate>`
  - `pub fn marker(name: &str) -> String`
  - `impl SessionTemplate { pub fn to_kdl(&self, plugin_location: &str, plugin_configuration: &BTreeMap<String, String>, home: &str) -> Result<String, String> }`

- [ ] **Step 1: Write the failing test**

Append to `tests/session_templates.rs`:

```rust
use session_templates::{built_in, effective, marker, BUILT_IN_NAME};
use std::collections::BTreeMap;
use zellij_utils::input::layout::Layout;

const PLUGIN: &str = "file:~/.config/zellij/plugins/zellaude.wasm";

fn compile(t: &SessionTemplate) -> String {
    t.to_kdl(PLUGIN, &BTreeMap::new(), "/home/tester").unwrap()
}

/// Parse the generated document the way Zellij will. `tabs` is a field, not a
/// method, and each entry is `(Option<String>, tiled, floating)` — see
/// `tests/custom_layouts.rs:224`.
fn parse(kdl: &str) -> Layout {
    Layout::from_kdl(kdl, Some("generated.kdl".to_string()), None, None)
        .unwrap_or_else(|error| panic!("generated KDL did not parse: {error}\n{kdl}"))
}

#[test]
fn a_generated_layout_parses_and_carries_a_bar_in_every_tab() {
    let kdl = compile(&built_in());
    assert!(
        kdl.starts_with(&marker(BUILT_IN_NAME)),
        "first line must be the ownership marker, got: {}",
        kdl.lines().next().unwrap_or("")
    );

    let layout = parse(&kdl);
    assert_eq!(layout.tabs.len(), 4);
    let names: Vec<_> = layout
        .tabs
        .iter()
        .map(|(name, _, _)| name.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(names, vec!["git", "claude", "editor", "shell"]);

    // The template is a session layout, so later tabs the user opens by hand
    // must also get a bar. That comes from default_tab_template, not from a
    // bar pane repeated inside each tab.
    assert!(kdl.contains("default_tab_template"), "{kdl}");
    assert_eq!(kdl.matches("plugin location=").count(), 1, "{kdl}");
}

#[test]
fn commands_become_panes_in_reading_order_with_a_default_single_row() {
    let template = SessionTemplate {
        name: "grid".to_string(),
        cwd: None,
        tabs: vec![{
            let mut t = tab(&["one", "two", "three"]);
            t.name = Some("row".to_string());
            t
        }],
    };
    let kdl = compile(&template);
    parse(&kdl);

    let order: Vec<_> = ["one", "two", "three"]
        .iter()
        .map(|c| kdl.find(&format!("\"{c}\"")).unwrap())
        .collect();
    assert!(order[0] < order[1] && order[1] < order[2], "{kdl}");
    assert_eq!(kdl.matches("command=\"sh\"").count(), 3, "{kdl}");
    assert!(kdl.contains("split_direction=\"vertical\""), "{kdl}");
}

#[test]
fn a_tab_without_commands_is_a_plain_shell_tab() {
    let template = SessionTemplate {
        name: "bare".to_string(),
        cwd: None,
        tabs: vec![tab(&[])],
    };
    let kdl = compile(&template);
    parse(&kdl);
    assert!(!kdl.contains("command=\"sh\""), "{kdl}");
}

#[test]
fn cwd_is_omitted_relative_or_home_expanded() {
    let mut absent = tab(&["a"]);
    absent.name = Some("absent".to_string());
    let mut relative = tab(&["b"]);
    relative.name = Some("relative".to_string());
    relative.cwd = Some("src".to_string());
    let mut home = tab(&["c"]);
    home.name = Some("home".to_string());
    home.cwd = Some("~/other".to_string());

    let template = SessionTemplate {
        name: "dirs".to_string(),
        cwd: None,
        tabs: vec![absent, relative, home],
    };
    let kdl = compile(&template);
    parse(&kdl);

    assert!(kdl.contains("tab name=\"absent\" {"), "{kdl}");
    assert!(kdl.contains("tab name=\"relative\" cwd=\"src\""), "{kdl}");
    assert!(kdl.contains("tab name=\"home\" cwd=\"/home/tester/other\""), "{kdl}");
    assert!(!kdl.contains("\"~/"), "tilde must be expanded: {kdl}");
}

#[test]
fn a_tab_cwd_overrides_the_template_cwd() {
    let mut inherits = tab(&["a"]);
    inherits.name = Some("inherits".to_string());
    let mut overrides = tab(&["b"]);
    overrides.name = Some("overrides".to_string());
    overrides.cwd = Some("/elsewhere".to_string());

    let template = SessionTemplate {
        name: "dirs".to_string(),
        cwd: Some("~/base".to_string()),
        tabs: vec![inherits, overrides],
    };
    let kdl = compile(&template);
    parse(&kdl);

    assert!(kdl.contains("tab name=\"inherits\" cwd=\"/home/tester/base\""), "{kdl}");
    assert!(kdl.contains("tab name=\"overrides\" cwd=\"/elsewhere\""), "{kdl}");
}

#[test]
fn focus_marks_exactly_one_tab_and_defaults_to_none() {
    let template = SessionTemplate {
        name: "focused".to_string(),
        cwd: None,
        tabs: vec![tab(&["a"]), { let mut t = tab(&["b"]); t.focus = true; t }],
    };
    let kdl = compile(&template);
    parse(&kdl);
    assert_eq!(kdl.matches("focus=true").count(), 1, "{kdl}");

    let unfocused = SessionTemplate {
        name: "unfocused".to_string(),
        cwd: None,
        tabs: vec![tab(&["a"]), tab(&["b"])],
    };
    assert!(!compile(&unfocused).contains("focus=true"));
}

#[test]
fn plugin_configuration_is_carried_into_the_generated_bar() {
    let mut configuration = BTreeMap::new();
    configuration.insert("custom_states".to_string(), r#"[{"id":"x"}]"#.to_string());
    let kdl = built_in()
        .to_kdl(PLUGIN, &configuration, "/home/tester")
        .unwrap();
    parse(&kdl);
    assert!(kdl.contains("\"custom_states\""), "{kdl}");
}

#[test]
fn compilation_refuses_an_empty_plugin_location_and_invalid_templates() {
    assert!(built_in().to_kdl("", &BTreeMap::new(), "/home/tester").is_err());

    let invalid = SessionTemplate { name: "..".to_string(), cwd: None, tabs: vec![tab(&[])] };
    assert!(compile_err(&invalid).contains("reserved"));
}

fn compile_err(t: &SessionTemplate) -> String {
    t.to_kdl(PLUGIN, &BTreeMap::new(), "/home/tester").unwrap_err()
}

#[test]
fn the_built_in_is_always_present_and_yields_to_a_user_template_of_the_same_name() {
    let none = effective(None);
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].name, BUILT_IN_NAME);
    assert_eq!(none[0].tabs.len(), 4);
    assert_eq!(none[0].tabs[0].commands, vec!["lazygit", "btop"]);
    assert_eq!(none[0].tabs[1].commands, vec!["claude"]);
    assert_eq!(none[0].tabs[2].commands, vec!["nvim"]);
    assert!(none[0].tabs[3].commands.is_empty());

    let alongside = effective(Some(vec![template("work", vec![tab(&["a"])])]));
    let names: Vec<_> = alongside.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec![BUILT_IN_NAME, "work"]);

    let overridden = effective(Some(vec![template(BUILT_IN_NAME, vec![tab(&["mine"])])]));
    assert_eq!(overridden.len(), 1);
    assert_eq!(overridden[0].tabs[0].commands, vec!["mine"]);

    assert!(effective(Some(vec![])).iter().any(|t| t.name == BUILT_IN_NAME));
}

#[test]
fn the_built_in_default_is_itself_valid() {
    built_in().validate().unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test session_templates`
Expected: FAIL — `cannot find function built_in`, `no method named to_kdl`

- [ ] **Step 3: Write minimal implementation**

Append to `src/session_templates.rs`:

```rust
use std::collections::BTreeMap;
use std::fmt::Write;

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
```

Also move `kdl_string` into shared reach: in `src/custom_layouts.rs` change

```rust
fn kdl_string(value: &str) -> String {
```

to

```rust
pub fn kdl_string(value: &str) -> String {
```

and in `src/session_templates.rs` add it to the existing import:

```rust
use crate::custom_layouts::{kdl_string, MAX_COMMAND_BYTES, MAX_PANES, MAX_TOTAL_COMMAND_BYTES};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test session_templates`
Expected: PASS, 14 tests

Run: `cargo test`
Expected: PASS — the whole suite, confirming the `kdl_string` visibility change broke nothing

- [ ] **Step 5: Commit**

```bash
git add src/session_templates.rs src/custom_layouts.rs tests/session_templates.rs
git commit -m "feat: compile session templates to zellij layouts"
```

---

### Task 3: Host-side writing and pruning

Put the generated documents on disk without ever clobbering a file Zellaude does not own.

**Files:**
- Modify: `src/session_templates.rs`
- Test: `tests/session_templates.rs`

**Interfaces:**
- Consumes: `marker`, `MARKER_PREFIX` semantics from Task 2
- Produces:
  - `pub const WRITE_LAYOUT_SCRIPT: &str`
  - `pub const PRUNE_LAYOUTS_SCRIPT: &str`

Both scripts are run as `sh -c SCRIPT <argv0> <arg1> [layouts_dir]`, mirroring `SAVE_CONFIG_SCRIPT` in `src/main.rs:27` — the trailing directory argument exists so tests can point them at a temporary directory.

- `WRITE_LAYOUT_SCRIPT`: `$1` is the file basename (`<name>.kdl`), `$2` is the full KDL content, `$3` is the optional layouts directory.
- `PRUNE_LAYOUTS_SCRIPT`: `$1` is a newline-separated list of basenames to keep, `$2` is the optional layouts directory.

- [ ] **Step 1: Write the failing test**

Append to `tests/session_templates.rs`:

```rust
use session_templates::{PRUNE_LAYOUTS_SCRIPT, WRITE_LAYOUT_SCRIPT};
use std::path::{Path, PathBuf};
use std::process::Output;

fn temp_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zellaude-{tag}-{unique}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_layout(dir: &Path, basename: &str, content: &str) -> Output {
    std::process::Command::new("sh")
        .args(["-c", WRITE_LAYOUT_SCRIPT, "zellaude-write-layout-test", basename, content])
        .arg(dir)
        .output()
        .unwrap()
}

fn prune_layouts(dir: &Path, keep: &str) -> Output {
    std::process::Command::new("sh")
        .args(["-c", PRUNE_LAYOUTS_SCRIPT, "zellaude-prune-layouts-test", keep])
        .arg(dir)
        .output()
        .unwrap()
}

fn owned(name: &str, body: &str) -> String {
    format!("{}\n{body}\n", marker(name))
}

#[test]
fn writing_creates_then_overwrites_only_files_zellaude_owns() {
    let dir = temp_dir("write");

    let created = write_layout(&dir, "work.kdl", &owned("work", "layout {}"));
    assert!(created.status.success(), "{created:?}");
    let path = dir.join("work.kdl");
    assert!(std::fs::read_to_string(&path).unwrap().contains("layout {}"));

    let updated = write_layout(&dir, "work.kdl", &owned("work", "layout { tab }"));
    assert!(updated.status.success(), "{updated:?}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("tab"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn writing_never_touches_a_file_without_the_marker() {
    let dir = temp_dir("protect");
    let path = dir.join("default.kdl");
    std::fs::write(&path, "layout { /* mine */ }\n").unwrap();

    let result = write_layout(&dir, "default.kdl", &owned("default", "layout {}"));
    assert!(!result.status.success(), "must refuse: {result:?}");
    assert!(String::from_utf8_lossy(&result.stderr).contains("not generated by zellaude"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "layout { /* mine */ }\n",
        "the user's file must be untouched"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rewriting_identical_content_leaves_the_file_alone() {
    let dir = temp_dir("noop");
    let content = owned("work", "layout {}");
    write_layout(&dir, "work.kdl", &content);
    let path = dir.join("work.kdl");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let again = write_layout(&dir, "work.kdl", &content);
    assert!(again.status.success(), "{again:?}");
    let after = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(before, after, "identical content must not be rewritten");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pruning_removes_orphans_and_spares_everything_else() {
    let dir = temp_dir("prune");
    std::fs::write(dir.join("keep.kdl"), owned("keep", "layout {}")).unwrap();
    std::fs::write(dir.join("orphan.kdl"), owned("orphan", "layout {}")).unwrap();
    std::fs::write(dir.join("default.kdl"), "layout { /* mine */ }\n").unwrap();
    std::fs::write(dir.join("notes.txt"), format!("{}\n", marker("notes"))).unwrap();

    let result = prune_layouts(&dir, "keep.kdl\n");
    assert!(result.status.success(), "{result:?}");

    assert!(dir.join("keep.kdl").exists(), "listed file must survive");
    assert!(!dir.join("orphan.kdl").exists(), "marked orphan must go");
    assert!(dir.join("default.kdl").exists(), "unmarked file must survive");
    assert!(dir.join("notes.txt").exists(), "non-kdl file must survive");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pruning_an_empty_or_missing_directory_is_not_an_error() {
    let dir = temp_dir("empty");
    assert!(prune_layouts(&dir, "keep.kdl\n").status.success());
    std::fs::remove_dir_all(&dir).ok();

    let missing = temp_dir("missing");
    std::fs::remove_dir_all(&missing).unwrap();
    assert!(prune_layouts(&missing, "keep.kdl\n").status.success());
}

#[test]
fn writing_follows_a_symlink_instead_of_replacing_it() {
    let dir = temp_dir("symlink");
    let real = dir.join("real.kdl");
    std::fs::write(&real, owned("work", "layout {}")).unwrap();
    let link = dir.join("work.kdl");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let result = write_layout(&dir, "work.kdl", &owned("work", "layout { tab }"));
    assert!(result.status.success(), "{result:?}");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(std::fs::read_to_string(&real).unwrap().contains("tab"));

    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test session_templates`
Expected: FAIL — `cannot find value WRITE_LAYOUT_SCRIPT`

- [ ] **Step 3: Write minimal implementation**

Append to `src/session_templates.rs`:

```rust
/// Write one generated layout. `$1` basename, `$2` content, `$3` optional
/// directory (tests only). Refuses any existing file that Zellaude did not
/// write, so a name collision with a hand-written layout is inert rather than
/// destructive. The write is atomic because every tab runs its own plugin
/// instance and they compile concurrently.
pub const WRITE_LAYOUT_SCRIPT: &str = r#"
set -eu
if [ "$#" -ge 3 ]; then
    layouts_dir=$3
else
    layouts_dir="$HOME/.config/zellij/layouts"
fi
mkdir -p "$layouts_dir"
target="$layouts_dir/$1"

symlink_hops=0
while [ -L "$target" ]; do
    symlink_hops=$((symlink_hops + 1))
    if [ "$symlink_hops" -gt 40 ]; then
        printf '%s\n' "too many symlinks in zellaude layout path $1" >&2
        exit 1
    fi
    link_target=$(readlink "$target")
    case "$link_target" in
        /*) target=$link_target ;;
        *) target=$(dirname "$target")/$link_target ;;
    esac
done

if [ -e "$target" ]; then
    case "$(head -n 1 "$target" 2>/dev/null)" in
        '// zellaude-generated'*) ;;
        *)
            printf '%s\n' "$1 was not generated by zellaude; leaving it alone" >&2
            exit 1
            ;;
    esac
    # Both sides go through command substitution so that its trailing-newline
    # stripping applies equally; comparing against "$2" directly would never
    # match, because the generated document ends in a newline.
    if [ "$(cat "$target")" = "$(printf '%s' "$2")" ]; then
        exit 0
    fi
fi

target_dir=$(dirname "$target")
mkdir -p "$target_dir"
tmp_path=$(mktemp "$target_dir/.zellaude-layout.XXXXXX")
trap 'rm -f "$tmp_path"' 0 HUP INT TERM
printf '%s' "$2" > "$tmp_path"
chmod 644 "$tmp_path"
mv "$tmp_path" "$target"
trap - 0 HUP INT TERM
"#;

/// Delete generated layouts that no longer correspond to a template. `$1` is a
/// newline-separated list of basenames to keep, `$2` an optional directory.
/// Only `.kdl` files carrying the marker are eligible.
pub const PRUNE_LAYOUTS_SCRIPT: &str = r#"
set -eu
if [ "$#" -ge 2 ]; then
    layouts_dir=$2
else
    layouts_dir="$HOME/.config/zellij/layouts"
fi
[ -d "$layouts_dir" ] || exit 0

for path in "$layouts_dir"/*.kdl; do
    [ -f "$path" ] || continue
    case "$(head -n 1 "$path" 2>/dev/null)" in
        '// zellaude-generated'*) ;;
        *) continue ;;
    esac
    basename=${path##*/}
    if printf '%s\n' "$1" | grep -qxF "$basename"; then
        continue
    fi
    rm -f "$path"
done
"#;
```

- [ ] **Step 4: Add the end-to-end pipeline test**

This exercises everything Tasks 1–3 produce, from the document a user edits to
the bytes on disk. Append it to `tests/session_templates.rs`:

```rust
#[test]
fn a_settings_document_compiles_to_the_files_a_session_start_will_read() {
    let raw = r#"{
        "elapsed_time": true,
        "session_templates": [
            { "name": "work", "cwd": "~/repo", "tabs": [
                { "name": "code", "commands": ["nvim"] },
                { "name": "shell" }
            ] }
        ]
    }"#;
    let templates = effective(parse_config_document(raw).unwrap());
    let names: Vec<_> = templates.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["zellaude", "work"]);

    let dir = temp_dir("endtoend");
    for template in &templates {
        let kdl = template
            .to_kdl(PLUGIN, &BTreeMap::new(), "/home/tester")
            .unwrap();
        parse(&kdl);
        let result = write_layout(&dir, &format!("{}.kdl", template.name), &kdl);
        assert!(result.status.success(), "{result:?}");
    }

    let work = std::fs::read_to_string(dir.join("work.kdl")).unwrap();
    assert!(work.starts_with(&marker("work")), "{work}");
    assert!(work.contains("cwd=\"/home/tester/repo\""), "{work}");

    // Dropping "work" from the config orphans its file; the built-in stays.
    let pruned = prune_layouts(&dir, "zellaude.kdl\n");
    assert!(pruned.status.success(), "{pruned:?}");
    assert!(dir.join("zellaude.kdl").exists());
    assert!(!dir.join("work.kdl").exists());

    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test session_templates`
Expected: PASS, 21 tests

- [ ] **Step 6: Commit**

```bash
git add src/session_templates.rs tests/session_templates.rs
git commit -m "feat: write generated session layouts without clobbering user files"
```

---

### Task 4: Wire compilation into the plugin

Make the status bar actually compile templates once it has a config and knows its own URL.

**Files:**
- Modify: `src/state.rs` (add three fields to `State`, after `custom_layout_prompt` at line 222)
- Modify: `src/main.rs` (parse in `load_config`'s result at ~line 285, add `maybe_compile_session_templates`, call it from `PaneUpdate`, handle its `RunCommandResult`)

**Interfaces:**
- Consumes: `session_templates::{effective, parse_config_document, marker, WRITE_LAYOUT_SCRIPT, PRUNE_LAYOUTS_SCRIPT}`, `SessionTemplate::to_kdl`
- Produces: `State` fields `session_templates: Option<Vec<SessionTemplate>>`, `session_templates_compiled: bool`, `session_template_config_error: Option<String>`

**No new automated test.** This task adds no logic of its own — every branch it
introduces is a call into code Tasks 1–3 already cover, and the parts that
cannot be covered are `run_command`, `get_plugin_ids` and the pane manifest,
which only exist inside a running Zellij host. Writing a test here would mean
mocking the host to assert that a function was called, which tests the mock.
Step 6's real-session check is the verification that matters, and it is not
optional.

- [ ] **Step 1: Add the state fields**

In `src/state.rs`, after `pub custom_layout_prompt: Option<custom_layouts::Prompt>,`:

```rust
    /// `None` until the settings file has been read; `Some` afterwards, even
    /// when the key is absent, so the built-in still compiles.
    pub session_templates: Option<Vec<session_templates::SessionTemplate>>,
    pub session_templates_compiled: bool,
    pub session_template_config_error: Option<String>,
```

and extend the module import at the top of `src/state.rs`:

```rust
use crate::{custom_layouts, session_templates, split_three};
```

No test file includes `src/state.rs` via `#[path]`, so this import is safe to
widen.

- [ ] **Step 2: Parse the key when the settings file arrives**

In `src/main.rs`, inside the `Some("load_config")` arm, after the existing
`custom_layouts::parse_config_document` block and before `self.config_loaded = true;`:

```rust
                        match session_templates::parse_config_document(raw.trim()) {
                            Ok(configured) => {
                                self.session_templates =
                                    Some(session_templates::effective(configured));
                                self.session_template_config_error = None;
                            }
                            Err(error) => {
                                // Keep whatever is already on disk: a typo in
                                // one key must not strand a session template
                                // the user relies on mid-day.
                                self.session_template_config_error = Some(error);
                            }
                        }
```

- [ ] **Step 3: Add the compiler**

Add it alongside the other `maybe_*` helpers in `impl State`:

```rust
    /// Compile every template to `~/.config/zellij/layouts/`. Runs once per
    /// plugin instance, as soon as both the settings file and this plugin's own
    /// URL are known — the URL only becomes available once a pane manifest
    /// describing this pane arrives.
    fn maybe_compile_session_templates(&mut self) {
        if self.session_templates_compiled || !self.command_permissions_granted {
            return;
        }
        let Some(templates) = self.session_templates.clone() else {
            return;
        };
        let plugin_id = *self
            .plugin_id
            .get_or_insert_with(|| get_plugin_ids().plugin_id);
        let plugin_location = self
            .pane_manifest
            .as_ref()
            .into_iter()
            .flat_map(|manifest| manifest.panes.values())
            .flatten()
            .find(|pane| pane.is_plugin && pane.id == plugin_id)
            .and_then(|pane| pane.plugin_url.clone());
        let Some(plugin_location) = plugin_location else {
            return;
        };
        let home = std::env::var("HOME").unwrap_or_default();

        self.session_templates_compiled = true;
        if let Some(error) = self.session_template_config_error.as_deref() {
            eprintln!("Zellaude could not read session templates: {error}");
        }

        let mut keep = String::new();
        for template in &templates {
            let kdl = match template.to_kdl(
                &plugin_location,
                &self.plugin_configuration,
                &home,
            ) {
                Ok(kdl) => kdl,
                Err(error) => {
                    eprintln!(
                        "Zellaude could not compile session template {:?}: {error}",
                        template.name
                    );
                    continue;
                }
            };
            let basename = format!("{}.kdl", template.name);
            let mut ctx = BTreeMap::new();
            ctx.insert("type".into(), "write_layout".into());
            ctx.insert("layout".into(), basename.clone());
            run_command(
                &[
                    "sh",
                    "-c",
                    session_templates::WRITE_LAYOUT_SCRIPT,
                    "zellaude-write-layout",
                    &basename,
                    &kdl,
                ],
                ctx,
            );
            keep.push_str(&basename);
            keep.push('\n');
        }

        let mut ctx = BTreeMap::new();
        ctx.insert("type".into(), "prune_layouts".into());
        run_command(
            &[
                "sh",
                "-c",
                session_templates::PRUNE_LAYOUTS_SCRIPT,
                "zellaude-prune-layouts",
                &keep,
            ],
            ctx,
        );
    }
```

- [ ] **Step 4: Call it, and report what the host says**

Call it where the pane manifest is refreshed. In the `Event::PaneUpdate` arm of
`update`, after `self.maybe_install_runtime_bindings();`:

```rust
                self.maybe_compile_session_templates();
```

and in the `Some("load_config")` arm, immediately after
`self.on_command_permissions_granted();`:

```rust
                        self.maybe_compile_session_templates();
```

Report the host-side outcome. Add two arms to the `RunCommandResult` match,
beside `Some("save_config")`:

```rust
                    Some("write_layout") => {
                        if exit_code != Some(0) {
                            eprintln!(
                                "Zellaude could not write layout {}: {}",
                                context.get("layout").map(String::as_str).unwrap_or("?"),
                                String::from_utf8_lossy(&stderr).trim()
                            );
                        }
                        false
                    }
                    Some("prune_layouts") => {
                        if exit_code != Some(0) {
                            eprintln!(
                                "Zellaude could not prune generated layouts: {}",
                                String::from_utf8_lossy(&stderr).trim()
                            );
                        }
                        false
                    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS — the whole suite, 21 tests in `session_templates`

Run: `cargo build --release`
Expected: builds clean for `wasm32-wasip1`, no warnings from the new module

- [ ] **Step 6: Verify against a real Zellij session**

```bash
./install.sh
```

Then, from a directory you can recognise:

```bash
cd ~ && env -u ZELLIJ -u ZELLIJ_SESSION_NAME -u ZELLIJ_PANE_ID \
    zellij -s tmpl-check -n zellaude
```

Expected: four tabs — `git` (lazygit beside btop), `claude`, `editor`, `shell` —
all starting in `~`, each with the Zellaude bar. Confirm
`head -n 1 ~/.config/zellij/layouts/zellaude.kdl` shows the marker and that
`~/.config/zellij/layouts/default.kdl` is unchanged. Then:

```bash
zellij kill-session tmpl-check && zellij delete-session tmpl-check --force
```

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/state.rs
git commit -m "feat: compile session templates when the bar loads"
```

---

### Task 5: Documentation

**Files:**
- Modify: `README.md` — add a "Session templates" section after "Custom states", add a bullet to the Features list, add a row to the "What it touches" table

- [ ] **Step 1: Add the feature bullet**

In the `## Features` list, after the `**Custom states**` bullet:

```markdown
- **Session templates** — describe a multi-tab session once and start it with `zellij -s work -n <name>`
```

- [ ] **Step 2: Add the section**

Immediately after the "### Custom states" section and before "### Settings":

````markdown
### Session templates

A session template describes a whole session — its tabs, their commands and
their directories — and Zellaude compiles it into a layout Zellij starts
natively:

```bash
zellij -s work -n zellaude
```

`zellaude` is the built-in template, available without any configuration:

| Tab | Panes |
|-----|-------|
| `git` | `lazygit` beside `btop` |
| `claude` | `claude` |
| `editor` | `nvim` |
| `shell` | plain shell |

Define your own in `~/.config/zellij/plugins/zellaude.json`:

```json
{
  "session_templates": [
    {
      "name": "work",
      "tabs": [
        { "name": "git", "commands": ["lazygit", "btop"] },
        { "name": "claude", "commands": ["claude"] },
        { "name": "editor", "cwd": "src", "commands": ["nvim"] },
        { "name": "shell" }
      ]
    }
  ]
}
```

Reload the plugin after editing, then `zellij -s work -n work`. A template named
`zellaude` replaces the built-in.

A tab's `commands` become one pane each, arranged in a single row by default;
`width` and `height` lay them out as a grid, reading order left-to-right and
top-to-bottom, with any spare cells opening as plain shell panes. A tab without
`commands` is a plain shell tab. `focus` starts the session on that tab.
Commands run through `sh -lc`, so template configuration is trusted local shell
code, exactly as for custom states.

Omitting `cwd` opens panes in the directory `zellij` was run from, which is
usually what you want. A relative `cwd` resolves against that same directory, so
`"src"` means `<where you ran zellij>/src`; absolute paths and `~` are used as
given. A tab's `cwd` overrides the template's.

Templates are compiled to `~/.config/zellij/layouts/<name>.kdl`, so template
names must be filename-safe: letters, digits, `.`, `_` and `-`, up to 64
characters. Every generated file begins with a `// zellaude-generated` marker,
and Zellaude only ever writes or removes files carrying it — a layout you wrote
yourself is never overwritten, even if a template shares its name. Problems are
reported to the plugin log; run Zellij with `--debug` to see them.
````

- [ ] **Step 3: Add the table row**

In the "#### What it touches" table, after the `zellaude-hook.sh` row:

```markdown
| `~/.config/zellij/layouts/<name>.kdl` | created or replaced, but only when marked `// zellaude-generated`; unmarked files are never touched |
```

- [ ] **Step 4: Check the rendered result**

Run: `grep -n "Session templates" README.md`
Expected: two hits — the Features bullet and the section heading

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document session templates"
```

---

## Verification

After the final task:

```bash
cargo test
cargo build --release
```

Both must pass. Task 4 Step 5 covers the real-session check that no automated
test can: that Zellij accepts the generated layout and opens the tabs described.
