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
