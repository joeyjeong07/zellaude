#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
TEST_HOME="$TEST_DIR/home"
CLAUDE_SETTINGS="$TEST_HOME/.claude/settings.json"
CODEX_HOOKS="$TEST_HOME/.codex/hooks.json"
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$(dirname "$CLAUDE_SETTINGS")" "$(dirname "$CODEX_HOOKS")"

seed_settings() {
  local file=$1
  cat > "$file" <<'JSON'
{
  "unrelated_setting": true,
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "keep",
        "hooks": [
          {"type": "command", "command": "/bin/keep"},
          {"type": "command", "command": "${HOME}/.config/zellij/plugins/zellaude-hook.sh"}
        ]
      },
      {
        "hooks": [
          {"type": "command", "command": "${HOME}/.config/zellij/plugins/zellaude-hook.sh"}
        ]
      }
    ],
    "UnrelatedEvent": [
      {
        "hooks": [
          {"type": "command", "command": "/bin/unrelated"}
        ]
      }
    ]
  }
}
JSON
}

zellaude_count() {
  local file=$1
  jq '[
    .hooks[]?[]?
    | .hooks[]?
    | select((.command // "") | contains("zellaude-hook.sh"))
  ] | length' "$file"
}

assert_install() {
  local expect_unrelated=${1:-true}
  local expected_version claude_events codex_events
  expected_version=$(awk -F '"' '/^version = "/ { print $2; exit }' "$PROJECT_DIR/Cargo.toml")
  claude_events='["PreToolUse","PostToolUse","PostToolUseFailure","UserPromptSubmit","PermissionRequest","Notification","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
  codex_events='["PreToolUse","PostToolUse","UserPromptSubmit","PermissionRequest","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'

  [ "$(zellaude_count "$CLAUDE_SETTINGS")" -eq 11 ]
  [ "$(zellaude_count "$CODEX_HOOKS")" -eq 9 ]
  jq -e --argjson events "$claude_events" '
    . as $root
    | all(
      $events[];
      . as $event
      | [$root.hooks[$event][]?.hooks[]?
          | select((.command // "") | contains("zellaude-hook.sh"))]
      | length == 1
    )
  ' "$CLAUDE_SETTINGS" >/dev/null
  jq -e --argjson events "$codex_events" '
    . as $root
    | all(
      $events[];
      . as $event
      | [$root.hooks[$event][]?.hooks[]?
          | select((.command // "") | contains("zellaude-hook.sh"))]
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
  fi
  grep -qxF "# zellaude v$expected_version" \
    "$TEST_HOME/.config/zellij/plugins/zellaude-hook.sh"
  bash -n "$TEST_HOME/.config/zellij/plugins/zellaude-hook.sh"
}

seed_settings "$CLAUDE_SETTINGS"
seed_settings "$CODEX_HOOKS"

# Ordinary reinstalls stay idempotent.
for _ in 1 2; do
  HOME="$TEST_HOME" CODEX_HOME="$TEST_HOME/.codex" \
    "$PROJECT_DIR/scripts/install-hooks.sh" >/dev/null
done
assert_install

# Every status-bar instance reloads together and can start the embedded
# installer concurrently. The resulting settings must still contain one
# Zellaude handler per supported event.
seed_settings "$CLAUDE_SETTINGS"
seed_settings "$CODEX_HOOKS"
pids=()
for _ in {1..8}; do
  HOME="$TEST_HOME" CODEX_HOME="$TEST_HOME/.codex" \
    "$PROJECT_DIR/scripts/install-hooks.sh" >/dev/null &
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
  HOME="$TEST_HOME" CODEX_HOME="$TEST_HOME/.codex" \
    "$PROJECT_DIR/scripts/install-hooks.sh" >/dev/null &
  pids+=("$!")
done
for pid in "${pids[@]}"; do
  wait "$pid"
done
assert_install false

printf 'hook installation idempotency tests passed\n'
