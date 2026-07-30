#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
CAPTURE_FILE="$TEST_DIR/payload.json"
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$TEST_DIR/bin"
cat > "$TEST_DIR/bin/zellij" <<'FAKE_ZELLIJ'
#!/usr/bin/env bash
printf '%s' "${!#}" > "$ZELLAUDE_TEST_CAPTURE"
FAKE_ZELLIJ
chmod +x "$TEST_DIR/bin/zellij"

cat > "$TEST_DIR/fake-claude" <<'FAKE_CLAUDE'
#!/usr/bin/env bash
printf '%s' "$ZELLAUDE_TEST_INPUT" | "$ZELLAUDE_TEST_HOOK"
FAKE_CLAUDE
chmod +x "$TEST_DIR/fake-claude"

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
    printf '%s' "$input" |
      env -u CLAUDE_EFFORT \
        -u CLAUDE_CODE_EFFORT_LEVEL \
        -u ZELLAUDE_CLAUDE_MODE \
        PATH="$TEST_DIR/bin:$PATH" \
        ZELLIJ_SESSION_NAME="test-session" \
        ZELLIJ_PANE_ID="7" \
        ZELLAUDE_TEST_CAPTURE="$CAPTURE_FILE" \
        "$PROJECT_DIR/scripts/zellaude-hook.sh" --client codex >/dev/null
  else
    printf '%s' "$input" |
      env -u CLAUDE_EFFORT \
        -u CLAUDE_CODE_EFFORT_LEVEL \
        ZELLAUDE_CLAUDE_MODE="$mode" \
        PATH="$TEST_DIR/bin:$PATH" \
        ZELLIJ_SESSION_NAME="test-session" \
        ZELLIJ_PANE_ID="7" \
        ZELLAUDE_TEST_CAPTURE="$CAPTURE_FILE" \
        "$PROJECT_DIR/scripts/zellaude-hook.sh" >/dev/null
  fi

  actual=$(jq -c '.rainbow_name' "$CAPTURE_FILE")
  if [ "$actual" != "$expected" ]; then
    printf 'expected %s %s, got %s\n' "$client" "$expected" "$actual" >&2
    printf 'payload: %s\n' "$(cat "$CAPTURE_FILE")" >&2
    exit 1
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
  env -u CLAUDE_EFFORT \
    -u CLAUDE_CODE_EFFORT_LEVEL \
    -u ZELLAUDE_CLAUDE_MODE \
    PATH="$TEST_DIR/bin:$PATH" \
    ZELLIJ_SESSION_NAME="test-session" \
    ZELLIJ_PANE_ID="7" \
    ZELLAUDE_TEST_CAPTURE="$CAPTURE_FILE" \
    ZELLAUDE_TEST_HOOK="$PROJECT_DIR/scripts/zellaude-hook.sh" \
    ZELLAUDE_TEST_INPUT="$input" \
    "$TEST_DIR/fake-claude" --effort "$effort" >/dev/null

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
  env -u CLAUDE_EFFORT \
    -u CLAUDE_CODE_EFFORT_LEVEL \
    -u ZELLAUDE_CLAUDE_MODE \
    PATH="$TEST_DIR/bin:$PATH" \
    ZELLIJ_SESSION_NAME="test-session" \
    ZELLIJ_PANE_ID="7" \
    ZELLAUDE_TEST_CAPTURE="$CAPTURE_FILE" \
    ZELLAUDE_TEST_HOOK="$PROJECT_DIR/scripts/zellaude-hook.sh" \
    ZELLAUDE_TEST_INPUT='{"session_id":"claude-settings","hook_event_name":"SessionStart"}' \
    "$TEST_DIR/fake-claude" --settings "$settings" >/dev/null

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
run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_TRANSCRIPT" \
  '{session_id:"codex-high",hook_event_name:"PreToolUse",turn_id:"turn-high",transcript_path:$transcript}')" \
  false
run_hook codex "$(jq -nc \
  --arg transcript "$CODEX_TRANSCRIPT" \
  '{session_id:"codex-missing-turn",hook_event_name:"PreToolUse",turn_id:"missing-turn",transcript_path:$transcript}')" \
  null

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

CLAUDE_XHIGH_TRANSCRIPT="$TEST_DIR/claude-xhigh.jsonl"
cat > "$CLAUDE_XHIGH_TRANSCRIPT" <<'CLAUDE_XHIGH_JSONL'
{"type":"user","uuid":"effort-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"effort-command","message":{"content":"<local-command-stdout>Set effort level to xhigh (saved as your default)</local-command-stdout>"}}
CLAUDE_XHIGH_JSONL

run_hook claude "$(jq -nc \
  --arg transcript "$CLAUDE_XHIGH_TRANSCRIPT" \
  '{session_id:"claude-xhigh",hook_event_name:"UserPromptSubmit",transcript_path:$transcript,effort:{level:"xhigh"}}')" \
  false
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
run_inline_settings_flag '{"ultracode":true}' true
run_inline_settings_flag '{"ultracode":false,"other":true}' false

printf 'hook mode detection tests passed\n'
