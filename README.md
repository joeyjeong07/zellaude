# Zellaude

A Zellij status bar plugin that replaces the default tab bar with Claude Code and Codex activity awareness.

![Zellaude status bar example](assets/bar-example.svg)

## Features

- **Full tab bar** — shows all Zellij tabs (not just agent sessions), replacing the native tab bar
- **Session & mode display** — shows the Zellij session name and current input mode (NORMAL, LOCKED, PANE, etc.) with color-coded indicators
- **Live activity indicators** — see what every Claude Code and Codex session is doing at a glance; non-agent tabs remain visible without activity glyphs
- **Theme-aware palette** — follows Zellij's live theme colors; Gruvbox Dark is explicitly verified
- **Ultra-mode rainbow** — tab names shimmer through rainbow colors for Codex `ultra` sessions and Claude Code `ultracode` sessions
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

### Settings

Click the **Zellaude** prefix on the left side of the bar to open the settings menu. Click it again (or the `×` button) to close. Settings are persisted to `~/.config/zellij/plugins/zellaude.json`.

| Setting | Options | Default | Description |
|---------|---------|---------|-------------|
| Notifications | Always / Unfocused / Off | Always | Desktop notifications on permission requests. "Unfocused" only notifies when the requesting pane is on a different tab. |
| Flash | Persist / Brief / Off | Brief | Theme-colored flash on permission requests. "Persist" keeps flashing until resolved, "Brief" flashes for 2 seconds. |
| Elapsed time | On / Off | On | Show time since last activity (appears after 30s). |

## Install

### Prerequisites

- [Zellij](https://zellij.dev)
- [jq](https://jqlang.github.io/jq/) — used by the hook script at runtime

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

Two components:

1. **WASM plugin** — runs inside Zellij, receives events, maintains state in memory, renders the status bar, and sends desktop notifications. On first load, it writes the hook script to `~/.config/zellij/plugins/zellaude-hook.sh` and registers it in `~/.claude/settings.json` and `~/.codex/hooks.json`.
2. **Hook script** — a thin bash bridge that forwards Claude Code and Codex hook events to the plugin via `zellij pipe`

```
Claude Code / Codex hook → zellaude-hook.sh → zellij pipe → plugin → render
```

The hook script and registration are version-tagged and updated automatically when the plugin version changes.
The registered hook command uses `${HOME}/.config/zellij/plugins/zellaude-hook.sh`; Claude Code expands `${HOME}` when it runs hooks, keeping the settings entry portable across machines.

Codex currently records its active reasoning effort in the hook transcript rather than hook input, while Claude Code reports `ultracode` as ordinary `xhigh` effort. Zellaude resolves both best-effort from the live session transcript and launch flags. Custom Claude launchers that hide `--effort ultracode` can export `ZELLAUDE_CLAUDE_MODE=ultracode` when Claude's active effort remains `xhigh`.

All state lives in WASM memory. No temp files, no race conditions. Multiple plugin instances (one per tab) sync state automatically via inter-plugin messaging. Sessions are cleaned up automatically when tabs are closed.

## License

MIT
