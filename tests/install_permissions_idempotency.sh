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
  ZELLAUDE_INSTALL_HOME="$TEST_HOME" \
    ZELLAUDE_CACHE_DIR="$TEST_HOME/.cache/zellij" \
    "$PROJECT_DIR/scripts/install-permissions.sh" "$@" >/dev/null
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

# Replacing the cache contents must not replace a user-managed symlink with a
# regular file. The atomic rewrite happens beside the resolved target instead.
EXTERNAL_PERMISSIONS="$TEST_DIR/external-cache/permissions.kdl"
mkdir -p "$(dirname "$EXTERNAL_PERMISSIONS")"
rm -f "$PERMISSIONS_FILE"
printf '%s\n' '"/home/someone/.config/zellij/plugins/other.wasm" {' \
  '    ReadApplicationState' '}' > "$EXTERNAL_PERMISSIONS"
ln -s "$EXTERNAL_PERMISSIONS" "$PERMISSIONS_FILE"
run_install
[ -L "$PERMISSIONS_FILE" ]
assert_installed
assert_unrelated_preserved

# A truncated block could swallow the appended grant. Both check and install
# must reject malformed cache KDL byte-for-byte instead of reporting success.
rm -f "$PERMISSIONS_FILE"
printf '%s\n' '"/tmp/truncated.wasm" {' '    RunCommands' > "$PERMISSIONS_FILE"
cp "$PERMISSIONS_FILE" "$TEST_DIR/permissions-malformed.before"
if run_install --check 2>/dev/null; then
  echo "expected malformed permission cache check to fail" >&2
  exit 1
fi
if run_install 2>/dev/null; then
  echo "expected malformed permission cache install to fail" >&2
  exit 1
fi
cmp -s "$PERMISSIONS_FILE" "$TEST_DIR/permissions-malformed.before"

# Uninstall removes only Zellaude's entry.
rm -f "$PERMISSIONS_FILE"
seed_permissions
run_install
run_install --uninstall
[ "$(zellaude_block_count)" -eq 0 ]
assert_unrelated_preserved

# Uninstalling twice is not an error.
run_install --uninstall
[ "$(zellaude_block_count)" -eq 0 ]

# --granted reports the grant without writing anything.
rm -f "$PERMISSIONS_FILE"
if run_install --granted; then exit 1; fi
run_install
run_install --granted

# A partial grant — Zellij's per-permission check would still prompt — is not
# "granted", and a reinstall completes it.
printf '"%s" {\n    ReadApplicationState\n}\n' "$PLUGIN_KEY" > "$PERMISSIONS_FILE"
if run_install --granted; then exit 1; fi
run_install
run_install --granted
assert_installed

# A complete grant is left untouched: reinstalls on top of it must not rewrite
# the file (the atomic rename would swap the inode), so routine re-runs cannot
# race a live Zellij server that also writes this file.
inode_before=$(ls -i "$PERMISSIONS_FILE" | awk '{print $1}')
run_install
[ "$(ls -i "$PERMISSIONS_FILE" | awk '{print $1}')" = "$inode_before" ]

printf 'permission installation idempotency tests passed\n'
