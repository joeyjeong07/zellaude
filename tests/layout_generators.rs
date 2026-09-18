#![allow(dead_code)]

#[path = "../src/custom_layouts.rs"]
mod custom_layouts;
#[path = "../src/layout_generators.rs"]
mod layout_generators;

use custom_layouts::{tabs_to_kdl, CommandGrid, CustomLayout, TabChrome};
use layout_generators::{
    invoke, parse_floor_overrides, parse_generator_files, plan_rows, select_generator, Bindings,
    FloorOverrides, GeneratorFile, LayoutGenerator, PaneFloors, SourceTab, TabGeometry,
    DEFAULT_MIN_PANE_HEIGHT, DEFAULT_MIN_PANE_WIDTH,
};
use std::collections::BTreeMap;
use std::process::Command;
use zellij_utils::input::layout::{Layout, Run};
use zellij_utils::pane_size::PaneGeom;

const ZELLAUDE_URL: &str = "file:/tmp/zellaude.wasm";

/// The canonical `madev.kdl` of the PLAN's vocabulary block, verbatim.
pub const MADEV_GENERATOR: &str = r#"
command "impl"
arg "n"
flag "crit-per-impl" value="m" default=1
flag "single-tab"
flag "only-crit" optional-value="from" default=1
min_pane_width 54
min_pane_height 12

tab "{tab}-impl" unless="single_tab only_crit" {
    each for="i" in="1..=n" {
        pane "claude -n impl{i} '/madev-impl impl{i}'"
    }
}
each for="k" in="from..from+m" {
    tab "{tab}-crit{k}" unless="single_tab" {
        each for="i" in="1..=n" {
            pane "claude -n impl{i}-crit{k} '/madev-impl-crit impl{i}-crit{k}'"
        }
    }
}
tab if="single_tab" {
    each for="i" in="1..=n" {
        pane "claude -n impl{i} '/madev-impl impl{i}'" unless="only_crit"
        each for="k" in="from..from+m" {
            pane "claude -n impl{i}-crit{k} '/madev-impl-crit impl{i}-crit{k}'"
        }
    }
}
"#;

/// Every string shape: a positional, a `value` flag with a default and an
/// `optional-value` flag, used in a tab name and in a command.
pub const STRING_GENERATOR: &str = r#"
command "g"
arg "feature" type="string"
arg "n" type="integer"
flag "branch" value="b" type="string" default="main"
flag "model" optional-value="model_name" type="string" default="opus"

tab "{tab}-{feature}" {
    each for="i" in="1..=n" {
        pane "claude -n {feature}{i} '/madev-impl {feature}{i} {b}' --model {model_name}"
    }
}
"#;

fn file(path: &str, content: &str) -> GeneratorFile {
    GeneratorFile {
        path: path.to_string(),
        content: content.to_string(),
    }
}

fn parse_one(content: &str) -> LayoutGenerator {
    let mut generators = parse_generator_files(&[file("/g/impl.kdl", content)])
        .unwrap_or_else(|error| panic!("expected a parse, got {error}"));
    generators.pop().expect("one generator")
}

fn refusal(content: &str) -> String {
    parse_generator_files(&[file("/g/impl.kdl", content)])
        .expect_err("expected a refusal")
}

fn body(nodes: &str) -> String {
    format!("command \"c\"\narg \"n\"\nflag \"only-crit\"\n{nodes}\n")
}

#[test]
fn parses_the_madev_generator() {
    let generator = parse_one(MADEV_GENERATOR);
    assert_eq!(generator.command, "impl");
    assert_eq!(generator.source, "impl.kdl");
}

#[test]
fn keeps_file_order_and_names_both_files_on_a_duplicate_command() {
    let files = [
        file("/g/a.kdl", "command \"impl\"\n"),
        file("/g/b.kdl", "command \"chat\"\n"),
    ];
    let commands: Vec<String> = parse_generator_files(&files)
        .unwrap()
        .into_iter()
        .map(|generator| generator.command)
        .collect();
    assert_eq!(commands, ["impl", "chat"]);

    let clash = [
        file("/g/a.kdl", "command \"impl\"\n"),
        file("/g/b.kdl", "command \"impl\"\n"),
    ];
    let error = parse_generator_files(&clash).unwrap_err();
    assert!(
        error.starts_with("b.kdl:") && error.contains("a.kdl"),
        "{error}"
    );
}

#[test]
fn refuses_an_unknown_node() {
    assert_eq!(
        refusal("command \"c\"\nwindow \"x\"\n"),
        "impl.kdl: unknown node \"window\""
    );
    // A misspelling inside a block is the likeliest typo, and the refusal is the
    // whole diagnostic: no line numbers reach the one-row bar.
    assert_eq!(
        refusal(&body("tab { pnae \"cmd\"; }")),
        "impl.kdl: unknown node \"pnae\""
    );
    assert_eq!(
        refusal(&body("each for=\"i\" in=\"1..2\" { taab { pane \"a\"; }; }")),
        "impl.kdl: unknown node \"taab\""
    );
    assert_eq!(
        refusal(&body("tab { pnae for=\"i\" \"cmd\"; }")),
        "impl.kdl: unknown node \"pnae\""
    );
}

#[test]
fn refuses_an_unknown_property() {
    let error = refusal(&body("tab \"x\" when=\"n\" { pane \"a\"; }"));
    assert!(
        error.contains("unknown property \"when\" on a tab node"),
        "{error}"
    );
}

#[test]
fn refuses_a_file_without_a_command() {
    assert_eq!(
        refusal("arg \"n\"\n"),
        "impl.kdl: a generator file must declare a command"
    );
}

#[test]
fn refuses_a_duplicate_variable() {
    assert!(refusal("command \"c\"\narg \"n\"\narg \"n\"\n").contains("already declared"));
    assert!(refusal("command \"c\"\narg \"tab\"\n").contains("already declared"));
    assert!(
        refusal("command \"c\"\nflag \"single-tab\"\narg \"single_tab\"\n")
            .contains("already declared")
    );
    assert!(
        refusal(&body("each for=\"n\" in=\"1..2\" { tab { pane \"a\"; }; }"))
            .contains("already declared")
    );
}

#[test]
fn refuses_a_declaration_after_the_body() {
    let error = refusal("command \"c\"\ntab { pane \"a\"; }\narg \"n\"\n");
    assert_eq!(
        error,
        "impl.kdl: \"arg\" must be declared before the first tab, pane or each node"
    );
}

#[test]
fn refuses_misplaced_tab_and_pane_nodes() {
    assert!(refusal(&body("tab { tab { pane \"a\"; }; }"))
        .contains("must not be nested inside another tab"));
    assert!(refusal(&body("pane \"a\"")).contains("a pane node must be inside a tab"));
    assert!(refusal(&body("each for=\"i\" in=\"1..2\" { pane \"a\"; }"))
        .contains("a pane node must be inside a tab"));
}

#[test]
fn accepts_a_tab_under_a_top_level_each_chain() {
    parse_one(&body(
        "each for=\"i\" in=\"1..2\" { each for=\"j\" in=\"1..2\" { tab { pane \"a{i}{j}\"; }; }; }",
    ));
}

#[test]
fn refuses_an_optional_value_flag_without_a_default() {
    let error = refusal("command \"c\"\nflag \"only-crit\" optional-value=\"from\"\n");
    assert!(error.contains("needs a default"), "{error}");
    assert!(
        refusal("command \"c\"\nflag \"a\" value=\"v\" optional-value=\"w\" default=1\n")
            .contains("both value and optional-value")
    );
    assert!(refusal("command \"c\"\nflag \"a\" default=1\n").contains("default without a value"));
}

#[test]
fn accepts_string_args_and_flag_values() {
    assert_eq!(parse_one(STRING_GENERATOR).command, "g");
}

#[test]
fn refuses_a_malformed_type() {
    assert_eq!(
        refusal("command \"c\"\narg \"a\" type=\"float\"\n"),
        "impl.kdl: property \"type\" must be \"integer\" or \"string\""
    );
    assert_eq!(
        refusal("command \"c\"\narg \"a\" type=1\n"),
        "impl.kdl: property \"type\" must be a string"
    );
    assert_eq!(
        refusal("command \"c\"\nflag \"solo\" type=\"string\"\n"),
        "impl.kdl: flag \"solo\" sets a type without a value"
    );
}

#[test]
fn refuses_a_default_of_the_wrong_type() {
    assert_eq!(
        refusal("command \"c\"\nflag \"b\" value=\"b\" type=\"string\" default=1\n"),
        "impl.kdl: property \"default\" must be a string"
    );
    assert_eq!(
        refusal("command \"c\"\nflag \"m\" value=\"m\" default=\"x\"\n"),
        "impl.kdl: property \"default\" must be an integer"
    );
}

#[test]
fn refuses_a_string_in_a_range_or_a_condition() {
    let declared = "command \"c\"\narg \"s\" type=\"string\"\n";
    assert!(
        refusal(&format!("{declared}tab if=\"s\" {{ pane \"a\"; }}\n"))
            .contains("variable \"s\" in if is not a flag presence")
    );
    assert!(refusal(&format!(
        "{declared}each for=\"i\" in=\"1..s\" {{ tab {{ pane \"a\"; }}; }}\n"
    ))
    .contains("range bound \"s\" is not an integer variable"));
}

#[test]
fn refuses_a_presence_outside_a_condition() {
    let error = refusal(&body("tab \"{only_crit}\" { pane \"a\"; }"));
    assert!(error.contains("legal only in if or unless"), "{error}");
    assert!(refusal(&body("tab { pane \"a{only_crit}\"; }")).contains("legal only in if or unless"));
    assert!(refusal(&body("each for=\"i\" in=\"1..only_crit\" { tab { pane \"a\"; }; }"))
        .contains("not an integer variable"));
}

#[test]
fn refuses_an_integer_or_the_source_tab_inside_a_condition() {
    assert!(refusal(&body("tab if=\"n\" { pane \"a\"; }")).contains("is not a flag presence"));
    assert!(refusal(&body("tab if=\"tab\" { pane \"a\"; }")).contains("is not a flag presence"));
    assert!(
        refusal(&body("each for=\"i\" in=\"1..tab\" { tab { pane \"a\"; }; }"))
            .contains("not an integer variable")
    );
}

#[test]
fn refuses_an_unknown_variable() {
    assert!(refusal(&body("tab if=\"nope\" { pane \"a\"; }")).contains("unknown variable \"nope\""));
    assert!(
        refusal(&body("each for=\"i\" in=\"1..nope\" { tab { pane \"a\"; }; }"))
            .contains("unknown variable \"nope\"")
    );
}

#[test]
fn refuses_a_malformed_range() {
    assert!(refusal(&body("each for=\"i\" in=\"1\" { tab { pane \"a\"; }; }")).contains("separator"));
    assert!(
        refusal(&body("each for=\"i\" in=\"1..n+1+2\" { tab { pane \"a\"; }; }"))
            .contains("may use at most one")
    );
    assert!(refusal(&body("each for=\"i\" in=\"..n\" { tab { pane \"a\"; }; }"))
        .contains("range bound is empty"));
    assert!(refusal(&body("each for=\"i\" in=\"-1..n\" { tab { pane \"a\"; }; }"))
        .contains("must start with a number or a variable"));
}

#[test]
fn accepts_every_range_shape() {
    parse_one(&body(
        "each for=\"i\" in=\"1..n\" { tab { pane \"a{i}\"; }; }\n\
         each for=\"j\" in=\"1..=n\" { tab { pane \"b{j}\"; }; }\n\
         each for=\"k\" in=\"n+1..=n-1\" { tab { pane \"c{k}\"; }; }\n\
         each for=\"l\" in=\" 2 .. n \" { tab { pane \"d{l}\"; }; }",
    ));
}

#[test]
fn refuses_a_bad_floor_declaration() {
    assert!(refusal("command \"c\"\nmin_pane_width \"wide\"\n")
        .contains("min_pane_width must be a non-negative integer"));
    assert!(refusal("command \"c\"\nmin_pane_height -1\n")
        .contains("min_pane_height must be a non-negative integer"));
    assert!(refusal("command \"c\"\nmin_pane_width 10\nmin_pane_width 20\n")
        .contains("declared more than once"));
}

#[test]
fn refuses_a_malformed_command_or_flag_name() {
    assert!(refusal("command \"two words\"\n").contains("must be a single word"));
    assert!(refusal("command \"c\"\nflag \"-lead\"\n").contains("flag name"));
    assert!(refusal("command \"c\"\nflag \"a b\"\n").contains("flag name"));
}

#[test]
fn refuses_a_block_where_none_belongs() {
    assert!(refusal("command \"c\" { arg \"n\"; }\n").contains("must not have a block"));
    assert!(refusal(&body("tab { pane \"a\" { pane \"b\"; }; }")).contains("must not have a block"));
}

#[test]
fn floors_resolve_from_the_file_then_the_global_then_the_constants() {
    let file_floors = FloorOverrides {
        min_pane_width: Some(80),
        min_pane_height: None,
    };
    let global = FloorOverrides {
        min_pane_width: Some(100),
        min_pane_height: Some(20),
    };
    assert_eq!(
        PaneFloors::resolve(file_floors, global),
        PaneFloors {
            min_pane_width: 80,
            min_pane_height: 20,
        }
    );
    assert_eq!(
        PaneFloors::resolve(FloorOverrides::default(), global),
        PaneFloors {
            min_pane_width: 100,
            min_pane_height: 20,
        }
    );
    assert_eq!(
        PaneFloors::resolve(FloorOverrides::default(), FloorOverrides::default()),
        PaneFloors {
            min_pane_width: DEFAULT_MIN_PANE_WIDTH,
            min_pane_height: DEFAULT_MIN_PANE_HEIGHT,
        }
    );
}

#[test]
fn reads_the_floor_keys_of_zellaude_json() {
    assert_eq!(
        parse_floor_overrides(r#"{"min_pane_width": 100, "min_pane_height": 5}"#).unwrap(),
        FloorOverrides {
            min_pane_width: Some(100),
            min_pane_height: Some(5),
        }
    );
    assert_eq!(
        parse_floor_overrides(r#"{"custom_states": []}"#).unwrap(),
        FloorOverrides::default()
    );
    assert_eq!(parse_floor_overrides("{}").unwrap(), FloorOverrides::default());
    assert_eq!(parse_floor_overrides("[]").unwrap(), FloorOverrides::default());
    assert!(parse_floor_overrides(r#"{"min_pane_width": "wide"}"#)
        .unwrap_err()
        .contains("min_pane_width must be a non-negative integer"));
    assert!(parse_floor_overrides(r#"{"min_pane_height": -1}"#).is_err());
    assert!(parse_floor_overrides("{").is_err());
}

fn bind(content: &str, line: &str) -> Result<Bindings, String> {
    let generators = parse_generator_files(&[file("/g/impl.kdl", content)]).unwrap();
    let (generator, tokens) = select_generator(&generators, line).expect("a command matches");
    generator.bind_arguments(&tokens)
}

fn madev_bindings(line: &str) -> Bindings {
    bind(MADEV_GENERATOR, line).unwrap_or_else(|error| panic!("{line:?} refused: {error}"))
}

fn madev_refusal(line: &str) -> String {
    bind(MADEV_GENERATOR, line).expect_err("expected a refusal")
}

#[test]
fn selects_the_generator_by_the_first_token() {
    let generators = parse_generator_files(&[
        file("/g/a.kdl", "command \"impl\"\narg \"n\"\n"),
        file("/g/b.kdl", "command \"chat\"\n"),
    ])
    .unwrap();
    let (generator, tokens) = select_generator(&generators, "chat --x 1").unwrap();
    assert_eq!(generator.command, "chat");
    assert_eq!(tokens, ["--x", "1"]);
    assert!(select_generator(&generators, "nope 4").is_none());
    assert!(select_generator(&generators, "").is_none());
    assert!(select_generator(&generators, "   ").is_none());
}

#[test]
fn binds_positionals_and_flags_in_any_order() {
    for line in [
        "impl 4 --crit-per-impl 2 --only-crit 3",
        "impl --crit-per-impl 2 4 --only-crit 3",
        "impl --only-crit 3 --crit-per-impl 2 4",
    ] {
        let bindings = madev_bindings(line);
        assert_eq!(bindings.integer("n"), Some(4), "{line}");
        assert_eq!(bindings.integer("m"), Some(2), "{line}");
        assert_eq!(bindings.integer("from"), Some(3), "{line}");
        assert!(bindings.is_present("only_crit"), "{line}");
        assert!(bindings.is_present("crit_per_impl"), "{line}");
        assert!(!bindings.is_present("single_tab"), "{line}");
    }
}

#[test]
fn binds_every_flags_presence() {
    let bindings = madev_bindings("impl 4 --single-tab");
    assert!(bindings.is_present("single_tab"));
    assert!(!bindings.is_present("crit_per_impl"));
    assert!(!bindings.is_present("only_crit"));
}

#[test]
fn fills_defaults_for_whatever_the_line_leaves_out() {
    let bindings = madev_bindings("impl 4");
    assert_eq!(bindings.integer("n"), Some(4));
    assert_eq!(bindings.integer("m"), Some(1));
    assert_eq!(bindings.integer("from"), Some(1));
    assert!(!bindings.is_present("only_crit"));
}

#[test]
fn takes_an_optional_value_only_when_one_follows() {
    let given = madev_bindings("impl 4 --only-crit 2");
    assert_eq!(given.integer("from"), Some(2));
    assert!(given.is_present("only_crit"));

    // Bare, at the end of the line and before another flag.
    for line in ["impl 4 --only-crit", "impl 4 --only-crit --single-tab"] {
        let bindings = madev_bindings(line);
        assert_eq!(bindings.integer("from"), Some(1), "{line}");
        assert!(bindings.is_present("only_crit"), "{line}");
    }
    assert!(madev_bindings("impl 4 --only-crit --single-tab").is_present("single_tab"));
}

#[test]
fn requires_a_value_flag_that_has_no_default() {
    let content = "command \"c\"\nflag \"width\" value=\"w\"\n";
    assert_eq!(bind(content, "c --width 7").unwrap().integer("w"), Some(7));
    assert_eq!(
        bind(content, "c").unwrap_err(),
        "missing required flag --width"
    );
}

#[test]
fn refuses_a_repeated_flag() {
    assert_eq!(
        madev_refusal("impl 4 --crit-per-impl 2 --crit-per-impl 3"),
        "flag --crit-per-impl is given more than once"
    );
    assert_eq!(
        madev_refusal("impl 4 --single-tab --single-tab"),
        "flag --single-tab is given more than once"
    );
    assert_eq!(
        madev_refusal("impl 4 --only-crit 2 --only-crit"),
        "flag --only-crit is given more than once"
    );
}

#[test]
fn refuses_an_unknown_flag() {
    assert_eq!(madev_refusal("impl 4 --nope"), "unknown flag --nope");
    assert_eq!(madev_refusal("impl 4 --"), "unknown flag --");
}

#[test]
fn refuses_a_missing_or_non_integer_flag_value() {
    assert_eq!(
        madev_refusal("impl 4 --crit-per-impl"),
        "flag --crit-per-impl needs an integer value"
    );
    assert_eq!(
        madev_refusal("impl 4 --crit-per-impl two"),
        "flag --crit-per-impl needs an integer value, got \"two\""
    );
    assert_eq!(
        madev_refusal("impl 4 --crit-per-impl --single-tab"),
        "flag --crit-per-impl needs an integer value, got \"--single-tab\""
    );
}

#[test]
fn refuses_a_non_integer_missing_or_leftover_positional() {
    assert_eq!(
        madev_refusal("impl four"),
        "argument \"four\" must be an integer"
    );
    assert_eq!(madev_refusal("impl --single-tab"), "missing argument \"n\"");
    assert_eq!(madev_refusal("impl 4 5"), "unexpected argument \"5\"");
}

fn string_bindings(line: &str) -> Bindings {
    bind(STRING_GENERATOR, line).unwrap_or_else(|error| panic!("{line:?} refused: {error}"))
}

#[test]
fn binds_string_positionals_and_flag_values_as_whole_tokens() {
    let given = string_bindings("g auth 2 --branch dev");
    assert_eq!(given.text("feature"), Some("auth"));
    assert_eq!(given.text("b"), Some("dev"));
    assert_eq!(given.integer("n"), Some(2));

    // A string takes any token that is not a flag, digits and dashes included.
    let odd = string_bindings("g -1 2");
    assert_eq!(odd.text("feature"), Some("-1"));
    assert_eq!(odd.text("b"), Some("main"));
}

#[test]
fn takes_an_optional_string_value_unless_a_flag_follows() {
    assert_eq!(
        string_bindings("g auth 2 --model sonnet").text("model_name"),
        Some("sonnet")
    );
    for line in ["g auth 2 --model", "g auth 2 --model --branch dev"] {
        let bindings = string_bindings(line);
        assert_eq!(bindings.text("model_name"), Some("opus"), "{line}");
        assert!(bindings.is_present("model"), "{line}");
    }
    // Unlike an integer flag, a string flag swallows a following positional.
    assert_eq!(
        bind(STRING_GENERATOR, "g auth --model 2").unwrap_err(),
        "missing argument \"n\""
    );
}

#[test]
fn refuses_a_missing_string_flag_value() {
    assert_eq!(
        bind(STRING_GENERATOR, "g auth 2 --branch").unwrap_err(),
        "flag --branch needs a string value"
    );
    assert_eq!(
        bind(STRING_GENERATOR, "g auth 2 --branch --model").unwrap_err(),
        "flag --branch needs a string value, got \"--model\""
    );
}

/// A tab big enough for 64 panes under the default floors: 17 columns fit, so
/// four rows of 13 content rows each clear the 12-row floor.
fn wide_source(name: &str) -> SourceTab {
    SourceTab {
        name: name.to_string(),
        geometry: TabGeometry {
            columns: 1000,
            rows: 60,
        },
    }
}

fn source(name: &str) -> SourceTab {
    SourceTab {
        name: name.to_string(),
        geometry: TabGeometry {
            columns: 284,
            rows: 50,
        },
    }
}

fn open(content: &str, line: &str, source: &SourceTab) -> Result<Vec<CustomLayout>, String> {
    let generators = parse_generator_files(&[file("/g/impl.kdl", content)]).unwrap();
    invoke(&generators, line, source, FloorOverrides::default())
}

fn madev(line: &str) -> Vec<CustomLayout> {
    open(MADEV_GENERATOR, line, &source("src"))
        .unwrap_or_else(|error| panic!("{line:?} refused: {error}"))
}

fn names(layouts: &[CustomLayout]) -> Vec<&str> {
    layouts.iter().map(|layout| layout.id.as_str()).collect()
}

fn rows(layout: &CustomLayout) -> &Vec<Vec<String>> {
    match &layout.commands {
        CommandGrid::Rows(rows) => rows,
        CommandGrid::Flat(_) => panic!("a generator emits rows"),
    }
}

fn commands(layout: &CustomLayout) -> Vec<&str> {
    rows(layout)
        .iter()
        .flatten()
        .map(String::as_str)
        .collect()
}

fn row_shape(layout: &CustomLayout) -> Vec<usize> {
    rows(layout).iter().map(Vec::len).collect()
}

fn default_floors() -> PaneFloors {
    PaneFloors::resolve(FloorOverrides::default(), FloorOverrides::default())
}

#[test]
fn expands_the_madev_file_the_three_documented_ways() {
    let per_role = madev("impl 4 --crit-per-impl 2");
    assert_eq!(names(&per_role), ["src-impl", "src-crit1", "src-crit2"]);
    assert_eq!(commands(&per_role[0]).len(), 4);
    assert_eq!(commands(&per_role[0])[0], "claude -n impl1 '/madev-impl impl1'");
    assert_eq!(commands(&per_role[2])[3], "claude -n impl4-crit2 '/madev-impl-crit impl4-crit2'");

    // One tab, impl-major: every implementer sits beside its own critics.
    let single = madev("impl 4 --single-tab");
    assert_eq!(names(&single), ["src-1"]);
    assert_eq!(
        commands(&single[0]),
        [
            "claude -n impl1 '/madev-impl impl1'",
            "claude -n impl1-crit1 '/madev-impl-crit impl1-crit1'",
            "claude -n impl2 '/madev-impl impl2'",
            "claude -n impl2-crit1 '/madev-impl-crit impl2-crit1'",
            "claude -n impl3 '/madev-impl impl3'",
            "claude -n impl3-crit1 '/madev-impl-crit impl3-crit1'",
            "claude -n impl4 '/madev-impl impl4'",
            "claude -n impl4-crit1 '/madev-impl-crit impl4-crit1'",
        ]
    );

    let resumed = madev("impl 4 --crit-per-impl 2 --only-crit 2");
    assert_eq!(names(&resumed), ["src-crit2", "src-crit3"]);
    assert_eq!(commands(&resumed[0])[0], "claude -n impl1-crit2 '/madev-impl-crit impl1-crit2'");
}

#[test]
fn names_unnamed_tabs_after_the_source_tab_in_emission_order() {
    let content = "command \"g\"\narg \"n\"\neach for=\"k\" in=\"1..=n\" {\n tab {\n pane \"p{k}\"\n }\n}\n";
    let layouts = open(content, "g 3", &source("e2e")).unwrap();
    assert_eq!(names(&layouts), ["e2e-1", "e2e-2", "e2e-3"]);
    assert_eq!(commands(&layouts[2]), ["p3"]);
}

#[test]
fn orders_nested_each_loops_i_major() {
    let content = "command \"g\"\narg \"n\"\ntab {\n each for=\"i\" in=\"1..=n\" {\n each for=\"j\" in=\"1..=2\" {\n pane \"p{i}{j}\"\n }\n }\n}\n";
    let layouts = open(content, "g 2", &source("src")).unwrap();
    assert_eq!(commands(&layouts[0]), ["p11", "p12", "p21", "p22"]);
}

#[test]
fn substitutes_declared_names_and_leaves_other_braces_alone() {
    let content = "command \"g\"\narg \"n\"\ntab \"t\" {\n each for=\"i\" in=\"1..=n\" {\n pane \"run {i} ${HOME}/{a,b} {nope}\"\n }\n}\n";
    let layouts = open(content, "g 1", &source("src")).unwrap();
    assert_eq!(commands(&layouts[0]), ["run 1 ${HOME}/{a,b} {nope}"]);
}

#[test]
fn renders_strings_raw_in_tab_names_and_single_quoted_in_commands() {
    let layouts = open(STRING_GENERATOR, "g auth 2 --model", &source("src")).unwrap();
    assert_eq!(names(&layouts), ["src-auth"]);
    assert_eq!(
        commands(&layouts[0]),
        [
            "claude -n 'auth'1 '/madev-impl 'auth'1 'main'' --model 'opus'",
            "claude -n 'auth'2 '/madev-impl 'auth'2 'main'' --model 'opus'",
        ]
    );
}

#[test]
fn a_hostile_string_cannot_leave_its_shell_word() {
    let content = "command \"g\"\narg \"s\" type=\"string\"\ntab \"t\" {\n pane \"echo {s}\"\n}\n";
    let layouts = open(content, "g ';rm${HOME}'$(id)", &source("src")).unwrap();
    assert_eq!(commands(&layouts[0]), ["echo ''\\'';rm${HOME}'\\''$(id)'"]);
}

#[test]
fn judges_a_string_in_a_tab_name_as_it_judges_the_source_tab() {
    let content = "command \"g\"\narg \"s\" type=\"string\"\ntab \"{s}\" {\n pane \"p\"\n}\n";
    let long = format!("g {}", "x".repeat(200));
    let refusal = open(content, &long, &source("src")).unwrap_err();
    assert!(refusal.starts_with("impl.kdl: tab name"), "{refusal}");
    assert!(refusal.contains("exceeds"), "{refusal}");
    let control = open(content, "g a\u{1}b", &source("src")).unwrap_err();
    assert!(control.contains("control characters"), "{control}");
}

#[test]
fn conditions_apply_to_tab_pane_and_each_alike() {
    let content = "command \"c\"\narg \"n\"\nflag \"wide\"\nflag \"solo\"\n\
tab \"keep\" unless=\"solo\" {\n pane \"always\"\n pane \"wide-only\" if=\"wide\"\n \
each for=\"i\" in=\"1..=n\" if=\"wide\" {\n pane \"loop{i}\"\n }\n}\n\
tab \"solo-only\" if=\"solo\" {\n pane \"s\"\n}\n";

    let plain = open(content, "c 2", &source("src")).unwrap();
    assert_eq!(names(&plain), ["keep"]);
    assert_eq!(commands(&plain[0]), ["always"]);

    let wide = open(content, "c 2 --wide", &source("src")).unwrap();
    assert_eq!(commands(&wide[0]), ["always", "wide-only", "loop1", "loop2"]);

    let solo = open(content, "c 2 --solo", &source("src")).unwrap();
    assert_eq!(names(&solo), ["solo-only"]);
}

#[test]
fn plans_rows_with_the_larger_rows_last() {
    let geometry = TabGeometry {
        columns: 284,
        rows: 50,
    };
    assert_eq!(plan_rows(7, geometry, default_floors()).unwrap(), [3, 4]);
    assert_eq!(plan_rows(12, geometry, default_floors()).unwrap(), [4, 4, 4]);
    assert_eq!(plan_rows(1, geometry, default_floors()).unwrap(), [1]);
    assert_eq!(plan_rows(5, geometry, default_floors()).unwrap(), [5]);
    assert!(plan_rows(0, geometry, default_floors()).is_err());
}

#[test]
fn refuses_a_layout_that_does_not_fit_the_tab() {
    let narrow = TabGeometry {
        columns: 50,
        rows: 50,
    };
    let width = plan_rows(2, narrow, default_floors()).unwrap_err();
    assert!(width.contains("does not fit"), "{width}");

    let short = TabGeometry {
        columns: 284,
        rows: 20,
    };
    let height = plan_rows(12, short, default_floors()).unwrap_err();
    assert!(height.contains("does not fit"), "{height}");

    // Through invoke, the refusal carries the file's basename.
    let content = "command \"g\"\narg \"n\"\ntab {\n each for=\"i\" in=\"1..=n\" {\n pane \"p{i}\"\n }\n}\n";
    let refusal = open(
        content,
        "g 12",
        &SourceTab {
            name: "src".to_string(),
            geometry: short,
        },
    )
    .unwrap_err();
    assert!(refusal.starts_with("impl.kdl: "), "{refusal}");
    assert!(refusal.contains("does not fit"), "{refusal}");
}

#[test]
fn refuses_a_negative_overflowing_or_overlong_range() {
    let content = "command \"g\"\narg \"n\"\ntab {\n each for=\"i\" in=\"1..=n\" {\n pane \"p{i}\"\n }\n}\n";
    let negative = open(content, "g -4", &source("src")).unwrap_err();
    assert_eq!(
        negative,
        "impl.kdl: range \"1..=n\" must not run below zero, but evaluates to 1..=-4"
    );

    let overflow = "command \"g\"\narg \"n\"\ntab {\n each for=\"i\" in=\"n..=n+9223372036854775807\" {\n pane \"p{i}\"\n }\n}\n";
    assert_eq!(
        open(overflow, "g 1", &source("src")).unwrap_err(),
        "impl.kdl: range \"n..=n+9223372036854775807\" overflows"
    );

    let long = open(content, "g 200", &source("src")).unwrap_err();
    assert!(long.contains("runs for 200 steps"), "{long}");

    // The exclusive end is stepped back only after the sign check, so a bound of
    // i64::MIN refuses instead of underflowing.
    let floor = "command \"g\"\narg \"n\"\ntab {\n each for=\"i\" in=\"0..n-9223372036854775807\" {\n pane \"p{i}\"\n }\n}\n";
    assert!(open(floor, "g -1", &source("src"))
        .unwrap_err()
        .contains("must not run below zero"));
}

#[test]
fn an_empty_range_leaves_the_tab_empty_and_refuses() {
    let content = "command \"g\"\narg \"n\"\ntab \"t\" {\n each for=\"i\" in=\"1..n\" {\n pane \"p{i}\"\n }\n}\n";
    assert_eq!(
        open(content, "g 1", &source("src")).unwrap_err(),
        "impl.kdl: tab \"t\" has no panes"
    );
}

#[test]
fn refuses_an_invocation_that_opens_no_tabs() {
    let content = "command \"g\"\nflag \"solo\"\ntab \"t\" if=\"solo\" {\n pane \"p\"\n}\n";
    assert_eq!(
        open(content, "g", &source("src")).unwrap_err(),
        "impl.kdl: this invocation opens no tabs"
    );
}

#[test]
fn refuses_a_source_tab_name_that_cannot_be_an_id() {
    let content = "command \"g\"\ntab \"{tab}-t\" {\n pane \"p\"\n}\n";
    let long = "x".repeat(200);
    let refusal = open(content, "g", &source(&long)).unwrap_err();
    assert!(refusal.starts_with("impl.kdl: tab name"), "{refusal}");
    assert!(refusal.contains("exceeds"), "{refusal}");

    let control = open(content, "g", &source("a\u{1}b")).unwrap_err();
    assert!(control.contains("control characters"), "{control}");

    // A tab can be renamed to " x ", which CustomLayout::validate would reject
    // in its own voice if the generator did not judge it first.
    assert_eq!(
        open(content, "g", &source(" x ")).unwrap_err(),
        "impl.kdl: tab name \" x -t\" must not start or end with whitespace"
    );
}

#[test]
fn refuses_more_panes_than_the_maximum_before_finishing_expansion() {
    // The second `each` would refuse with a negative bound if it were ever
    // reached, so seeing the pane-cap message instead is what proves expansion
    // stopped at the 65th push rather than building all 4096 and refusing late.
    let content = "command \"g\"\ntab \"t\" {\n each for=\"i\" in=\"1..=64\" {\n each for=\"j\" in=\"1..=64\" {\n pane \"p{i}{j}\"\n }\n }\n each for=\"k\" in=\"0..0-1\" {\n pane \"never\"\n }\n}\n";
    assert_eq!(
        open(content, "g", &source("src")).unwrap_err(),
        "impl.kdl: tab \"t\" opens more than 64 panes"
    );

    // The boundary itself: 64 panes open, 65 refuse.
    let counted = "command \"g\"\narg \"n\"\ntab \"t\" {\n each for=\"i\" in=\"1..=n\" {\n pane \"p{i}\"\n }\n}\n";
    let full = open(counted, "g 64", &wide_source("src")).unwrap();
    assert_eq!(commands(&full[0]).len(), 64);
    assert_eq!(
        open(counted, "g 65", &source("src")).unwrap_err(),
        "impl.kdl: range \"1..=n\" runs for 65 steps; the maximum is 64"
    );
    // Two ranges of 8 reach 64 without either passing the per-range cap; 9x8
    // reaches 72 and only the pane cap can speak.
    let grid = "command \"g\"\narg \"a\"\narg \"b\"\ntab \"t\" {\n each for=\"i\" in=\"1..=a\" {\n each for=\"j\" in=\"1..=b\" {\n pane \"p{i}{j}\"\n }\n }\n}\n";
    assert_eq!(
        commands(&open(grid, "g 8 8", &wide_source("src")).unwrap()[0]).len(),
        64
    );
    assert_eq!(
        open(grid, "g 9 8", &source("src")).unwrap_err(),
        "impl.kdl: tab \"t\" opens more than 64 panes"
    );
}

#[test]
fn refuses_more_tabs_than_the_maximum() {
    // Nested each levels multiply past the per-range cap, so the total is
    // bounded where tabs are created rather than after they are all built.
    let nested = "command \"g\"\neach for=\"i\" in=\"1..=64\" {\n each for=\"j\" in=\"1..=64\" {\n tab {\n pane \"p\"\n }\n }\n}\n";
    assert_eq!(
        open(nested, "g", &source("src")).unwrap_err(),
        "impl.kdl: this invocation opens more than 64 tabs"
    );

    // A single range of 65 steps trips the per-range cap first, so it says
    // nothing about the tab cap -- pin which cap speaks.
    let counted = "command \"g\"\narg \"n\"\neach for=\"i\" in=\"1..=n\" {\n tab {\n pane \"p\"\n }\n}\n";
    assert_eq!(open(counted, "g 64", &source("src")).unwrap().len(), 64);
    assert_eq!(
        open(counted, "g 65", &source("src")).unwrap_err(),
        "impl.kdl: range \"1..=n\" runs for 65 steps; the maximum is 64"
    );

    // The tab cap's own boundary, with neither range above the per-range cap.
    let grid = "command \"g\"\narg \"a\"\narg \"b\"\neach for=\"i\" in=\"1..=a\" {\n each for=\"j\" in=\"1..=b\" {\n tab {\n pane \"p\"\n }\n }\n}\n";
    assert_eq!(open(grid, "g 16 4", &source("src")).unwrap().len(), 64);
    assert_eq!(
        open(grid, "g 13 5", &source("src")).unwrap_err(),
        "impl.kdl: this invocation opens more than 64 tabs"
    );
    assert_eq!(
        open(grid, "g 33 2", &source("src")).unwrap_err(),
        "impl.kdl: this invocation opens more than 64 tabs"
    );
}

#[test]
fn refuses_an_unknown_command() {
    let generators = parse_generator_files(&[file("/g/impl.kdl", MADEV_GENERATOR)]).unwrap();
    assert_eq!(
        invoke(&generators, "nope 4", &source("src"), FloorOverrides::default()).unwrap_err(),
        "Unknown custom state or generator \"nope\""
    );
}

#[test]
fn takes_the_files_floors_over_the_global_ones() {
    let content = "command \"g\"\narg \"n\"\nmin_pane_width 140\ntab {\n each for=\"i\" in=\"1..=n\" {\n pane \"p{i}\"\n }\n}\n";
    // 284 columns fit two panes at a 140-column floor, so four panes need two rows.
    let layouts = open(content, "g 4", &source("src")).unwrap();
    assert_eq!(row_shape(&layouts[0]), [2, 2]);
}

#[test]
fn single_quotes_the_source_tab_name_inside_a_command() {
    let content = "command \"g\"\ntab \"{tab}-t\" {\n pane \"printf %s {tab}\"\n}\n";
    for name in ["it's", "x'; id; '"] {
        let layouts = open(content, "g", &source(name)).unwrap();
        assert_eq!(layouts[0].id, format!("{name}-t"), "raw in the tab name");

        let emitted = shell_commands(&layouts);
        assert_eq!(emitted.len(), 1);
        let printed = Command::new("sh")
            .arg("-lc")
            .arg(&emitted[0])
            .output()
            .expect("sh runs");
        assert_eq!(
            String::from_utf8_lossy(&printed.stdout),
            *name,
            "{:?} must reach printf as one literal word",
            emitted[0]
        );
    }
}

/// The shell commands of a generated layout, read back out of the emitted KDL.
fn shell_commands(layouts: &[CustomLayout]) -> Vec<String> {
    let kdl = tabs_to_kdl(
        layouts,
        ZELLAUDE_URL,
        &BTreeMap::new(),
        None,
        &TabChrome::default(),
    )
    .unwrap();
    let parsed = Layout::from_kdl(&kdl, Some("generated.kdl".to_string()), None, None).unwrap();
    parsed
        .tabs
        .iter()
        .flat_map(|(_, tiled, _)| tiled.extract_run_instructions())
        .filter_map(|run| match run {
            Some(Run::Command(command)) => command.args.get(1).cloned(),
            _ => None,
        })
        .collect()
}

#[test]
fn every_generated_tab_round_trips_through_zellij_with_the_planned_geometry() {
    let content = "command \"g\"\narg \"n\"\ntab {\n each for=\"i\" in=\"1..=n\" {\n pane \"cmd{i}\"\n }\n}\n";
    let layouts = open(content, "g 7", &source("src")).unwrap();
    assert_eq!(row_shape(&layouts[0]), [3, 4]);

    let kdl = tabs_to_kdl(
        &layouts,
        ZELLAUDE_URL,
        &BTreeMap::new(),
        None,
        &TabChrome::default(),
    )
    .unwrap();
    let parsed = Layout::from_kdl(&kdl, Some("generated.kdl".to_string()), None, None).unwrap();
    assert_eq!(parsed.tabs.len(), 1);

    let mut space = PaneGeom::default();
    space.cols.set_inner(284);
    space.rows.set_inner(50);
    let mut positioned: Vec<(String, usize, usize, usize, usize)> = parsed.tabs[0]
        .1
        .position_panes_in_space(&space, None, false, false)
        .unwrap()
        .iter()
        .filter_map(|(pane, geometry)| match &pane.run {
            Some(Run::Command(command)) => Some((
                command.args.get(1).cloned().unwrap_or_default(),
                geometry.x,
                geometry.y,
                geometry.cols.as_usize(),
                geometry.rows.as_usize(),
            )),
            _ => None,
        })
        .collect();
    positioned.sort_by_key(|(_, x, y, _, _)| (*y, *x));
    let shape: Vec<(String, usize, usize)> = positioned
        .iter()
        .map(|(command, _, y, cols, _)| (command.clone(), *y, *cols))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("cmd1".to_string(), 1, 94),
            ("cmd2".to_string(), 1, 94),
            ("cmd3".to_string(), 1, 96),
            ("cmd4".to_string(), 26, 71),
            ("cmd5".to_string(), 26, 71),
            ("cmd6".to_string(), 26, 71),
            ("cmd7".to_string(), 26, 71),
        ]
    );
}
