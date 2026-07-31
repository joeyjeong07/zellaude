#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
TEST_HOME="$TEST_DIR/home"
PROC_ROOT="$TEST_DIR/proc"
RUNTIME_DIR="$TEST_DIR/runtime"
SCAN_STARTED_MS=424242
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$TEST_HOME" "$PROC_ROOT" "$RUNTIME_DIR"

write_stat() {
  local file=$1 process_id=$2 tpgid=$3 start_time=$4 parent_id=${5:-1}
  printf '%s (agent) S %s %s %s 0 %s 0 0 0 0 0 0 0 0 0 0 0 0 0 %s 0 0 0\n' \
    "$process_id" "$parent_id" "$process_id" "$process_id" "$tpgid" \
    "$start_time" > "$file"
}

write_environ() {
  local file=$1 session_name=$2 pane_id=$3 entry
  shift 3
  {
    printf 'ZELLIJ_SESSION_NAME=%s\0ZELLIJ_PANE_ID=%s\0' \
      "$session_name" "$pane_id"
    for entry in "$@"; do
      printf '%s\0' "$entry"
    done
  } > "$file"
}

# Pane 10: Zellij's PTY leader (100) points at foreground Codex PID 101.
CODEX_CWD="$TEST_HOME/work/codex"
CODEX_HOME="$TEST_HOME/.codex"
CODEX_SESSIONS="$CODEX_HOME/sessions/2026/07/31"
mkdir -p "$PROC_ROOT/100" "$PROC_ROOT/101/fd" "$PROC_ROOT/101/fdinfo"
mkdir -p "$CODEX_CWD" "$CODEX_SESSIONS"
write_stat "$PROC_ROOT/100/stat" 100 101 1000
write_stat "$PROC_ROOT/101/stat" 101 101 1001
printf 'codex\n' > "$PROC_ROOT/101/comm"
printf 'codex\0--dangerously-bypass-approvals-and-sandbox\0' \
  > "$PROC_ROOT/101/cmdline"
write_environ \
  "$PROC_ROOT/101/environ" \
  main \
  10 \
  "CODEX_HOME=$CODEX_HOME"
ln -s "$CODEX_CWD" "$PROC_ROOT/101/cwd"

CODEX_ROOT="$CODEX_SESSIONS/root.jsonl"
cat > "$CODEX_ROOT" <<CODEX_ROOT_JSONL
{"type":"session_meta","payload":{"id":"codex-root","cwd":"$CODEX_CWD","source":"cli"}}
{"type":"turn_context","payload":{"turn_id":"root-turn","effort":"ultra"}}
CODEX_ROOT_JSONL
ln -s "$CODEX_ROOT" "$PROC_ROOT/101/fd/41"
printf 'pos:\t%s\n' "$(stat -c '%s' "$CODEX_ROOT")" \
  > "$PROC_ROOT/101/fdinfo/41"

# Child writers are at EOF too, but source.subagent must exclude them.
CODEX_CHILD="$CODEX_SESSIONS/child.jsonl"
cat > "$CODEX_CHILD" <<CODEX_CHILD_JSONL
{"type":"session_meta","payload":{"id":"codex-child","cwd":"$CODEX_CWD","source":{"subagent":{"thread_spawn":{"parent_thread_id":"codex-root"}}}}}
{"type":"turn_context","payload":{"turn_id":"child-turn","effort":"high"}}
CODEX_CHILD_JSONL
ln -s "$CODEX_CHILD" "$PROC_ROOT/101/fd/42"
printf 'pos:\t%s\n' "$(stat -c '%s' "$CODEX_CHILD")" \
  > "$PROC_ROOT/101/fdinfo/42"

# Historical imported roots are reader FDs, not writers positioned at EOF.
CODEX_HISTORY="$CODEX_SESSIONS/history.jsonl"
cat > "$CODEX_HISTORY" <<CODEX_HISTORY_JSONL
{"type":"session_meta","payload":{"id":"codex-history","cwd":"$CODEX_CWD","source":"cli"}}
{"type":"turn_context","payload":{"turn_id":"history-turn","effort":"high"}}
CODEX_HISTORY_JSONL
ln -s "$CODEX_HISTORY" "$PROC_ROOT/101/fd/43"
printf 'pos:\t1\n' > "$PROC_ROOT/101/fdinfo/43"

# Pane 0: leader 200 points at foreground Claude PID 201. Pane zero is valid
# in Zellij and must not be confused with an invalid process ID.
CLAUDE_CWD="$TEST_HOME/work/claude"
CLAUDE_HOME="$TEST_HOME/.claude"
CLAUDE_SESSION="claude-root"
mkdir -p "$PROC_ROOT/200" "$PROC_ROOT/201"
mkdir -p "$CLAUDE_CWD" "$CLAUDE_HOME/sessions"
mkdir -p "$CLAUDE_HOME/projects/-test-project"
write_stat "$PROC_ROOT/200/stat" 200 201 2000
write_stat "$PROC_ROOT/201/stat" 201 201 555
printf 'claude\n' > "$PROC_ROOT/201/comm"
printf 'claude\0--dangerously-skip-permissions\0' > "$PROC_ROOT/201/cmdline"
write_environ \
  "$PROC_ROOT/201/environ" \
  main \
  0 \
  "CLAUDE_CONFIG_DIR=$CLAUDE_HOME"
ln -s "$CLAUDE_CWD" "$PROC_ROOT/201/cwd"
cat > "$CLAUDE_HOME/sessions/201.json" <<CLAUDE_REGISTRY_JSON
{
  "pid": 201,
  "procStart": "555",
  "startedAt": 1785405600000,
  "sessionId": "$CLAUDE_SESSION",
  "cwd": "$CLAUDE_CWD",
  "kind": "interactive",
  "entrypoint": "cli"
}
CLAUDE_REGISTRY_JSON
cat > "$CLAUDE_HOME/projects/-test-project/$CLAUDE_SESSION.jsonl" <<'CLAUDE_JSONL'
{"type":"user","uuid":"effort-command","message":{"content":"<command-name>/effort</command-name>"}}
{"type":"user","parentUuid":"effort-command","message":{"content":"<local-command-stdout>Set effort level to ultracode (this session only)</local-command-stdout>"}}
CLAUDE_JSONL

run_attach() {
  local records
  if [ "$#" -eq 0 ]; then
    records="0:200:claude,10:100:codex"
  else
    records=$1
  fi
  HOME="$TEST_HOME" \
    XDG_RUNTIME_DIR="$RUNTIME_DIR" \
    ZELLAUDE_PROC_ROOT="$PROC_ROOT" \
    ZELLAUDE_ATTACH_HOOK="$PROJECT_DIR/scripts/zellaude-hook.sh" \
    "$PROJECT_DIR/scripts/zellaude-attach.sh" \
      main \
      "$records" \
      "$SCAN_STARTED_MS"
}

# The cache is the portable attach path and must be restored even when pane
# introspection produces no agent process records.
CACHE_DIR="$RUNTIME_DIR/zellaude-$(id -u)"
CACHE_FILE="$CACHE_DIR/main.77.json"
mkdir -p "$CACHE_DIR"
CACHE_TS_MS=$(jq -nr 'now * 1000 | floor')
CACHE_MODE_TS_MS=$((CACHE_TS_MS - 1000))
cat > "$CACHE_FILE" <<CACHE_JSON
{
  "pane_id": 77,
  "session_id": "cache-only-root",
  "hook_event": "Notification",
  "zellij_session": "main",
  "client": "codex",
  "ts_ms": $CACHE_TS_MS,
  "is_subagent": false,
  "rainbow_name": true,
  "rainbow_mode_ts_ms": $CACHE_MODE_TS_MS,
  "rainbow_mode_marker": "cached-ultra"
}
CACHE_JSON
OUTPUT=$(run_attach "")
printf '%s\n' "$OUTPUT" |
  jq -s -e --argjson mode_ts "$CACHE_MODE_TS_MS" '
    length == 1
    and .[0].pane_id == 77
    and .[0].session_id == "cache-only-root"
    and .[0].rainbow_name == true
    and .[0].rainbow_mode_ts_ms == $mode_ts
  ' >/dev/null

# Persistent fallback entries expire instead of painting a reused pane
# indefinitely after an agent crashes without SessionEnd.
jq '.ts_ms = 1' "$CACHE_FILE" > "$CACHE_FILE.tmp"
mv "$CACHE_FILE.tmp" "$CACHE_FILE"
OUTPUT=$(run_attach "")
[ -z "$OUTPUT" ]
rm -f "$CACHE_FILE"

OUTPUT=$(run_attach)
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    length == 2
    and any(
      .[];
      .pane_id == 10
      and .session_id == "codex-root"
      and .hook_event == "SessionRestore"
      and .ts_ms == 424242
      and .rainbow_name == true
      and .rainbow_mode_ts_ms == 424242
      and .is_subagent == false
    )
    and any(
      .[];
      .pane_id == 0
      and .session_id == "claude-root"
      and .hook_event == "SessionRestore"
      and .ts_ms == 424242
      and .rainbow_name == true
      and .rainbow_mode_ts_ms == 424242
      and .is_subagent == false
    )
  ' >/dev/null

# A foreground child tool can temporarily replace the command Zellij reports.
# Walking its parent chain must still find the owning root agent.
mkdir -p "$PROC_ROOT/102"
write_stat "$PROC_ROOT/100/stat" 100 102 1000
write_stat "$PROC_ROOT/102/stat" 102 102 1002 101
printf 'bash\n' > "$PROC_ROOT/102/comm"
OUTPUT=$(run_attach "0:200:unknown,10:100:unknown")
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    length == 2
    and any(
      .[];
      .pane_id == 10
      and .session_id == "codex-root"
      and .rainbow_name == true
    )
  ' >/dev/null
write_stat "$PROC_ROOT/100/stat" 100 101 1000

# Opening the same canonical root transcript through two FDs is not ambiguous.
# The probe must deduplicate the path rather than count descriptors.
ln -s "$CODEX_ROOT" "$PROC_ROOT/101/fd/44"
printf 'pos:\t%s\n' "$(stat -c '%s' "$CODEX_ROOT")" \
  > "$PROC_ROOT/101/fdinfo/44"
OUTPUT=$(run_attach)
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    length == 2
    and any(
      .[];
      .pane_id == 10
      and .session_id == "codex-root"
      and .rainbow_name == true
    )
    and any(
      .[];
      .pane_id == 0
      and .session_id == "claude-root"
    )
  ' >/dev/null
rm -f "$PROC_ROOT/101/fd/44" "$PROC_ROOT/101/fdinfo/44"

# A second eligible Codex root is ambiguous and must fail closed for that pane.
CODEX_AMBIGUOUS="$CODEX_SESSIONS/ambiguous.jsonl"
cat > "$CODEX_AMBIGUOUS" <<CODEX_AMBIGUOUS_JSONL
{"type":"session_meta","payload":{"id":"codex-ambiguous","cwd":"$CODEX_CWD","source":"cli"}}
{"type":"turn_context","payload":{"turn_id":"ambiguous-turn","effort":"ultra"}}
CODEX_AMBIGUOUS_JSONL
ln -s "$CODEX_AMBIGUOUS" "$PROC_ROOT/101/fd/44"
printf 'pos:\t%s\n' "$(stat -c '%s' "$CODEX_AMBIGUOUS")" \
  > "$PROC_ROOT/101/fdinfo/44"

OUTPUT=$(run_attach)
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    length == 1
    and .[0].pane_id == 0
    and .[0].session_id == "claude-root"
  ' >/dev/null

# Claude PID reuse or stale registry metadata must also fail closed.
rm -f "$PROC_ROOT/101/fd/44" "$PROC_ROOT/101/fdinfo/44"
jq '.procStart = "556"' "$CLAUDE_HOME/sessions/201.json" \
  > "$CLAUDE_HOME/sessions/201.json.tmp"
mv "$CLAUDE_HOME/sessions/201.json.tmp" "$CLAUDE_HOME/sessions/201.json"
OUTPUT=$(run_attach)
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    length == 1
    and .[0].pane_id == 10
    and .[0].session_id == "codex-root"
  ' >/dev/null

# A custom Claude launcher can expose ultracode only through its documented
# sentinel. The attach subprocess must recover that value from the target
# process environment because it does not inherit the agent's environment.
jq '.procStart = "555"' "$CLAUDE_HOME/sessions/201.json" \
  > "$CLAUDE_HOME/sessions/201.json.tmp"
mv "$CLAUDE_HOME/sessions/201.json.tmp" "$CLAUDE_HOME/sessions/201.json"
cat > "$CLAUDE_HOME/projects/-test-project/$CLAUDE_SESSION.jsonl" <<'CLAUDE_NO_EFFORT_JSONL'
{"type":"assistant","message":{"content":"No effort command in this transcript."}}
CLAUDE_NO_EFFORT_JSONL

# Parse launch options as NUL-delimited argv. Prompt text containing a flag is
# not itself a launch option.
write_environ \
  "$PROC_ROOT/201/environ" \
  main \
  0 \
  "CLAUDE_CONFIG_DIR=$CLAUDE_HOME"
printf 'claude\0explain --effort ultracode\0' > "$PROC_ROOT/201/cmdline"
OUTPUT=$(run_attach)
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    any(
      .[];
      .pane_id == 0
      and .session_id == "claude-root"
      and .rainbow_name == null
    )
  ' >/dev/null

# Repeated options use the last recognized value.
printf 'claude\0--effort\0ultracode\0--effort=high\0' \
  > "$PROC_ROOT/201/cmdline"
OUTPUT=$(run_attach)
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    any(
      .[];
      .pane_id == 0
      and .session_id == "claude-root"
      and .rainbow_name == false
    )
  ' >/dev/null

printf 'claude\0--dangerously-skip-permissions\0' > "$PROC_ROOT/201/cmdline"
write_environ \
  "$PROC_ROOT/201/environ" \
  main \
  0 \
  "CLAUDE_CONFIG_DIR=$CLAUDE_HOME" \
  "ZELLAUDE_CLAUDE_MODE=ultracode"
OUTPUT=$(run_attach)
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    length == 2
    and any(
      .[];
      .pane_id == 0
      and .session_id == "claude-root"
      and .rainbow_name == true
    )
  ' >/dev/null

printf 'attach detection tests passed\n'
