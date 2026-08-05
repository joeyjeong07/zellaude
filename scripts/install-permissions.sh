#!/usr/bin/env bash
# install-permissions.sh — Pre-grant Zellaude's plugin permissions
#
# Zellij asks for plugin permissions by drawing its own prompt inside the
# plugin's pane. Zellaude's documented layout mounts it as a one-row borderless
# status bar, so that prompt has nowhere to render and normal focus navigation
# skips the pane — a new install shows an empty bar with no reachable way to
# answer. Everything the plugin does is gated behind the grant (hook install,
# runtime keybindings, rendering), so the bar stays inert until it arrives.
#
# Seeding Zellij's permission cache at install time removes the prompt for the
# plugin the user just chose to install. Other plugins' entries are preserved.
#
# Usage: ./scripts/install-permissions.sh [--uninstall]
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PLUGIN_PATH="$HOME/.config/zellij/plugins/zellaude.wasm"

dim() { printf '\033[2m%s\033[0m\n' "$*"; }

# ── Permission list ────────────────────────────────────────
#
# Read from REQUIRED_PERMISSIONS so this file cannot drift from the set the
# plugin actually requests. A smaller granted set would leave the bar inert in
# exactly the way this script exists to prevent.

read_required_permissions() {
  awk '
    /^const REQUIRED_PERMISSIONS/ { in_list = 1; next }
    in_list && /\];/ { exit }
    in_list && match($0, /PermissionType::[A-Za-z]+/) {
      print substr($0, RSTART + 16, RLENGTH - 16)
    }
  ' "$PROJECT_DIR/src/main.rs"
}

PERMISSIONS=$(read_required_permissions || true)

if [ -z "$PERMISSIONS" ]; then
  echo "Error: Could not read REQUIRED_PERMISSIONS from $PROJECT_DIR/src/main.rs" >&2
  exit 1
fi

# ── Locate Zellij's permission cache ───────────────────────
#
# Ask Zellij rather than guessing: the cache lives under different roots per
# platform. Fall back to the XDG default when Zellij isn't installed yet.

resolve_cache_dir() {
  local dir=""
  if command -v zellij >/dev/null 2>&1; then
    dir=$(zellij setup --check 2>/dev/null |
      awk -F': ' '/^\[CACHE DIR\]:/ { gsub(/^"|"$/, "", $2); print $2; exit }')
  fi
  if [ -z "$dir" ]; then
    dir="${XDG_CACHE_HOME:-$HOME/.cache}/zellij"
  fi
  printf '%s\n' "$dir"
}

CACHE_DIR=$(resolve_cache_dir)
PERMISSIONS_FILE="$CACHE_DIR/permissions.kdl"

# Drop any existing block for one key, preserving every other plugin's grants.
strip_entry() {
  local file=$1 key=$2
  [ -f "$file" ] || return 0
  awk -v key="$key" '
    function trim(s) { sub(/^[[:space:]]+/, "", s); sub(/[[:space:]]+$/, "", s); return s }
    BEGIN { dropping = 0; depth = 0 }
    {
      line = trim($0)
      if (dropping) {
        depth += gsub(/\{/, "{", line) - gsub(/\}/, "}", line)
        if (depth <= 0) dropping = 0
        next
      }
      if (line == "\"" key "\" {") { dropping = 1; depth = 1; next }
      print
    }
  ' "$file"
}

write_permissions() {
  local tmp
  mkdir -p "$CACHE_DIR"
  tmp=$(mktemp "$CACHE_DIR/.zellaude-permissions.XXXXXX")
  strip_entry "$PERMISSIONS_FILE" "$PLUGIN_PATH" > "$tmp"
  if [ "${1:-}" = "--with-entry" ]; then
    printf '"%s" {\n' "$PLUGIN_PATH" >> "$tmp"
    while IFS= read -r permission; do
      [ -n "$permission" ] || continue
      printf '    %s\n' "$permission" >> "$tmp"
    done <<< "$PERMISSIONS"
    printf '}\n' >> "$tmp"
  fi
  # Concurrent status-bar instances may install at once; the rename is atomic.
  mv "$tmp" "$PERMISSIONS_FILE"
}

case "${1:-}" in
  --uninstall)
    if [ -f "$PERMISSIONS_FILE" ]; then
      write_permissions
      echo "Removed zellaude permissions from $PERMISSIONS_FILE"
    else
      echo "No zellaude permissions found"
    fi
    ;;
  *)
    write_permissions --with-entry
    echo "Pre-granted Zellaude permissions in $PERMISSIONS_FILE"
    dim "  $(printf '%s' "$PERMISSIONS" | tr '\n' ' ')"
    dim "  Skip this step with: ./install.sh --no-permissions"
    ;;
esac
