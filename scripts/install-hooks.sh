#!/usr/bin/env bash
# install-hooks.sh — Install the bridge and register Claude Code/Codex hooks
#
# Usage: ./scripts/install-hooks.sh [--check|--uninstall]
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PACKAGE_VERSION=$(awk -F '"' '/^version = "/ { print $2; exit }' "$PROJECT_DIR/Cargo.toml")
INSTALL_HOME="${ZELLAUDE_INSTALL_HOME:-$HOME}"
CLAUDE_SETTINGS="$INSTALL_HOME/.claude/settings.json"
CODEX_CONFIG_DIR="${ZELLAUDE_CODEX_HOME:-${CODEX_HOME:-$INSTALL_HOME/.codex}}"
CODEX_HOOKS="$CODEX_CONFIG_DIR/hooks.json"
SOURCE_HOOK="$(cd "$(dirname "$0")" && pwd)/zellaude-hook.sh"
INSTALLED_HOOK="$INSTALL_HOME/.config/zellij/plugins/zellaude-hook.sh"
CLAUDE_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh'
CODEX_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh --client codex'
LOCK_DIR="$INSTALL_HOME/.config/zellij/plugins/.zellaude-install.lock"

CLAUDE_EVENTS='["PreToolUse","PostToolUse","PostToolUseFailure","UserPromptSubmit","PermissionRequest","Notification","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
CODEX_EVENTS='["PreToolUse","PostToolUse","UserPromptSubmit","PermissionRequest","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'

LOCK_HELD=false

release_lock() {
  if [ "$LOCK_HELD" = true ]; then
    rm -f "$LOCK_DIR/pid"
    rmdir "$LOCK_DIR" 2>/dev/null || true
    LOCK_HELD=false
  fi
}

acquire_lock() {
  local attempts=0 owner=""
  mkdir -p "$(dirname "$LOCK_DIR")"
  while ! mkdir "$LOCK_DIR" 2>/dev/null; do
    owner=""
    if [ -r "$LOCK_DIR/pid" ]; then
      owner=$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null || true)
    fi
    local stale=false current_owner=""
    case "$owner" in
      ""|*[!0-9]*)
        [ "$attempts" -ge 20 ] && stale=true
        ;;
      *)
        kill -0 "$owner" 2>/dev/null || stale=true
        ;;
    esac
    if [ "$stale" = true ]; then
      current_owner=$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null || true)
      if [ "$current_owner" = "$owner" ]; then
        [ ! -e "$LOCK_DIR/pid" ] || rm -f "$LOCK_DIR/pid"
        if rmdir "$LOCK_DIR" 2>/dev/null; then
          continue
        fi
      fi
    fi
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 200 ]; then
      echo "Error: Timed out waiting for Zellaude's installer lock: $LOCK_DIR" >&2
      exit 1
    fi
    sleep 0.05
  done
  LOCK_HELD=true
  printf '%s\n' "$$" > "$LOCK_DIR/pid"
}

resolve_file_symlink() {
  local path dir target hops=0
  path=$1
  while [ -L "$path" ]; do
    hops=$((hops + 1))
    if [ "$hops" -gt 40 ]; then
      echo "Error: Too many symbolic links while resolving $1" >&2
      return 1
    fi
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
if [ -z "$PACKAGE_VERSION" ]; then
  echo "Error: Could not read the zellaude version from $PROJECT_DIR/Cargo.toml" >&2
  exit 1
fi

backup_file() {
  local file=$1 tmp
  if [ -f "$file" ]; then
    tmp=$(mktemp "$(dirname "$file")/.zellaude-backup.XXXXXX")
    cp "$file" "$tmp"
    mv "$tmp" "$file.bak"
    echo "Backed up $file to $file.bak"
  fi
}

ensure_json_file() {
  local file=$1 tmp
  if [ ! -f "$file" ]; then
    mkdir -p "$(dirname "$file")"
    tmp=$(mktemp "$(dirname "$file")/.zellaude-hooks.XXXXXX")
    printf '{}\n' > "$tmp"
    mv "$tmp" "$file"
  fi
}

remove_zellaude_entries() {
  local file=$1
  local owned_command=$2
  [ -f "$file" ] || return 0

  local tmp
  tmp=$(mktemp "$(dirname "$file")/.zellaude-hooks.XXXXXX")
  jq --arg owned "$owned_command" '
    if .hooks and (.hooks | type == "object") then
      .hooks |= with_entries(
        .value |= [
          .[] | . as $group |
          ($group.hooks // []) as $original |
          ($original | map(select((.command // "") != $owned))) as $filtered |
          if ($original | length) == 0
          then $group
          elif ($filtered | length) > 0
          then ($group | .hooks = $filtered)
          else empty
          end
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

replace_zellaude_entries() {
  local file=$1
  local events=$2
  local entry=$3
  local owned_command=$4
  local tmp
  tmp=$(mktemp "$(dirname "$file")/.zellaude-hooks.XXXXXX")
  jq --argjson events "$events" --argjson entry "$entry" --arg owned "$owned_command" '
    .hooks //= {} |
    .hooks |= with_entries(
      .value |= [
        .[] | . as $group |
        ($group.hooks // []) as $original |
        ($original | map(select((.command // "") != $owned))) as $filtered |
        if ($original | length) == 0
        then $group
        elif ($filtered | length) > 0
        then ($group | .hooks = $filtered)
        else empty
        end
      ]
    ) |
    .hooks |= with_entries(select(.value | length > 0)) |
    .hooks //= {} |
    reduce ($events[]) as $event (
      .;
      .hooks[$event] = (.hooks[$event] // []) + $entry
    )
  ' "$file" > "$tmp"
  mv "$tmp" "$file"
}

validate_settings_file() {
  local file=$1
  [ ! -e "$file" ] && return 0
  if [ ! -f "$file" ] || ! jq -se '
    length == 1 and
    (
      .[0] |
      type == "object" and
      (
        (.hooks? == null) or
        (
          (.hooks | type == "object") and
          all(
            .hooks[]?;
            type == "array" and all(
              .[];
              type == "object" and
              (
                (.hooks? == null) or
                (
                  (.hooks | type == "array") and
                  all(.hooks[]; type == "object")
                )
              )
            )
          )
        )
      )
    )
  ' "$file" >/dev/null; then
    echo "Error: $file contains an invalid hooks configuration" >&2
    return 1
  fi
}

uninstall() {
  local found=false
  local file owned_command
  validate_settings_file "$CLAUDE_SETTINGS"
  validate_settings_file "$CODEX_HOOKS"
  for file in "$CLAUDE_SETTINGS" "$CODEX_HOOKS"; do
    if [ -f "$file" ]; then
      found=true
      backup_file "$file"
      if [ "$file" = "$CLAUDE_SETTINGS" ]; then
        owned_command=$CLAUDE_HOOK_CMD
      else
        owned_command=$CODEX_HOOK_CMD
      fi
      remove_zellaude_entries "$file" "$owned_command"
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
  local hook_tmp
  # Validate every existing input before changing any user-owned file or the
  # installed bridge. A malformed settings file should fail closed instead of
  # being silently replaced or leaving a half-installed hook set.
  check_settings
  ensure_json_file "$CLAUDE_SETTINGS"
  ensure_json_file "$CODEX_HOOKS"

  mkdir -p "$(dirname "$INSTALLED_HOOK")"
  hook_tmp=$(mktemp "$(dirname "$INSTALLED_HOOK")/.zellaude-hook.XXXXXX")
  awk -v version="$PACKAGE_VERSION" '
    NR == 1 {
      print
      print "# zellaude v" version
      next
    }
    { print }
  ' "$SOURCE_HOOK" > "$hook_tmp"
  chmod +x "$hook_tmp"
  mv "$hook_tmp" "$INSTALLED_HOOK"

  backup_file "$CLAUDE_SETTINGS"
  backup_file "$CODEX_HOOKS"

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

  # Remove and replace zellaude's handlers in one transaction per file. This
  # stays idempotent when several plugin instances auto-install concurrently.
  replace_zellaude_entries \
    "$CLAUDE_SETTINGS" "$CLAUDE_EVENTS" "$claude_entry" "$CLAUDE_HOOK_CMD"
  replace_zellaude_entries \
    "$CODEX_HOOKS" "$CODEX_EVENTS" "$codex_entry" "$CODEX_HOOK_CMD"

  echo "Installed zellaude bridge: $INSTALLED_HOOK"
  echo "Installed Claude Code hooks: $CLAUDE_SETTINGS"
  echo "Installed Codex hooks: $CODEX_HOOKS"
  echo "Codex will ask you to review and trust the new hooks once via /hooks."
}

check_settings() {
  validate_settings_file "$CLAUDE_SETTINGS"
  validate_settings_file "$CODEX_HOOKS"
  if [ -e "$INSTALLED_HOOK" ] && [ ! -f "$INSTALLED_HOOK" ]; then
    echo "Error: Cannot install hook over non-file destination: $INSTALLED_HOOK" >&2
    return 1
  fi
}

case "${1:-}" in
  --check)
    check_settings
    echo "Hook settings are valid"
    ;;
  --uninstall)
    trap release_lock EXIT
    acquire_lock
    uninstall
    ;;
  "")
    trap release_lock EXIT
    acquire_lock
    install
    ;;
  *)
    echo "Usage: $0 [--check|--uninstall]" >&2
    exit 1
    ;;
esac
