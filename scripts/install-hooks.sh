#!/usr/bin/env bash
# install-hooks.sh — Install the bridge and register Claude Code/Codex hooks
#
# Usage: ./scripts/install-hooks.sh [--uninstall]
set -euo pipefail

CLAUDE_SETTINGS="$HOME/.claude/settings.json"
CODEX_CONFIG_DIR="${CODEX_HOME:-$HOME/.codex}"
CODEX_HOOKS="$CODEX_CONFIG_DIR/hooks.json"
SOURCE_HOOK="$(cd "$(dirname "$0")" && pwd)/zellaude-hook.sh"
INSTALLED_HOOK="$HOME/.config/zellij/plugins/zellaude-hook.sh"
CLAUDE_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh'
CODEX_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh --client codex'

CLAUDE_EVENTS='["PreToolUse","PostToolUse","PostToolUseFailure","UserPromptSubmit","PermissionRequest","Notification","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
CODEX_EVENTS='["PreToolUse","PostToolUse","UserPromptSubmit","PermissionRequest","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'

resolve_file_symlink() {
  local path dir target
  path=$1
  while [ -L "$path" ]; do
    dir=$(cd "$(dirname "$path")" && pwd -P)
    target=$(readlink "$path")
    case "$target" in
      /*) path=$target ;;
      *) path=$dir/$target ;;
    esac
  done
  dir=$(cd "$(dirname "$path")" && pwd -P)
  printf '%s/%s\n' "$dir" "$(basename "$path")"
}

if [ -L "$CLAUDE_SETTINGS" ]; then
  CLAUDE_SETTINGS="$(resolve_file_symlink "$CLAUDE_SETTINGS")"
fi
if [ -L "$CODEX_HOOKS" ]; then
  CODEX_HOOKS="$(resolve_file_symlink "$CODEX_HOOKS")"
fi

if ! command -v jq &>/dev/null; then
  echo "Error: jq is required. Install with: brew install jq" >&2
  exit 1
fi

if [ ! -f "$SOURCE_HOOK" ]; then
  echo "Error: Hook script not found at $SOURCE_HOOK" >&2
  exit 1
fi

backup_file() {
  local file=$1
  if [ -f "$file" ]; then
    cp "$file" "$file.bak"
    echo "Backed up $file to $file.bak"
  fi
}

ensure_json_file() {
  local file=$1
  if [ ! -f "$file" ]; then
    mkdir -p "$(dirname "$file")"
    echo '{}' > "$file"
  fi
}

remove_zellaude_entries() {
  local file=$1
  [ -f "$file" ] || return 0

  local tmp
  tmp=$(mktemp)
  jq '
    if .hooks and (.hooks | type == "object") then
      .hooks |= with_entries(
        .value |= [
          .[] | . as $group |
          ($group.hooks // [])
            | map(select(((.command // "") | contains("zellaude-hook.sh")) | not))
            | . as $filtered |
          if length > 0 then ($group | .hooks = $filtered) else empty end
        ]
      ) |
      .hooks |= with_entries(select(.value | length > 0)) |
      if .hooks == {} then del(.hooks) else . end
    else
      .
    end
  ' "$file" > "$tmp"
  mv "$tmp" "$file"
}

add_hook_entries() {
  local file=$1
  local events=$2
  local entry=$3
  local tmp
  tmp=$(mktemp)
  jq --argjson events "$events" --argjson entry "$entry" '
    .hooks //= {} |
    reduce ($events[]) as $event (
      .;
      .hooks[$event] = (.hooks[$event] // []) + $entry
    )
  ' "$file" > "$tmp"
  mv "$tmp" "$file"
}

uninstall() {
  local found=false
  for file in "$CLAUDE_SETTINGS" "$CODEX_HOOKS"; do
    if [ -f "$file" ]; then
      found=true
      backup_file "$file"
      remove_zellaude_entries "$file"
      echo "Uninstalled zellaude hooks from $file"
    fi
  done

  if [ -f "$INSTALLED_HOOK" ]; then
    found=true
    rm -f "$INSTALLED_HOOK"
    echo "Removed $INSTALLED_HOOK"
  fi

  if [ "$found" = false ]; then
    echo "No zellaude hooks found"
  fi
}

install() {
  mkdir -p "$(dirname "$INSTALLED_HOOK")"
  cp "$SOURCE_HOOK" "$INSTALLED_HOOK"
  chmod +x "$INSTALLED_HOOK"

  ensure_json_file "$CLAUDE_SETTINGS"
  ensure_json_file "$CODEX_HOOKS"
  backup_file "$CLAUDE_SETTINGS"
  backup_file "$CODEX_HOOKS"

  # Replace only zellaude's handlers, preserving all unrelated hook groups.
  remove_zellaude_entries "$CLAUDE_SETTINGS"
  remove_zellaude_entries "$CODEX_HOOKS"

  local claude_entry codex_entry
  claude_entry=$(jq -nc --arg cmd "$CLAUDE_HOOK_CMD" '[{
    "hooks": [{
      "type": "command",
      "command": $cmd,
      "timeout": 5,
      "async": true
    }]
  }]')
  codex_entry=$(jq -nc --arg cmd "$CODEX_HOOK_CMD" '[{
    "hooks": [{
      "type": "command",
      "command": $cmd,
      "timeout": 3
    }]
  }]')

  add_hook_entries "$CLAUDE_SETTINGS" "$CLAUDE_EVENTS" "$claude_entry"
  add_hook_entries "$CODEX_HOOKS" "$CODEX_EVENTS" "$codex_entry"

  echo "Installed zellaude bridge: $INSTALLED_HOOK"
  echo "Installed Claude Code hooks: $CLAUDE_SETTINGS"
  echo "Installed Codex hooks: $CODEX_HOOKS"
  echo "Codex will ask you to review and trust the new hooks once via /hooks."
}

case "${1:-}" in
  --uninstall)
    uninstall
    ;;
  *)
    install
    ;;
esac
