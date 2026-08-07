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
