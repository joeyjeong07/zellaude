#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
TEST_HOME="$TEST_DIR/home"
PERMISSIONS_FILE="$TEST_HOME/.cache/zellij/permissions.kdl"
PLUGIN_KEY="$TEST_HOME/.config/zellij/plugins/zellaude.wasm"
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$(dirname "$PERMISSIONS_FILE")"

run_install() {
  HOME="$TEST_HOME" "$PROJECT_DIR/scripts/install-permissions.sh" "$@" >/dev/null
}

seed_permissions() {
  cat > "$PERMISSIONS_FILE" <<'KDL'
"/home/someone/.config/zellij/plugins/other.wasm" {
    ReadApplicationState
    RunCommands
}
KDL
}

zellaude_block_count() {
  grep -cxF "\"$PLUGIN_KEY\" {" "$PERMISSIONS_FILE" || true
}

# The granted set must match REQUIRED_PERMISSIONS exactly. A smaller set is
# granted silently and still leaves the bar inert, which is the failure this
# whole script exists to prevent.
assert_permissions_match_source() {
  local declared granted
  declared=$(awk '
    /^const REQUIRED_PERMISSIONS/ { in_list = 1; next }
    in_list && /\];/ { exit }
    in_list && match($0, /PermissionType::[A-Za-z]+/) {
      print substr($0, RSTART + 16, RLENGTH - 16)
    }
  ' "$PROJECT_DIR/src/main.rs" | sort)
  granted=$(awk -v key="\"$PLUGIN_KEY\" {" '
    $0 == key { inside = 1; next }
    inside && /^}/ { exit }
    inside { gsub(/^[[:space:]]+|[[:space:]]+$/, ""); if ($0 != "") print }
  ' "$PERMISSIONS_FILE" | sort)
  [ -n "$declared" ]
  [ "$declared" = "$granted" ]
}

assert_unrelated_preserved() {
  [ "$(grep -cxF '"/home/someone/.config/zellij/plugins/other.wasm" {' "$PERMISSIONS_FILE")" -eq 1 ]
}

assert_installed() {
  [ "$(zellaude_block_count)" -eq 1 ]
  assert_permissions_match_source
}

# Ordinary reinstalls stay idempotent and leave other plugins alone.
seed_permissions
for _ in 1 2; do
  run_install
done
assert_installed
assert_unrelated_preserved

# Every status-bar instance reloads together and can seed concurrently. The
# result must still be exactly one block.
seed_permissions
pids=()
for _ in {1..8}; do
  run_install &
  pids+=("$!")
done
for pid in "${pids[@]}"; do
  wait "$pid"
done
assert_installed
assert_unrelated_preserved

# A first install must also be safe when no cache file exists yet.
rm -f "$PERMISSIONS_FILE"
pids=()
for _ in {1..8}; do
  run_install &
  pids+=("$!")
done
for pid in "${pids[@]}"; do
  wait "$pid"
done
assert_installed

# Uninstall removes only Zellaude's entry.
seed_permissions
run_install
run_install --uninstall
[ "$(zellaude_block_count)" -eq 0 ]
assert_unrelated_preserved

# Uninstalling twice is not an error.
run_install --uninstall
[ "$(zellaude_block_count)" -eq 0 ]

printf 'permission installation idempotency tests passed\n'
