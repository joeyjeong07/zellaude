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
- [jq](https://jqlang.github.io/jq/) — used by hooks and settings persistence at runtime

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

On first load, the plugin automatically installs the hook script and registers it with Claude Code and Codex. No cloning, no install scripts.

[Codex requires a one-time review](https://developers.openai.com/codex/hooks) before running newly installed user hooks. Start Codex, open `/hooks`, inspect the Zellaude handlers, and trust them.

### Build from source

Prerequisites: [Rust](https://rustup.rs) (in addition to the above)

```bash
git clone https://github.com/ishefi/zellaude.git
cd zellaude
./install.sh
```

This builds the WASM plugin and copies it to `~/.config/zellij/plugins/`. Hook registration happens automatically when the plugin loads.

Then add the plugin to your Zellij layout (replaces the default tab bar):

```kdl
default_tab_template {
    pane size=1 borderless=true {
        plugin location="file:~/.config/zellij/plugins/zellaude.wasm"
    }
    children
}
```

Or try the included layout directly:

```bash
zellij --layout layout.kdl
```

### Optional: click-to-focus notifications

For desktop notifications that focus the right pane when clicked, install [terminal-notifier](https://github.com/julienXX/terminal-notifier):

```bash
brew install terminal-notifier
```

Without it, notifications still appear via osascript but clicking them won't focus the pane.

## Uninstall

```bash
./install.sh --uninstall
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
