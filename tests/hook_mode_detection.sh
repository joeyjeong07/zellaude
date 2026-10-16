#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
CAPTURE_FILE="$TEST_DIR/payload.json"
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$TEST_DIR/bin"
mkdir -p "$TEST_DIR/runtime"
# The hook reads the launch-env allowlist from the settings file under HOME, so
# without this the developer's own zellaude.json decides what these cases see.
mkdir -p "$TEST_DIR/home"
export HOME="$TEST_DIR/home"
# Logs each call and its end; ZELLAUDE_TEST_ZELLIJ_SECONDS slows each pipe,
# ZELLAUDE_TEST_ZELLIJ_HANGS makes it never answer.
TIMELINE="$TEST_DIR/timeline.log"
cat > "$TEST_DIR/bin/zellij" <<'FAKE_ZELLIJ'
#!/usr/bin/env bash
printf '%s' "${!#}" > "$ZELLAUDE_TEST_CAPTURE"
printf 'zellij %s\n' "$*" >> "$ZELLAUDE_TEST_TIMELINE"
seconds=${ZELLAUDE_TEST_ZELLIJ_SECONDS:-0}
[ -z "${ZELLAUDE_TEST_ZELLIJ_HANGS:-}" ] || seconds=30
if [ "$seconds" != 0 ]; then
  trap 'kill "$sleeper"; printf "zellij killed\n" >> "$ZELLAUDE_TEST_TIMELINE"; exit 143' TERM
  command -p sleep "$seconds" &
  sleeper=$!
  wait "$sleeper"
fi
printf 'zellij done\n' >> "$ZELLAUDE_TEST_TIMELINE"
FAKE_ZELLIJ
chmod +x "$TEST_DIR/bin/zellij"

# A fixture tree, or the climb reaches whatever runs the suite (a real claude).
TREE_ROOT="$TEST_DIR/tree"
mkdir -p "$TREE_ROOT"
HOOK_PARENT="$TEST_DIR/hook-parent.sh"
cat > "$HOOK_PARENT" <<'HOOK_PARENT_SCRIPT'
# The hook's parent; its fixture entries go when it does, as in /proc.
published=()
if [ -d "$ZELLAUDE_TREE_PROC_ROOT" ]; then
  node="$ZELLAUDE_TREE_PROC_ROOT/$$"
  mkdir -p "$node"
  printf '%s\n' "$ZELLAUDE_TEST_PARENT_COMM" > "$node/comm"
  printf '%s (%s) S %s 0 0\n' "$$" "$ZELLAUDE_TEST_PARENT_COMM" \
    "${ZELLAUDE_TEST_GRANDPARENT_PID:-1}" > "$node/stat"
  printf '%s\0' "$ZELLAUDE_TEST_PARENT_COMM" "$@" > "$node/cmdline"
  published+=("$node")
fi
if [ -s "${ZELLAUDE_TEST_ENVIRON:-}" ]; then
  mkdir -p "$ZELLAUDE_PROC_ROOT/$$"
  cp "$ZELLAUDE_TEST_ENVIRON" "$ZELLAUDE_PROC_ROOT/$$/environ"
  published+=("$ZELLAUDE_PROC_ROOT/$$")
fi
printf '%s' "$ZELLAUDE_TEST_INPUT" |
  PATH=${ZELLAUDE_TEST_HOOK_PATH:-$PATH} ZELLAUDE_TEST_PARENT_PID=$$ \
    "$BASH" "$ZELLAUDE_TEST_HOOK" ${ZELLAUDE_TEST_HOOK_ARGS:-}
hook_status=$?
printf 'hook returned\n' >> "$ZELLAUDE_TEST_TIMELINE"
rm -rf "${published[@]}"
exit "$hook_status"
HOOK_PARENT_SCRIPT

# hook_process INPUT [NAME=value]... CMD...; parent comm bash, so no agent.
# The reaper defaults to 1 s unless the caller exports another, even empty.
hook_process() {
  local input=$1
  shift
  env -u CLAUDE_EFFORT \
    -u CLAUDE_CODE_EFFORT_LEVEL \
    -u ZELLAUDE_CLAUDE_MODE \
    PATH="$TEST_DIR/bin:$PATH" \
    XDG_RUNTIME_DIR="$TEST_DIR/runtime" \
    XDG_CACHE_HOME="$TEST_DIR/xdg-cache" \
    ZELLAUDE_DELIVERY_COALESCE_SECONDS=0 \
    ZELLAUDE_DELIVERY_REAPER_SECONDS="${ZELLAUDE_DELIVERY_REAPER_SECONDS-1}" \
    ZELLAUDE_TEST_TIMELINE="$TIMELINE" \
    ZELLIJ_SESSION_NAME="test-session" \
    ZELLIJ_PANE_ID="7" \
    ZELLAUDE_TEST_CAPTURE="$CAPTURE_FILE" \
    ZELLAUDE_TREE_PROC_ROOT="$TREE_ROOT" \
    ZELLAUDE_PROC_ROOT="$TEST_DIR/no-environ" \
    ZELLAUDE_TEST_PARENT_COMM=bash \
    ZELLAUDE_TEST_HOOK="$PROJECT_DIR/scripts/zellaude-hook.sh" \
    ZELLAUDE_TEST_INPUT="$input" \
    "$@"
}

# Runs "$@" until every process it started, deliverers included, closes fd 9.
await_descendants() {
  { { "$@" 9>&1 1>&3 3>&-; } | cat; } 3>&1
}

run_hook_process() {
  await_descendants hook_process "$@"
}

run_hook() {
  local client=$1
  local input=$2
  local expected=$3
  local mode=${4:-}
  local expected_marker=${5:-skip}
  local expected_session_id=${6:-skip}
  local expected_subagent=${7:-skip}
  local actual actual_marker actual_session_id actual_subagent

  : > "$CAPTURE_FILE"
  if [ "$client" = "codex" ]; then
    run_hook_process "$input" \
      ZELLAUDE_TEST_HOOK_ARGS="--client codex" \
      bash "$HOOK_PARENT" >/dev/null
  else
    run_hook_process "$input" \
      ZELLAUDE_CLAUDE_MODE="$mode" \
      bash "$HOOK_PARENT" >/dev/null
  fi

  actual=$(jq -c '.rainbow_name' "$CAPTURE_FILE")
  if [ "$actual" != "$expected" ]; then
    printf 'expected %s %s, got %s\n' "$client" "$expected" "$actual" >&2
    printf 'payload: %s\n' "$(cat "$CAPTURE_FILE")" >&2
    exit 1
  fi

  if [ "$expected" = "null" ]; then
    jq -e '.rainbow_mode_ts_ms == null' "$CAPTURE_FILE" >/dev/null
  else
    jq -e '
      (.rainbow_mode_ts_ms | type) == "number"
      and .rainbow_mode_ts_ms == .ts_ms
    ' "$CAPTURE_FILE" >/dev/null
  fi

  if [ "$expected_marker" != "skip" ]; then
    actual_marker=$(jq -c '.rainbow_mode_marker' "$CAPTURE_FILE")
    if [ "$actual_marker" != "$expected_marker" ]; then
      printf 'expected %s marker %s, got %s\n' \
        "$client" "$expected_marker" "$actual_marker" >&2
      exit 1
    fi
  fi

  if [ "$expected_session_id" != "skip" ]; then
    actual_session_id=$(jq -c '.session_id' "$CAPTURE_FILE")
    if [ "$actual_session_id" != "$expected_session_id" ]; then
      printf 'expected %s session id %s, got %s\n' \
        "$client" "$expected_session_id" "$actual_session_id" >&2
      exit 1
    fi
  fi

  if [ "$expected_subagent" != "skip" ]; then
    actual_subagent=$(jq -c '.is_subagent' "$CAPTURE_FILE")
    if [ "$actual_subagent" != "$expected_subagent" ]; then
      printf 'expected %s subagent %s, got %s\n' \
        "$client" "$expected_subagent" "$actual_subagent" >&2
      exit 1
    fi
  fi
}

run_launch_flag() {
  local effort=$1
  local input=$2
  local expected=$3
  local expected_marker=$4
  local actual actual_marker

  : > "$CAPTURE_FILE"
  run_hook_process "$input" \
    ZELLAUDE_TEST_PARENT_COMM=claude \
    bash "$HOOK_PARENT" --effort "$effort" >/dev/null

  actual=$(jq -c '.rainbow_name' "$CAPTURE_FILE")
  actual_marker=$(jq -c '.rainbow_mode_marker' "$CAPTURE_FILE")
  if [ "$actual" != "$expected" ] ||
     [ "$actual_marker" != "$expected_marker" ]; then
    printf 'launch --effort %s expected %s/%s, got %s/%s\n' \
      "$effort" "$expected" "$expected_marker" "$actual" "$actual_marker" >&2
    exit 1
  fi
}

run_inline_settings_flag() {
  local settings=$1
  local expected=$2
  local actual

  : > "$CAPTURE_FILE"
  run_hook_process \
    '{"session_id":"claude-settings","hook_event_name":"SessionStart"}' \
    ZELLAUDE_TEST_PARENT_COMM=claude \
    bash "$HOOK_PARENT" --settings "$settings" >/dev/null

  actual=$(jq -c '.rainbow_name' "$CAPTURE_FILE")
  if [ "$actual" != "$expected" ]; then
    printf 'inline settings %s expected %s, got %s\n' \
      "$settings" "$expected" "$actual" >&2
    exit 1
  fi
}

CODEX_TRANSCRIPT="$TEST_DIR/codex.jsonl"
cat > "$CODEX_TRANSCRIPT" <<'CODEX_JSONL'
{"type":"turn_context","payload":{"turn_id":"turn-ultra","effort":"ultra"}}
{"type":"turn_context","payload":{"turn_id":"turn-high","effort":"high"}}
{"type":"partially-written"
CODEX_JSONL

run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_TRANSCRIPT" \
  '{session_id:"codex-ultra",hook_event_name:"PreToolUse",turn_id:"turn-ultra",transcript_path:$transcript}')" \
  true
STATE_FILE="$TEST_DIR/runtime/zellaude-$(id -u)/test-session.7.json"
jq -e '
  .session_id == "codex-ultra"
  and .rainbow_name == true
' "$STATE_FILE" >/dev/null
INITIAL_MODE_TS=$(jq -r '.rainbow_mode_ts_ms' "$STATE_FILE")

# An unknown event in the same root session must preserve the cached mode.
run_hook codex \
  '{"session_id":"codex-ultra","hook_event_name":"Notification"}' \
  null
jq -e --argjson mode_ts "$INITIAL_MODE_TS" '
  .session_id == "codex-ultra"
  and .rainbow_name == true
  and .rainbow_mode_ts_ms == $mode_ts
  and .ts_ms >= $mode_ts
' "$STATE_FILE" >/dev/null
XDG_RUNTIME_DIR="$TEST_DIR/runtime" \
  "$PROJECT_DIR/scripts/zellaude-hook.sh" --restore test-session |
  jq -e '
    .session_id == "codex-ultra"
    and .pane_id == 7
    and .rainbow_name == true
  ' >/dev/null

# Cache writes can complete out of order when asynchronous hooks overlap. An
# older hook must not replace the newer state merely because its process exits
# last.
CACHE_FUTURE_TS=9999999999999
jq --arg session_id "cache-order" \
  --argjson ts_ms "$CACHE_FUTURE_TS" '
    .session_id = $session_id
    | .hook_event = "PreToolUse"
    | .ts_ms = $ts_ms
    | .rainbow_name = true
    | .rainbow_mode_marker = "newer-mode"
  ' "$STATE_FILE" > "$STATE_FILE.tmp"
mv "$STATE_FILE.tmp" "$STATE_FILE"
run_hook codex \
  '{"session_id":"cache-order","hook_event_name":"PreToolUse","reasoning_effort":"high"}' \
  false
jq -e --argjson ts_ms "$CACHE_FUTURE_TS" '
  .session_id == "cache-order"
  and .ts_ms == $ts_ms
  and .rainbow_name == true
  and .rainbow_mode_marker == "newer-mode"
' "$STATE_FILE" >/dev/null

# A delayed SessionEnd from the previous process in a reused pane must not
# erase the cache owned by its replacement.
rm -f "$STATE_FILE"
run_hook codex \
  '{"session_id":"cache-new-owner","hook_event_name":"PreToolUse","reasoning_effort":"ultra"}' \
  true
run_hook codex \
  '{"session_id":"cache-old-owner","hook_event_name":"SessionEnd"}' \
  null
jq -e '
  .session_id == "cache-new-owner"
  and .rainbow_name == true
' "$STATE_FILE" >/dev/null

# The matching owner's normal end still removes its own cache entry.
run_hook codex \
  '{"session_id":"cache-new-owner","hook_event_name":"SessionEnd"}' \
  null
[ ! -e "$STATE_FILE" ]

run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_TRANSCRIPT" \
  '{session_id:"codex-high",hook_event_name:"PreToolUse",turn_id:"turn-high",transcript_path:$transcript}')" \
  false
run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_TRANSCRIPT" \
  '{session_id:"codex-missing-turn",hook_event_name:"PreToolUse",turn_id:"missing-turn",transcript_path:$transcript}')" \
  null

CODEX_LONG_TRANSCRIPT="$TEST_DIR/codex-long.jsonl"
cat > "$CODEX_LONG_TRANSCRIPT" <<'CODEX_LONG_JSONL'
{"type":"turn_context","payload":{"turn_id":"turn-long-ultra","effort":"ultra"}}
CODEX_LONG_JSONL
{
  printf '{"type":"response_item","payload":"'
  head -c 2200000 /dev/zero | tr '\0' x
  printf '"}\n'
} >> "$CODEX_LONG_TRANSCRIPT"
run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_LONG_TRANSCRIPT" \
  '{session_id:"codex-long",hook_event_name:"PreToolUse",turn_id:"turn-long-ultra",transcript_path:$transcript}')" \
  true
run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_LONG_TRANSCRIPT" \
  '{session_id:"codex-long-resume",hook_event_name:"SessionStart",transcript_path:$transcript}')" \
  true

CODEX_AGENT_TRANSCRIPT="$TEST_DIR/codex-agent.jsonl"
cat > "$CODEX_AGENT_TRANSCRIPT" <<'CODEX_AGENT_JSONL'
{"timestamp":"2026-07-30T00:00:00Z","type":"turn_context","payload":{"turn_id":"child-turn","model":"gpt-test","effort":"ultra"}}
CODEX_AGENT_JSONL
run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_TRANSCRIPT" \
  --arg agent_transcript "$CODEX_AGENT_TRANSCRIPT" \
  '{session_id:"codex-agent",hook_event_name:"SubagentStop",turn_id:"child-turn",transcript_path:$transcript,agent_transcript_path:$agent_transcript}')" \
  null \
  "" \
  skip \
  '""' \
  true

run_hook codex \
  '{"session_id":"codex-unknown","hook_event_name":"SessionStart"}' \
  null
run_hook codex \
  '{"session_id":"codex-child","hook_event_name":"PreToolUse","agent_id":"child-1","reasoning_effort":"high"}' \
  null \
  "" \
  skip \
  '""' \
  true

STATE_BEFORE_CHILD=$(cat "$STATE_FILE")
run_hook codex \
  '{"session_id":"codex-child","hook_event_name":"PostToolUse","agent_id":"child-1","reasoning_effort":"high"}' \
  null \
  "" \
  skip \
  '""' \
  true
[ "$(cat "$STATE_FILE")" = "$STATE_BEFORE_CHILD" ]

CODEX_INTERNAL_CHILD_TRANSCRIPT="$TEST_DIR/codex-internal-child.jsonl"
cat > "$CODEX_INTERNAL_CHILD_TRANSCRIPT" <<'CODEX_INTERNAL_CHILD_JSONL'
{"type":"session_meta","payload":{"source":{"subagent":{"review":{}}}}}
{"type":"turn_context","payload":{"turn_id":"internal-child-turn","effort":"high"}}
CODEX_INTERNAL_CHILD_JSONL
run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_INTERNAL_CHILD_TRANSCRIPT" \
  '{session_id:"codex-internal-child",hook_event_name:"PreToolUse",turn_id:"internal-child-turn",transcript_path:$transcript}')" \
  null \
  "" \
  skip \
  '""' \
  true

CLAUDE_ULTRA_TRANSCRIPT="$TEST_DIR/claude-ultra.jsonl"
cat > "$CLAUDE_ULTRA_TRANSCRIPT" <<'CLAUDE_ULTRA_JSONL'
{"type":"user","uuid":"effort-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"effort-command","message":{"content":"<local-command-stdout>Set effort level to ultracode (this session only): xhigh + dynamic workflow orchestration</local-command-stdout>"}}
CLAUDE_ULTRA_JSONL

run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_ULTRA_TRANSCRIPT" \
  '{session_id:"claude-ultra",hook_event_name:"UserPromptSubmit",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  true

CLAUDE_LONG_TRANSCRIPT="$TEST_DIR/claude-long.jsonl"
cat > "$CLAUDE_LONG_TRANSCRIPT" <<'CLAUDE_LONG_JSONL'
{"type":"user","uuid":"long-effort-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"long-effort-command","message":{"content":"<local-command-stdout>Set effort level to ultracode (this session only)</local-command-stdout>"}}
CLAUDE_LONG_JSONL
{
  printf '{"type":"assistant","message":{"content":"'
  head -c 2200000 /dev/zero | tr '\0' x
  printf '"}}\n'
} >> "$CLAUDE_LONG_TRANSCRIPT"
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_LONG_TRANSCRIPT" \
  '{session_id:"claude-long",hook_event_name:"SessionRestore",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  true

CLAUDE_XHIGH_TRANSCRIPT="$TEST_DIR/claude-xhigh.jsonl"
cat > "$CLAUDE_XHIGH_TRANSCRIPT" <<'CLAUDE_XHIGH_JSONL'
{"type":"user","uuid":"effort-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"effort-command","message":{"content":"<local-command-stdout>Set effort level to xhigh (saved as your default)</local-command-stdout>"}}
CLAUDE_XHIGH_JSONL

run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_XHIGH_TRANSCRIPT" \
  '{session_id:"claude-xhigh",hook_event_name:"UserPromptSubmit",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  false

# Attach recovery treats the target process launch mode as a baseline. A
# successful /effort command at or after the registry start time supersedes it.
CLAUDE_AFTER_XHIGH_TRANSCRIPT="$TEST_DIR/claude-after-xhigh.jsonl"
cat > "$CLAUDE_AFTER_XHIGH_TRANSCRIPT" <<'CLAUDE_AFTER_XHIGH_JSONL'
{"type":"user","uuid":"after-xhigh","timestamp":"2026-07-30T10:00:00.100Z","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"after-xhigh","timestamp":"2026-07-30T10:00:00.123Z","message":{"content":"<local-command-stdout>Set effort level to xhigh (this session only)</local-command-stdout>"}}
CLAUDE_AFTER_XHIGH_JSONL
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_AFTER_XHIGH_TRANSCRIPT" \
  '{session_id:"claude-restore-after-xhigh",hook_event_name:"SessionRestore",transcript_path:$transcript,session_started_at_ms:1785405600000,launch_ultracode:true}')" \
  false \
  "" \
  '"after-xhigh"'

CLAUDE_AT_START_ULTRA_TRANSCRIPT="$TEST_DIR/claude-at-start-ultra.jsonl"
cat > "$CLAUDE_AT_START_ULTRA_TRANSCRIPT" <<'CLAUDE_AT_START_ULTRA_JSONL'
{"type":"user","uuid":"at-start-ultra","timestamp":"2026-07-30T10:00:00.000Z","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"at-start-ultra","timestamp":"2026-07-30T10:00:00.000Z","message":{"content":"<local-command-stdout>Set effort level to ultracode (this session only)</local-command-stdout>"}}
CLAUDE_AT_START_ULTRA_JSONL
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_AT_START_ULTRA_TRANSCRIPT" \
  '{session_id:"claude-restore-at-start",hook_event_name:"SessionRestore",transcript_path:$transcript,session_started_at_ms:1785405600000,launch_ultracode:false}')" \
  true \
  "" \
  '"at-start-ultra"'

# Historical commands in a resumed transcript cannot override an explicit
# choice made by the newer process invocation.
CLAUDE_BEFORE_XHIGH_TRANSCRIPT="$TEST_DIR/claude-before-xhigh.jsonl"
cat > "$CLAUDE_BEFORE_XHIGH_TRANSCRIPT" <<'CLAUDE_BEFORE_XHIGH_JSONL'
{"type":"user","uuid":"before-xhigh","timestamp":"2026-07-30T09:59:59.900Z","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"before-xhigh","timestamp":"2026-07-30T09:59:59.999Z","message":{"content":"<local-command-stdout>Set effort level to xhigh (this session only)</local-command-stdout>"}}
CLAUDE_BEFORE_XHIGH_JSONL
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_BEFORE_XHIGH_TRANSCRIPT" \
  '{session_id:"claude-restore-before-xhigh",hook_event_name:"SessionRestore",transcript_path:$transcript,session_started_at_ms:1785405600000,launch_ultracode:true}')" \
  true \
  "" \
  '"before-xhigh"'

run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_ULTRA_TRANSCRIPT" \
  '{session_id:"claude-restore-undated",hook_event_name:"SessionRestore",transcript_path:$transcript,session_started_at_ms:1785405600000,launch_ultracode:false}')" \
  false \
  "" \
  '"effort-command"'

run_hook claude \
  '{"session_id":"claude-ambiguous","hook_event_name":"PreToolUse","effort":{"level":"xhigh"}}' \
  null
run_hook claude \
  '{"session_id":"claude-high","hook_event_name":"PreToolUse","effort":{"level":"high"}}' \
  false
run_hook claude \
  '{"session_id":"claude-explicit","hook_event_name":"SessionStart","ultracode":true}' \
  true
run_hook claude \
  '{"session_id":"claude-child","hook_event_name":"PreToolUse","agent_id":"child-1","ultracode":false}' \
  null \
  "" \
  skip \
  '""' \
  true
run_hook claude \
  '{"session_id":"claude-sentinel","hook_event_name":"SessionStart"}' \
  true \
  ultracode

CLAUDE_FAILED_TRANSCRIPT="$TEST_DIR/claude-failed.jsonl"
cat > "$CLAUDE_FAILED_TRANSCRIPT" <<'CLAUDE_FAILED_JSONL'
{"type":"user","uuid":"failed-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"failed-command","message":{"content":"<local-command-stdout>Failed to set effort level: ultracode is unavailable</local-command-stdout>"}}
CLAUDE_FAILED_JSONL
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_FAILED_TRANSCRIPT" \
  '{session_id:"claude-failed",hook_event_name:"UserPromptSubmit",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  null

CLAUDE_INCOMPLETE_TRANSCRIPT="$TEST_DIR/claude-incomplete.jsonl"
cat > "$CLAUDE_INCOMPLETE_TRANSCRIPT" <<'CLAUDE_INCOMPLETE_JSONL'
{"type":"user","uuid":"old-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"old-command","message":{"content":"<local-command-stdout>Set effort level to ultracode (this session only)</local-command-stdout>"}}
{"type":"user","uuid":"new-command","message":{"content":"<command-name>/effort</command-name>"}}
CLAUDE_INCOMPLETE_JSONL
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_INCOMPLETE_TRANSCRIPT" \
  '{session_id:"claude-incomplete",hook_event_name:"UserPromptSubmit",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  null

CLAUDE_TOGGLES_TRANSCRIPT="$TEST_DIR/claude-toggles.jsonl"
cat > "$CLAUDE_TOGGLES_TRANSCRIPT" <<'CLAUDE_TOGGLES_JSONL'
{"type":"user","uuid":"ultra-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"ultra-command","message":{"content":"<local-command-stdout>Set effort level to ultracode (this session only)</local-command-stdout>"}}
{"type":"user","uuid":"xhigh-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"xhigh-command","message":{"content":"<local-command-stdout>Set effort level to xhigh (saved as your default)</local-command-stdout>"}}
CLAUDE_TOGGLES_JSONL
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_TOGGLES_TRANSCRIPT" \
  '{session_id:"claude-toggles",hook_event_name:"UserPromptSubmit",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  false \
  "" \
  '"xhigh-command"'

# A fresh launch choice must outrank historical commands in a resumed file.
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_XHIGH_TRANSCRIPT" \
  '{session_id:"claude-resume-ultra",hook_event_name:"SessionStart",transcript_path:$transcript}')" \
  true \
  ultracode \
  '"effort-command"'
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_ULTRA_TRANSCRIPT" \
  '{session_id:"claude-resume-high",hook_event_name:"SessionStart",transcript_path:$transcript,effort:{level:"high"}}')" \
  false
run_launch_flag ultracode "$(jq -nc \
  --arg transcript "$CLAUDE_XHIGH_TRANSCRIPT" \
  '{session_id:"claude-launch-ultra",hook_event_name:"SessionStart",transcript_path:$transcript}')" \
  true \
  '"effort-command"'
run_launch_flag high "$(jq -nc \
  --arg transcript "$CLAUDE_ULTRA_TRANSCRIPT" \
  '{session_id:"claude-launch-high",hook_event_name:"SessionStart",transcript_path:$transcript}')" \
  false \
  '"effort-command"'

# A launch override carries the latest transcript marker as its baseline.
# Replaying that exact marker on a later hook must not replace the cached mode
# or its observation timestamp; Rust applies the same marker-idempotence rule.
LAUNCH_OVERRIDE_MODE_TS=$(jq -r '.rainbow_mode_ts_ms' "$STATE_FILE")
run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_ULTRA_TRANSCRIPT" \
  '{session_id:"claude-launch-high",hook_event_name:"UserPromptSubmit",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  true \
  "" \
  '"effort-command"'
jq -e --argjson mode_ts "$LAUNCH_OVERRIDE_MODE_TS" '
  .session_id == "claude-launch-high"
  and .rainbow_name == false
  and .rainbow_mode_marker == "effort-command"
  and .rainbow_mode_ts_ms == $mode_ts
  and .ts_ms >= $mode_ts
' "$STATE_FILE" >/dev/null

run_inline_settings_flag '{"ultracode":true}' true
run_inline_settings_flag '{"ultracode":false,"other":true}' false

# HOOK_PARENT plays the agent; an empty environ fixture means "not captured".
LAUNCH_ENV_PROC_ROOT="$TEST_DIR/launch-proc"
LAUNCH_ENV_ENVIRON="$TEST_DIR/launch-environ"
LAUNCH_ENV_STDOUT="$TEST_DIR/launch-stdout.json"
mkdir -p "$LAUNCH_ENV_PROC_ROOT"

write_environ() {
  local entry
  : > "$LAUNCH_ENV_ENVIRON"
  for entry in "$@"; do
    printf '%s\0' "$entry" >> "$LAUNCH_ENV_ENVIRON"
  done
}

run_launch_env_hook() {
  local client=$1 input=$2 hook_args=""
  shift 2
  [ "$client" != "codex" ] || hook_args="--client codex"

  : > "$CAPTURE_FILE"
  run_hook_process "$input" \
    ZELLAUDE_TEST_PARENT_COMM="$client" \
    ZELLAUDE_TEST_HOOK_ARGS="$hook_args" \
    ZELLAUDE_TEST_ENVIRON="$LAUNCH_ENV_ENVIRON" \
    ZELLAUDE_PROC_ROOT="$LAUNCH_ENV_PROC_ROOT" \
    "$@" \
    bash "$HOOK_PARENT" > "$LAUNCH_ENV_STDOUT"
}

rm -f "$STATE_FILE"
write_environ \
  'ANTHROPIC_BASE_URL=http://127.0.0.1:9/' \
  'ANTHROPIC_AUTH_TOKEN=local' \
  'ANTHROPIC_API_KEY=not-a-real-key' \
  'ANTHROPIC_SMALL_FAST_MODEL=fast-model' \
  'CLAUDE_CODE_EFFORT_LEVEL=high' \
  'CLAUDE_CODE_MAX_CONTEXT_TOKENS=100000' \
  'ANTHROPIC_BASE_URL_EXTRA=prefix-trap' \
  'CLAUDE_CODE_BRIDGE_SESSION_ID=injected' \
  'PATH=/usr/bin'
run_launch_env_hook claude \
  '{"session_id":"launch-env","hook_event_name":"SessionStart"}'
# Config names verbatim, secrets by tier, and the whole key set: an exact-name
# allowlist cannot leak by construction, so what is worth asserting is that the
# reader captured nothing beyond its list.
jq -e '
  .launch_env.ANTHROPIC_BASE_URL == "http://127.0.0.1:9/"
  and .launch_env.ANTHROPIC_AUTH_TOKEN == "local"
  and .launch_env.ANTHROPIC_API_KEY == "<set>"
  and .launch_env.ANTHROPIC_SMALL_FAST_MODEL == "fast-model"
  and .launch_env.CLAUDE_CODE_EFFORT_LEVEL == "high"
  and .launch_env.CLAUDE_CODE_MAX_CONTEXT_TOKENS == "100000"
  and (.launch_env | keys) == [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS"
  ]
' "$CAPTURE_FILE" >/dev/null
if grep -q 'not-a-real-key' "$CAPTURE_FILE" "$STATE_FILE"; then
  printf 'a secret value reached the payload or the cache\n' >&2
  exit 1
fi

# A launch environ is fixed at exec, so within one session a null is a failed
# read, never a newer answer — it must not erase a capture, on any event.
# Anything non-null still lands, or the guard would be a blanket keep-previous.
write_environ
run_launch_env_hook claude \
  '{"session_id":"launch-env","hook_event_name":"PostToolUse"}' \
  CLAUDE_EFFORT=max
jq -e '.launch_env == null and .current_effort_level == "max"' \
  "$CAPTURE_FILE" >/dev/null
jq -e '
  .launch_env.ANTHROPIC_BASE_URL == "http://127.0.0.1:9/"
  and .current_effort_level == "max"
' "$STATE_FILE" >/dev/null

# The two merge rules coexist rather than one swallowing the other: a
# SessionStart is the authoritative new signal for rainbow_name, and in the same
# event the launch fields still keep what a failed read could not supply.
run_launch_env_hook claude \
  '{"session_id":"launch-env","hook_event_name":"SessionStart"}'
jq -e '
  .rainbow_name == null
  and .launch_env.ANTHROPIC_BASE_URL == "http://127.0.0.1:9/"
  and .current_effort_level == "max"
' "$STATE_FILE" >/dev/null

XDG_RUNTIME_DIR="$TEST_DIR/runtime" \
  "$PROJECT_DIR/scripts/zellaude-hook.sh" --restore test-session |
  jq -e '
    .session_id == "launch-env"
    and (has("launch_env") | not)
    and .current_effort_level == "max"
  ' >/dev/null

# A different session inherits nothing: the previous agent's environ is not this
# agent's, however recently it was cached.
run_launch_env_hook claude \
  '{"session_id":"launch-env-other","hook_event_name":"SessionStart"}'
jq -e '.launch_env == null and .current_effort_level == null' \
  "$STATE_FILE" >/dev/null

# Read but nothing matched is not the same as never read.
write_environ 'PATH=/usr/bin'
run_launch_env_hook claude \
  '{"session_id":"launch-env-empty","hook_event_name":"SessionStart"}'
jq -e '.launch_env == {}' "$CAPTURE_FILE" >/dev/null

# --inspect runs outside the agent's process tree, so the environment reachable
# there is the caller's, not the agent's — null even when a readable environ and
# a resolvable agent_pid would otherwise produce a capture.
write_environ 'ANTHROPIC_BASE_URL=http://127.0.0.1:9/'
run_launch_env_hook claude \
  '{"session_id":"launch-env-inspect","hook_event_name":"PostToolUse"}' \
  ZELLAUDE_TEST_HOOK_ARGS=--inspect
jq -e '.launch_env == null and (.agent_pid | type) == "number"' \
  "$LAUNCH_ENV_STDOUT" >/dev/null

# Every effort source is normalized, so an uppercase CLAUDE_EFFORT reaches the
# payload lowercased and takes the explicit-effort early return instead of
# falling through to the transcript's ultracode state.
write_environ
run_launch_env_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_ULTRA_TRANSCRIPT" \
  '{session_id:"launch-env-effort",hook_event_name:"PreToolUse",transcript_path:$transcript}')" \
  CLAUDE_EFFORT=HIGH
jq -e '.current_effort_level == "high" and .rainbow_name == false' \
  "$CAPTURE_FILE" >/dev/null
run_launch_env_hook claude \
  '{"session_id":"launch-env-effort","hook_event_name":"PostToolUse","effort":{"level":"MAX"}}' \
  CLAUDE_EFFORT=high
jq -e '.current_effort_level == "max"' "$CAPTURE_FILE" >/dev/null

# The asymmetry on a codex pane is deliberate, and the two halves are asserted
# together so it cannot be tidied away: an inherited ANTHROPIC_BASE_URL was
# genuinely in this process's environ at exec, while an inherited CLAUDE_EFFORT
# would assert a Claude session's effort about a codex agent.
write_environ \
  'ANTHROPIC_BASE_URL=http://127.0.0.1:9/' \
  'CODEX_HOME=/tmp/codex-home' \
  'CODEX_API_KEY=not-a-real-key'
run_launch_env_hook codex \
  '{"session_id":"launch-env-codex","hook_event_name":"PreToolUse"}' \
  CLAUDE_EFFORT=high
jq -e '
  .client == "codex"
  and .current_effort_level == null
  and .launch_env.ANTHROPIC_BASE_URL == "http://127.0.0.1:9/"
  and .launch_env.CODEX_HOME == "/tmp/codex-home"
  and .launch_env.CODEX_API_KEY == "<set>"
' "$CAPTURE_FILE" >/dev/null

LAUNCH_ENV_SETTINGS="$HOME/.config/zellij/plugins/zellaude.json"
mkdir -p "$(dirname "$LAUNCH_ENV_SETTINGS")"

write_settings() {
  if [ "$#" -eq 0 ]; then
    rm -f "$LAUNCH_ENV_SETTINGS"
  else
    printf '%s' "$1" > "$LAUNCH_ENV_SETTINGS"
  fi
}

# With no settings file the event still pipes, and the built-ins are the whole
# list — the regression test for a hook that now reads that file on every
# event, or the suite has gone back to reading whoever runs it.
write_settings
write_environ \
  'CODEX_SQLITE_HOME=/tmp/codex-sqlite' \
  'OPENAI_BASE_URL=http://127.0.0.1:9/v1' \
  'OPENAI_BASE_URL_EXTRA=prefix-trap' \
  'MY_BASE_URL=http://127.0.0.1:9/mine'
run_launch_env_hook claude \
  '{"session_id":"launch-env-builtin","hook_event_name":"SessionStart"}'
jq -e '
  .hook_event == "SessionStart"
  and .launch_env.CODEX_SQLITE_HOME == "/tmp/codex-sqlite"
  and .launch_env.OPENAI_BASE_URL == "http://127.0.0.1:9/v1"
  and (.launch_env | keys) == ["CODEX_SQLITE_HOME", "OPENAI_BASE_URL"]
' "$CAPTURE_FILE" >/dev/null

# The complete built-in list, each name under its tier, and nothing beyond it.
# One line per name on each side, so a name added to the hook fails here until
# it is classified, and a re-tiered one fails on its value. The two traps are
# a documented display-label sibling an exact match must not admit, and a
# codex-injected name.
LAUNCH_ENV_BUILTIN_VERBATIM_ENTRIES=(
  'ANTHROPIC_BASE_URL=http://127.0.0.1:9/'
  'CLAUDE_CODE_USE_BEDROCK=1'
  'ANTHROPIC_BEDROCK_BASE_URL=http://127.0.0.1:9/bedrock'
  'ANTHROPIC_BEDROCK_REGION_PREFIX=eu'
  'ANTHROPIC_SMALL_FAST_MODEL_AWS_REGION=eu-west-1'
  'CLAUDE_CODE_SKIP_BEDROCK_AUTH=1'
  'CLAUDE_CODE_USE_MANTLE=1'
  'CLAUDE_CODE_SKIP_MANTLE_AUTH=1'
  'CLAUDE_CODE_USE_VERTEX=1'
  'ANTHROPIC_VERTEX_BASE_URL=http://127.0.0.1:9/vertex'
  'ANTHROPIC_VERTEX_PROJECT_ID=my-project'
  'CLAUDE_CODE_SKIP_VERTEX_AUTH=1'
  'CLAUDE_CODE_USE_FOUNDRY=1'
  'ANTHROPIC_FOUNDRY_BASE_URL=http://127.0.0.1:9/foundry'
  'ANTHROPIC_FOUNDRY_RESOURCE=my-resource'
  'CLAUDE_CODE_SKIP_FOUNDRY_AUTH=1'
  'CLAUDE_CODE_USE_ANTHROPIC_AWS=1'
  'ANTHROPIC_AWS_BASE_URL=http://127.0.0.1:9/aws'
  'ANTHROPIC_AWS_WORKSPACE_ID=wrkspc_test'
  'CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH=1'
  'ANTHROPIC_PROFILE=work'
  'ANTHROPIC_FEDERATION_RULE_ID=fedrule_test'
  'ANTHROPIC_ORGANIZATION_ID=org_test'
  'ANTHROPIC_WORKSPACE_ID=wrkspc_test'
  'CLAUDE_CODE_OAUTH_SCOPES=user:profile user:inference'
  'CLAUDE_CONFIG_DIR=/tmp/claude-home'
  'CLAUDE_CODE_PROJECT_DIR_NAME=my-project'
  'ANTHROPIC_MODEL=main-model'
  'ANTHROPIC_DEFAULT_MODEL=opus'
  'ANTHROPIC_DEFAULT_OPUS_MODEL=opus-model'
  'ANTHROPIC_DEFAULT_SONNET_MODEL=sonnet-model'
  'ANTHROPIC_DEFAULT_HAIKU_MODEL=haiku-model'
  'ANTHROPIC_DEFAULT_FABLE_MODEL=fable-model'
  'ANTHROPIC_SMALL_FAST_MODEL=fast-model'
  'ANTHROPIC_CUSTOM_MODEL_OPTION=custom-model'
  'CLAUDE_CODE_SUBAGENT_MODEL=sonnet'
  'CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1'
  'CLAUDE_CODE_EFFORT_LEVEL=high'
  'CLAUDE_CODE_MAX_CONTEXT_TOKENS=100000'
  'CLAUDE_CODE_MAX_OUTPUT_TOKENS=32000'
  'CLAUDE_CODE_AUTO_COMPACT_WINDOW=80000'
  'CLAUDE_CODE_DISABLE_1M_CONTEXT=1'
  'ZELLAUDE_CLAUDE_MODE=ultracode'
  'CODEX_HOME=/tmp/codex-home'
  'CODEX_SQLITE_HOME=/tmp/codex-sqlite'
  'CODEX_CA_CERTIFICATE=/tmp/ca.pem'
  'CODEX_OSS_BASE_URL=http://127.0.0.1:9/v1'
  'CODEX_OSS_PORT=11434'
  'OPENAI_BASE_URL=http://127.0.0.1:9/v1'
  'OPENAI_ORGANIZATION=org-test'
  'OPENAI_PROJECT=proj_test'
  'OPENAI_FEDERATION_RULE_ID=fedrule_test'
  'OPENAI_IDENTITY_TOKEN_FILE=/tmp/token.jwt'
  'OPENAI_WORKLOAD_IDENTITY_CONTEXT={"run":"test"}'
)
LAUNCH_ENV_BUILTIN_SECRET_NAMES=(
  ANTHROPIC_AUTH_TOKEN
  ANTHROPIC_API_KEY
  CLAUDE_CODE_OAUTH_TOKEN
  CLAUDE_CODE_OAUTH_REFRESH_TOKEN
  ANTHROPIC_FOUNDRY_API_KEY
  ANTHROPIC_FOUNDRY_AUTH_TOKEN
  ANTHROPIC_AWS_API_KEY
  CODEX_API_KEY
  CODEX_ACCESS_TOKEN
  OPENAI_API_KEY
)
write_settings
write_environ \
  "${LAUNCH_ENV_BUILTIN_VERBATIM_ENTRIES[@]}" \
  "${LAUNCH_ENV_BUILTIN_SECRET_NAMES[@]/%/=not-a-real-key}" \
  'ANTHROPIC_DEFAULT_OPUS_MODEL_NAME=prefix-trap' \
  'CODEX_SANDBOX=seatbelt'
run_launch_env_hook claude \
  '{"session_id":"launch-env-all-builtins","hook_event_name":"SessionStart"}'
jq -e \
  --arg verbatim "$(printf '%s\n' "${LAUNCH_ENV_BUILTIN_VERBATIM_ENTRIES[@]}")" \
  --arg secret "$(printf '%s\n' "${LAUNCH_ENV_BUILTIN_SECRET_NAMES[@]}")" '
  def lines: split("\n") | map(select(. != ""));
  (
    ($verbatim | lines | map(capture("^(?<key>[^=]+)=(?<value>.*)$")))
    + ($secret | lines | map({key: ., value: "<set>"}))
    | from_entries
  ) as $expected
  | .launch_env == $expected
' "$CAPTURE_FILE" >/dev/null
if grep -q 'not-a-real-key' "$CAPTURE_FILE" "$STATE_FILE"; then
  printf 'a built-in secret reached the payload or the cache by value\n' >&2
  exit 1
fi

# Each tier extends, and an added secret is still a secret: the tier the user
# picks decides the value policy, exactly as for a predefined name.
write_settings '{"launch_env_names":{"verbatim":["MY_BASE_URL"],"secret":["MY_TOKEN"]}}'
write_environ \
  'MY_BASE_URL=http://127.0.0.1:9/mine' \
  'MY_TOKEN=not-a-real-key'
run_launch_env_hook claude \
  '{"session_id":"launch-env-extend","hook_event_name":"SessionStart"}'
jq -e '
  .launch_env.MY_BASE_URL == "http://127.0.0.1:9/mine"
  and .launch_env.MY_TOKEN == "<set>"
' "$CAPTURE_FILE" >/dev/null
if grep -q 'not-a-real-key' "$CAPTURE_FILE"; then
  printf 'a user-added secret reached the payload by value\n' >&2
  exit 1
fi

# The frozen tiers and the escape set under a hostile config: it lists a
# predefined secret as verbatim, and offers a safe value of its own. Neither
# door opens — the escape set is not read from the file at all, so
# ANTHROPIC_AUTH_TOKEN=hunter2 stays redacted while the built-in escape still
# admits the literal "local".
write_settings '{
  "launch_env_names":{"verbatim":["ANTHROPIC_API_KEY","ANTHROPIC_AUTH_TOKEN"]},
  "launch_env_safe_values":["hunter2"]
}'
write_environ \
  'ANTHROPIC_API_KEY=not-a-real-key' \
  'ANTHROPIC_AUTH_TOKEN=hunter2'
run_launch_env_hook claude \
  '{"session_id":"launch-env-frozen","hook_event_name":"SessionStart"}'
jq -e '
  .launch_env.ANTHROPIC_API_KEY == "<set>"
  and .launch_env.ANTHROPIC_AUTH_TOKEN == "<set>"
' "$CAPTURE_FILE" >/dev/null
write_environ 'ANTHROPIC_AUTH_TOKEN=local'
run_launch_env_hook claude \
  '{"session_id":"launch-env-escape","hook_event_name":"SessionStart"}'
jq -e '.launch_env.ANTHROPIC_AUTH_TOKEN == "local"' "$CAPTURE_FILE" >/dev/null

# Frozen in the other direction too: a predefined verbatim name listed as secret
# stays verbatim. Promotion is as much a re-tier as demotion.
write_settings '{"launch_env_names":{"secret":["ANTHROPIC_BASE_URL"]}}'
write_environ 'ANTHROPIC_BASE_URL=http://127.0.0.1:9/'
run_launch_env_hook claude \
  '{"session_id":"launch-env-promote","hook_event_name":"SessionStart"}'
jq -e '.launch_env.ANTHROPIC_BASE_URL == "http://127.0.0.1:9/"' \
  "$CAPTURE_FILE" >/dev/null

# A name under both tiers is a secret: the ambiguity resolves fail-closed.
write_settings '{"launch_env_names":{"verbatim":["MY_DUAL"],"secret":["MY_DUAL"]}}'
write_environ 'MY_DUAL=not-a-real-key'
run_launch_env_hook claude \
  '{"session_id":"launch-env-dual","hook_event_name":"SessionStart"}'
jq -e '.launch_env.MY_DUAL == "<set>"' "$CAPTURE_FILE" >/dev/null

# An environ entry can begin with '=', so an empty name in the settings file
# would capture it under an empty key. The name never enters the list, which is
# why nothing on the environ side has to defend against it.
write_settings '{"launch_env_names":{"verbatim":["","MY_BASE_URL"]}}'
write_environ '=weird-empty-name' 'MY_BASE_URL=http://mine/'
run_launch_env_hook claude \
  '{"session_id":"launch-env-empty-name","hook_event_name":"SessionStart"}'
jq -e '(.launch_env | keys) == ["MY_BASE_URL"]' "$CAPTURE_FILE" >/dev/null

# A bad file yields the built-ins: never fail-open, never fail-dead. Malformed
# and unreadable are separate paths — one fails to parse, one fails to open.
write_settings '{"launch_env_names":{"verbatim":['
write_environ 'ANTHROPIC_BASE_URL=http://127.0.0.1:9/' 'MY_BASE_URL=ignored'
run_launch_env_hook claude \
  '{"session_id":"launch-env-malformed","hook_event_name":"SessionStart"}'
jq -e '
  (.launch_env | keys) == ["ANTHROPIC_BASE_URL"]
' "$CAPTURE_FILE" >/dev/null

# A bare array is a SUPPORTED settings shape, not a malformed one: it is custom
# states without the `custom_states` wrapper (README) and `parse_config_document`
# matches `Value::Array` ahead of both object arms. Worth its own case despite
# asserting the same outcome as the malformed one, because the failure it guards
# differs in kind: indexing an array with a string errors, so if `added`'s `try`
# is ever dropped the jq exits non-zero, read_launch_env returns 1, and
# launch_env is null on every event for that user — a documented config silently
# losing the feature outright, not merely getting the wrong list. Asserting the
# key set catches exactly that, since `null | keys` fails.
write_settings '[{"id":"claude6","width":3,"height":2,"commands":["claude"]}]'
write_environ 'ANTHROPIC_BASE_URL=http://127.0.0.1:9/' 'MY_BASE_URL=ignored'
run_launch_env_hook claude \
  '{"session_id":"launch-env-array","hook_event_name":"SessionStart"}'
jq -e '
  (.launch_env | keys) == ["ANTHROPIC_BASE_URL"]
' "$CAPTURE_FILE" >/dev/null

# Skipped for root, which can read a 000 file and would fail this for the wrong
# reason.
if [ "$(id -u)" -ne 0 ]; then
  write_settings '{"launch_env_names":{"verbatim":["MY_BASE_URL"]}}'
  chmod 000 "$LAUNCH_ENV_SETTINGS"
  run_launch_env_hook claude \
    '{"session_id":"launch-env-unreadable","hook_event_name":"SessionStart"}'
  chmod 600 "$LAUNCH_ENV_SETTINGS"
  jq -e '
    (.launch_env | keys) == ["ANTHROPIC_BASE_URL"]
  ' "$CAPTURE_FILE" >/dev/null
fi

# Narrowing is the null rule's sibling: the config is a redaction control, so a
# name the user just removed must not survive in the cache. The smaller object
# is non-null, so the merge takes it wholesale — this is the one place a
# shrinking launch_env is correct rather than the bug it resembles.
write_settings '{"launch_env_names":{"verbatim":["MY_BASE_URL"]}}'
write_environ 'ANTHROPIC_BASE_URL=http://127.0.0.1:9/' 'MY_BASE_URL=http://mine/'
run_launch_env_hook claude \
  '{"session_id":"launch-env-narrow","hook_event_name":"SessionStart"}'
jq -e '.launch_env.MY_BASE_URL == "http://mine/"' "$STATE_FILE" >/dev/null
write_settings
run_launch_env_hook claude \
  '{"session_id":"launch-env-narrow","hook_event_name":"PostToolUse"}'
jq -e '
  (.launch_env | has("MY_BASE_URL") | not)
  and .launch_env.ANTHROPIC_BASE_URL == "http://127.0.0.1:9/"
' "$STATE_FILE" >/dev/null

# A redacted subset of a real claude 2.1.252 PreToolUse payload: fields removed,
# none altered. It is the evidence that the vendor populates `.effort`, so
# current_effort_level has a source on the events that carry one. The harness
# unsets CLAUDE_EFFORT, which is what makes "high" attributable to the payload
# rather than to the environment.
VENDOR_PRETOOLUSE="$TEST_DIR/vendor-pretooluse.json"
cat > "$VENDOR_PRETOOLUSE" <<'VENDOR_PRETOOLUSE_JSON'
{
  "effort": {
    "level": "high"
  },
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash"
}
VENDOR_PRETOOLUSE_JSON
run_hook claude "$(cat "$VENDOR_PRETOOLUSE")" false
jq -e '.current_effort_level == "high"' "$CAPTURE_FILE" >/dev/null

# The hook's PATH holds only shims: a name not in EXEC_SHIM_NAMES logs MISSING.
EXEC_SHIM_BIN="$TEST_DIR/exec-shim-bin"
EXEC_LOG="$TEST_DIR/exec.log"
EXEC_SHIM_NAMES="awk cat chmod date grep head hostname id jq mkdir mktemp mv od
  ps rm rmdir sha256sum sleep stat tail tr uname zellij"
mkdir -p "$EXEC_SHIM_BIN"
cat > "$TEST_DIR/exec-shim" <<'EXEC_SHIM'
#!/bin/sh
printf '%s\n' "${0##*/}" >> "$ZELLAUDE_TEST_EXEC_LOG"
PATH=$ZELLAUDE_TEST_REAL_PATH
export PATH
exec "${0##*/}" "$@"
EXEC_SHIM
chmod +x "$TEST_DIR/exec-shim"
for name in $EXEC_SHIM_NAMES; do
  if PATH="$TEST_DIR/bin:$PATH" command -v "$name" >/dev/null; then
    ln -s "$TEST_DIR/exec-shim" "$EXEC_SHIM_BIN/$name"
  fi
done
cat > "$TEST_DIR/exec-bash-env" <<'EXEC_BASH_ENV'
command_not_found_handle() {
  printf 'MISSING %s\n' "$1" >> "$ZELLAUDE_TEST_EXEC_LOG"
  return 127
}
EXEC_BASH_ENV

# Prints "name=count" per exec'd command; sleep is left out, its count races.
exec_profile() {
  local input=$1
  shift
  : > "$EXEC_LOG"
  run_hook_process "$input" \
    ZELLAUDE_TEST_HOOK_PATH="$EXEC_SHIM_BIN" \
    ZELLAUDE_TEST_REAL_PATH="$TEST_DIR/bin:$PATH" \
    ZELLAUDE_TEST_EXEC_LOG="$EXEC_LOG" \
    BASH_ENV="$TEST_DIR/exec-bash-env" \
    "$@" \
    bash "$HOOK_PARENT" >/dev/null
  if grep '^MISSING ' "$EXEC_LOG" >&2; then
    exit 1
  fi
  { grep -vx sleep "$EXEC_LOG" || true; } |
    sort | uniq -c | awk '{ printf "%s%s=%s", sep, $2, $1; sep = " " }'
}

exec_count() {
  grep -cx "$1" "$EXEC_LOG" || true
}

expect_equal() {
  local label=$1 expected=$2 actual=$3
  if [ "$actual" != "$expected" ]; then
    printf '%s\n  expected: %s\n  actual:   %s\n' \
      "$label" "$expected" "$actual" >&2
    exit 1
  fi
}

# Process facts: fixture agents 4242 and 4343 serve as HOOK_PARENT's parent.
plant_process() {
  local pid=$1 comm=$2
  mkdir -p "$TREE_ROOT/$pid"
  printf '%s\n' "$comm" > "$TREE_ROOT/$pid/comm"
  printf '%s (%s) S 1 0 0\n' "$pid" "$comm" > "$TREE_ROOT/$pid/stat"
}
plant_process 4242 claude
plant_process 4343 claude

TREE_TOOL_EVENT='{"session_id":"tree-root","hook_event_name":"PreToolUse"}'
TREE_SESSION_START='{"session_id":"tree-root","hook_event_name":"SessionStart"}'

# $1 the input, $2 the grandparent; the rest are run_hook_process overrides.
run_tree_hook() {
  local input=$1 grandparent=$2
  shift 2
  : > "$CAPTURE_FILE"
  run_hook_process "$input" \
    ZELLAUDE_TEST_GRANDPARENT_PID="$grandparent" \
    "$@" \
    bash "$HOOK_PARENT" >/dev/null
}

expect_agent_pid() {
  expect_equal "$1: agent_pid" "$2" "$(jq -r '.agent_pid' "$CAPTURE_FILE")"
}

rm -f "$STATE_FILE"
# The stat parse counts fields from the last ')', since comm may hold both.
run_tree_hook "$TREE_SESSION_START" 4242 ZELLAUDE_TEST_PARENT_COMM='sh (x) y'
expect_agent_pid "climb through a parent named with parens" 4242

# The memo outranks a moved tree, except on SessionStart: a resume reuses ids.
run_tree_hook "$TREE_TOOL_EVENT" 4343
expect_agent_pid "memo hit" 4242
run_tree_hook "$TREE_SESSION_START" 4343
expect_agent_pid "SessionStart climbs" 4343

# A recycled pid fails the comm check, and the climb answers instead.
plant_process 4343 vim
run_tree_hook "$TREE_TOOL_EVENT" 4242
expect_agent_pid "recycled memo pid" 4242
plant_process 4343 claude

# Another host's record, another session's and a subagent event all climb.
OTHER_HOST_BIN="$TEST_DIR/other-host-bin"
mkdir -p "$OTHER_HOST_BIN"
cat > "$OTHER_HOST_BIN/hostname" <<'FAKE_HOSTNAME'
#!/usr/bin/env bash
printf 'other-host.invalid\n'
FAKE_HOSTNAME
chmod +x "$OTHER_HOST_BIN/hostname"
run_tree_hook "$TREE_TOOL_EVENT" 4343 PATH="$OTHER_HOST_BIN:$TEST_DIR/bin:$PATH"
expect_agent_pid "other host" 4343
run_tree_hook "$TREE_TOOL_EVENT" 4242
expect_agent_pid "record from another host" 4242
run_tree_hook '{"session_id":"tree-other","hook_event_name":"PreToolUse"}' 4343
expect_agent_pid "other session" 4343
run_tree_hook "$TREE_SESSION_START" 4242
run_tree_hook \
  '{"session_id":"tree-root","hook_event_name":"PreToolUse","agent_id":"child"}' \
  4343
expect_agent_pid "subagent event" 4343

# --inspect, outside the agent's tree, neither reads nor writes the record.
cp "$STATE_FILE" "$TEST_DIR/record.before"
run_hook_process '{"session_id":"tree-root","hook_event_name":"SessionRestore"}' \
  ZELLAUDE_TEST_GRANDPARENT_PID=4343 \
  ZELLAUDE_TEST_HOOK_ARGS=--inspect \
  bash "$HOOK_PARENT" > "$TEST_DIR/inspect.json"
jq -e '.agent_pid == 4343' "$TEST_DIR/inspect.json" >/dev/null
cmp -s "$STATE_FILE" "$TEST_DIR/record.before"

# Requested-mode detection reads argv from the same tree.
plant_process 4444 claude
printf 'claude\0--effort\0ultracode\0' > "$TREE_ROOT/4444/cmdline"
run_tree_hook '{"session_id":"tree-mode","hook_event_name":"SessionStart"}' 4444
jq -e '.rainbow_name == true and .agent_pid == 4444' "$CAPTURE_FILE" >/dev/null

# The Linux climb is builtins only: a memo hit and a climb cost the same execs.
run_tree_hook "$TREE_SESSION_START" 4242
LINUX_TOOL_EVENT_EXECS=$(exec_profile "$TREE_TOOL_EVENT" \
  ZELLAUDE_TEST_GRANDPARENT_PID=4343)
expect_agent_pid "memo hit, counted" 4242
# A coalesced event: its deliverer claims the stream marker (one mv), pipes,
# and releases the session pipe lock (one rm).
expect_equal "Linux tool event execs" \
  "date=1 hostname=1 jq=3 mkdir=2 mktemp=1 mv=2 rm=1 rmdir=2 stat=2 tr=1 zellij=1" \
  "$LINUX_TOOL_EVENT_EXECS"
expect_equal "Linux climbing tool event execs" "$LINUX_TOOL_EVENT_EXECS" \
  "$(exec_profile '{"session_id":"tree-climb","hook_event_name":"PreToolUse"}' \
    ZELLAUDE_TEST_GRANDPARENT_PID=4343)"
# An urgent event skips the window but otherwise costs the same.
LINUX_URGENT_EVENT_EXECS=$(exec_profile "$TREE_SESSION_START" \
  ZELLAUDE_TEST_GRANDPARENT_PID=4242)
expect_equal "Linux SessionStart execs" \
  "date=1 hostname=1 jq=3 mkdir=2 mktemp=1 mv=2 rm=1 rmdir=2 stat=2 tr=1 zellij=1" \
  "$LINUX_URGENT_EVENT_EXECS"
# A subagent writes no state record, but its stream marker is in the cache.
expect_equal "Linux subagent tool event execs" \
  "hostname=1 jq=2 mv=1 rm=1 stat=2 tr=1 zellij=1" \
  "$(exec_profile \
    '{"session_id":"tree-root","hook_event_name":"PreToolUse","agent_id":"child"}' \
    ZELLAUDE_TEST_GRANDPARENT_PID=4343)"
# The common Linux path: the agent is found and its environ is readable.
write_environ 'ANTHROPIC_BASE_URL=http://127.0.0.1:9/'
expect_equal "Linux tool event with launch env execs" \
  "date=1 hostname=1 jq=4 mkdir=2 mktemp=1 mv=2 rm=1 rmdir=2 stat=2 tr=1 zellij=1" \
  "$(exec_profile '{"session_id":"tree-launch-env","hook_event_name":"PreToolUse"}' \
    ZELLAUDE_TEST_PARENT_COMM=claude \
    ZELLAUDE_TEST_ENVIRON="$LAUNCH_ENV_ENVIRON" \
    ZELLAUDE_PROC_ROOT="$LAUNCH_ENV_PROC_ROOT")"
jq -e '.launch_env.ANTHROPIC_BASE_URL == "http://127.0.0.1:9/"' \
  "$CAPTURE_FILE" >/dev/null
# The plugin orders racing events by ts_ms, so it stays in milliseconds.
jq -e '(.ts_ms - now * 1000 | fabs) < 60000' "$CAPTURE_FILE" >/dev/null

# Only turn boundaries and attach recovery scan; mid-turn reads .effort alone.
claude_ultra_event() {
  jq -nc --arg event "$1" --arg session_id "$2" \
    --arg transcript "$CLAUDE_ULTRA_TRANSCRIPT" '{
      session_id: $session_id,
      hook_event_name: $event,
      transcript_path: $transcript,
      effort: {level: "xhigh"}
    }'
}
for event in PreToolUse PostToolUse PostToolUseFailure Notification Stop; do
  exec_profile "$(claude_ultra_event "$event" scan-rule)" >/dev/null
  expect_equal "$event: rainbow_name" null "$(jq -c '.rainbow_name' "$CAPTURE_FILE")"
  expect_equal "$event: tail, grep" "0 0" "$(exec_count tail) $(exec_count grep)"
done
for event in SessionStart UserPromptSubmit SessionRestore; do
  exec_profile "$(claude_ultra_event "$event" scan-rule)" >/dev/null
  expect_equal "$event: rainbow_name" true "$(jq -c '.rainbow_name' "$CAPTURE_FILE")"
  expect_equal "$event: tail" 2 "$(exec_count tail)"
done

# Rainbow off skips detection and sends null, never false: the cache keeps true.
rm -f "$STATE_FILE"
run_hook claude "$(claude_ultra_event UserPromptSubmit rainbow-off)" true
write_settings '{"rainbow":false}'
expect_equal "rainbow off execs" "$LINUX_URGENT_EVENT_EXECS" \
  "$(exec_profile "$(claude_ultra_event UserPromptSubmit rainbow-off)" \
    ZELLAUDE_TEST_GRANDPARENT_PID=4242)"
jq -e '.rainbow_name == null and .rainbow_mode_ts_ms == null' \
  "$CAPTURE_FILE" >/dev/null
jq -e '.session_id == "rainbow-off" and .rainbow_name == true' \
  "$STATE_FILE" >/dev/null
run_hook codex \
  '{"session_id":"rainbow-off-codex","hook_event_name":"PreToolUse","reasoning_effort":"ultra"}' \
  null
# Only a literal false turns it off: any other value or a bad file leaves it on.
for settings in '{"rainbow":true}' '{"rainbow":"no"}' '{"rainbow":' \
  '[{"id":"claude6","width":3,"height":2,"commands":["claude"]}]'; do
  write_settings "$settings"
  run_hook claude "$(claude_ultra_event UserPromptSubmit rainbow-on)" true
done
# Past one argument's 128 KiB the settings still apply: jq reads them by path.
write_settings "$(printf '{"rainbow":false,%s,"padding":"%140000s"}' \
  '"launch_env_names":{"verbatim":["MY_BASE_URL"]}' '')"
run_hook claude "$(claude_ultra_event UserPromptSubmit big-settings)" null
write_environ 'MY_BASE_URL=http://127.0.0.1:9/mine'
run_launch_env_hook claude \
  '{"session_id":"big-settings","hook_event_name":"PreToolUse"}'
jq -e '.launch_env.MY_BASE_URL == "http://127.0.0.1:9/mine"' \
  "$CAPTURE_FILE" >/dev/null
write_settings

# The cache write: one stat per directory, and chmod only when not already 700.
CACHE_DIR="$TEST_DIR/runtime/zellaude-$(id -u)"
is_private_dir() {
  [ "$(ls -ld "$1" | cut -c1-10)" = "drwx------" ]
}
chmod 750 "$CACHE_DIR"
exec_profile "$TREE_TOOL_EVENT" >/dev/null
expect_equal "loose cache dir: chmod" 1 "$(exec_count chmod)"
is_private_dir "$CACHE_DIR"
exec_profile "$TREE_TOOL_EVENT" >/dev/null
expect_equal "private cache dir: chmod" 0 "$(exec_count chmod)"

# A group-writable runtime dir falls back to ~/.cache, at one stat per level.
HOME_STATE_FILE="$HOME/.cache/zellaude/runtime/test-session.7.json"
chmod 770 "$TEST_DIR/runtime"
run_tree_hook "$TREE_TOOL_EVENT" 1
chmod 700 "$TEST_DIR/runtime"
jq -e '.session_id == "tree-root"' "$HOME_STATE_FILE" >/dev/null
is_private_dir "$HOME/.cache/zellaude/runtime"
expect_equal "home cache execs" \
  "date=1 hostname=1 jq=3 mkdir=2 mktemp=1 mv=2 rm=1 rmdir=2 stat=3 tr=1 zellij=1" \
  "$(exec_profile "$TREE_TOOL_EVENT" XDG_RUNTIME_DIR=)"

# A record jq cannot read, empty or malformed, is replaced, not kept forever.
for record in '' '{"session_id":'; do
  printf '%s' "$record" > "$STATE_FILE"
  run_tree_hook "$TREE_TOOL_EVENT" 4242
  jq -e '.session_id == "tree-root" and .hook_event == "PreToolUse"' \
    "$STATE_FILE" >/dev/null
done

# A 700 cache dir owned by someone else is refused; the event still pipes.
FOREIGN_STAT_BIN="$TEST_DIR/foreign-stat-bin"
mkdir -p "$FOREIGN_STAT_BIN"
cat > "$FOREIGN_STAT_BIN/stat" <<'FAKE_STAT'
#!/usr/bin/env bash
if [ "${!#}" = "$ZELLAUDE_TEST_FOREIGN_DIR" ]; then
  printf '0 700\n'
else
  command -p stat "$@"
fi
FAKE_STAT
chmod +x "$FOREIGN_STAT_BIN/stat"
rm -f "$STATE_FILE"
: > "$CAPTURE_FILE"
run_tree_hook "$TREE_TOOL_EVENT" 4242 \
  PATH="$FOREIGN_STAT_BIN:$TEST_DIR/bin:$PATH" \
  ZELLAUDE_TEST_FOREIGN_DIR="$CACHE_DIR"
[ ! -e "$STATE_FILE" ]
jq -e '.session_id == "tree-root"' "$CAPTURE_FILE" >/dev/null

# Delivery. The fake sleep logs to the timeline, "slept" only when it ran out.
DELIVERY_BIN="$TEST_DIR/delivery-bin"
mkdir -p "$DELIVERY_BIN"
cat > "$DELIVERY_BIN/sleep" <<'FAKE_SLEEP'
#!/usr/bin/env bash
printf 'sleep %s\n' "$*" >> "$ZELLAUDE_TEST_TIMELINE"
trap 'kill "$sleeper"; exit 143' TERM
command -p sleep "$@" &
sleeper=$!
wait "$sleeper"
printf 'slept %s\n' "$*" >> "$ZELLAUDE_TEST_TIMELINE"
FAKE_SLEEP
chmod +x "$DELIVERY_BIN/sleep"
# Distinct from the reaper's 1 second, so the timeline tells them apart.
DELIVERY_WINDOW=1.5

# One event per input, sharing one window; wrap in await_descendants to wait.
# "@<pane> <input>" runs in that pane; "pause:<seconds>" waits between events.
run_delivery_burst() {
  local input pane
  for input in "$@"; do
    pane=7
    case "$input" in
      pause:*) sleep "${input#pause:}"; continue ;;
      @*) pane=${input%% *}; pane=${pane#@}; input=${input#* } ;;
    esac
    hook_process "$input" \
      PATH="$DELIVERY_BIN:$TEST_DIR/bin:$PATH" \
      ZELLIJ_PANE_ID="$pane" \
      ZELLAUDE_DELIVERY_COALESCE_SECONDS="$DELIVERY_WINDOW" \
      bash "$HOOK_PARENT" >/dev/null
  done
}

# Each piped payload as event[:tool][@subagent], in pipe order.
piped_events() {
  sed -n 's/^zellij action pipe --name zellaude -- //p' "$TIMELINE" |
    jq -r '.hook_event
      + (if .tool_name then ":" + .tool_name else "" end)
      + (if .is_subagent then "@subagent" else "" end)'
}

delivered() {
  piped_events | sort | tr '\n' ' ' | sed 's/ $//'
}

piped_in_order() {
  piped_events | tr '\n' ' ' | sed 's/ $//'
}

delivery_event() {
  jq -nc --arg event "$1" --arg tool "${2:-}" --arg agent_id "${3:-}" '
    {session_id: "delivery", hook_event_name: $event}
    + (if $tool == "" then {} else {tool_name: $tool} end)
    + (if $agent_id == "" then {} else {agent_id: $agent_id} end)'
}

expect_delivered() {
  local label=$1
  shift
  : > "$TIMELINE"
  await_descendants run_delivery_burst "$@"
  expect_equal "$label: delivered" "$EXPECTED_DELIVERED" "$(delivered)"
}

EXPECTED_DELIVERED="PreToolUse:last"
expect_delivered "superseded events never pipe" \
  "$(delivery_event PreToolUse first)" \
  "$(delivery_event PostToolUse first)" \
  "$(delivery_event PreToolUse last)"

# An urgent event pipes before the pending window closes, and supersedes it.
EXPECTED_DELIVERED="Stop"
expect_delivered "urgent supersedes" \
  "$(delivery_event PreToolUse first)" "$(delivery_event Stop)"
expect_equal "urgent pipes at once" "pipe window" \
  "$(awk '/^zellij action / { print "pipe" } $0 == "slept 1.5" { print "window" }' \
    "$TIMELINE" | tr '\n' ' ' | sed 's/ $//')"

EXPECTED_DELIVERED="PreToolUse:first"
expect_delivered "Notification never displaces a pending event" \
  "$(delivery_event PreToolUse first)" "$(delivery_event Notification)"
EXPECTED_DELIVERED="Notification"
expect_delivered "Notification alone" "$(delivery_event Notification)"
EXPECTED_DELIVERED="PreToolUse:last"
expect_delivered "Notification yields to a later pending event" \
  "$(delivery_event Notification)" "$(delivery_event PreToolUse last)"

# A Notification fired inside an urgent event's claim never supersedes it.
MID_CLAIM_BIN="$TEST_DIR/mid-claim-bin"
mkdir -p "$MID_CLAIM_BIN"
cat > "$MID_CLAIM_BIN/mv" <<'MID_CLAIM_MV'
#!/usr/bin/env bash
command -p mv "$@" || exit
# Fires once: the Notification's own hook runs disarmed.
if [ -n "${ZELLAUDE_TEST_MID_CLAIM_INPUT:-}" ] && [[ ${!#} == *.claimed ]]; then
  ZELLAUDE_TEST_INPUT=$ZELLAUDE_TEST_MID_CLAIM_INPUT ZELLAUDE_TEST_MID_CLAIM_INPUT= \
    bash "$ZELLAUDE_TEST_HOOK_PARENT" >/dev/null
fi
exit 0
MID_CLAIM_MV
chmod +x "$MID_CLAIM_BIN/mv"
EXPECTED_DELIVERED="Notification Stop"
ZELLAUDE_TEST_MID_CLAIM_INPUT=$(delivery_event Notification) \
  ZELLAUDE_TEST_HOOK_PARENT=$HOOK_PARENT PATH="$MID_CLAIM_BIN:$PATH" \
  expect_delivered "Notification mid-claim" "$(delivery_event Stop)"
expect_equal "Notification mid-claim: order" "Stop Notification" \
  "$(piped_in_order)"

# Root and subagent streams never displace each other, urgent events included.
EXPECTED_DELIVERED="PreToolUse:root PreToolUse:sub@subagent"
expect_delivered "streams are separate" \
  "$(delivery_event PreToolUse root)" \
  "$(delivery_event PreToolUse sub child-1)"
EXPECTED_DELIVERED="PreToolUse:sub@subagent Stop"
expect_delivered "root urgent leaves the subagent stream" \
  "$(delivery_event PreToolUse sub child-1)" "$(delivery_event Stop)"

# Presence: no client gets urgent events only; missing or odd metadata delivers.
case "$OSTYPE" in
  darwin*) ZELLIJ_CACHE="$HOME/Library/Caches/org.Zellij-Contributors.Zellij" ;;
  *) ZELLIJ_CACHE="$TEST_DIR/xdg-cache/zellij" ;;
esac
SESSION_INFO_DIR="$ZELLIJ_CACHE/contract_version_1/session_info/test-session"
mkdir -p "$SESSION_INFO_DIR"
printf 'name "test-session"\nconnected_clients 0\nweb_clients_allowed false\n' \
  > "$SESSION_INFO_DIR/session-metadata.kdl"
EXPECTED_DELIVERED=""
expect_delivered "no client: coalesced skipped" "$(delivery_event PreToolUse)"
EXPECTED_DELIVERED="Stop"
expect_delivered "no client: urgent delivered" "$(delivery_event Stop)"
printf 'name "test-session"\nconnected_clients 1\n' \
  > "$SESSION_INFO_DIR/session-metadata.kdl"
EXPECTED_DELIVERED="PreToolUse"
expect_delivered "a client attached" "$(delivery_event PreToolUse)"
printf 'connected_clients zero\n' > "$SESSION_INFO_DIR/session-metadata.kdl"
expect_delivered "unparseable metadata fails open" "$(delivery_event PreToolUse)"
rm -rf "$ZELLIJ_CACHE"

# The reaper dies with an answered client; a hung one is killed at the 1 s
# test bound.
EXPECTED_DELIVERED="Stop"
expect_delivered "reaper, answered" "$(delivery_event Stop)"
if grep -qx 'slept 1' "$TIMELINE"; then
  printf 'the reaper outlived an answered pipe\n' >&2
  exit 1
fi
ZELLAUDE_TEST_ZELLIJ_HANGS=1 expect_delivered "reaper, never answered" \
  "$(delivery_event Stop)"
expect_equal "hung client" "hook returned|slept 1|zellij killed" \
  "$(grep -x -e 'hook returned' -e 'slept 1' -e 'zellij killed' \
    -e 'zellij done' "$TIMELINE" | tr '\n' '|' | sed 's/|$//')"

# One pipe in flight per session. The session lock and urgent markers sit
# beside the stream markers.
SESSION_PREFIX="$CACHE_DIR/test-session.$(hostname)"
PIPE_LOCK="$SESSION_PREFIX.pipe-lock"

# The pipes of a burst, as s(tart) and d(one) in timeline order.
pipe_spans() {
  awk '/^zellij action / { print "s" } $0 == "zellij done" { print "d" }' \
    "$TIMELINE" | tr '\n' ' ' | sed 's/ $//'
}

# Coalesced events of separate streams have no urgent marker to order them, so
# only the lock keeps their pipes apart.
EXPECTED_DELIVERED="PreToolUse:p7 PreToolUse:p8 PreToolUse:p9 Stop:p10"
DELIVERY_WINDOW=0.05 ZELLAUDE_TEST_ZELLIJ_SECONDS=0.3 expect_delivered \
  "one pipe at a time" \
  "@7 $(delivery_event PreToolUse p7)" "@8 $(delivery_event PreToolUse p8)" \
  "@9 $(delivery_event PreToolUse p9)" "@10 $(delivery_event Stop p10)"
expect_equal "one pipe at a time: spans" "s d s d s d s d" "$(pipe_spans)"
if [ -e "$PIPE_LOCK" ] || compgen -G "$SESSION_PREFIX.urgent.*" >/dev/null; then
  printf 'a finished burst left its lock or urgent markers\n' >&2
  exit 1
fi

# Events that pile up behind a running pipe collapse to each stream's latest.
EXPECTED_DELIVERED="PreToolUse:a3 PreToolUse:s2@subagent Stop:p7"
# Tests with a slow holder raise the reaper, so its pipe outlasts the burst.
DELIVERY_WINDOW=0.05 ZELLAUDE_DELIVERY_REAPER_SECONDS=5 \
  ZELLAUDE_TEST_ZELLIJ_SECONDS=4 expect_delivered \
  "piled-up events collapse" \
  "@7 $(delivery_event Stop p7)" \
  "@8 $(delivery_event PreToolUse a1)" \
  "@8 $(delivery_event PreToolUse s1 child-1)" pause:0.3 \
  "@8 $(delivery_event PreToolUse a2)" pause:0.3 \
  "@8 $(delivery_event PreToolUse a3)" \
  "@8 $(delivery_event PreToolUse s2 child-1)"

# An urgent event takes the lock ahead of a coalesced waiter.
EXPECTED_DELIVERED="PreToolUse:c Stop:p7 UserPromptSubmit:p9"
DELIVERY_WINDOW=0.05 ZELLAUDE_DELIVERY_REAPER_SECONDS=5 \
  ZELLAUDE_TEST_ZELLIJ_SECONDS=3 expect_delivered "urgent before coalesced" \
  "@7 $(delivery_event Stop p7)" "@8 $(delivery_event PreToolUse c)" pause:0.3 \
  "@9 $(delivery_event UserPromptSubmit p9)"
expect_equal "urgent before coalesced: order" \
  "Stop:p7 UserPromptSubmit:p9 PreToolUse:c" "$(piped_in_order)"

# Priority, against a live urgent marker no deliverer will clear: a coalesced
# event yields until it gives up, and an urgent one waits behind an older one.
PHANTOM_MS=$(($(date +%s) * 1000))
: > "$SESSION_PREFIX.urgent.$PHANTOM_MS.99999"
EXPECTED_DELIVERED=""
DELIVERY_WINDOW=0.05 expect_delivered "coalesced yields to urgent" \
  "$(delivery_event PreToolUse)"
expect_equal "coalesced poll" "sleep 0.25" \
  "$(grep -x -e 'sleep 0.1' -e 'sleep 0.25' "$TIMELINE" | sort -u)"
EXPECTED_DELIVERED="Stop"
expect_delivered "urgent waits behind an older urgent" "$(delivery_event Stop)"
if ! grep -qx 'slept 0.1' "$TIMELINE"; then
  printf 'an urgent event piped ahead of an older one\n' >&2
  exit 1
fi
expect_equal "urgent poll" "sleep 0.1" \
  "$(grep -x -e 'sleep 0.1' -e 'sleep 0.25' "$TIMELINE" | sort -u)"
rm -f "$SESSION_PREFIX.urgent.$PHANTOM_MS.99999"

# A newer event of its stream supersedes a waiting urgent event; SessionEnd is
# never superseded, and urgent events go oldest first.
EXPECTED_DELIVERED="PreToolUse:newer Stop:p7"
DELIVERY_WINDOW=0.05 ZELLAUDE_DELIVERY_REAPER_SECONDS=5 \
  ZELLAUDE_TEST_ZELLIJ_SECONDS=3 expect_delivered "waiting urgent superseded" \
  "@7 $(delivery_event Stop p7)" "@8 $(delivery_event Stop p8)" \
  "@8 $(delivery_event PreToolUse newer)"
EXPECTED_DELIVERED="SessionEnd SessionStart Stop:p7"
ZELLAUDE_DELIVERY_REAPER_SECONDS=5 ZELLAUDE_TEST_ZELLIJ_SECONDS=3 \
  expect_delivered "SessionEnd always pipes" \
  "@7 $(delivery_event Stop p7)" "@8 $(delivery_event SessionEnd)" \
  "@8 $(delivery_event SessionStart)"
expect_equal "SessionEnd always pipes: order" "Stop:p7 SessionEnd SessionStart" \
  "$(piped_in_order)"

# A lock older than the reaper plus 2 s is broken by its age, even one
# unreadable since its writer died mid-write; a coalesced event then still
# pipes rather than give up.
printf '%s deadbeef\n' "$(($(date +%s) - 100))" > "$PIPE_LOCK"
EXPECTED_DELIVERED="PreToolUse"
DELIVERY_WINDOW=0.05 expect_delivered "stale lock broken" \
  "$(delivery_event PreToolUse)"
: > "$PIPE_LOCK"
ZELLAUDE_DELIVERY_REAPER_SECONDS=2 DELIVERY_WINDOW=0.05 \
  expect_delivered "unreadable lock broken" "$(delivery_event PreToolUse)"

# The unreadable-lock clock covers one unbroken run of unreadable reads: a
# readable or absent lock in between restarts it, so a fresh lock caught
# mid-write is never aged by an earlier run.
STALE_LOCK_FN=$(sed -n '/^break_stale_pipe_lock() {/,/^}/p' \
  "$PROJECT_DIR/scripts/zellaude-hook.sh")
for interruption in readable absent; do
  bash -c "$STALE_LOCK_FN"'
    PIPE_LOCK=$1 DELIVERY_REAPER_SECONDS=1 TS_MS=$(($(date +%s) * 1000))
    UNREADABLE_LOCK_SINCE=$((SECONDS - 100))
    if [ "$2" = readable ]; then
      printf "%s fresh\n" "$((TS_MS / 1000))" > "$PIPE_LOCK"
    else
      rm -f "$PIPE_LOCK"
    fi
    break_stale_pipe_lock
    : > "$PIPE_LOCK"
    break_stale_pipe_lock
    [ -e "$PIPE_LOCK" ]' _ "$TEST_DIR/unit-pipe-lock" "$interruption" || {
    printf 'a %s lock did not restart the unreadable-lock clock\n' \
      "$interruption" >&2
    exit 1
  }
done
rm -f "$TEST_DIR/unit-pipe-lock"

# A live holder that never lets go: after three reaper spans a coalesced event
# is dropped and an urgent one pipes without the lock.
printf '%s alive\n' "$(($(date +%s) + 1000))" > "$PIPE_LOCK"
EXPECTED_DELIVERED="Stop:p8"
DELIVERY_WINDOW=0.05 expect_delivered "give up" \
  "@7 $(delivery_event PreToolUse dropped)" "@8 $(delivery_event Stop p8)"
rm -f "$PIPE_LOCK"

# The reaper's default bound is five seconds.
EXPECTED_DELIVERED="Stop"
ZELLAUDE_DELIVERY_REAPER_SECONDS= ZELLAUDE_TEST_ZELLIJ_HANGS=1 \
  expect_delivered "default reaper" "$(delivery_event Stop)"
expect_equal "default reaper: killed" "slept 5|zellij killed" \
  "$(grep -x -e 'slept 5' -e 'zellij killed' -e 'zellij done' "$TIMELINE" |
    tr '\n' '|' | sed 's/|$//')"

# --inspect prints the payload back to zellaude-attach.sh and never pipes.
: > "$TIMELINE"
run_hook_process '{"session_id":"delivery","hook_event_name":"SessionRestore"}' \
  ZELLAUDE_TEST_HOOK_ARGS=--inspect \
  bash "$HOOK_PARENT" > "$TEST_DIR/inspect.json"
jq -e '.hook_event == "SessionRestore"' "$TEST_DIR/inspect.json" >/dev/null
if grep -q '^zellij ' "$TIMELINE"; then
  printf -- '--inspect piped its payload\n' >&2
  exit 1
fi

# Codex reads Stop stdout as JSON to EOF: no deliverer may hold it, hung or not.
codex_stop_stdout() {
  local stdout
  stdout=$(hook_process '{"session_id":"delivery","hook_event_name":"Stop"}' \
    ZELLAUDE_TEST_HOOK_ARGS="--client codex" bash "$HOOK_PARENT")
  printf 'stdout closed %s\n' "$stdout" >> "$TIMELINE"
}
: > "$TIMELINE"
ZELLAUDE_TEST_ZELLIJ_HANGS=1 await_descendants codex_stop_stdout
expect_equal "codex Stop stdout" "stdout closed {}|zellij killed" \
  "$(grep -e '^stdout closed' -e '^zellij killed' "$TIMELINE" |
    tr '\n' '|' | sed 's/|$//')"

# SessionEnd removes this host's stream markers for the pane, not another's.
STREAM_PREFIX="$CACHE_DIR/test-session.7.$(hostname)"
compgen -G "$STREAM_PREFIX.root.*" >/dev/null
run_hook_process '{"session_id":"delivery","hook_event_name":"SessionEnd"}' \
  bash "$HOOK_PARENT" >/dev/null
if compgen -G "$STREAM_PREFIX.root.*" >/dev/null ||
   compgen -G "$STREAM_PREFIX.subagents.*" >/dev/null; then
  printf 'SessionEnd left stream markers behind\n' >&2
  exit 1
fi
compgen -G "$CACHE_DIR/test-session.7.other-host.invalid.root.*" >/dev/null

# The stub answers as macOS ps does: comm a full path, ucomm the process title.
MACOS_PS_BIN="$TEST_DIR/macos-ps-bin"
mkdir -p "$MACOS_PS_BIN"
cat > "$MACOS_PS_BIN/ps" <<'FAKE_MACOS_PS'
#!/usr/bin/env bash
pid=${!#}
case " $* " in
  *" -A "*)
    [ "${ZELLAUDE_TEST_PS_TABLE:-}" != empty ] || exit 0
    printf '    1     0 /sbin/launchd\n'
    printf ' 4242     1 /Users/x/.local/bin/claude\n'
    printf '%5s  4242 /Applications/My Term.app/Contents/MacOS/My Term\n' \
      "$ZELLAUDE_TEST_PARENT_PID"
    [ "${ZELLAUDE_TEST_PS_TABLE:-}" != failed ] || exit 1
    ;;
  *" -o comm= "*)
    if [ "$pid" = 4242 ]; then
      printf '/Users/x/.local/bin/claude\n'
    else
      printf '/bin/bash\n'
    fi
    ;;
  *" -o ucomm= "*) printf '2.1.191\n' ;;
  *" -o ppid= "*)
    if [ "$pid" = 4242 ]; then printf '1\n'; else printf '4242\n'; fi
    ;;
esac
FAKE_MACOS_PS
chmod +x "$MACOS_PS_BIN/ps"
MACOS_PATH="$MACOS_PS_BIN:$TEST_DIR/bin:$PATH"

# $1 the input, $2 the stub's table mode: "", empty or failed.
macos_exec_profile() {
  exec_profile "$1" \
    ZELLAUDE_TREE_PROC_ROOT="$TEST_DIR/no-procfs" \
    ZELLAUDE_TEST_REAL_PATH="$MACOS_PATH" \
    ZELLAUDE_TEST_PS_TABLE="$2" >/dev/null
}

rm -f "$STATE_FILE"
macos_exec_profile '{"session_id":"macos-root","hook_event_name":"PreToolUse"}' ""
expect_agent_pid "macOS batched ps" 4242
expect_equal "macOS batched ps: ps, awk" "1 1" \
  "$(exec_count ps) $(exec_count awk)"
# The memo answers first, unconfirmed: macOS has no fork-free comm check.
macos_exec_profile '{"session_id":"macos-root","hook_event_name":"PreToolUse"}' empty
expect_agent_pid "macOS memo hit" 4242
expect_equal "macOS memo hit: ps, awk" "0 0" \
  "$(exec_count ps) $(exec_count awk)"
for table_mode in empty failed; do
  macos_exec_profile \
    "{\"session_id\":\"macos-$table_mode\",\"hook_event_name\":\"PreToolUse\"}" \
    "$table_mode"
  expect_agent_pid "macOS $table_mode table falls back" 4242
  expect_equal "macOS $table_mode table: ps, awk" "4 0" \
    "$(exec_count ps) $(exec_count awk)"
done

printf 'hook mode detection tests passed\n'
