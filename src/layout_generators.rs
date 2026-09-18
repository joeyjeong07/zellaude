//! Generator files: one KDL file per generator under the plugin's `generators`
//! directory, re-parsed when the prompt opens and again on every Enter, so an
//! edit lands on the next submit. A file declares its prompt-line vocabulary,
//! then a body of `tab`/`pane`/`each` nodes that expands into the tabs an
//! invocation opens.

use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::fmt::Write;

use crate::custom_layouts::{CommandGrid, CustomLayout, MAX_ID_CHARACTERS, MAX_PANES};

pub const DEFAULT_MIN_PANE_WIDTH: usize = 54; // content columns
pub const DEFAULT_MIN_PANE_HEIGHT: usize = 12; // content rows
pub const PANE_FRAME_COLUMNS: usize = 2; // Zellij frame cost per pane
pub const PANE_FRAME_ROWS: usize = 2;

/// Tabs one invocation may open. Shares MAX_PANES' value but not its meaning:
/// nested `each` levels multiply past the per-range cap, and the cost of those
/// tabs lands on Zellij rather than inside one tab.
pub const MAX_GENERATED_TABS: usize = 64;

/// Bound to the name of the tab the prompt was opened from.
const SOURCE_TAB_VARIABLE: &str = "tab";

/// The body nodes. Naming them as a type is what makes both walkers
/// exhaustive: a fourth one cannot be added without the compiler demanding that
/// each of them say where it belongs.
enum BodyNode {
    Tab,
    Pane,
    Each,
}

impl BodyNode {
    fn from_node_name(name: &str) -> Option<Self> {
        Some(match name {
            "tab" => Self::Tab,
            "pane" => Self::Pane,
            "each" => Self::Each,
            _ => return None,
        })
    }
}

/// The declaration nodes. Naming them as a type is what makes `declare`
/// exhaustive: a sixth one cannot be added without the compiler demanding it be
/// handled there too.
enum Declaration {
    Command,
    Arg,
    Flag,
    MinPaneWidth,
    MinPaneHeight,
}

impl Declaration {
    fn from_node_name(name: &str) -> Option<Self> {
        Some(match name {
            "command" => Self::Command,
            "arg" => Self::Arg,
            "flag" => Self::Flag,
            "min_pane_width" => Self::MinPaneWidth,
            "min_pane_height" => Self::MinPaneHeight,
            _ => return None,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct GeneratorFile {
    pub path: String,
    pub content: String,
}

/// The envelope the reload script prints: the raw zellaude.json text and every
/// generator file, in `LC_ALL=C` name order.
#[derive(Debug, Deserialize)]
pub struct CustomStateSources {
    pub settings_json: String,
    pub generator_files: Vec<GeneratorFile>,
}

/// One file's or zellaude.json's partial floors.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FloorOverrides {
    pub min_pane_width: Option<usize>,
    pub min_pane_height: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneFloors {
    pub min_pane_width: usize,
    pub min_pane_height: usize,
}

impl PaneFloors {
    pub fn resolve(file: FloorOverrides, global: FloorOverrides) -> Self {
        Self {
            min_pane_width: file
                .min_pane_width
                .or(global.min_pane_width)
                .unwrap_or(DEFAULT_MIN_PANE_WIDTH),
            min_pane_height: file
                .min_pane_height
                .or(global.min_pane_height)
                .unwrap_or(DEFAULT_MIN_PANE_HEIGHT),
        }
    }
}

/// Read the floor keys from the top level of zellaude.json. A document that is
/// not an object carries no floors; the custom-state parser reports its shape.
pub fn parse_floor_overrides(settings_json: &str) -> Result<FloorOverrides, String> {
    let Value::Object(object) =
        serde_json::from_str(settings_json).map_err(|error| error.to_string())?
    else {
        return Ok(FloorOverrides::default());
    };
    Ok(FloorOverrides {
        min_pane_width: floor_key(&object, "min_pane_width")?,
        min_pane_height: floor_key(&object, "min_pane_height")?,
    })
}

fn floor_key(object: &Map<String, Value>, key: &str) -> Result<Option<usize>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|floor| usize::try_from(floor).ok())
            .map(Some)
            .ok_or_else(|| format!("zellaude.json {key} must be a non-negative integer")),
    }
}

/// Cells the grid may use: the display area minus the bar rows the new tab
/// carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabGeometry {
    pub columns: usize,
    pub rows: usize,
}

/// The tab the prompt was opened from: bound as `{tab}`, and the size every
/// generated tab is planned against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTab {
    pub name: String,
    pub geometry: TabGeometry,
}

#[derive(Debug)]
pub struct LayoutGenerator {
    pub command: String,
    /// File basename, so every refusal names the file that caused it.
    pub source: String,
    args: Vec<ArgDeclaration>,
    flags: Vec<FlagDeclaration>,
    floors: FloorOverrides,
    body: Vec<TabNode>,
}

#[derive(Debug)]
struct ArgDeclaration {
    name: String,
    kind: ValueKind,
}

#[derive(Debug)]
struct FlagDeclaration {
    /// As typed on the prompt line, without the leading `--`.
    flag: String,
    presence: String,
    value: Option<FlagValue>,
}

#[derive(Debug)]
struct FlagValue {
    variable: String,
    kind: ValueKind,
    /// `--flag` may stand alone; `default` then supplies the value.
    optional: bool,
    /// Absent means the flag itself is required on the prompt line.
    default: Option<BoundValue>,
}

/// What a positional or a flag value parses as on the prompt line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Integer,
    Text,
}

impl ValueKind {
    fn variable_kind(self) -> VariableKind {
        match self {
            Self::Integer => VariableKind::Integer,
            Self::Text => VariableKind::Text,
        }
    }

    /// A string takes any token that is not a flag, so `-1` and `42` bind as
    /// text when text is what was declared.
    fn parse_token(self, token: &str) -> Option<BoundValue> {
        match self {
            Self::Integer => token.parse().ok().map(BoundValue::Integer),
            Self::Text => (!token.starts_with("--")).then(|| BoundValue::Text(token.to_string())),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Integer => "an integer",
            Self::Text => "a string",
        }
    }
}

/// One bound value: what `{name}` substitutes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BoundValue {
    Integer(i64),
    Text(String),
}

/// A node in tab position: the top level, or inside an `each` rooted there.
#[derive(Debug)]
enum TabNode {
    Tab {
        name: Option<Vec<TemplatePiece>>,
        condition: Condition,
        body: Vec<PaneNode>,
    },
    Each {
        variable: String,
        range: Range,
        condition: Condition,
        body: Vec<TabNode>,
    },
}

/// A node in pane position: inside a `tab`, or inside an `each` rooted there.
#[derive(Debug)]
enum PaneNode {
    Pane {
        command: Vec<TemplatePiece>,
        condition: Condition,
    },
    Each {
        variable: String,
        range: Range,
        condition: Condition,
        body: Vec<PaneNode>,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum TemplatePiece {
    Literal(String),
    Variable(String),
}

#[derive(Debug, Default)]
struct Condition {
    required: Vec<String>,
    forbidden: Vec<String>,
}

#[derive(Debug)]
struct Range {
    /// As written in `in=`, for refusals.
    text: String,
    start: Endpoint,
    end: Endpoint,
    inclusive: bool,
}

#[derive(Debug)]
struct Endpoint {
    head: Term,
    tail: Option<(Sign, Term)>,
}

#[derive(Debug)]
enum Term {
    Literal(i64),
    Variable(String),
}

#[derive(Debug, Clone, Copy)]
enum Sign {
    Plus,
    Minus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VariableKind {
    Integer,
    Presence,
    /// The built-in `tab` and every `type="string"` declaration: substituted
    /// in templates, never an operand.
    Text,
}

/// The variable names visible at one point of a file, innermost last.
#[derive(Debug)]
struct Scope(Vec<(String, VariableKind)>);

impl Scope {
    fn with_source_tab() -> Self {
        Self(vec![(
            SOURCE_TAB_VARIABLE.to_string(),
            VariableKind::Text,
        )])
    }

    fn declare(&mut self, name: &str, kind: VariableKind) -> Result<(), String> {
        validate_variable_name(name)?;
        if self.kind(name).is_some() {
            return Err(format!("variable {name:?} is already declared"));
        }
        self.0.push((name.to_string(), kind));
        Ok(())
    }

    fn kind(&self, name: &str) -> Option<VariableKind> {
        self.0
            .iter()
            .find(|(declared, _)| declared == name)
            .map(|(_, kind)| *kind)
    }

    fn depth(&self) -> usize {
        self.0.len()
    }

    fn unwind(&mut self, depth: usize) {
        self.0.truncate(depth);
    }
}

pub fn parse_generator_files(files: &[GeneratorFile]) -> Result<Vec<LayoutGenerator>, String> {
    let mut generators: Vec<LayoutGenerator> = Vec::with_capacity(files.len());
    for file in files {
        let source = basename(&file.path);
        let generator = parse_generator(&file.content, source)
            .map_err(|message| format!("{source}: {message}"))?;
        if let Some(earlier) = generators
            .iter()
            .find(|other| other.command == generator.command)
        {
            return Err(format!(
                "{source}: command {:?} is already declared by {}",
                generator.command, earlier.source
            ));
        }
        generators.push(generator);
    }
    Ok(generators)
}

/// The variable values one prompt line binds: positionals, flag values and the
/// flags that were given. Every name binds at most once -- a repeated flag
/// refuses, and the parser already refused a duplicate declaration.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Bindings {
    values: Vec<(String, BoundValue)>,
    presences: Vec<String>,
}

impl Bindings {
    pub(crate) fn integer(&self, name: &str) -> Option<i64> {
        match self.value(name)? {
            BoundValue::Integer(value) => Some(*value),
            BoundValue::Text(_) => None,
        }
    }

    /// Inspected by the binding tests; the plugin reads strings only through
    /// `render`. `allow(dead_code)` covers the `src/main.rs` `--test` build.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn text(&self, name: &str) -> Option<&str> {
        match self.value(name)? {
            BoundValue::Text(text) => Some(text),
            BoundValue::Integer(_) => None,
        }
    }

    pub(crate) fn is_present(&self, name: &str) -> bool {
        self.presences.iter().any(|bound| bound == name)
    }

    fn value(&self, name: &str) -> Option<&BoundValue> {
        self.values
            .iter()
            .find(|(bound, _)| bound == name)
            .map(|(_, value)| value)
    }

    fn bind(&mut self, name: &str, value: BoundValue) {
        self.values.push((name.to_string(), value));
    }

    fn bind_loop(&mut self, name: &str, value: i64) {
        self.bind(name, BoundValue::Integer(value));
    }

    fn unbind_loop(&mut self) {
        self.values.pop();
    }
}

impl Condition {
    fn holds(&self, bindings: &Bindings) -> bool {
        self.required.iter().all(|name| bindings.is_present(name))
            && !self.forbidden.iter().any(|name| bindings.is_present(name))
    }
}

/// Match the line's first whitespace-separated token against the declared
/// commands, returning the generator and the tokens left to bind.
pub(crate) fn select_generator<'a, 'b>(
    generators: &'a [LayoutGenerator],
    input: &'b str,
) -> Option<(&'a LayoutGenerator, Vec<&'b str>)> {
    let mut tokens = input.split_whitespace();
    let command = tokens.next()?;
    let generator = generators
        .iter()
        .find(|generator| generator.command == command)?;
    Some((generator, tokens.collect()))
}

impl LayoutGenerator {
    /// Bind the tokens after the command: positionals in declaration order,
    /// flags in any order, defaults for whatever the line left out.
    pub(crate) fn bind_arguments(&self, tokens: &[&str]) -> Result<Bindings, String> {
        let mut bindings = Bindings::default();
        let mut positionals: Vec<&str> = Vec::new();
        let mut index = 0;
        while let Some(token) = tokens.get(index) {
            index += 1;
            let Some(name) = token.strip_prefix("--") else {
                positionals.push(token);
                continue;
            };
            let flag = self
                .flags
                .iter()
                .find(|declared| declared.flag == name)
                .ok_or_else(|| format!("unknown flag --{name}"))?;
            if bindings.is_present(&flag.presence) {
                return Err(format!("flag --{name} is given more than once"));
            }
            bindings.presences.push(flag.presence.clone());
            let Some(value) = &flag.value else { continue };
            let noun = value.kind.noun();
            match tokens.get(index).and_then(|token| value.kind.parse_token(token)) {
                Some(parsed) => {
                    bindings.bind(&value.variable, parsed);
                    index += 1;
                }
                None if value.optional => {}
                None => {
                    return Err(match tokens.get(index) {
                        Some(token) => format!("flag --{name} needs {noun} value, got {token:?}"),
                        None => format!("flag --{name} needs {noun} value"),
                    })
                }
            }
        }

        for (position, arg) in self.args.iter().enumerate() {
            let Some(token) = positionals.get(position) else {
                return Err(format!("missing argument {:?}", arg.name));
            };
            let parsed = arg
                .kind
                .parse_token(token)
                .ok_or_else(|| format!("argument {token:?} must be {}", arg.kind.noun()))?;
            bindings.bind(&arg.name, parsed);
        }
        if let Some(token) = positionals.get(self.args.len()) {
            return Err(format!("unexpected argument {token:?}"));
        }

        for flag in &self.flags {
            let Some(value) = &flag.value else { continue };
            if bindings.value(&value.variable).is_some() {
                continue;
            }
            match &value.default {
                Some(default) => bindings.bind(&value.variable, default.clone()),
                None => return Err(format!("missing required flag --{}", flag.flag)),
            }
        }
        Ok(bindings)
    }
}

/// Expand a prompt line into one `CustomLayout` per tab it opens.
pub fn invoke(
    generators: &[LayoutGenerator],
    input: &str,
    source: &SourceTab,
    global_floors: FloorOverrides,
) -> Result<Vec<CustomLayout>, String> {
    let Some((generator, tokens)) = select_generator(generators, input) else {
        let command = input.split_whitespace().next().unwrap_or(input);
        return Err(format!("Unknown custom state or generator {command:?}"));
    };
    generator
        .expand(&tokens, source, global_floors)
        .map_err(|message| format!("{}: {message}", generator.source))
}

/// One tab an invocation opens, before its grid is planned.
struct EmittedTab {
    name: String,
    commands: Vec<String>,
}

/// Where a template renders, which decides how a string goes in. A tab name is
/// not shell, so it takes the string raw; a command is, so it takes it
/// single-quoted, and nothing typed on the prompt line or held in a tab name
/// can be re-parsed by `sh -lc`.
#[derive(Clone, Copy)]
enum TemplateTarget {
    TabName,
    ShellCommand,
}

impl LayoutGenerator {
    fn expand(
        &self,
        tokens: &[&str],
        source: &SourceTab,
        global_floors: FloorOverrides,
    ) -> Result<Vec<CustomLayout>, String> {
        let mut bindings = self.bind_arguments(tokens)?;
        bindings.bind(SOURCE_TAB_VARIABLE, BoundValue::Text(source.name.clone()));
        let mut tabs = Vec::new();
        expand_tabs(&self.body, &mut bindings, &source.name, &mut tabs)?;
        if tabs.is_empty() {
            return Err("this invocation opens no tabs".to_string());
        }
        let floors = PaneFloors::resolve(self.floors, global_floors);
        tabs.into_iter()
            .map(|tab| build_layout(tab, source.geometry, floors))
            .collect()
    }
}

fn expand_tabs(
    nodes: &[TabNode],
    bindings: &mut Bindings,
    source_name: &str,
    tabs: &mut Vec<EmittedTab>,
) -> Result<(), String> {
    for node in nodes {
        match node {
            TabNode::Tab {
                name,
                condition,
                body,
            } => {
                if !condition.holds(bindings) {
                    continue;
                }
                let name = match name {
                    Some(pieces) => render(pieces, bindings, TemplateTarget::TabName)?,
                    None => format!("{source_name}-{}", tabs.len() + 1),
                };
                validate_tab_name(&name)?;
                let mut commands = Vec::new();
                expand_panes(body, bindings, &name, &mut commands)?;
                if commands.is_empty() {
                    return Err(format!("tab {name:?} has no panes"));
                }
                tabs.push(EmittedTab { name, commands });
                if tabs.len() > MAX_GENERATED_TABS {
                    return Err(format!(
                        "this invocation opens more than {MAX_GENERATED_TABS} tabs"
                    ));
                }
            }
            TabNode::Each {
                variable,
                range,
                condition,
                body,
            } => {
                if !condition.holds(bindings) {
                    continue;
                }
                for value in evaluate_range(range, bindings)? {
                    bindings.bind_loop(variable, value);
                    let expanded = expand_tabs(body, bindings, source_name, tabs);
                    bindings.unbind_loop();
                    expanded?;
                }
            }
        }
    }
    Ok(())
}

fn expand_panes(
    nodes: &[PaneNode],
    bindings: &mut Bindings,
    tab: &str,
    commands: &mut Vec<String>,
) -> Result<(), String> {
    for node in nodes {
        match node {
            PaneNode::Pane { command, condition } => {
                if condition.holds(bindings) {
                    commands.push(render(command, bindings, TemplateTarget::ShellCommand)?);
                    if commands.len() > MAX_PANES {
                        return Err(format!(
                            "tab {tab:?} opens more than {MAX_PANES} panes"
                        ));
                    }
                }
            }
            PaneNode::Each {
                variable,
                range,
                condition,
                body,
            } => {
                if !condition.holds(bindings) {
                    continue;
                }
                for value in evaluate_range(range, bindings)? {
                    bindings.bind_loop(variable, value);
                    let expanded = expand_panes(body, bindings, tab, commands);
                    bindings.unbind_loop();
                    expanded?;
                }
            }
        }
    }
    Ok(())
}

fn evaluate_range(range: &Range, bindings: &Bindings) -> Result<Vec<i64>, String> {
    let text = &range.text;
    let start = evaluate_endpoint(&range.start, bindings, text)?;
    let end = evaluate_endpoint(&range.end, bindings, text)?;
    if start < 0 || end < 0 {
        return Err(format!(
            "range {text:?} must not run below zero, but evaluates to {start}..{}{end}",
            if range.inclusive { "=" } else { "" }
        ));
    }
    // Only now can the exclusive end be stepped back without underflowing.
    let last = if range.inclusive { end } else { end - 1 };
    let steps = last.saturating_sub(start).saturating_add(1).max(0);
    if steps > MAX_PANES as i64 {
        return Err(format!(
            "range {text:?} runs for {steps} steps; the maximum is {MAX_PANES}"
        ));
    }
    Ok((start..=last).collect())
}

fn evaluate_endpoint(endpoint: &Endpoint, bindings: &Bindings, text: &str) -> Result<i64, String> {
    let head = evaluate_term(&endpoint.head, bindings)?;
    let Some((sign, term)) = &endpoint.tail else {
        return Ok(head);
    };
    let tail = evaluate_term(term, bindings)?;
    match sign {
        Sign::Plus => head.checked_add(tail),
        Sign::Minus => head.checked_sub(tail),
    }
    .ok_or_else(|| format!("range {text:?} overflows"))
}

fn evaluate_term(term: &Term, bindings: &Bindings) -> Result<i64, String> {
    match term {
        Term::Literal(value) => Ok(*value),
        Term::Variable(name) => bindings
            .integer(name)
            .ok_or_else(|| format!("variable {name:?} is not bound")),
    }
}

fn render(
    pieces: &[TemplatePiece],
    bindings: &Bindings,
    target: TemplateTarget,
) -> Result<String, String> {
    let mut rendered = String::new();
    for piece in pieces {
        match piece {
            TemplatePiece::Literal(text) => rendered.push_str(text),
            TemplatePiece::Variable(name) => match bindings.value(name) {
                Some(BoundValue::Integer(value)) => {
                    let _ = write!(rendered, "{value}");
                }
                Some(BoundValue::Text(text)) => match target {
                    TemplateTarget::TabName => rendered.push_str(text),
                    TemplateTarget::ShellCommand => rendered.push_str(&shell_quote(text)),
                },
                None => return Err(format!("variable {name:?} is not bound")),
            },
        }
    }
    Ok(rendered)
}

/// A single-quoted word for `sh -lc`; the quote is the only character that can
/// leave such a word, so closing and re-opening around an escaped one is enough.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Judge a resolved name here, so a hostile or oversized string refuses in the
/// generator's voice rather than as a custom-state id further down.
fn validate_tab_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a tab name must not be empty".to_string());
    }
    if name.trim() != name {
        return Err(format!(
            "tab name {name:?} must not start or end with whitespace"
        ));
    }
    if name.chars().count() > MAX_ID_CHARACTERS {
        return Err(format!(
            "tab name {name:?} exceeds {MAX_ID_CHARACTERS} characters"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(format!(
            "tab name {name:?} must not contain control characters"
        ));
    }
    Ok(())
}

/// Panes per row, larger rows last, for a tab of `geometry` under `floors`.
pub fn plan_rows(
    pane_count: usize,
    geometry: TabGeometry,
    floors: PaneFloors,
) -> Result<Vec<usize>, String> {
    if pane_count == 0 {
        return Err("a tab must hold at least one pane".to_string());
    }
    let columns_per_pane = floors.min_pane_width.saturating_add(PANE_FRAME_COLUMNS);
    let max_columns = geometry.columns / columns_per_pane;
    if max_columns == 0 {
        return Err(format!(
            "a {pane_count}-pane layout does not fit: each pane needs {columns_per_pane} columns and the tab has {}",
            geometry.columns
        ));
    }
    let rows = pane_count.div_ceil(max_columns);
    let content_rows = (geometry.rows / rows) as i64 - PANE_FRAME_ROWS as i64;
    if content_rows < floors.min_pane_height as i64 {
        return Err(format!(
            "a {pane_count}-pane layout does not fit: {rows} rows leave {content_rows} rows per pane and the floor is {}",
            floors.min_pane_height
        ));
    }
    let base = pane_count / rows;
    let larger = pane_count % rows;
    Ok((0..rows)
        .map(|row| base + usize::from(row >= rows - larger))
        .collect())
}

fn build_layout(
    tab: EmittedTab,
    geometry: TabGeometry,
    floors: PaneFloors,
) -> Result<CustomLayout, String> {
    let rows = plan_rows(tab.commands.len(), geometry, floors)?;
    let mut commands = tab.commands.into_iter();
    let grid = rows
        .iter()
        .map(|count| commands.by_ref().take(*count).collect())
        .collect();
    let layout = CustomLayout {
        id: tab.name,
        width: None,
        height: None,
        commands: CommandGrid::Rows(grid),
    };
    layout.validate()?;
    Ok(layout)
}

fn parse_generator(content: &str, source: &str) -> Result<LayoutGenerator, String> {
    let document = content
        .parse::<KdlDocument>()
        .map_err(|error| format!("invalid KDL: {error}"))?;

    let mut declarations = Declarations::new();
    let mut body_nodes: Vec<&KdlNode> = Vec::new();
    for node in document.nodes() {
        let name = node.name().value();
        if BodyNode::from_node_name(name).is_some() {
            body_nodes.push(node);
        } else if let Some(declaration) = Declaration::from_node_name(name) {
            if !body_nodes.is_empty() {
                return Err(format!(
                    "{name:?} must be declared before the first tab, pane or each node"
                ));
            }
            declarations.declare(declaration, node)?;
        } else {
            return Err(format!("unknown node {name:?}"));
        }
    }

    let command = declarations
        .command
        .ok_or_else(|| "a generator file must declare a command".to_string())?;
    let body = parse_tab_nodes(&body_nodes, &mut declarations.scope)?;
    Ok(LayoutGenerator {
        command,
        source: source.to_string(),
        args: declarations.args,
        flags: declarations.flags,
        floors: declarations.floors,
        body,
    })
}

/// What the declaration nodes of one file build up.
#[derive(Debug)]
struct Declarations {
    command: Option<String>,
    args: Vec<ArgDeclaration>,
    flags: Vec<FlagDeclaration>,
    floors: FloorOverrides,
    scope: Scope,
}

impl Declarations {
    fn new() -> Self {
        Self {
            command: None,
            args: Vec::new(),
            flags: Vec::new(),
            floors: FloorOverrides::default(),
            scope: Scope::with_source_tab(),
        }
    }

    fn declare(&mut self, declaration: Declaration, node: &KdlNode) -> Result<(), String> {
        reject_children(node)?;
        reject_unknown_properties(node)?;
        match declaration {
            Declaration::Command => {
                if self.command.is_some() {
                    return Err("command is declared more than once".to_string());
                }
                let command = single_string_argument(node)?;
                if command.split_whitespace().count() != 1 {
                    return Err(format!("command {command:?} must be a single word"));
                }
                self.command = Some(command.to_string());
            }
            Declaration::Arg => {
                let name = single_string_argument(node)?;
                let kind = parse_value_kind(node)?;
                self.scope.declare(name, kind.variable_kind())?;
                self.args.push(ArgDeclaration {
                    name: name.to_string(),
                    kind,
                });
            }
            Declaration::Flag => self.declare_flag(node)?,
            Declaration::MinPaneWidth => set_floor(&mut self.floors.min_pane_width, node)?,
            Declaration::MinPaneHeight => set_floor(&mut self.floors.min_pane_height, node)?,
        }
        Ok(())
    }

    fn declare_flag(&mut self, node: &KdlNode) -> Result<(), String> {
        let flag = single_string_argument(node)?;
        validate_flag_name(flag)?;
        let value = parse_flag_value(node, flag)?;
        let presence = flag.replace('-', "_");
        self.scope.declare(&presence, VariableKind::Presence)?;
        if let Some(value) = &value {
            self.scope
                .declare(&value.variable, value.kind.variable_kind())?;
        }
        self.flags.push(FlagDeclaration {
            flag: flag.to_string(),
            presence,
            value,
        });
        Ok(())
    }
}

/// The `type` property of an arg or flag; integer when absent.
fn parse_value_kind(node: &KdlNode) -> Result<ValueKind, String> {
    match property_string(node, "type")? {
        None | Some("integer") => Ok(ValueKind::Integer),
        Some("string") => Ok(ValueKind::Text),
        Some(_) => Err("property \"type\" must be \"integer\" or \"string\"".to_string()),
    }
}

fn parse_default(node: &KdlNode, kind: ValueKind) -> Result<Option<BoundValue>, String> {
    Ok(match kind {
        ValueKind::Integer => property_integer(node, "default")?.map(BoundValue::Integer),
        ValueKind::Text => {
            property_string(node, "default")?.map(|text| BoundValue::Text(text.to_string()))
        }
    })
}

fn parse_flag_value(node: &KdlNode, flag: &str) -> Result<Option<FlagValue>, String> {
    let required_value = property_string(node, "value")?;
    let optional_value = property_string(node, "optional-value")?;
    let kind = parse_value_kind(node)?;
    let default = parse_default(node, kind)?;
    match (required_value, optional_value) {
        (Some(_), Some(_)) => Err(format!(
            "flag {flag:?} sets both value and optional-value"
        )),
        (Some(variable), None) => Ok(Some(FlagValue {
            variable: variable.to_string(),
            kind,
            optional: false,
            default,
        })),
        (None, Some(variable)) => {
            if default.is_none() {
                return Err(format!(
                    "flag {flag:?} sets optional-value and needs a default, since {:?} would be unbound when the flag stands alone",
                    variable
                ));
            }
            Ok(Some(FlagValue {
                variable: variable.to_string(),
                kind,
                optional: true,
                default,
            }))
        }
        (None, None) if default.is_some() => {
            Err(format!("flag {flag:?} sets a default without a value"))
        }
        (None, None) if node.get("type").is_some() => {
            Err(format!("flag {flag:?} sets a type without a value"))
        }
        (None, None) => Ok(None),
    }
}

fn set_floor(slot: &mut Option<usize>, node: &KdlNode) -> Result<(), String> {
    let name = node.name().value();
    if slot.is_some() {
        return Err(format!("{name} is declared more than once"));
    }
    *slot = Some(single_usize_argument(node)?);
    Ok(())
}

/// Refuse an unknown node before anything else, so a misspelling is the message
/// the user sees rather than whatever property check it happens to trip.
fn begin_body_node(node: &KdlNode, scope: &Scope) -> Result<(BodyNode, Condition), String> {
    let name = node.name().value();
    let Some(kind) = BodyNode::from_node_name(name) else {
        return Err(format!("unknown node {name:?}"));
    };
    reject_unknown_properties(node)?;
    Ok((kind, parse_condition(node, scope)?))
}

fn parse_tab_nodes(nodes: &[&KdlNode], scope: &mut Scope) -> Result<Vec<TabNode>, String> {
    let mut body = Vec::with_capacity(nodes.len());
    for node in nodes {
        let (kind, condition) = begin_body_node(node, scope)?;
        body.push(match kind {
            BodyNode::Tab => TabNode::Tab {
                name: match node_arguments(node).as_slice() {
                    [] => None,
                    [value] => Some(parse_template(expect_string(value, "a tab name")?, scope)?),
                    _ => return Err("a tab node takes at most one name argument".to_string()),
                },
                condition,
                body: parse_pane_nodes(&child_nodes(node), scope)?,
            },
            BodyNode::Each => {
                let (variable, range, body) = parse_each_node(node, scope, parse_tab_nodes)?;
                TabNode::Each {
                    variable,
                    range,
                    condition,
                    body,
                }
            }
            BodyNode::Pane => return Err("a pane node must be inside a tab".to_string()),
        });
    }
    Ok(body)
}

fn parse_pane_nodes(nodes: &[&KdlNode], scope: &mut Scope) -> Result<Vec<PaneNode>, String> {
    let mut body = Vec::with_capacity(nodes.len());
    for node in nodes {
        let (kind, condition) = begin_body_node(node, scope)?;
        body.push(match kind {
            BodyNode::Pane => {
                reject_children(node)?;
                let [value] = node_arguments(node)[..] else {
                    return Err("a pane node takes exactly one command argument".to_string());
                };
                PaneNode::Pane {
                    command: parse_template(expect_string(value, "a pane command")?, scope)?,
                    condition,
                }
            }
            BodyNode::Each => {
                let (variable, range, body) = parse_each_node(node, scope, parse_pane_nodes)?;
                PaneNode::Each {
                    variable,
                    range,
                    condition,
                    body,
                }
            }
            BodyNode::Tab => {
                return Err("a tab node must not be nested inside another tab".to_string())
            }
        });
    }
    Ok(body)
}

/// The loop variable is in scope only for the block, and never for its own range.
fn parse_each_node<T>(
    node: &KdlNode,
    scope: &mut Scope,
    parse_body: fn(&[&KdlNode], &mut Scope) -> Result<Vec<T>, String>,
) -> Result<(String, Range, Vec<T>), String> {
    if !node_arguments(node).is_empty() {
        return Err("an each node takes no arguments".to_string());
    }
    let variable = required_property_string(node, "for")?;
    let range = parse_range(required_property_string(node, "in")?, scope)?;
    let depth = scope.depth();
    scope.declare(variable, VariableKind::Integer)?;
    let body = parse_body(&child_nodes(node), scope)?;
    scope.unwind(depth);
    Ok((variable.to_string(), range, body))
}

fn parse_condition(node: &KdlNode, scope: &Scope) -> Result<Condition, String> {
    Ok(Condition {
        required: parse_presence_list(node, "if", scope)?,
        forbidden: parse_presence_list(node, "unless", scope)?,
    })
}

fn parse_presence_list(
    node: &KdlNode,
    property: &str,
    scope: &Scope,
) -> Result<Vec<String>, String> {
    let Some(raw) = property_string(node, property)? else {
        return Ok(Vec::new());
    };
    raw.split_whitespace()
        .map(|name| match scope.kind(name) {
            Some(VariableKind::Presence) => Ok(name.to_string()),
            Some(_) => Err(format!(
                "variable {name:?} in {property} is not a flag presence"
            )),
            None => Err(format!("unknown variable {name:?} in {property}")),
        })
        .collect()
}

fn parse_range(raw: &str, scope: &Scope) -> Result<Range, String> {
    let raw = raw.trim();
    let Some(separator) = raw.find("..") else {
        return Err(format!("range {raw:?} needs a '..' or '..=' separator"));
    };
    let rest = &raw[separator + 2..];
    let (inclusive, end) = match rest.strip_prefix('=') {
        Some(end) => (true, end),
        None => (false, rest),
    };
    Ok(Range {
        text: raw.to_string(),
        start: parse_endpoint(&raw[..separator], scope)?,
        end: parse_endpoint(end, scope)?,
        inclusive,
    })
}

fn parse_endpoint(raw: &str, scope: &Scope) -> Result<Endpoint, String> {
    let raw = raw.trim();
    let mut operators = raw
        .char_indices()
        .filter(|(_, character)| matches!(character, '+' | '-'));
    let operator = operators.next();
    if operators.next().is_some() {
        return Err(format!("range bound {raw:?} may use at most one '+' or '-'"));
    }
    let (head, tail) = match operator {
        Some((0, _)) => {
            return Err(format!(
                "range bound {raw:?} must start with a number or a variable"
            ))
        }
        Some((index, character)) => {
            let sign = if character == '+' { Sign::Plus } else { Sign::Minus };
            let term = parse_term(&raw[index + 1..], scope)?;
            (&raw[..index], Some((sign, term)))
        }
        None => (raw, None),
    };
    Ok(Endpoint {
        head: parse_term(head, scope)?,
        tail,
    })
}

fn parse_term(raw: &str, scope: &Scope) -> Result<Term, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("a range bound is empty".to_string());
    }
    if raw.chars().all(|character| character.is_ascii_digit()) {
        return raw
            .parse::<i64>()
            .map(Term::Literal)
            .map_err(|_| format!("range bound {raw:?} is too large"));
    }
    match scope.kind(raw) {
        Some(VariableKind::Integer) => Ok(Term::Variable(raw.to_string())),
        Some(_) => Err(format!("range bound {raw:?} is not an integer variable")),
        None => Err(format!("unknown variable {raw:?} in a range")),
    }
}

/// Split a name or command into literals and substitutions. Only a brace group
/// naming an integer or string variable substitutes, so shell syntax such as
/// `${HOME}` and `{a,b}` survives verbatim.
fn parse_template(raw: &str, scope: &Scope) -> Result<Vec<TemplatePiece>, String> {
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut rest = raw;
    while let Some(open) = rest.find('{') {
        let after_open = &rest[open + 1..];
        let name = after_open
            .find('}')
            .map(|close| (&after_open[..close], close));
        match name.map(|(name, close)| (name, close, scope.kind(name))) {
            Some((name, _, Some(VariableKind::Presence))) => {
                return Err(format!(
                    "variable {name:?} is a flag presence and is legal only in if or unless"
                ))
            }
            Some((name, close, Some(_))) => {
                literal.push_str(&rest[..open]);
                if !literal.is_empty() {
                    pieces.push(TemplatePiece::Literal(std::mem::take(&mut literal)));
                }
                pieces.push(TemplatePiece::Variable(name.to_string()));
                rest = &after_open[close + 1..];
            }
            _ => {
                literal.push_str(&rest[..=open]);
                rest = after_open;
            }
        }
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        pieces.push(TemplatePiece::Literal(literal));
    }
    Ok(pieces)
}

fn validate_variable_name(name: &str) -> Result<(), String> {
    let mut characters = name.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !valid {
        return Err(format!(
            "variable name {name:?} must be a letter or '_' followed by letters, digits or '_'"
        ));
    }
    Ok(())
}

fn validate_flag_name(flag: &str) -> Result<(), String> {
    let mut characters = flag.chars();
    let valid = characters.next().is_some_and(|first| first.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if !valid {
        return Err(format!(
            "flag name {flag:?} must be a letter followed by letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

fn allowed_properties(node_name: &str) -> &'static [&'static str] {
    match node_name {
        "arg" => &["type"],
        "flag" => &["value", "optional-value", "default", "type"],
        "tab" | "pane" => &["if", "unless"],
        "each" => &["for", "in", "if", "unless"],
        _ => &[],
    }
}

fn reject_unknown_properties(node: &KdlNode) -> Result<(), String> {
    let name = node.name().value();
    let allowed = allowed_properties(name);
    for (property, _) in node_properties(node) {
        if !allowed.contains(&property) {
            return Err(format!("unknown property {property:?} on a {name} node"));
        }
    }
    Ok(())
}

fn reject_children(node: &KdlNode) -> Result<(), String> {
    if node.children().is_some() {
        return Err(format!(
            "a {} node must not have a block",
            node.name().value()
        ));
    }
    Ok(())
}

fn child_nodes(node: &KdlNode) -> Vec<&KdlNode> {
    node.children()
        .map(|children| children.nodes().iter().collect())
        .unwrap_or_default()
}

fn node_arguments(node: &KdlNode) -> Vec<&KdlValue> {
    node.entries()
        .iter()
        .filter(|entry| entry.name().is_none())
        .map(|entry| entry.value())
        .collect()
}

fn node_properties(node: &KdlNode) -> Vec<(&str, &KdlValue)> {
    node.entries()
        .iter()
        .filter_map(|entry| entry.name().map(|name| (name.value(), entry.value())))
        .collect()
}

fn property_string<'a>(node: &'a KdlNode, key: &str) -> Result<Option<&'a str>, String> {
    node.get(key)
        .map(|entry| expect_string(entry.value(), &format!("property {key:?}")))
        .transpose()
}

fn required_property_string<'a>(node: &'a KdlNode, key: &str) -> Result<&'a str, String> {
    property_string(node, key)?.ok_or_else(|| {
        format!(
            "a {} node needs a {key:?} property",
            node.name().value()
        )
    })
}

fn property_integer(node: &KdlNode, key: &str) -> Result<Option<i64>, String> {
    node.get(key)
        .map(|entry| {
            entry
                .value()
                .as_i64()
                .ok_or_else(|| format!("property {key:?} must be an integer"))
        })
        .transpose()
}

fn expect_string<'a>(value: &'a KdlValue, what: &str) -> Result<&'a str, String> {
    value
        .as_string()
        .ok_or_else(|| format!("{what} must be a string"))
}

fn single_string_argument(node: &KdlNode) -> Result<&str, String> {
    let name = node.name().value();
    let [value] = node_arguments(node)[..] else {
        return Err(format!("a {name} node takes exactly one argument"));
    };
    expect_string(value, &format!("the {name} argument"))
}

fn single_usize_argument(node: &KdlNode) -> Result<usize, String> {
    let name = node.name().value();
    let [value] = node_arguments(node)[..] else {
        return Err(format!("a {name} node takes exactly one argument"));
    };
    value
        .as_i64()
        .and_then(|number| usize::try_from(number).ok())
        .ok_or_else(|| format!("{name} must be a non-negative integer"))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
