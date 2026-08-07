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
- **Custom states** — open a named command grid in a new tab with `Ctrl+t`, `Shift+n`
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

Reload the plugin (or restart the Zellij session) after editing the file. Then press `Ctrl+t`, `Shift+n`, type the state ID, and press `Enter`. Zellaude opens a new tab with the configured grid, mapping the command array to panes from left to right, top to bottom. Press `Esc` or `Ctrl+c` to cancel the prompt.

`width` and `height` may be JSON numbers or numeric strings. A state needs at least one command, the command count may not exceed `width × height`, and a state may contain at most 64 panes; the resulting grid must also fit the current terminal. When there are fewer commands than cells, the unused bottom-right cells open as normal shell panes. Commands run through `sh -lc` in the directory of the pane where the prompt was opened when Zellij can resolve it, falling back to Zellij's default working directory otherwise. Custom-state configuration should therefore be treated as trusted local shell code.

The settings file may also contain a single state object or an array of states, although the `custom_states` wrapper is recommended because it coexists with Zellaude's UI settings. As an alternative, states can be supplied directly in the plugin block; this takes precedence over the settings file:

```kdl
plugin location="file:~/.config/zellij/plugins/zellaude.wasm" {
    custom_states r#"[{"id":"shells","width":2,"height":1,"commands":["htop","git status"]}]"#
}
```

Zellaude installs `Shift+n` in Tab mode for the running client without changing `config.kdl`. If that key already has a user binding, the user binding wins and the custom-state shortcut remains unavailable.

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

### Settings

Click the **Zellaude** prefix on the left side of the bar to open the settings menu. Click it again (or the `×` button) to close. Settings are persisted to `~/.config/zellij/plugins/zellaude.json`.

| Setting | Options | Default | Description |
|---------|---------|---------|-------------|
| Notifications | Always / Unfocused / Off | Always | Desktop notifications on permission requests. "Unfocused" only notifies when the requesting pane is on a different tab. |
| Flash | Persist / Brief / Off | Brief | Theme-colored flash on permission requests. "Persist" keeps flashing until resolved, "Brief" flashes for 2 seconds. |
| Elapsed time | On / Off | On | Show time since last activity (appears after 30s). |

## Install

### Prerequisites

- [Zellij 0.44 or newer](https://zellij.dev)
- [jq](https://jqlang.github.io/jq/) — required by the hook bridge, by settings persistence, and by the install scripts

### Quick install

Add the plugin to your Zellij layout — that's it:

```kdl
default_tab_template {
    pane size=1 borderless=true {
        plugin location="https://github.com/ishefi/zellaude/releases/latest/download/zellaude.wasm"
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
once in a full-size pane instead:

```bash
zellij action new-pane --plugin file:~/.config/zellij/plugins/zellaude.wasm
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
git clone https://github.com/ishefi/zellaude.git
cd zellaude
./install.sh
```

The script needs `jq`, `cargo` and `rustup` on `PATH` (it adds `~/.cargo/bin`
itself if `cargo` is missing from the environment) and installs the
`wasm32-wasip1` target if it is not already present. It then builds the plugin
and copies it to `~/.config/zellij/plugins/`, pre-grants the permissions above
so the first run is not an empty bar, and registers the hooks. Registration also
happens automatically when the plugin loads, so the script's copy of it is
belt-and-braces for a first run that has no working bar yet.

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
add or remove entries whose command contains `zellaude-hook.sh` — re-running the
script is idempotent and leaves unrelated hooks alone. Your Zellij layout is
never modified; that step is below.

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
  first line starts with `// zellaude-generated`; list them, then delete the
  ones you no longer want:

  ```bash
  grep -l '^// zellaude-generated ' ~/.config/zellij/layouts/*.kdl
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
