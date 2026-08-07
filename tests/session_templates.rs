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
use zellij_utils::input::layout::{Layout, Run, RunPluginOrAlias, TiledPaneLayout};

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

/// Depth-first search for the first plugin pane in a parsed tab's tree. Used
/// to prove `default_tab_template` inheritance: a tab that never mentions the
/// plugin in its own body still carries a bar once Zellij merges the template.
fn find_plugin(node: &TiledPaneLayout) -> Option<&RunPluginOrAlias> {
    if let Some(Run::Plugin(plugin)) = &node.run {
        return Some(plugin);
    }
    node.children.iter().find_map(find_plugin)
}

/// Depth-first search for the first node whose own children are split
/// vertically (ie. a column layout), mirroring `tests/custom_layouts.rs`'s use
/// of `children_split_direction` to prove pane geometry post-parse.
fn find_vertical_split(node: &TiledPaneLayout) -> Option<&TiledPaneLayout> {
    use zellij_utils::input::layout::SplitDirection;
    if node.children_split_direction == SplitDirection::Vertical {
        return Some(node);
    }
    node.children.iter().find_map(find_vertical_split)
}

/// Depth-first search for the command pane running `command`'s resolved cwd,
/// as Zellij will actually run it, rather than the pre-parse KDL text.
fn command_cwd(node: &TiledPaneLayout, command: &str) -> Option<Option<std::path::PathBuf>> {
    if let Some(Run::Command(run_command)) = &node.run {
        if run_command.args.get(1).map(String::as_str) == Some(command) {
            return Some(run_command.cwd.clone());
        }
    }
    node.children.iter().find_map(|child| command_cwd(child, command))
}

fn maybe_shell_command(run: &Option<Run>) -> Option<String> {
    let Some(Run::Command(command)) = run else {
        return None;
    };
    command.args.get(1).cloned()
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
    // bar pane repeated inside each tab: the plugin is declared once in the
    // source text, and Zellij's own parser is trusted to fan it out below.
    assert!(kdl.contains("default_tab_template"), "{kdl}");
    assert_eq!(kdl.matches("plugin location=").count(), 1, "{kdl}");

    // Prove the fan-out actually happened by walking each *parsed* tab's own
    // tree, not the source text: every tab must independently carry the bar.
    // Zellij expands `~` in plugin locations itself (unlike `cwd`, see
    // `expand_home`), so compare on the stable suffix rather than the exact
    // string.
    for (name, tiled, _) in &layout.tabs {
        let plugin = find_plugin(tiled)
            .unwrap_or_else(|| panic!("tab {name:?} has no bar in its parsed tree"));
        assert!(
            plugin
                .location_string()
                .ends_with(".config/zellij/plugins/zellaude.wasm"),
            "tab {name:?} bar has unexpected plugin location: {}",
            plugin.location_string()
        );
    }
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
    let layout = parse(&kdl);
    let (_, tiled, _) = &layout.tabs[0];

    // `extract_run_instructions` returns panes in the flattened, geometric
    // reading order Zellij itself uses (see zellij_utils' own doc comment on
    // it) — a stronger claim than "these three substrings appear in this
    // relative order in the source text".
    let order: Vec<String> = tiled
        .extract_run_instructions()
        .iter()
        .filter_map(maybe_shell_command)
        .collect();
    assert_eq!(order, vec!["one", "two", "three"], "{kdl}");

    // A default single row is three columns of one command each: the grid
    // node's own children are split vertically (columns), and there are
    // exactly as many columns as commands.
    let grid = find_vertical_split(tiled).unwrap_or_else(|| panic!("no vertical split: {kdl}"));
    assert_eq!(grid.children.len(), 3, "{kdl}");
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

/// Look up the tab named `tab_name` in a parsed layout and find the resolved
/// `cwd` Zellij will actually launch `command` with.
fn cwd_of(layout: &Layout, tab_name: &str, command: &str) -> Option<std::path::PathBuf> {
    let (_, tiled, _) = layout
        .tabs
        .iter()
        .find(|(name, _, _)| name.as_deref() == Some(tab_name))
        .unwrap_or_else(|| panic!("no tab named {tab_name:?}"));
    command_cwd(tiled, command)
        .unwrap_or_else(|| panic!("command {command:?} not found in tab {tab_name:?}"))
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
    let layout = parse(&kdl);

    // Verified against the resolved `RunCommand.cwd` Zellij will actually use,
    // not the pre-parse text.
    assert_eq!(cwd_of(&layout, "absent", "a"), None, "{kdl}");
    assert_eq!(
        cwd_of(&layout, "relative", "b"),
        Some(std::path::PathBuf::from("src")),
        "{kdl}"
    );
    assert_eq!(
        cwd_of(&layout, "home", "c"),
        Some(std::path::PathBuf::from("/home/tester/other")),
        "{kdl}"
    );
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
    let layout = parse(&kdl);

    assert_eq!(
        cwd_of(&layout, "inherits", "a"),
        Some(std::path::PathBuf::from("/home/tester/base")),
        "{kdl}"
    );
    assert_eq!(
        cwd_of(&layout, "overrides", "b"),
        Some(std::path::PathBuf::from("/elsewhere")),
        "{kdl}"
    );
}

#[test]
fn focus_marks_exactly_one_tab_and_defaults_to_none() {
    let template = SessionTemplate {
        name: "focused".to_string(),
        cwd: None,
        tabs: vec![tab(&["a"]), { let mut t = tab(&["b"]); t.focus = true; t }],
    };
    let kdl = compile(&template);
    let layout = parse(&kdl);
    // The parsed layout's own notion of which tab is focused, rather than
    // counting `focus=true` occurrences in the source text.
    assert_eq!(layout.focused_tab_index, Some(1), "{kdl}");

    let unfocused = SessionTemplate {
        name: "unfocused".to_string(),
        cwd: None,
        tabs: vec![tab(&["a"]), tab(&["b"])],
    };
    let unfocused_kdl = compile(&unfocused);
    let unfocused_layout = parse(&unfocused_kdl);
    assert_eq!(unfocused_layout.focused_tab_index, None, "{unfocused_kdl}");
}

#[test]
fn a_grid_with_spare_cells_gets_a_plain_shell_pane_at_the_reading_order_tail() {
    // 2 columns x 2 rows with only 3 commands: the fourth, spare cell must
    // still render (a rectangular grid), but as a plain shell pane, and it
    // must land bottom-right — the last position in reading order.
    let mut t = tab(&["one", "two", "three"]);
    t.name = Some("grid".to_string());
    t.width = Some(2);
    t.height = Some(2);
    let template = SessionTemplate {
        name: "spare".to_string(),
        cwd: None,
        tabs: vec![t],
    };
    let kdl = compile(&template);
    let layout = parse(&kdl);
    let (_, tiled, _) = &layout.tabs[0];

    let order: Vec<String> = tiled
        .extract_run_instructions()
        .iter()
        .filter_map(maybe_shell_command)
        .collect();
    assert_eq!(order, vec!["one", "two", "three"], "{kdl}");

    let grid = find_vertical_split(tiled).unwrap_or_else(|| panic!("no vertical split: {kdl}"));
    assert_eq!(grid.children.len(), 2, "expected two columns: {kdl}");
    let bottom_right = grid
        .children
        .last()
        .and_then(|column| column.children.last())
        .unwrap_or_else(|| panic!("no bottom-right cell: {kdl}"));
    assert!(
        bottom_right.run.is_none(),
        "bottom-right cell must be a plain shell pane, not a fourth command: {kdl}"
    );
    assert_eq!(tiled.pane_count(), 5, "bar + 3 commands + 1 spare pane: {kdl}");
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
fn validation_rejects_more_than_one_focused_tab() {
    let mut first = tab(&["a"]);
    first.focus = true;
    let mut second = tab(&["b"]);
    second.focus = true;
    let doubly_focused = template("dupe-focus", vec![first, second]);

    let error = doubly_focused.validate().unwrap_err();
    assert!(error.contains("dupe-focus"), "{error}");
    assert!(error.contains("focus"), "{error}");

    // A silently-dropped setting is worse than a config error, so this must
    // also fail at compile time, not just quietly keep the first focus.
    assert!(compile_err(&doubly_focused).contains("focus"));
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

