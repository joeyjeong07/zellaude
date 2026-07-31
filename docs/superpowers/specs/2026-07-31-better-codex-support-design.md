# better-codex support & pre-prompt agent pane recognition

Date: 2026-07-31
Status: approved

## Background

better-codex (a Codex fork with a customized TUI, installed as `better-codex`)
shares `~/.codex` with stock Codex: same `hooks.json`, same trust state, same
hook stdin schema (`hook_event_name`, `session_id`, `turn_id`,
`transcript_path`, `cwd`), same rollout/transcript format including the
`turn_context.effort` field zellaude reads for ultra detection. Verified
empirically: exec-mode and TUI-mode hooks all fire and forward through
`zellaude-hook.sh` unchanged.

Two real gaps remain, and both also affect stock Codex UX:

1. `attach.rs::client_for_command` does not know the executable name
   `better-codex`, so a command pane launched as `zellij run -- better-codex`
   classifies as "unknown" (self-healing via dual discovery, but imprecise).
2. Codex-family TUIs start their session lazily on the first prompt, so no
   `SessionStart` hook fires at TUI launch and the pane is invisible to
   zellaude until the user submits a prompt.

## Design

### 1. Executable mapping

Add `"better-codex" => Some("codex")` to `client_for_command`. In practice the
wrapper script `exec`s a binary named `codex`, so the OS-level query almost
always reports `codex` already; the mapping covers the pre-exec window and
directly-named binaries.

### 2. Placeholder sessions for idle agent TUIs

`get_pane_running_command` (zellij-tile ≥ 0.44) queries the OS for the
*current* foreground command of a pane, not the launch command. The plugin can
therefore recognize a freshly launched agent TUI without any hook event:

- On `Timer` ticks (throttled to one poll per 2s), only in the plugin instance
  whose tab is active (`attach::is_active_instance`), and only when pane
  introspection is supported, query the running command of every pane that has
  no session entry (or only a placeholder one).
- If the command classifies as an agent (`codex`, `claude`, `better-codex`),
  insert a placeholder `SessionInfo`: `session_id = ""`, `Activity::Idle`,
  zero timestamps, `restored = true`. The status bar shows the pane as idle
  immediately.
- If a placeholder's pane no longer runs an agent (TUI exited to shell), the
  next poll removes it. Pane close is already handled by `remove_dead_panes`.
- The first real hook event promotes the placeholder in place: zero timestamps
  never win an ordering comparison, and `update_session_identity` fills in the
  real session id.

Placeholders are local, derived state: they are excluded from
`zellaude:sync` broadcasts and ignored when merging peer state, so a stale
placeholder can never be resurrected across instances. Each per-tab instance
re-derives them while its tab is active (inactive instances are not visible).

zellij < 0.44 keeps the current behavior (no polling).

## Testing

- Unit tests: executable classification; placeholder add/remove/keep
  transitions; promotion by a real hook event; sync/merge exclusion.
- Live verification: launch `better-codex` in a zellaude session — the pane
  must appear as idle within ~2s, before any prompt is submitted.
