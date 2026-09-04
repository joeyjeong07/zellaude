#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR=$(mktemp -d)
TEST_HOME="$TEST_DIR/home"
PROC_ROOT="$TEST_DIR/proc"
EMPTY_PROC_ROOT="$TEST_DIR/proc-empty"
RUNTIME_DIR="$TEST_DIR/runtime"
SCAN_STARTED_MS=424242
trap 'rm -rf "$TEST_DIR"' EXIT

mkdir -p "$TEST_HOME" "$PROC_ROOT" "$EMPTY_PROC_ROOT" "$RUNTIME_DIR"

write_stat() {
  local file=$1 process_id=$2 start_time=$3
  printf '%s (agent) S 1 %s %s 0 %s 0 0 0 0 0 0 0 0 0 0 0 0 0 %s 0 0 0\n' \
    "$process_id" "$process_id" "$process_id" "$process_id" \
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

# Pane 10's process set: the pane shell (100) and the Codex agent (101). Only
# the agent carries a claude/codex comm, so the walk must pick it out of the set.
CODEX_CWD="$TEST_HOME/work/codex"
CODEX_HOME="$TEST_HOME/.codex"
CODEX_SESSIONS="$CODEX_HOME/sessions/2026/07/31"
mkdir -p "$PROC_ROOT/100" "$PROC_ROOT/101/fd" "$PROC_ROOT/101/fdinfo"
mkdir -p "$CODEX_CWD" "$CODEX_SESSIONS"
printf 'bash\n' > "$PROC_ROOT/100/comm"
write_environ "$PROC_ROOT/100/environ" main 10
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

# Pane 0's process set: the pane shell (200), a nested `claude -p` (199) and the
# interactive agent (201). Pane zero is valid in Zellij and must not be confused
# with an invalid process ID.
CLAUDE_CWD="$TEST_HOME/work/claude"
CLAUDE_HOME="$TEST_HOME/.claude"
CLAUDE_SESSION="claude-root"
mkdir -p "$PROC_ROOT/199" "$PROC_ROOT/200" "$PROC_ROOT/201"
mkdir -p "$CLAUDE_CWD" "$CLAUDE_HOME/sessions"
mkdir -p "$CLAUDE_HOME/projects/-test-project"
printf 'bash\n' > "$PROC_ROOT/200/comm"
write_environ "$PROC_ROOT/200/environ" main 0
write_stat "$PROC_ROOT/201/stat" 201 555
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

# The nested agent sits at a lower pid, so ascending order reaches it first and
# only the registry's kind/entrypoint separates it from the interactive one —
# the single check direct selection leans on now that no leader is inferred.
write_stat "$PROC_ROOT/199/stat" 199 500
printf 'claude\n' > "$PROC_ROOT/199/comm"
printf 'claude\0-p\0summarize\0' > "$PROC_ROOT/199/cmdline"
write_environ \
  "$PROC_ROOT/199/environ" \
  main \
  0 \
  "CLAUDE_CONFIG_DIR=$CLAUDE_HOME"
cat > "$CLAUDE_HOME/sessions/199.json" <<CLAUDE_NESTED_JSON
{
  "pid": 199,
  "procStart": "500",
  "startedAt": 1785405500000,
  "sessionId": "claude-nested",
  "cwd": "$CLAUDE_CWD",
  "kind": "print",
  "entrypoint": "sdk"
}
CLAUDE_NESTED_JSON

# A shared /proc also holds other Zellij sessions' agents. The walk keys on
# ZELLIJ_SESSION_NAME, so this one belongs to no pane set here.
mkdir -p "$PROC_ROOT/301"
printf 'claude\n' > "$PROC_ROOT/301/comm"
write_environ \
  "$PROC_ROOT/301/environ" \
  other \
  0 \
  "CLAUDE_CONFIG_DIR=$CLAUDE_HOME"

# $1 is the /proc fixture root: discovery reads the walk rather than argv now,
# so an empty root is how a session with no agent processes is modelled.
run_attach() {
  local proc_root=$1
  HOME="$TEST_HOME" \
    XDG_RUNTIME_DIR="$RUNTIME_DIR" \
    ZELLAUDE_PROC_ROOT="$proc_root" \
    ZELLAUDE_ATTACH_HOOK="$PROJECT_DIR/scripts/zellaude-hook.sh" \
    "$PROJECT_DIR/scripts/zellaude-attach.sh" \
      main \
      "$SCAN_STARTED_MS"
}

# The cache is the portable attach path and must be restored even when the
# /proc walk finds no agent processes.
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
OUTPUT=$(run_attach "$EMPTY_PROC_ROOT")
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
OUTPUT=$(run_attach "$EMPTY_PROC_ROOT")
[ -z "$OUTPUT" ]
rm -f "$CACHE_FILE"

# length == 2 is what rejects the nested agent: a second pane-0 row would mean
# the registry check let `claude -p` through.
OUTPUT=$(run_attach "$PROC_ROOT")
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

# Two ACCEPTED agents in one pane set — the case the claim and the sort exist
# for. The pane is claimed once, by the lower pid: cutting the claim emits both
# rows for pane 0, and reversing the order emits claude-second instead. The
# fixture is removed afterwards so later cases keep a single accepted candidate.
CLAUDE_SECOND="claude-second"
mkdir -p "$PROC_ROOT/202"
write_stat "$PROC_ROOT/202/stat" 202 600
printf 'claude\n' > "$PROC_ROOT/202/comm"
printf 'claude\0--dangerously-skip-permissions\0' > "$PROC_ROOT/202/cmdline"
write_environ \
  "$PROC_ROOT/202/environ" \
  main \
  0 \
  "CLAUDE_CONFIG_DIR=$CLAUDE_HOME"
ln -s "$CLAUDE_CWD" "$PROC_ROOT/202/cwd"
cat > "$CLAUDE_HOME/sessions/202.json" <<CLAUDE_SECOND_JSON
{
  "pid": 202,
  "procStart": "600",
  "startedAt": 1785405700000,
  "sessionId": "$CLAUDE_SECOND",
  "cwd": "$CLAUDE_CWD",
  "kind": "interactive",
  "entrypoint": "cli"
}
CLAUDE_SECOND_JSON
cat > "$CLAUDE_HOME/projects/-test-project/$CLAUDE_SECOND.jsonl" \
  <<'CLAUDE_SECOND_JSONL'
{"type":"assistant","message":{"content":"second agent"}}
CLAUDE_SECOND_JSONL
OUTPUT=$(run_attach "$PROC_ROOT")
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    ([.[] | select(.pane_id == 0)] | length) == 1
    and any(.[]; .pane_id == 0 and .session_id == "claude-root")
  ' >/dev/null
rm -rf "$PROC_ROOT/202"
rm -f "$CLAUDE_HOME/sessions/202.json" \
  "$CLAUDE_HOME/projects/-test-project/$CLAUDE_SECOND.jsonl"

# Opening the same canonical root transcript through two FDs is not ambiguous.
# The probe must deduplicate the path rather than count descriptors.
ln -s "$CODEX_ROOT" "$PROC_ROOT/101/fd/44"
printf 'pos:\t%s\n' "$(stat -c '%s' "$CODEX_ROOT")" \
  > "$PROC_ROOT/101/fdinfo/44"
OUTPUT=$(run_attach "$PROC_ROOT")
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

OUTPUT=$(run_attach "$PROC_ROOT")
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
OUTPUT=$(run_attach "$PROC_ROOT")
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
OUTPUT=$(run_attach "$PROC_ROOT")
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
OUTPUT=$(run_attach "$PROC_ROOT")
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
OUTPUT=$(run_attach "$PROC_ROOT")
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

# Chunking must not change what the walk sees. At one file per invocation the
# only thing keeping a filename on each grep line is -H, and without it the awk
# never gets a pid. The grep count is compared rather than thresholded because a
# knob that silently stopped chunking would leave this comparing two identical
# runs — the count is what proves the multi-invocation path ran at all.
REAL_GREP=$(command -v grep)
GREP_COUNT_BIN="$TEST_DIR/grep-count-bin"
GREP_COUNT_LOG="$TEST_DIR/grep-count.log"
mkdir -p "$GREP_COUNT_BIN"
cat > "$GREP_COUNT_BIN/grep" <<COUNTING_GREP
#!/usr/bin/env bash
printf 'x\n' >> "$GREP_COUNT_LOG"
exec "$REAL_GREP" "\$@"
COUNTING_GREP
chmod +x "$GREP_COUNT_BIN/grep"

: > "$GREP_COUNT_LOG"
UNCHUNKED_OUTPUT=$(PATH="$GREP_COUNT_BIN:$PATH" run_attach "$PROC_ROOT")
UNCHUNKED_GREPS=$(wc -l < "$GREP_COUNT_LOG")
: > "$GREP_COUNT_LOG"
export ZELLAUDE_PROC_SCAN_MAX_ARGS=1
CHUNKED_OUTPUT=$(PATH="$GREP_COUNT_BIN:$PATH" run_attach "$PROC_ROOT")
unset ZELLAUDE_PROC_SCAN_MAX_ARGS
CHUNKED_GREPS=$(wc -l < "$GREP_COUNT_LOG")
[ "$CHUNKED_GREPS" -gt "$UNCHUNKED_GREPS" ]
[ "$CHUNKED_OUTPUT" = "$UNCHUNKED_OUTPUT" ]

# Run with the empty proc root so nothing is discovered and the only rows are
# cached ones, and with a fake ps on PATH. The hook exits on --restore before it
# reads any process, so the stub answers the validator alone.
LOCAL_HOSTNAME=$(hostname 2>/dev/null) || LOCAL_HOSTNAME=""
VALIDATOR_PS_BIN="$TEST_DIR/validator-ps-bin"
PS_UNAVAILABLE_BIN="$TEST_DIR/ps-unavailable-bin"
mkdir -p "$VALIDATOR_PS_BIN" "$PS_UNAVAILABLE_BIN"

# Exits are set deliberately — live 0, dead 1 with no output — because the
# validator decides on exit status, never on empty output. `-o comm=` also
# reproduces the macOS form measured 2026-09-03, a full path, while `-o ucomm=`
# answers with the rewritten process title: a ucomm implementation reads
# "2.1.191" for the live agent and drops the row this asserts it keeps.
cat > "$VALIDATOR_PS_BIN/ps" <<'FAKE_VALIDATOR_PS'
#!/usr/bin/env bash
case " $* " in
  *" -o ucomm= "*) printf '2.1.191\n'; exit 0 ;;
esac
case "${!#}" in
  4101) printf '/Users/x/.local/bin/claude\n'; exit 0 ;;
  4102) printf 'bash\n'; exit 0 ;;
  *) exit 1 ;;
esac
FAKE_VALIDATOR_PS
chmod +x "$VALIDATOR_PS_BIN/ps"

cat > "$PS_UNAVAILABLE_BIN/ps" <<'FAKE_UNAVAILABLE_PS'
#!/usr/bin/env bash
exit 127
FAKE_UNAVAILABLE_PS
chmod +x "$PS_UNAVAILABLE_BIN/ps"

HOSTLESS_BIN="$TEST_DIR/hostless-bin"
mkdir -p "$HOSTLESS_BIN"
cat > "$HOSTLESS_BIN/hostname" <<'FAKE_HOSTNAME'
#!/usr/bin/env bash
exit 1
FAKE_HOSTNAME
chmod +x "$HOSTLESS_BIN/hostname"

write_cache_entry() {
  local pane_id=$1 session_id=$2 agent_pid=$3 host=$4
  jq -nc \
    --argjson pane_id "$pane_id" \
    --arg session_id "$session_id" \
    --argjson agent_pid "$agent_pid" \
    --arg host "$host" \
    --argjson ts_ms "$(jq -nr 'now * 1000 | floor')" \
    '{
      pane_id: $pane_id,
      session_id: $session_id,
      hook_event: "Notification",
      zellij_session: "main",
      client: "claude",
      ts_ms: $ts_ms,
      is_subagent: false,
      rainbow_name: true,
      agent_pid: (if $agent_pid == 0 then null else $agent_pid end),
      host: (if $host == "" then null else $host end)
    }' > "$CACHE_DIR/main.$pane_id.json"
}

run_attach_validating() {
  local path_prefix=$1
  HOME="$TEST_HOME" \
    XDG_RUNTIME_DIR="$RUNTIME_DIR" \
    ZELLAUDE_PROC_ROOT="$EMPTY_PROC_ROOT" \
    ZELLAUDE_ATTACH_HOOK="$PROJECT_DIR/scripts/zellaude-hook.sh" \
    PATH="$path_prefix:$PATH" \
    "$PROJECT_DIR/scripts/zellaude-attach.sh" \
      main \
      "$SCAN_STARTED_MS"
}

write_cache_entry 80 live-agent            4101 "$LOCAL_HOSTNAME"
write_cache_entry 81 dead-agent            4199 "$LOCAL_HOSTNAME"
write_cache_entry 82 reused-pid            4102 "$LOCAL_HOSTNAME"
write_cache_entry 83 null-agent-pid           0 "$LOCAL_HOSTNAME"
write_cache_entry 84 foreign-host         4199 "elsewhere.invalid"
write_cache_entry 85 null-host            4199 ""

# Dropped only on positive evidence: 81 is gone (exit 1, no output) and 82 is a
# recycled pid running something else. The other four are kept — a live agent
# whose comm still matches, a null agent_pid, and two entries this host has no
# standing to judge.
OUTPUT=$(run_attach_validating "$VALIDATOR_PS_BIN")
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    ([.[] | .session_id] | sort)
    == ["foreign-host", "live-agent", "null-agent-pid", "null-host"]
  ' >/dev/null

# A ps that cannot run is evidence about nothing: it fails for every entry in
# the same pass, so treating its empty output as death would blank the restore
# rather than lose one row. All six survive, including the two just dropped.
OUTPUT=$(run_attach_validating "$PS_UNAVAILABLE_BIN")
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    ([.[] | .session_id] | sort)
    == ["dead-agent", "foreign-host", "live-agent", "null-agent-pid",
        "null-host", "reused-pid"]
  ' >/dev/null

# A local hostname that cannot be read must not make a hostless entry local:
# empty equals empty would validate a pid against a process table it never came
# from. Both sides have to be non-empty before the comparison means anything, so
# every entry survives here — including the two the same ps calls dead.
OUTPUT=$(run_attach_validating "$HOSTLESS_BIN:$VALIDATOR_PS_BIN")
printf '%s\n' "$OUTPUT" |
  jq -s -e '
    ([.[] | .session_id] | sort)
    == ["dead-agent", "foreign-host", "live-agent", "null-agent-pid",
        "null-host", "reused-pid"]
  ' >/dev/null

rm -f "$CACHE_DIR"/main.8*.json

printf 'attach detection tests passed\n'
