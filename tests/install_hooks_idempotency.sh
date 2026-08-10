#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
TEST_HOME="$TEST_DIR/home"
CLAUDE_SETTINGS="$TEST_HOME/.claude/settings.json"
CODEX_HOOKS="$TEST_HOME/.codex/hooks.json"
CLAUDE_OWNED='${HOME}/.config/zellij/plugins/zellaude-hook.sh'
CODEX_OWNED='${HOME}/.config/zellij/plugins/zellaude-hook.sh --client codex'
LOOKALIKE_COMMAND='/bin/wrapper --mentions zellaude-hook.sh'
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$(dirname "$CLAUDE_SETTINGS")" "$(dirname "$CODEX_HOOKS")"

seed_settings() {
  local file=$1
  local owned=$2
  jq -n --arg owned "$owned" --arg lookalike "$LOOKALIKE_COMMAND" '
    {
      unrelated_setting: true,
      hooks: {
        PreToolUse: [
          {
            matcher: "keep",
            hooks: [
              {type: "command", command: "/bin/keep"},
              {type: "command", command: $lookalike},
              {type: "command", command: $owned}
            ]
          },
          {
            hooks: [
              {type: "command", command: $owned}
            ]
          }
        ],
        UnrelatedEvent: [
          {
            hooks: [
              {type: "command", command: "/bin/unrelated"}
            ]
          }
        ],
        EmptyGroupEvent: [
          {
            matcher: "preserve-empty",
            hooks: []
          }
        ]
      }
    }
  ' > "$file"
}

owned_count() {
  local file=$1 owned=$2
  jq --arg owned "$owned" '[
    .hooks[]?[]?
    | .hooks[]?
    | select((.command // "") == $owned)
  ] | length' "$file"
}

lookalike_count() {
  local file=$1
  jq --arg command "$LOOKALIKE_COMMAND" '[
    .hooks[]?[]?
    | .hooks[]?
    | select((.command // "") == $command)
  ] | length' "$file"
}

run_hooks() {
  ZELLAUDE_INSTALL_HOME="$TEST_HOME" \
    ZELLAUDE_CODEX_HOME="$TEST_HOME/.codex" \
    "$PROJECT_DIR/scripts/install-hooks.sh" "$@"
}

assert_install() {
  local expect_unrelated=${1:-true}
  local expected_version claude_events codex_events
  expected_version=$(awk -F '"' '/^version = "/ { print $2; exit }' "$PROJECT_DIR/Cargo.toml")
  claude_events='["PreToolUse","PostToolUse","PostToolUseFailure","UserPromptSubmit","PermissionRequest","Notification","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
  codex_events='["PreToolUse","PostToolUse","UserPromptSubmit","PermissionRequest","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'

  [ "$(owned_count "$CLAUDE_SETTINGS" "$CLAUDE_OWNED")" -eq 11 ]
  [ "$(owned_count "$CODEX_HOOKS" "$CODEX_OWNED")" -eq 9 ]
  jq -e --argjson events "$claude_events" --arg owned "$CLAUDE_OWNED" '
    . as $root
    | all(
      $events[];
      . as $event
      | [$root.hooks[$event][]?.hooks[]?
          | select((.command // "") == $owned)]
      | length == 1
    )
  ' "$CLAUDE_SETTINGS" >/dev/null
  jq -e --argjson events "$codex_events" --arg owned "$CODEX_OWNED" '
    . as $root
    | all(
      $events[];
      . as $event
      | [$root.hooks[$event][]?.hooks[]?
          | select((.command // "") == $owned)]
      | length == 1
    )
  ' "$CODEX_HOOKS" >/dev/null
  if [ "$expect_unrelated" = "true" ]; then
    [ "$(jq -r '.unrelated_setting' "$CLAUDE_SETTINGS")" = "true" ]
    [ "$(jq -r '.unrelated_setting' "$CODEX_HOOKS")" = "true" ]
    [ "$(jq -r '[.hooks.PreToolUse[]?.hooks[]? | select(.command == "/bin/keep")] | length' "$CLAUDE_SETTINGS")" -eq 1 ]
    [ "$(jq -r '[.hooks.PreToolUse[]?.hooks[]? | select(.command == "/bin/keep")] | length' "$CODEX_HOOKS")" -eq 1 ]
    [ "$(jq -r '[.hooks.UnrelatedEvent[]?.hooks[]? | select(.command == "/bin/unrelated")] | length' "$CLAUDE_SETTINGS")" -eq 1 ]
    [ "$(jq -r '[.hooks.UnrelatedEvent[]?.hooks[]? | select(.command == "/bin/unrelated")] | length' "$CODEX_HOOKS")" -eq 1 ]
    [ "$(jq -r '[.hooks.EmptyGroupEvent[]? | select(.matcher == "preserve-empty" and (.hooks | length) == 0)] | length' "$CLAUDE_SETTINGS")" -eq 1 ]
    [ "$(jq -r '[.hooks.EmptyGroupEvent[]? | select(.matcher == "preserve-empty" and (.hooks | length) == 0)] | length' "$CODEX_HOOKS")" -eq 1 ]
    [ "$(lookalike_count "$CLAUDE_SETTINGS")" -eq 1 ]
    [ "$(lookalike_count "$CODEX_HOOKS")" -eq 1 ]
  fi
  grep -qxF "# zellaude v$expected_version" \
    "$TEST_HOME/.config/zellij/plugins/zellaude-hook.sh"
  bash -n "$TEST_HOME/.config/zellij/plugins/zellaude-hook.sh"
}

seed_settings "$CLAUDE_SETTINGS" "$CLAUDE_OWNED"
seed_settings "$CODEX_HOOKS" "$CODEX_OWNED"

# Ordinary reinstalls stay idempotent.
for _ in 1 2; do
  run_hooks >/dev/null
done
assert_install

# Every status-bar instance reloads together and can start the embedded
# installer concurrently. The resulting settings must still contain one
# Zellaude handler per supported event.
seed_settings "$CLAUDE_SETTINGS" "$CLAUDE_OWNED"
seed_settings "$CODEX_HOOKS" "$CODEX_OWNED"
pids=()
for _ in {1..8}; do
  run_hooks >/dev/null &
  pids+=("$!")
done
for pid in "${pids[@]}"; do
  wait "$pid"
done
assert_install

# A first install must also be safe when all instances see missing settings.
rm -f "$CLAUDE_SETTINGS" "$CODEX_HOOKS"
pids=()
for _ in {1..8}; do
  run_hooks >/dev/null &
  pids+=("$!")
done
for pid in "${pids[@]}"; do
  wait "$pid"
done
assert_install false

# Uninstall removes only the commands this installer owns. Commands that happen
# to mention the hook filename, along with unrelated settings, must survive.
seed_settings "$CLAUDE_SETTINGS" "$CLAUDE_OWNED"
seed_settings "$CODEX_HOOKS" "$CODEX_OWNED"
run_hooks >/dev/null
run_hooks --uninstall >/dev/null
[ "$(owned_count "$CLAUDE_SETTINGS" "$CLAUDE_OWNED")" -eq 0 ]
[ "$(owned_count "$CODEX_HOOKS" "$CODEX_OWNED")" -eq 0 ]
[ "$(lookalike_count "$CLAUDE_SETTINGS")" -eq 1 ]
[ "$(lookalike_count "$CODEX_HOOKS")" -eq 1 ]
[ "$(jq -r '[.hooks.EmptyGroupEvent[]? | select(.matcher == "preserve-empty")] | length' "$CLAUDE_SETTINGS")" -eq 1 ]
[ "$(jq -r '[.hooks.EmptyGroupEvent[]? | select(.matcher == "preserve-empty")] | length' "$CODEX_HOOKS")" -eq 1 ]
[ ! -e "$TEST_HOME/.config/zellij/plugins/zellaude-hook.sh" ]
run_hooks --uninstall >/dev/null

# Invalid hook structures fail before the bridge or either settings file is
# changed. Silently replacing these would destroy user configuration.
printf '{"hooks": []}\n' > "$CLAUDE_SETTINGS"
seed_settings "$CODEX_HOOKS" "$CODEX_OWNED"
cp "$CLAUDE_SETTINGS" "$TEST_DIR/claude.before"
cp "$CODEX_HOOKS" "$TEST_DIR/codex.before"
if run_hooks >/dev/null 2>&1; then
  echo "expected malformed hooks configuration to fail" >&2
  exit 1
fi
cmp -s "$CLAUDE_SETTINGS" "$TEST_DIR/claude.before"
cmp -s "$CODEX_HOOKS" "$TEST_DIR/codex.before"
[ ! -e "$TEST_HOME/.config/zellij/plugins/zellaude-hook.sh" ]

# Concatenated JSON documents are not a valid settings file even though jq can
# parse them as a stream. Reject them instead of rewriting another invalid file.
printf '{}\n{}\n' > "$CLAUDE_SETTINGS"
seed_settings "$CODEX_HOOKS" "$CODEX_OWNED"
cp "$CLAUDE_SETTINGS" "$TEST_DIR/claude-stream.before"
if run_hooks >/dev/null 2>&1; then
  echo "expected multiple JSON documents to fail" >&2
  exit 1
fi
cmp -s "$CLAUDE_SETTINGS" "$TEST_DIR/claude-stream.before"
[ ! -e "$TEST_HOME/.config/zellij/plugins/zellaude-hook.sh" ]

# Uninstall validates both files before modifying either, so one malformed file
# cannot leave the two clients in a half-uninstalled state.
seed_settings "$CLAUDE_SETTINGS" "$CLAUDE_OWNED"
printf '{"hooks": []}\n' > "$CODEX_HOOKS"
cp "$CLAUDE_SETTINGS" "$TEST_DIR/claude-uninstall.before"
cp "$CODEX_HOOKS" "$TEST_DIR/codex-uninstall.before"
if run_hooks --uninstall >/dev/null 2>&1; then
  echo "expected uninstall with malformed settings to fail" >&2
  exit 1
fi
cmp -s "$CLAUDE_SETTINGS" "$TEST_DIR/claude-uninstall.before"
cmp -s "$CODEX_HOOKS" "$TEST_DIR/codex-uninstall.before"

printf 'hook installation idempotency tests passed\n'
