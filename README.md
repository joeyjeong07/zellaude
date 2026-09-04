# Zellaude

A Zellij status bar plugin that replaces the default tab bar with Claude Code and Codex activity awareness.

![Zellaude status bar example](assets/bar-example.svg)

## Features

- **Full tab bar** — shows all Zellij tabs (not just agent sessions), replacing the native tab bar
- **Session & mode display** — shows the Zellij session name and current input mode (NORMAL, LOCKED, PANE, etc.) with color-coded indicators
- **Live activity indicators** — see what every Claude Code and Codex session is doing at a glance; non-agent tabs remain visible without activity glyphs
- **Attach-time recovery** — recognizes agent sessions and effort modes that were already running when the status bar attached
- **Theme-aware palette** — follows Zellij's live theme colors; Gruvbox Dark is explicitly verified
- **Ultra-mode rainbow** — tab names shimmer through rainbow colors for Codex `ultra` sessions and Claude Code `ultracode` sessions
- **Split Three** — upgraded Pane-mode versions of Split Right and Split Down create three equal panes at once
- **Custom states** — open a named command grid in a new tab with `Ctrl+t`, `Shift+n`; generator files expand one CLI-style line into a set of tabs, each laid out to fit the live tab size
- **Session templates** — describe a multi-tab session once and start it with `zellij -s work -n <name>`
- **Clickable tabs** — click any tab to switch to it
- **Smart pane focus** — clicking an agent-aware tab focuses its most recently active Claude Code or Codex pane, revealing it inside a stack; waiting (⚠) sessions retain priority
- **Permission flash** — sessions pulse with the theme's error color for 2 seconds when a permission request arrives
- **Desktop notifications** — macOS notification on permission requests (rate-limited to once per 10s per tab), with click-to-focus support via [terminal-notifier](https://github.com/julienXX/terminal-notifier)
- **Elapsed time** — shows how long a session has been in its current state (after 30s), making it easy to spot stuck sessions
- **Multi-instance sync** — all Zellij tabs show a unified view of all sessions

### Activity symbols

| Symbol | Meaning |
|--------|---------|
| ◆ | Session starting |
| ● | Thinking |
| ⚡ | Running Bash |
| ◉ | Reading / searching files |
| ✎ | Editing / writing files |
| ⊜ | Spawning subagent |
| ◈ | Web search / fetch |
| ⚙ | Other tool |
| ▶ | Waiting for user prompt |
| ⚠ | Waiting for permission |
| ✓ | Done |
| ○ | Idle |

Indicator colors come from the active Zellij theme. Ultra-mode tab names remain an intentional animated RGB exception.

### Pane mode bindings

Open Pane mode with `Ctrl+p`, then use:

| Key | Action |
|-----|--------|
| `Shift+r` | **Split Three Right** — split the current pane into three equal-width columns |
| `Shift+d` | **Split Three Down** — split the current pane into three equal-height rows |

Both commands focus the newest pane and return to Normal mode, matching Zellij's built-in Split Right (`r`) and Split Down (`d`) flow. When the available cells are not divisible by three, pane sizes differ by at most one cell. Zellaude installs the uppercase bindings for the running client without writing to `config.kdl`. If either key already has a custom Pane-mode binding, Zellaude leaves it untouched. Approve Zellij's **Change runtime configuration** and **Execute actions as the user** permissions when prompted so the session-only bindings and exact resize sequence can run.

### Custom states

Add one or more named states to `~/.config/zellij/plugins/zellaude.json`:

```json
{
  "custom_states": [
    {
      "id": "claude6",
      "width": 3,
      "height": 2,
      "commands": [
        "claude -n A1 \"/implementing-agent I'm A1\"",
        "claude -n A2 \"/implementing-agent I'm A2\"",
        "claude -n A3 \"/implementing-agent I'm A3\"",
        "claude -n A4 \"/implementing-agent I'm A4\"",
        "claude -n A5 \"/implementing-agent I'm A5\"",
        "claude -n A6 \"/implementing-agent I'm A6\""
      ]
    }
  ]
}
```

Zellaude re-reads this file, and every generator file, when the prompt opens and again on every Enter, so an edit applies to the next submit without reloading the plugin or restarting the session. A refusal leaves the prompt open with what you typed, so you can fix the file it names and press Enter again. Press `Ctrl+t`, `Shift+n`, type the state ID, and press `Enter`. Zellaude opens a new tab with the configured grid, mapping the command array to panes from left to right, top to bottom. Press `Esc` or `Ctrl+c` to cancel the prompt.

The new tab repeats the bars of the tab it was opened from — Zellaude above the grid, Zellij's status bar below it. Zellij parses a plugin-created layout on its own, so a generated tab never inherits `default_tab_template`; anything the layout leaves out is simply missing from that tab.

`width` and `height` may be JSON numbers or numeric strings. A state needs at least one command, the command count may not exceed `width × height`, and a state may contain at most 64 panes; Zellij has to fit the resulting grid in the tab. A state's grid is laid out exactly as configured — the minimum-pane floors described under [Generators](#generators) apply only to generated tabs. When there are fewer commands than cells, the unused bottom-right cells open as normal shell panes.

For counts that don't fill a rectangle, `commands` may instead be an array of arrays. Each inner array is one row, and the row shapes are the layout — rows may hold different pane counts, each row splits its full width equally among its panes, and no filler cells appear:

```json
{
  "custom_states": [
    {
      "id": "claude5",
      "commands": [
        ["claude -n A1", "claude -n A2"],
        ["claude -n A3", "claude -n A4", "claude -n A5"]
      ]
    }
  ]
}
```

A nested state takes no `width` or `height` (setting either is an error), every row needs at least one command, and the 64-pane cap still applies. Commands run through `sh -lc` in the directory of the pane where the prompt was opened when Zellij can resolve it, falling back to Zellij's default working directory otherwise. Custom-state configuration should therefore be treated as trusted local shell code.

The settings file may also contain a single state object or an array of states, although the `custom_states` wrapper is recommended because it coexists with Zellaude's UI settings. As an alternative, states can be supplied directly in the plugin block; this takes precedence over the settings file:

```kdl
plugin location="file:~/.config/zellij/plugins/zellaude.wasm" {
    custom_states r#"[{"id":"shells","width":2,"height":1,"commands":["htop","git status"]}]"#
}
```

Zellaude installs `Shift+n` in Tab mode for the running client without changing `config.kdl`. If that key already has a user binding, the user binding wins and the custom-state shortcut remains unavailable.

### Generators

A generator turns one prompt line into a whole layout. Put one KDL file per generator in `~/.config/zellij/plugins/zellaude/generators/`; the first word of the line picks the file and the rest are its arguments. Files are re-read when the prompt opens and again on Enter, so a new generator works without restarting anything, and an edit lands on the very next submit.

`~/.config/zellij/plugins/zellaude/generators/madev.kdl`:

```kdl
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
```

Opened from a tab named `work`, that file answers:

| Prompt line | Tabs it opens |
| --- | --- |
| `impl 4 --crit-per-impl 2` | `work-impl`, `work-crit1`, `work-crit2`, four panes each |
| `impl 4 --single-tab` | `work-1`, holding eight panes: impl1, impl1-crit1, impl2, impl2-crit1, … |
| `impl 4 --crit-per-impl 2 --only-crit 2` | `work-crit2` and `work-crit3` only |

KDL wants a node terminator before a closing brace on the same line, so write blocks across lines as above, or end the inner node with a semicolon: `tab { pane "a"; }`.

#### Declarations

Declarations come first, before any `tab`, `pane` or `each`:

| Node | Meaning |
| --- | --- |
| `command "impl"` | the first word of the prompt line; unique across files |
| `arg "n"` | a positional integer, required, in declaration order |
| `flag "crit-per-impl" value="m" default=1` | `--crit-per-impl <int>`, bound to `m`. `default` supplies the value when the flag is absent; a `value` flag without a `default` is required on the line |
| `flag "single-tab"` | a flag that carries no value |
| `flag "only-crit" optional-value="from" default=1` | `--only-crit [<int>]`: the value may be left off and `default` then supplies it. `optional-value` without a `default` is an error, since the bare form would leave `from` unbound |
| `min_pane_width 54`, `min_pane_height 12` | this file's minimum pane size, in content cells |

Every flag also declares a **presence** variable named after it, with `-` replaced by `_`, so `single-tab` gives `single_tab`. Presences are what `if` and `unless` test. Integers — positionals, flag values and `each` variables — are what `{…}` substitutes and what ranges count over. Using either kind in the other's place is an error, as is naming anything twice.

#### Body

`tab` sits at the top level, or inside an `each` that does; `pane` sits inside a `tab`; `each` nests anywhere. All three accept `if="a b"`, true when every named presence was given, and `unless="a b"`, true when none was — a false condition on an `each` skips its whole expansion.

A `tab` with a name argument takes a template. One without is named `<source tab>-<ordinal>`, counting the tabs the line opens from 1, which is where `work-1` above comes from.

`each for="k" in="from..from+m"` counts over a Rust-style range: `a..b` stops before `b`, `a..=b` includes it. Each end is an integer or a variable, optionally plus or minus one more term — one operator, no chaining. An empty range expands to nothing; a negative or overflowing bound refuses.

A `tab` that ends up with no panes refuses, and so does a line that opens no tabs at all.

#### Substitution

`{name}` is replaced for the names a file declares, plus the built-in `{tab}` — the name of the tab the prompt was opened from. **Anything else in braces is left alone**, so `${HOME}` and `{a,b}` reach the shell untouched. That is also the one place the language stays quiet rather than refusing: a misspelt `{crti}` is passed through as text instead of reported.

`{tab}` goes into a tab name raw and into a command single-quoted, so no tab name can change how `sh -lc` parses the command around it. Being a name rather than a number, it belongs only in templates: it cannot be tested by `if` or counted over in a range.

#### Pane size

Zellaude plans each generated tab from the live size of the tab the prompt was opened in. It fits as many columns as `min_pane_width` allows, splits the panes into rows with the larger rows last, and refuses with `does not fit` when a pane would fall below either floor. Floors are taken from the generator file first, then from the top level of `zellaude.json`, then from the built-in 54 × 12:

```json
{
  "min_pane_width": 100,
  "min_pane_height": 5
}
```

#### Limits and refusals

Two separate limits share the number 64. A single `each` may not run for more than 64 steps, checked before the range runs; and one prompt line may not open more than 64 tabs, checked as each tab is created. Nesting is why both exist: `each` inside `each` multiplies past the first limit, and the second catches the product. A tab still holds at most 64 panes, as a fixed state does. A typo such as `impl 1000000000` therefore costs nothing.

On the line itself, positionals fill in declaration order and may sit among the flags, which may come in any order. An unknown flag, a missing required flag, a missing or non-integer value, a repeated flag, or a leftover argument all refuse. A flag whose value is optional takes the next token whenever it reads as an integer, so write `impl 4 --only-crit`, not `impl --only-crit 4`.

Every refusal from a generator names the file it came from, so `madev.kdl: unknown node "pnae"` points at the file to fix. A file that fails to parse holds back the whole set: Zellaude reports the error and keeps the generators from the last successful read, so one broken file never silently drops the others. A line matching no state and no generator refuses on its own, without a file name.

Generator files run their pane commands through `sh -lc`, exactly as custom states do, and are equally trusted local shell code. The file itself contains nothing Zellaude evaluates: every value in it is an integer, a presence, or a tab name.

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
top-to-bottom. A tab may contain at most 64 panes, and its command count may
not exceed `width × height`; when there are fewer commands than cells, the
spare cells open as plain shell panes at the reading-order tail. Setting
`width` or `height` without `commands` is a config error. A tab without
`commands` is a plain shell tab, and a template needs at least one tab.
`focus` starts the session on that tab — at most one tab may set it, and
setting it on more than one is a config error. Commands run through `sh -lc`,
so template configuration is trusted local shell code, exactly as for custom
states.

Omitting `cwd` opens panes in the directory `zellij` was run from, which is
usually what you want. A relative `cwd` resolves against that same directory,
so `"src"` means `<where you ran zellij>/src`; absolute paths are used as
given, and `~` is expanded at compile time. A tab's `cwd` overrides the
template's.

Templates are compiled to `~/.config/zellij/layouts/<name>.kdl`, so template
names are held to filesystem- and flag-safe rules: 1 to 64 characters, only
letters, digits, `.`, `_` and `-`, never `.` or `..`, and never starting with
`-`. Every generated file begins with a `// zellaude-generated` marker, and
Zellaude only ever writes or removes files carrying it — a layout you wrote
yourself is never overwritten, even if a template shares its name. Problems
are reported to Zellij's own log rather than the bar — by default
`/tmp/zellij-<uid>/zellij-log/zellij.log`, written on every run regardless of
`--debug`; that flag only raises its verbosity.

### Launch environment

Agents are often started with their environment set on the command line —
`ANTHROPIC_BASE_URL=… ANTHROPIC_AUTH_TOKEN=… claude`. Zellaude records the
variables that decide where a session points, which account and config home
it runs under, and which model it uses, for both Claude Code and Codex, so a
tool that relaunches the pane can start it the same way instead of reaching
the default endpoint.

Which list a name sits in decides what happens to its value:

| List | Recorded as |
|------|-------------|
| `verbatim` | the value itself |
| `secret` | `<set>` — never the value |

One exception: a secret whose value is exactly `local` is recorded as `local`,
so a session pointed at a local proxy replays without you re-supplying
anything.

Built-in names, by what they decide:

| Decides | `verbatim` | `secret` |
|---------|------------|----------|
| Claude: endpoint and provider | `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_BEDROCK`, `ANTHROPIC_BEDROCK_BASE_URL`, `ANTHROPIC_BEDROCK_REGION_PREFIX`, `ANTHROPIC_SMALL_FAST_MODEL_AWS_REGION`, `CLAUDE_CODE_SKIP_BEDROCK_AUTH`, `CLAUDE_CODE_USE_MANTLE`, `CLAUDE_CODE_SKIP_MANTLE_AUTH`, `CLAUDE_CODE_USE_VERTEX`, `ANTHROPIC_VERTEX_BASE_URL`, `ANTHROPIC_VERTEX_PROJECT_ID`, `CLAUDE_CODE_SKIP_VERTEX_AUTH`, `CLAUDE_CODE_USE_FOUNDRY`, `ANTHROPIC_FOUNDRY_BASE_URL`, `ANTHROPIC_FOUNDRY_RESOURCE`, `CLAUDE_CODE_SKIP_FOUNDRY_AUTH`, `CLAUDE_CODE_USE_ANTHROPIC_AWS`, `ANTHROPIC_AWS_BASE_URL`, `ANTHROPIC_AWS_WORKSPACE_ID`, `CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH` | `ANTHROPIC_FOUNDRY_API_KEY`, `ANTHROPIC_FOUNDRY_AUTH_TOKEN`, `ANTHROPIC_AWS_API_KEY` |
| Claude: identity and config home | `ANTHROPIC_PROFILE`, `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID`, `ANTHROPIC_WORKSPACE_ID`, `CLAUDE_CODE_OAUTH_SCOPES`, `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_PROJECT_DIR_NAME` | `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN` |
| Claude: model and limits | `ANTHROPIC_MODEL`, `ANTHROPIC_DEFAULT_MODEL`, `ANTHROPIC_DEFAULT_OPUS_MODEL`, `ANTHROPIC_DEFAULT_SONNET_MODEL`, `ANTHROPIC_DEFAULT_HAIKU_MODEL`, `ANTHROPIC_DEFAULT_FABLE_MODEL`, `ANTHROPIC_SMALL_FAST_MODEL`, `ANTHROPIC_CUSTOM_MODEL_OPTION`, `CLAUDE_CODE_SUBAGENT_MODEL`, `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, `CLAUDE_CODE_EFFORT_LEVEL`, `CLAUDE_CODE_MAX_CONTEXT_TOKENS`, `CLAUDE_CODE_MAX_OUTPUT_TOKENS`, `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, `CLAUDE_CODE_DISABLE_1M_CONTEXT` | |
| Zellaude | `ZELLAUDE_CLAUDE_MODE` | |
| Codex | `CODEX_HOME`, `CODEX_SQLITE_HOME`, `CODEX_CA_CERTIFICATE`, `CODEX_OSS_BASE_URL`, `CODEX_OSS_PORT`, `OPENAI_BASE_URL`, `OPENAI_ORGANIZATION`, `OPENAI_PROJECT`, `OPENAI_FEDERATION_RULE_ID`, `OPENAI_IDENTITY_TOKEN_FILE`, `OPENAI_WORKLOAD_IDENTITY_CONTEXT` | `CODEX_API_KEY`, `CODEX_ACCESS_TOKEN`, `OPENAI_API_KEY` |

Add your own names in `~/.config/zellij/plugins/zellaude.json`:

```json
{
  "launch_env_names": {
    "verbatim": ["ANTHROPIC_CUSTOM_HEADERS"],
    "secret": ["MY_GATEWAY_TOKEN"]
  }
}
```

Both lists are optional. Names match exactly — no prefixes — so nothing is
recorded that you did not name. **A name you add to `verbatim` is written to
disk with its value**, so put anything credential-shaped under `secret`; a name
in both lists is treated as a secret.

The file can only add names. It cannot move a built-in name to the other list,
and it cannot extend the `local` exception — both are fixed in code, so no
settings file, mistaken or hostile, can turn a built-in secret into a recorded
value. A missing, unreadable or malformed file leaves the built-in lists
unchanged.

Removing a name you added stops it being recorded, and drops it from the pane's
cached entry on the next event.

### Settings

Click the **Zellaude** prefix on the left side of the bar to open the settings menu. Click it again (or the `×` button) to close. Settings are persisted to `~/.config/zellij/plugins/zellaude.json`.

| Setting | Options | Default | Description |
|---------|---------|---------|-------------|
| Notifications | Always / Unfocused / Off | Always | Desktop notifications on permission requests. "Unfocused" only notifies when the requesting pane is on a different tab. |
| Flash | Persist / Brief / Off | Brief | Theme-colored flash on permission requests. "Persist" keeps flashing until resolved, "Brief" flashes for 2 seconds. |
| Elapsed time | On / Off | On | Show time since last activity (appears after 30s). |
| Smart focus | On / Off | On | Clicking an agent-aware tab focuses its most recently active agent pane (waiting ⚠ sessions first). Off makes tab clicks plain tab switches. |

## Install

### Prerequisites

- [Zellij 0.44 or newer](https://zellij.dev)
- [jq](https://jqlang.github.io/jq/) — required by the hook bridge, by settings persistence, and by the install scripts

### Quick install

Add the plugin to your Zellij layout — that's it:

```kdl
default_tab_template {
    pane size=1 borderless=true {
        plugin location="https://github.com/joeyjeong07/zellaude/releases/latest/download/zellaude.wasm"
    }
    children
}
```

Once you grant its permissions (see below), the plugin installs the hook script and registers it with Claude Code and Codex on its own. No cloning, no install scripts.

[Codex requires a one-time review](https://developers.openai.com/codex/hooks) before running newly installed user hooks. Start Codex, open `/hooks`, inspect the Zellaude handlers, and trust them.

### Granting permissions

Zellaude needs seven Zellij permissions, and everything it does is gated behind
them — the hook install, the runtime keybindings, and the bar itself. Until they
are granted the bar renders empty.

Zellij asks by drawing its prompt *inside the plugin's pane*. As a one-row
borderless status bar there is nothing to draw into, and normal focus navigation
skips the pane, so the prompt cannot be answered where it appears. Grant them
once in a full-size pane instead. Zellij keys the grant to the plugin location,
so use the command matching how you installed it.

For the remote plugin in [Quick install](#quick-install):

```bash
zellij action new-pane --plugin https://github.com/joeyjeong07/zellaude/releases/latest/download/zellaude.wasm
```

For a plugin built and installed locally:

```bash
zellij action new-pane --plugin "file:$HOME/.config/zellij/plugins/zellaude.wasm"
```

Press `y` in that pane, then close it (`Ctrl+p`, `x`). The grant is cached, so
every bar instance — including new tabs and later sessions — picks it up. If the
bar is already on screen and blank, clicking it also re-raises the prompt.

The seven are `ReadApplicationState`, `ChangeApplicationState`, `RunCommands`,
`ReadCliPipes`, `MessageAndLaunchOtherPlugins`, `Reconfigure` and
`RunActionsAsUser`. `RunCommands` is what lets the plugin shell out to install
the hook script and read its settings file; `Reconfigure` and `RunActionsAsUser`
back the session-only keybindings described above. The grant is recorded in
Zellij's own cache — `zellij setup --check` prints the directory, typically
`~/.cache/zellij/permissions.kdl`.

Installing from source does this for you; see below.

### Build from source

Prerequisites: [Rust](https://rustup.rs) (in addition to the above)

```bash
git clone https://github.com/joeyjeong07/zellaude.git
cd zellaude
./install.sh
```

The script first checks for `jq`, `cargo`, `rustup`, and Zellij 0.44 or newer
before writing anything. It adds `~/.cargo/bin` to its `PATH` when needed and
installs the `wasm32-wasip1` target if it is not already present. It then runs a
locked release build with an explicit target and target directory, verifies the
resulting WASM is nonempty, and atomically installs it to
`~/.config/zellij/plugins/`. Finally, it pre-grants the permissions above so the
first run is not an empty bar and registers the hooks. You can invoke the script
from any directory; all project paths are resolved relative to `install.sh`.

Set `ZELLAUDE_INSTALL_HOME` when staging an installation for a different home
or running an isolated test. The helpers use that directory for the plugin,
Claude/Codex settings, and Zellij cache instead of inheriting the invoking
account's `CODEX_HOME` or `XDG_CACHE_HOME`. Hook registrations intentionally
remain portable as `${HOME}/.config/zellij/plugins/zellaude-hook.sh`, so the
staged directory must be that user's `HOME` when the clients later run. Set
`ZELLAUDE_BUILD_DIR` to choose the Cargo target directory; relative values are
resolved from the project directory. Run `./install.sh --help` for the complete
option summary.

Pass `--no-permissions` to skip the pre-grant and approve interactively instead:

```bash
./install.sh --no-permissions
```

That leaves `permissions.kdl` untouched, so the bar stays inert until you grant
the seven by hand in a full-size pane — see [Granting permissions](#granting-permissions).
Everything else the script does is unchanged.

#### What it touches

| Path | Change |
|------|--------|
| `~/.config/zellij/plugins/zellaude.wasm` | created |
| `~/.config/zellij/plugins/zellaude-hook.sh` | created, version-tagged, `+x` |
| `~/.cache/zellij/permissions.kdl` | zellaude's block replaced; other plugins' grants preserved |
| `~/.claude/settings.json` | zellaude hook entries replaced under 11 events; previous file copied to `.bak` |
| `${CODEX_HOME:-~/.codex}/hooks.json` | same under 9 events; previous file copied to `.bak` |

Both JSON files are created as `{}` if absent, symlinks are resolved before
writing so `mv` cannot replace them with regular files, and the jq filters only
replace the exact hook commands Zellaude owns. Re-running the script is
idempotent, while unrelated hooks—even commands that mention
`zellaude-hook.sh`—are left alone. Your Zellij layout is never modified; that
step is below.

### Wiring it into a layout

The plugin replaces the native tab bar, so it goes where `zellij:tab-bar` would.
For a fresh setup, add it to your layout:

```kdl
default_tab_template {
    pane size=1 borderless=true {
        plugin location="file:~/.config/zellij/plugins/zellaude.wasm"
    }
    children
}
```

If you already have a layout in `layout_dir`, swap the location string in every
template that draws a tab bar — a stock `default.kdl` has **two**,
`default_tab_template` and `new_tab_template`, and missing the second leaves new
tabs on the native bar:

```diff
 default_tab_template {
     pane size=1 borderless=true {
-        plugin location="zellij:tab-bar"
+        plugin location="file:~/.config/zellij/plugins/zellaude.wasm"
     }
     children
     pane size=2 borderless=true {
         plugin location="zellij:status-bar"
     }
 }
```

Leave the `zellij:status-bar` pane alone — Zellaude replaces the *tab* bar only.
Keep `size=1 borderless=true`: the bar renders one row, and a bordered pane
clips it.

Layouts are read at session start, so restart Zellij (or start a new session)
to pick the change up; running sessions keep the bar they launched with.

Or try the included layout directly, without touching your own:

```bash
zellij --layout layout.kdl
```

### Optional: desktop notifications

The bar itself needs nothing extra. Desktop notifications on permission requests
are delegated to whatever notifier the platform has, so they are silently
skipped when none is installed.

**macOS** — notifications work out of the box via `osascript`. For ones that
focus the requesting pane when clicked, install
[terminal-notifier](https://github.com/julienXX/terminal-notifier):

```bash
brew install terminal-notifier
```

Without it notifications still appear, but clicking them won't focus the pane.

**Linux** — install `notify-send`, or nothing is delivered at all:

```bash
sudo apt install libnotify-bin   # or: dnf install libnotify / pacman -S libnotify
```

There is no click-to-focus on Linux; `terminal-notifier` is macOS-only.

The **Unfocused** notification setting also needs a way to ask which window is
frontmost. macOS uses `osascript`. X11 needs [`xdotool`](https://github.com/jordansissel/xdotool);
without it — and on Wayland, which exposes no standard way to check — the
terminal never counts as focused, so Unfocused behaves like Always.

## Uninstall

Run from the clone — the script removes what it installed relative to its own
location, so keep the checkout around if you want this:

```bash
./install.sh --uninstall
```

That deletes `zellaude.wasm` and `zellaude-hook.sh`, drops zellaude's block from
`permissions.kdl`, and strips the hook entries from `~/.claude/settings.json`
and `~/.codex/hooks.json` (backing both up again first). Restart Zellij
afterwards.

Three things it does not touch:

- **Your Zellij layout** — put `zellij:tab-bar` back by hand.
- **`~/.config/zellij/plugins/zellaude.json`** — your settings, custom states
  and session templates.
- **Generated session layouts** in `~/.config/zellij/layouts/`. Every layout
  zellaude compiled is still there, and each one still points at the
  now-deleted `zellaude.wasm`, so `zellij -n zellaude` after an uninstall opens
  a session with a broken plugin pane in every tab. They are the files whose
  first line is exactly `// zellaude-generated <version> <name>` with
  `<name>` matching the filename (minus `.kdl`) — the same test the plugin
  itself uses, so this lists exactly the files it owns; list them, then
  delete the ones you no longer want:

  ```bash
  awk 'FNR==1 && NF==4 && $0 ~ /^\/\/ zellaude-generated / {n=FILENAME; sub(/^.*\//,"",n); sub(/\.kdl$/,"",n); if ($4==n) print FILENAME}' ~/.config/zellij/layouts/*.kdl
  ```

## How it works

Three components:

1. **WASM plugin** — runs inside Zellij, receives events, maintains state in memory, renders the status bar, and sends desktop notifications. On first load, it writes the hook script to `~/.config/zellij/plugins/zellaude-hook.sh` and registers it in `~/.claude/settings.json` and `~/.codex/hooks.json`.
2. **Hook script** — a thin bash bridge that forwards Claude Code and Codex hook events to the plugin via `zellij pipe`
3. **Attach probe** — runs once when the plugin attaches, maps live agent processes to their real Zellij pane IDs, and restores their current effort modes without waiting for another prompt

```
Claude Code / Codex hook → zellaude-hook.sh → zellij pipe → plugin → render
```

The hook script and registration are version-tagged and updated automatically when the plugin version changes.
The registered hook command uses `${HOME}/.config/zellij/plugins/zellaude-hook.sh`; Claude Code expands `${HOME}` when it runs hooks, keeping the settings entry portable across machines.

Codex currently records its active reasoning effort in the hook transcript rather than hook input, while Claude Code reports `ultracode` as ordinary `xhigh` effort. Zellaude resolves both best-effort from the live session transcript and launch flags. Custom Claude launchers that hide `--effort ultracode` can export `ZELLAUDE_CLAUDE_MODE=ultracode` when Claude's active effort remains `xhigh`.

The hook also keeps the last root-session state in a private per-user cache so a
new plugin instance can restore it. On Linux, attach recovery additionally uses
Zellij's pane PID and procfs to identify an already-running root session exactly;
ambiguous matches are ignored. Multiple plugin instances (one per tab) sync
state automatically via inter-plugin messaging. Cache entries are removed on a
normal session end, and sessions are cleaned up automatically when tabs close.

## License

MIT
