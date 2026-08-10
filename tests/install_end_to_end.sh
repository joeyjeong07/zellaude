#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REAL_JQ=$(command -v jq) || {
  echo "install end-to-end tests require jq" >&2
  exit 1
}

fail() {
  echo "install end-to-end test failed: $*" >&2
  exit 1
}

# Refuse to run against an installer that has not wired every isolation seam.
# In particular, falling back to HOME here would turn a test into a real install.
require_seam() {
  local file=$1 seam=$2
  grep -q "$seam" "$PROJECT_DIR/$file" ||
    fail "$file does not honor $seam; refusing to run outside the sandbox"
}

require_seam install.sh ZELLAUDE_INSTALL_HOME
require_seam install.sh ZELLAUDE_BUILD_DIR
require_seam scripts/install-hooks.sh ZELLAUDE_INSTALL_HOME
require_seam scripts/install-hooks.sh ZELLAUDE_CODEX_HOME
require_seam scripts/install-permissions.sh ZELLAUDE_INSTALL_HOME
require_seam scripts/install-permissions.sh ZELLAUDE_CACHE_DIR

TEST_PARENT=${TMPDIR:-/tmp}
case "$TEST_PARENT/" in
  "$HOME/"*) TEST_PARENT=/tmp ;;
esac
TEST_DIR=$(mktemp -d "$TEST_PARENT/zellaude-install-e2e.XXXXXX")
trap 'rm -rf "$TEST_DIR"' EXIT
case "$TEST_DIR/" in
  "$HOME/"*) fail "sandbox unexpectedly lives under the real HOME" ;;
esac

CLAUDE_EVENTS='["PreToolUse","PostToolUse","PostToolUseFailure","UserPromptSubmit","PermissionRequest","Notification","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
CODEX_EVENTS='["PreToolUse","PostToolUse","UserPromptSubmit","PermissionRequest","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
CLAUDE_OWNED='${HOME}/.config/zellij/plugins/zellaude-hook.sh'
CODEX_OWNED='${HOME}/.config/zellij/plugins/zellaude-hook.sh --client codex'
PACKAGE_VERSION=$(awk -F '"' '/^version = "/ { print $2; exit }' "$PROJECT_DIR/Cargo.toml")

link_real_tool() {
  local name=$1 path
  path=$(command -v "$name") || fail "required test utility is missing: $name"
  ln -s "$path" "$FAKE_BIN/$name"
}

write_fake_commands() {
  cat > "$FAKE_BIN/cargo" <<'SH'
#!/bin/sh
set -eu
: "${ZELLAUDE_TEST_LOG_DIR:?}"
{
  printf 'CALL\n'
  printf 'PWD=%s\n' "$PWD"
  printf 'CARGO_TARGET_DIR=%s\n' "${CARGO_TARGET_DIR-}"
  for arg do
    printf 'ARG=%s\n' "$arg"
  done
  printf 'END\n'
} >> "$ZELLAUDE_TEST_LOG_DIR/cargo.args"

if [ "${1:-}" = "--version" ]; then
  printf '%s\n' 'cargo 1.88.0 (test)'
  exit 0
fi
if [ "${ZELLAUDE_TEST_CARGO_FAIL:-0}" = 1 ]; then
  printf '%s\n' 'intentional cargo failure' >&2
  exit 42
fi

target=wasm32-wasip1
target_dir=${CARGO_TARGET_DIR:-${ZELLAUDE_BUILD_DIR:?}}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target)
      shift
      target=$1
      ;;
    --target=*) target=${1#--target=} ;;
    --target-dir)
      shift
      target_dir=$1
      ;;
    --target-dir=*) target_dir=${1#--target-dir=} ;;
  esac
  shift
done

if [ "${ZELLAUDE_TEST_CARGO_ARTIFACT:-normal}" = missing ]; then
  exit 0
fi

mkdir -p "$target_dir/$target/release"
if [ "${ZELLAUDE_TEST_CARGO_ARTIFACT:-normal}" = empty ]; then
  : > "$target_dir/$target/release/zellaude.wasm"
else
  printf '\000asm-zellaude-install-test\n' > "$target_dir/$target/release/zellaude.wasm"
fi
SH

  cat > "$FAKE_BIN/rustup" <<'SH'
#!/bin/sh
set -eu
: "${ZELLAUDE_TEST_LOG_DIR:?}"
printf '%s\n' "$*" >> "$ZELLAUDE_TEST_LOG_DIR/rustup.args"
case "$*" in
  '--version') printf '%s\n' 'rustup 1.28.2 (test)' ;;
  'target list --installed')
    if [ "${ZELLAUDE_TEST_TARGET_INSTALLED:-1}" = 1 ]; then
      printf '%s\n' 'wasm32-wasip1'
    fi
    ;;
  'target add wasm32-wasip1')
    if [ "${ZELLAUDE_TEST_TARGET_ADD_FAIL:-0}" = 1 ]; then
      printf '%s\n' 'intentional target-add failure' >&2
      exit 55
    fi
    ;;
  *) printf 'unexpected rustup arguments: %s\n' "$*" >&2; exit 64 ;;
esac
SH

  cat > "$FAKE_BIN/zellij" <<'SH'
#!/bin/sh
set -eu
: "${ZELLAUDE_TEST_LOG_DIR:?}"
printf '%s\n' "$*" >> "$ZELLAUDE_TEST_LOG_DIR/zellij.args"
case "${1:-}" in
  --version)
    printf 'zellij %s\n' "${ZELLAUDE_TEST_ZELLIJ_VERSION:-0.44.3}"
    ;;
  setup)
    printf '[CACHE DIR]: "%s"\n' "${ZELLAUDE_CACHE_DIR:?}"
    ;;
  *)
    printf 'unexpected zellij arguments: %s\n' "$*" >&2
    exit 64
    ;;
esac
SH

  chmod +x "$FAKE_BIN/cargo" "$FAKE_BIN/rustup" "$FAKE_BIN/zellij"
}

prepare_case() {
  local name=$1 tool
  CASE_ROOT="$TEST_DIR/$name"
  INSTALL_HOME="$CASE_ROOT/install-home"
  CODEX_INSTALL_HOME="$CASE_ROOT/codex-home"
  CACHE_DIR="$CASE_ROOT/cache/zellij"
  AMBIENT_CODEX_HOME="$CASE_ROOT/ambient-codex-home"
  AMBIENT_XDG_CACHE_HOME="$CASE_ROOT/ambient-xdg-cache"
  BUILD_DIR="$CASE_ROOT/build"
  AMBIENT_CARGO_TARGET="$CASE_ROOT/ambient-cargo-target"
  OUTSIDE_DIR="$CASE_ROOT/outside-repository"
  FAKE_BIN="$CASE_ROOT/bin"
  LOG_DIR="$CASE_ROOT/log"
  mkdir -p "$OUTSIDE_DIR" "$FAKE_BIN" "$LOG_DIR"

  # Keep PATH hermetic so a host cargo, rustup, or zellij cannot make a
  # deliberately missing-tool case pass. jq is the real binary, not a stub.
  for tool in \
    bash jq awk grep sed cut sort tail uname head tr mkdir rmdir sleep cp mv \
    chmod mktemp readlink cat rm dirname basename; do
    if [ "$tool" = jq ]; then
      ln -s "$REAL_JQ" "$FAKE_BIN/jq"
    else
      link_real_tool "$tool"
    fi
  done
  write_fake_commands
}

run_installer() {
  (
    cd "$OUTSIDE_DIR"
    PATH="$FAKE_BIN" \
      CARGO_TARGET_DIR="$AMBIENT_CARGO_TARGET" \
      ZELLAUDE_INSTALL_HOME="$INSTALL_HOME" \
      ZELLAUDE_CODEX_HOME="$CODEX_INSTALL_HOME" \
      ZELLAUDE_CACHE_DIR="$CACHE_DIR" \
      ZELLAUDE_BUILD_DIR="$BUILD_DIR" \
      ZELLAUDE_TEST_LOG_DIR="$LOG_DIR" \
      ZELLAUDE_TEST_CARGO_FAIL="${FAKE_CARGO_FAIL:-0}" \
      ZELLAUDE_TEST_CARGO_ARTIFACT="${FAKE_CARGO_ARTIFACT:-normal}" \
      ZELLAUDE_TEST_ZELLIJ_VERSION="${FAKE_ZELLIJ_VERSION:-0.44.3}" \
      ZELLAUDE_TEST_TARGET_INSTALLED="${FAKE_TARGET_INSTALLED:-1}" \
      ZELLAUDE_TEST_TARGET_ADD_FAIL="${FAKE_TARGET_ADD_FAIL:-0}" \
      ZELLAUDE_TEST_MISSING_COMMAND="${FAKE_MISSING_COMMAND:-}" \
      BASH_ENV="${FAKE_BASH_ENV:-/dev/null}" \
      /bin/bash "$PROJECT_DIR/install.sh" "$@"
  )
}

run_installer_with_ambient_destinations() {
  (
    cd "$OUTSIDE_DIR"
    PATH="$FAKE_BIN" \
      CARGO_TARGET_DIR="$AMBIENT_CARGO_TARGET" \
      CODEX_HOME="$AMBIENT_CODEX_HOME" \
      XDG_CACHE_HOME="$AMBIENT_XDG_CACHE_HOME" \
      ZELLAUDE_INSTALL_HOME="$INSTALL_HOME" \
      ZELLAUDE_BUILD_DIR="$BUILD_DIR" \
      ZELLAUDE_TEST_LOG_DIR="$LOG_DIR" \
      ZELLAUDE_TEST_CARGO_ARTIFACT=normal \
      ZELLAUDE_TEST_TARGET_INSTALLED=1 \
      ZELLAUDE_TEST_TARGET_ADD_FAIL=0 \
      BASH_ENV=/dev/null \
      /bin/bash "$PROJECT_DIR/install.sh" "$@"
  )
}

has_argument_pair() {
  local file=$1 flag=$2 value=$3
  awk -v flag="ARG=$flag" -v value="ARG=$value" '
    $0 == flag { getline; if ($0 == value) found = 1 }
    END { exit !found }
  ' "$file"
}

assert_build_invocation() {
  local log="$LOG_DIR/cargo.args"
  [ -f "$log" ] || fail "cargo was not invoked"
  grep -qxF 'ARG=build' "$log" || fail "cargo build was not invoked"
  grep -qxF 'ARG=--locked' "$log" || fail "cargo build omitted --locked"
  if ! has_argument_pair "$log" --target wasm32-wasip1 &&
     ! grep -qxF 'ARG=--target=wasm32-wasip1' "$log"; then
    fail "cargo build omitted the explicit wasm32-wasip1 target"
  fi
  if ! has_argument_pair "$log" --target-dir "$BUILD_DIR" &&
     ! grep -qxF "ARG=--target-dir=$BUILD_DIR" "$log" &&
     ! grep -qxF "CARGO_TARGET_DIR=$BUILD_DIR" "$log"; then
    fail "ZELLAUDE_BUILD_DIR did not override ambient CARGO_TARGET_DIR"
  fi
}

owned_hook_count() {
  local file=$1 owned=$2
  "$REAL_JQ" --arg owned "$owned" '[
    .hooks[]?[]?
    | .hooks[]?
    | select((.command // "") == $owned)
  ] | length' "$file"
}

assert_exact_hooks() {
  local file=$1 events=$2 expected_count=$3 owned=$4
  local actual_events expected_events event count
  [ -f "$file" ] || fail "missing hook settings: $file"
  [ "$(owned_hook_count "$file" "$owned")" -eq "$expected_count" ] ||
    fail "wrong number of Zellaude hook registrations in $file"

  actual_events=$("$REAL_JQ" -r --arg owned "$owned" '
    .hooks
    | to_entries[]?
    | select(any(.value[]?.hooks[]?; (.command // "") == $owned))
    | .key
  ' "$file" | sort)
  expected_events=$(printf '%s' "$events" | "$REAL_JQ" -r '.[]' | sort)
  [ "$actual_events" = "$expected_events" ] ||
    fail "Zellaude hook event set differs in $file"

  while IFS= read -r event; do
    count=$("$REAL_JQ" --arg event "$event" --arg owned "$owned" '[
      .hooks[$event][]?.hooks[]?
      | select((.command // "") == $owned)
    ] | length' "$file")
    [ "$count" -eq 1 ] || fail "$event does not have exactly one Zellaude hook in $file"
  done < <(printf '%s' "$events" | "$REAL_JQ" -r '.[]')
}

assert_installed_files() {
  local plugin="$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm"
  local hook="$INSTALL_HOME/.config/zellij/plugins/zellaude-hook.sh"
  local built="$BUILD_DIR/wasm32-wasip1/release/zellaude.wasm"
  [ -s "$plugin" ] || fail "installed WASM is missing or empty"
  [ -f "$built" ] || fail "fake build artifact is missing"
  cmp -s "$built" "$plugin" || fail "installed WASM is not the freshly built artifact"
  [ -x "$hook" ] || fail "installed hook is not executable"
  grep -qxF "# zellaude v$PACKAGE_VERSION" "$hook" ||
    fail "installed hook has no package version marker"
}

declared_permissions() {
  awk '
    /^const REQUIRED_PERMISSIONS/ { in_list = 1; next }
    in_list && /\];/ { exit }
    in_list && match($0, /PermissionType::[A-Za-z]+/) {
      print substr($0, RSTART + 16, RLENGTH - 16)
    }
  ' "$PROJECT_DIR/src/main.rs" | sort
}

granted_permissions() {
  local file=$1 key=$2
  awk -v key="\"$key\" {" '
    $0 == key { inside = 1; next }
    inside && /^}/ { exit }
    inside {
      gsub(/^[[:space:]]+|[[:space:]]+$/, "")
      if ($0 != "") print
    }
  ' "$file" | sort
}

permission_block_count() {
  local file=$1 key=$2
  grep -cxF "\"$key\" {" "$file" 2>/dev/null || true
}

assert_exact_permissions() {
  local file="$CACHE_DIR/permissions.kdl"
  local key="$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm"
  local expected actual
  [ -f "$file" ] || fail "permissions cache was not created"
  [ "$(permission_block_count "$file" "$key")" -eq 1 ] ||
    fail "permissions cache does not contain exactly one Zellaude block"
  expected=$(declared_permissions)
  actual=$(granted_permissions "$file" "$key")
  [ -n "$expected" ] || fail "source permission list is empty"
  [ "$actual" = "$expected" ] || fail "granted permissions differ from REQUIRED_PERMISSIONS"
}

assert_complete_install() {
  assert_installed_files
  assert_exact_hooks \
    "$INSTALL_HOME/.claude/settings.json" "$CLAUDE_EVENTS" 11 "$CLAUDE_OWNED"
  assert_exact_hooks \
    "$CODEX_INSTALL_HOME/hooks.json" "$CODEX_EVENTS" 9 "$CODEX_OWNED"
  assert_exact_permissions
}

seed_unrelated_data() {
  local file tmp
  for file in \
    "$INSTALL_HOME/.claude/settings.json" \
    "$CODEX_INSTALL_HOME/hooks.json"; do
    tmp=$(mktemp "$(dirname "$file")/.unrelated.XXXXXX")
    "$REAL_JQ" '
      .unrelated_setting = "preserve-me"
      | .hooks.UnrelatedEvent = [{
          "matcher": "keep",
          "hooks": [{"type": "command", "command": "/bin/unrelated"}]
        }]
    ' "$file" > "$tmp"
    mv "$tmp" "$file"
  done
  cat >> "$CACHE_DIR/permissions.kdl" <<'KDL'
"/tmp/unrelated-plugin.wasm" {
    ReadApplicationState
}
KDL
}

assert_uninstall_preserved_unrelated_data() {
  local file owned plugin_key
  [ ! -e "$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm" ] ||
    fail "uninstall left the plugin installed"
  [ ! -e "$INSTALL_HOME/.config/zellij/plugins/zellaude-hook.sh" ] ||
    fail "uninstall left the hook bridge installed"

  for file in \
    "$INSTALL_HOME/.claude/settings.json" \
    "$CODEX_INSTALL_HOME/hooks.json"; do
    if [ "$file" = "$INSTALL_HOME/.claude/settings.json" ]; then
      owned=$CLAUDE_OWNED
    else
      owned=$CODEX_OWNED
    fi
    [ "$(owned_hook_count "$file" "$owned")" -eq 0 ] ||
      fail "uninstall left Zellaude hook entries in $file"
    "$REAL_JQ" -e '
      .unrelated_setting == "preserve-me"
      and ([.hooks.UnrelatedEvent[]?.hooks[]? | select(.command == "/bin/unrelated")] | length == 1)
    ' "$file" >/dev/null || fail "uninstall removed unrelated settings from $file"
  done

  plugin_key="$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm"
  [ "$(permission_block_count "$CACHE_DIR/permissions.kdl" "$plugin_key")" -eq 0 ] ||
    fail "uninstall left Zellaude permissions behind"
  [ "$(permission_block_count "$CACHE_DIR/permissions.kdl" /tmp/unrelated-plugin.wasm)" -eq 1 ] ||
    fail "uninstall removed another plugin's permissions"
}

assert_no_install_writes() {
  [ ! -e "$INSTALL_HOME" ] || fail "failed preflight wrote under install home"
  [ ! -e "$CODEX_INSTALL_HOME" ] || fail "failed preflight wrote under Codex home"
  [ ! -e "$CACHE_DIR" ] || fail "failed preflight wrote under cache directory"
  [ ! -e "$BUILD_DIR" ] || fail "failed preflight wrote under build directory"
}

assert_no_cargo_build() {
  local log="$LOG_DIR/cargo.args"
  if [ -f "$log" ] && grep -qxF 'ARG=build' "$log"; then
    fail "cargo build ran before prerequisite validation completed"
  fi
}

# A complete install must work while the caller is outside the repository.
prepare_case complete
run_installer > "$CASE_ROOT/install.out" 2>&1
assert_build_invocation
assert_complete_install

# Reinstalling must not duplicate any owned registrations or grants.
run_installer > "$CASE_ROOT/reinstall.out" 2>&1
assert_complete_install

# Uninstall removes only Zellaude's data.
seed_unrelated_data
run_installer --uninstall > "$CASE_ROOT/uninstall.out" 2>&1
assert_uninstall_preserved_unrelated_data

# A missing WASI target is added before the build and then installs normally.
prepare_case target-install
FAKE_TARGET_INSTALLED=0
run_installer > "$CASE_ROOT/install.out" 2>&1
unset FAKE_TARGET_INSTALLED
grep -qxF 'target add wasm32-wasip1' "$LOG_DIR/rustup.args" ||
  fail "installer did not add a missing wasm32-wasip1 target"
assert_complete_install

# An explicit staging home is an isolation boundary. Ambient Codex and XDG
# paths from the invoking account must not receive any writes.
prepare_case isolated-staging-home
run_installer_with_ambient_destinations > "$CASE_ROOT/install.out" 2>&1
CODEX_INSTALL_HOME="$INSTALL_HOME/.codex"
CACHE_DIR="$INSTALL_HOME/.cache/zellij"
assert_complete_install
[ ! -e "$AMBIENT_CODEX_HOME" ] || fail "staged install wrote to ambient CODEX_HOME"
[ ! -e "$AMBIENT_XDG_CACHE_HOME" ] || fail "staged install wrote to ambient XDG_CACHE_HOME"
HOME="$INSTALL_HOME" /bin/bash -c \
  'test -x "${HOME}/.config/zellij/plugins/zellaude-hook.sh"' ||
  fail "staged hook command does not resolve when the staged home becomes HOME"

# --no-permissions leaves the cache byte-for-byte unchanged while installing
# the plugin and both hook integrations normally.
prepare_case no-permissions
mkdir -p "$CACHE_DIR"
cat > "$CACHE_DIR/permissions.kdl" <<'KDL'
"/tmp/existing-plugin.wasm" {
    RunCommands
}
KDL
cp "$CACHE_DIR/permissions.kdl" "$CASE_ROOT/permissions.before"
run_installer --no-permissions > "$CASE_ROOT/install.out" 2>&1
assert_installed_files
assert_exact_hooks \
  "$INSTALL_HOME/.claude/settings.json" "$CLAUDE_EVENTS" 11 "$CLAUDE_OWNED"
assert_exact_hooks \
  "$CODEX_INSTALL_HOME/hooks.json" "$CODEX_EVENTS" 9 "$CODEX_OWNED"
cmp -s "$CASE_ROOT/permissions.before" "$CACHE_DIR/permissions.kdl" ||
  fail "--no-permissions changed the permissions cache"

# Missing prerequisites and an unsupported Zellij must fail before cargo or
# any install destination is touched.
prepare_case help-without-toolchain
rm "$FAKE_BIN/jq" "$FAKE_BIN/cargo" "$FAKE_BIN/rustup" "$FAKE_BIN/zellij"
run_installer --help > "$CASE_ROOT/help.out" 2>&1 ||
  fail "--help required installation prerequisites"
grep -q '^Usage:' "$CASE_ROOT/help.out" || fail "--help did not print usage"
assert_no_install_writes

prepare_case missing-jq
rm "$FAKE_BIN/jq"
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install succeeded without jq"
fi
grep -qi jq "$CASE_ROOT/install.out" || fail "missing-jq error did not name jq"
assert_no_cargo_build
assert_no_install_writes

prepare_case missing-cargo
rm "$FAKE_BIN/cargo"
cat > "$CASE_ROOT/mask-command.bash" <<'SH'
command() {
  if [ "${1:-}" = -v ] && [ "${2:-}" = "${ZELLAUDE_TEST_MISSING_COMMAND:-}" ]; then
    return 1
  fi
  builtin command "$@"
}
SH
FAKE_MISSING_COMMAND=cargo
FAKE_BASH_ENV="$CASE_ROOT/mask-command.bash"
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install succeeded without Cargo"
fi
unset FAKE_MISSING_COMMAND FAKE_BASH_ENV
grep -qi cargo "$CASE_ROOT/install.out" || fail "missing-Cargo error did not name Cargo"
assert_no_cargo_build
assert_no_install_writes

prepare_case missing-rustup
rm "$FAKE_BIN/rustup"
cat > "$CASE_ROOT/mask-command.bash" <<'SH'
command() {
  if [ "${1:-}" = -v ] && [ "${2:-}" = "${ZELLAUDE_TEST_MISSING_COMMAND:-}" ]; then
    return 1
  fi
  builtin command "$@"
}
SH
FAKE_MISSING_COMMAND=rustup
FAKE_BASH_ENV="$CASE_ROOT/mask-command.bash"
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install succeeded without rustup"
fi
unset FAKE_MISSING_COMMAND FAKE_BASH_ENV
grep -qi rustup "$CASE_ROOT/install.out" || fail "missing-rustup error did not name rustup"
assert_no_cargo_build
assert_no_install_writes

prepare_case target-add-failure
FAKE_TARGET_INSTALLED=0
FAKE_TARGET_ADD_FAIL=1
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install succeeded after rustup target add failed"
fi
unset FAKE_TARGET_INSTALLED FAKE_TARGET_ADD_FAIL
grep -q 'target add wasm32-wasip1' "$LOG_DIR/rustup.args" ||
  fail "target-add failure case never attempted the target install"
assert_no_cargo_build
assert_no_install_writes

prepare_case missing-zellij
rm "$FAKE_BIN/zellij"
cat > "$CASE_ROOT/mask-command.bash" <<'SH'
command() {
  if [ "${1:-}" = -v ] && [ "${2:-}" = "${ZELLAUDE_TEST_MISSING_COMMAND:-}" ]; then
    return 1
  fi
  builtin command "$@"
}
SH
FAKE_MISSING_COMMAND=zellij
FAKE_BASH_ENV="$CASE_ROOT/mask-command.bash"
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install succeeded without Zellij"
fi
unset FAKE_MISSING_COMMAND FAKE_BASH_ENV
grep -qi zellij "$CASE_ROOT/install.out" || fail "missing-Zellij error did not name Zellij"
assert_no_cargo_build
assert_no_install_writes

prepare_case unsupported-zellij
FAKE_ZELLIJ_VERSION=0.43.1
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install accepted unsupported Zellij 0.43.1"
fi
unset FAKE_ZELLIJ_VERSION
grep -Eq '0\.44|unsupported|Unsupported' "$CASE_ROOT/install.out" ||
  fail "unsupported-Zellij error did not explain the version requirement"
assert_no_cargo_build
assert_no_install_writes

# Existing hook files are user configuration. Invalid structures must be
# rejected byte-for-byte before Cargo or any install destination is touched.
prepare_case malformed-hooks
mkdir -p "$INSTALL_HOME/.claude" "$CODEX_INSTALL_HOME"
printf '{"hooks": []}\n' > "$INSTALL_HOME/.claude/settings.json"
printf '{"unrelated": true}\n' > "$CODEX_INSTALL_HOME/hooks.json"
cp "$INSTALL_HOME/.claude/settings.json" "$CASE_ROOT/claude.before"
cp "$CODEX_INSTALL_HOME/hooks.json" "$CASE_ROOT/codex.before"
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install accepted malformed hook settings"
fi
grep -qi 'invalid hooks' "$CASE_ROOT/install.out" ||
  fail "malformed-hooks error did not explain the invalid configuration"
assert_no_cargo_build
cmp -s "$CASE_ROOT/claude.before" "$INSTALL_HOME/.claude/settings.json" ||
  fail "malformed Claude settings changed"
cmp -s "$CASE_ROOT/codex.before" "$CODEX_INSTALL_HOME/hooks.json" ||
  fail "Codex settings changed after another config failed validation"
[ ! -e "$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm" ] ||
  fail "malformed settings left a plugin installed"
[ ! -e "$CACHE_DIR" ] || fail "malformed settings changed the permission cache"
[ ! -e "$BUILD_DIR" ] || fail "malformed settings started a build"

# A malformed permission cache is rejected during the read-only preflight, not
# after the plugin has already been built or installed.
prepare_case malformed-permissions
mkdir -p "$CACHE_DIR"
printf '%s\n' '"/tmp/truncated.wasm" {' '    RunCommands' > "$CACHE_DIR/permissions.kdl"
cp "$CACHE_DIR/permissions.kdl" "$CASE_ROOT/permissions.before"
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install accepted malformed permission-cache KDL"
fi
grep -qi 'malformed permission' "$CASE_ROOT/install.out" ||
  fail "malformed-permissions error did not explain the invalid cache"
assert_no_cargo_build
cmp -s "$CACHE_DIR/permissions.kdl" "$CASE_ROOT/permissions.before" ||
  fail "malformed permission cache changed"
[ ! -e "$INSTALL_HOME" ] || fail "malformed permission cache modified install home"
[ ! -e "$CODEX_INSTALL_HOME" ] || fail "malformed permission cache modified Codex home"
[ ! -e "$BUILD_DIR" ] || fail "malformed permission cache started a build"

# A build failure occurs after preflight but before installation, and must not
# leave any user-facing files behind.
prepare_case build-failure
FAKE_CARGO_FAIL=1
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install succeeded after cargo failed"
fi
unset FAKE_CARGO_FAIL
grep -qxF 'ARG=build' "$LOG_DIR/cargo.args" || fail "build-failure case never invoked cargo build"
[ ! -e "$INSTALL_HOME" ] || fail "build failure modified install home"
[ ! -e "$CODEX_INSTALL_HOME" ] || fail "build failure modified Codex home"
[ ! -e "$CACHE_DIR" ] || fail "build failure modified permissions cache"

# Cargo success is not enough: a missing or zero-byte artifact must never be
# copied into the plugin directory.
for artifact_mode in missing empty; do
  prepare_case "artifact-$artifact_mode"
  FAKE_CARGO_ARTIFACT=$artifact_mode
  if run_installer > "$CASE_ROOT/install.out" 2>&1; then
    fail "install accepted a $artifact_mode build artifact"
  fi
  unset FAKE_CARGO_ARTIFACT
  grep -qi 'nonempty plugin artifact' "$CASE_ROOT/install.out" ||
    fail "$artifact_mode-artifact error did not identify the build output"
  [ ! -e "$INSTALL_HOME" ] || fail "$artifact_mode artifact modified install home"
  [ ! -e "$CODEX_INSTALL_HOME" ] || fail "$artifact_mode artifact modified Codex home"
  [ ! -e "$CACHE_DIR" ] || fail "$artifact_mode artifact modified permissions cache"
done

# A directory at the plugin destination is an explicit error on both install
# and uninstall; mv/rm must not report success against the wrong file type.
prepare_case plugin-destination-directory
mkdir -p "$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm"
if run_installer > "$CASE_ROOT/install.out" 2>&1; then
  fail "install accepted a directory as the plugin destination"
fi
grep -qi 'non-file destination' "$CASE_ROOT/install.out" ||
  fail "directory-destination install error was not actionable"
[ -d "$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm" ] ||
  fail "install changed the destination directory"
[ ! -e "$INSTALL_HOME/.config/zellij/plugins/zellaude-hook.sh" ] ||
  fail "directory-destination failure installed hooks"
[ ! -e "$CODEX_INSTALL_HOME" ] || fail "directory-destination failure changed Codex hooks"
[ ! -e "$CACHE_DIR" ] || fail "directory-destination failure changed permissions"
if run_installer --uninstall > "$CASE_ROOT/uninstall.out" 2>&1; then
  fail "uninstall reported success while the plugin destination was a directory"
fi
[ -d "$INSTALL_HOME/.config/zellij/plugins/zellaude.wasm" ] ||
  fail "failed uninstall changed the destination directory"

printf 'top-level install end-to-end tests passed\n'
