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
# Usage: ./scripts/install-permissions.sh [--check|--granted|--uninstall]
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INSTALL_HOME="${ZELLAUDE_INSTALL_HOME:-$HOME}"
PLUGIN_PATH="$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm"

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
  if [ -n "${ZELLAUDE_CACHE_DIR:-}" ]; then
    printf '%s\n' "$ZELLAUDE_CACHE_DIR"
    return
  fi
  if command -v zellij >/dev/null 2>&1; then
    dir=$(zellij setup --check 2>/dev/null |
      awk -F': ' '/^\[CACHE DIR\]:/ { gsub(/^"|"$/, "", $2); print $2; exit }' || true)
  fi
  if [ -z "$dir" ]; then
    dir="${XDG_CACHE_HOME:-$INSTALL_HOME/.cache}/zellij"
  fi
  printf '%s\n' "$dir"
}

CACHE_DIR=$(resolve_cache_dir)
PERMISSIONS_FILE="$CACHE_DIR/permissions.kdl"

resolve_file_symlink() {
  local path=$1 dir target hops=0
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

if [ -L "$PERMISSIONS_FILE" ]; then
  PERMISSIONS_FILE=$(resolve_file_symlink "$PERMISSIONS_FILE")
  CACHE_DIR=$(dirname "$PERMISSIONS_FILE")
fi

LOCK_DIR="$CACHE_DIR/.zellaude-permissions.lock"
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
  mkdir -p "$CACHE_DIR"
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
      echo "Error: Timed out waiting for Zellaude's permission-cache lock: $LOCK_DIR" >&2
      exit 1
    fi
    sleep 0.05
  done
  LOCK_HELD=true
  printf '%s\n' "$$" > "$LOCK_DIR/pid"
}

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

# ── Grant status ───────────────────────────────────────────
#
# Zellij grants silently only when every requested permission is cached; a
# partial block still prompts. When several blocks share the key, Zellij's
# parser keeps the last one, so read the way it reads.

granted_permissions() {
  [ -f "$PERMISSIONS_FILE" ] || return 0
  awk -v key="\"$PLUGIN_PATH\" {" '
    $0 == key { inside = 1; block = ""; next }
    inside && /^}/ { inside = 0; next }
    inside {
      gsub(/^[[:space:]]+|[[:space:]]+$/, "")
      if ($0 != "") block = block $0 "\n"
    }
    END { printf "%s", block }
  ' "$PERMISSIONS_FILE"
}

all_permissions_granted() {
  local granted permission
  granted=$(granted_permissions)
  [ -n "$granted" ] || return 1
  while IFS= read -r permission; do
    [ -n "$permission" ] || continue
    printf '%s\n' "$granted" | grep -qx "$permission" || return 1
  done <<< "$PERMISSIONS"
}

case "${1:-}" in
  ""|--check|--granted|--uninstall) ;;
  *)
    echo "Usage: $0 [--check|--granted|--uninstall]" >&2
    exit 1
    ;;
esac

validate_permissions_file() {
  [ ! -e "$PERMISSIONS_FILE" ] && return 0
  if [ ! -f "$PERMISSIONS_FILE" ] || ! awk '
    function trim(s) {
      sub(/^[[:space:]]+/, "", s)
      sub(/[[:space:]]+$/, "", s)
      return s
    }
    {
      line = trim($0)
      if (line == "" || line ~ /^\/\// || line ~ /^#/) next
      if (line ~ /^".*"[[:space:]]*\{$/) {
        if (depth != 0) invalid = 1
        depth = 1
        next
      }
      if (line == "}") {
        if (depth != 1) invalid = 1
        depth = 0
        next
      }
      if (depth != 1 || line !~ /^[A-Za-z][A-Za-z0-9]*$/) invalid = 1
    }
    END { exit(invalid || depth != 0) }
  ' "$PERMISSIONS_FILE"; then
    echo "Error: $PERMISSIONS_FILE contains malformed permission-cache KDL" >&2
    return 1
  fi
}

validate_permissions_file

if [ "${1:-}" = "--check" ]; then
  echo "Permission cache is valid"
  exit 0
fi

# Exit 0 iff the full grant is cached. Read-only, so no lock is taken.
if [ "${1:-}" = "--granted" ]; then
  all_permissions_granted
  exit 0
fi

trap release_lock EXIT
acquire_lock
validate_permissions_file

case "${1:-}" in
  --uninstall)
    if [ -f "$PERMISSIONS_FILE" ]; then
      write_permissions
      echo "Removed zellaude permissions from $PERMISSIONS_FILE"
    else
      echo "No zellaude permissions found"
    fi
    ;;
  "")
    # A complete grant is left untouched: a live Zellij server also writes
    # this file from its in-memory grants, so routine re-runs (setup scripts
    # converging a wiped cache) must not rewrite it without need.
    if all_permissions_granted; then
      echo "Zellaude permissions already granted in $PERMISSIONS_FILE"
    else
      write_permissions --with-entry
      echo "Pre-granted Zellaude permissions in $PERMISSIONS_FILE"
      dim "  $(printf '%s' "$PERMISSIONS" | tr '\n' ' ')"
      dim "  Skip this step with: ./install.sh --no-permissions"
    fi
    ;;
esac
