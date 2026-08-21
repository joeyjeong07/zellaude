#!/usr/bin/env bash
# zellaude-hook.sh — agent hook → zellij pipe bridge
# Forwards Claude Code and Codex hook events to the zellaude Zellij plugin.
#
# Usage:
#   Claude Code: "/path/to/zellaude-hook.sh"
#   Codex:       "/path/to/zellaude-hook.sh --client codex"

path_owner_id() {
  stat -c '%u' "$1" 2>/dev/null ||
    stat -f '%u' "$1" 2>/dev/null
}

path_mode() {
  stat -c '%a' "$1" 2>/dev/null ||
    stat -f '%Lp' "$1" 2>/dev/null
}

is_owned_private_parent() {
  local path=$1 expected_owner=$2 owner mode
  [ -d "$path" ] && [ ! -L "$path" ] || return 1
  owner=$(path_owner_id "$path") || return 1
  [ "$owner" = "$expected_owner" ] || return 1
  mode=$(path_mode "$path") || return 1
  case "$mode" in
    ''|*[!0-7]*) return 1 ;;
  esac
  while [ "${#mode}" -gt 3 ]; do
    mode=${mode#?}
  done
  [ $((8#$mode & 8#022)) -eq 0 ]
}

ensure_private_cache_dir() {
  local path=$1 expected_owner=$2 owner
  if [ -e "$path" ] || [ -L "$path" ]; then
    [ -d "$path" ] && [ ! -L "$path" ] || return 1
  else
    (umask 077 && mkdir "$path") 2>/dev/null || return 1
  fi

  owner=$(path_owner_id "$path") || return 1
  [ "$owner" = "$expected_owner" ] || return 1
  chmod 700 "$path" 2>/dev/null || return 1
  is_owned_private_parent "$path" "$expected_owner"
}

state_cache_dir() {
  local runtime_root cache_root cache_dir user_id home_cache
  user_id=$(id -u 2>/dev/null) || return 1

  runtime_root=${XDG_RUNTIME_DIR:-}
  case "$runtime_root" in
    /*)
      if is_owned_private_parent "$runtime_root" "$user_id"; then
        cache_dir="$runtime_root/zellaude-$user_id"
        ensure_private_cache_dir "$cache_dir" "$user_id" || return 1
        printf '%s\n' "$cache_dir"
        return 0
      fi
      ;;
  esac

  case "${HOME:-}" in
    /*) ;;
    *) return 1 ;;
  esac
  home_cache="$HOME/.cache"
  if [ ! -e "$home_cache" ] && [ ! -L "$home_cache" ]; then
    (umask 077 && mkdir "$home_cache") 2>/dev/null || return 1
  fi
  is_owned_private_parent "$home_cache" "$user_id" || return 1

  cache_root="$home_cache/zellaude"
  ensure_private_cache_dir "$cache_root" "$user_id" || return 1
  cache_dir="$cache_root/runtime"
  ensure_private_cache_dir "$cache_dir" "$user_id" || return 1
  printf '%s\n' "$cache_dir"
}

state_cache_key() {
  local value=$1 safe digest hex
  safe=$(printf '%s' "$value" | LC_ALL=C tr -c 'A-Za-z0-9_.-' '_')
  if [ -n "$safe" ] && [ "$safe" = "$value" ]; then
    printf '%s\n' "$safe"
    return 0
  fi

  if command -v sha256sum >/dev/null 2>&1; then
    digest=$(printf '%s' "$value" | sha256sum | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    digest=$(printf '%s' "$value" | shasum -a 256 | awk '{print $1}')
  elif command -v openssl >/dev/null 2>&1; then
    digest=$(printf '%s' "$value" | openssl dgst -sha256 2>/dev/null |
      awk '{print $NF}')
  else
    hex=$(printf '%s' "$value" | LC_ALL=C od -An -tx1 |
      tr -d '[:space:]')
    [ -n "$hex" ] && [ "${#hex}" -le 240 ] || return 1
    digest="x$hex"
  fi
  [ -n "$digest" ] || return 1
  # '~' cannot occur in an unhashed key, so hashed and literal names cannot
  # collide even when one name is the sanitized spelling of another.
  printf '~%s\n' "$digest"
}

path_mtime() {
  stat -c '%Y' "$1" 2>/dev/null ||
    stat -f '%m' "$1" 2>/dev/null
}

CACHE_LOCK_TOKEN=""

acquire_cache_lock() {
  local lock_dir=$1 attempt now owner_path owner_token owner_count
  local owner_pid owner_started lock_mtime is_stale
  attempt=0

  while [ "$attempt" -lt 50 ]; do
    now=$(date +%s 2>/dev/null) || now=0
    CACHE_LOCK_TOKEN="owner.$$.$now.${RANDOM:-0}"
    if (umask 077 && mkdir "$lock_dir") 2>/dev/null; then
      if mkdir "$lock_dir/$CACHE_LOCK_TOKEN" 2>/dev/null; then
        return 0
      fi
      rmdir "$lock_dir" 2>/dev/null || true
      CACHE_LOCK_TOKEN=""
      return 1
    fi

    [ -d "$lock_dir" ] && [ ! -L "$lock_dir" ] || return 1
    owner_path=""
    owner_token=""
    owner_count=0
    for owner_path_candidate in "$lock_dir"/owner.*; do
      [ -d "$owner_path_candidate" ] &&
        [ ! -L "$owner_path_candidate" ] || continue
      owner_count=$((owner_count + 1))
      owner_path=$owner_path_candidate
      owner_token=${owner_path_candidate##*/}
    done

    is_stale=false
    if [ "$owner_count" -eq 1 ]; then
      owner_pid=${owner_token#owner.}
      owner_pid=${owner_pid%%.*}
      owner_started=${owner_token#owner.*.}
      owner_started=${owner_started%%.*}
      case "$owner_pid:$owner_started:$now" in
        *[!0-9:]*|:*|*::*|*:) ;;
        *)
          if ! kill -0 "$owner_pid" 2>/dev/null ||
             [ $((now - owner_started)) -ge 15 ]; then
            is_stale=true
          fi
          ;;
      esac
      if [ "$is_stale" = true ] &&
         rmdir "$owner_path" 2>/dev/null; then
        rmdir "$lock_dir" 2>/dev/null || true
        continue
      fi
    elif [ "$owner_count" -eq 0 ]; then
      lock_mtime=$(path_mtime "$lock_dir" 2>/dev/null) || lock_mtime=$now
      case "$lock_mtime:$now" in
        *[!0-9:]*|:*|*::*|*:) ;;
        *)
          if [ $((now - lock_mtime)) -ge 2 ] &&
             rmdir "$lock_dir" 2>/dev/null; then
            continue
          fi
          ;;
      esac
    fi

    attempt=$((attempt + 1))
    sleep 0.02
  done

  CACHE_LOCK_TOKEN=""
  return 1
}

release_cache_lock() {
  local lock_dir=$1
  [ -n "$CACHE_LOCK_TOKEN" ] || return 0
  rmdir "$lock_dir/$CACHE_LOCK_TOKEN" 2>/dev/null || true
  rmdir "$lock_dir" 2>/dev/null || true
  CACHE_LOCK_TOKEN=""
}

restore_cached_states() {
  local session_name cache_dir session_key state_file
  session_name=$1
  [ -n "$session_name" ] || return 0
  cache_dir=$(state_cache_dir) || return 0
  session_key=$(state_cache_key "$session_name") || return 0

  for state_file in "$cache_dir/$session_key".*.json; do
    [ -f "$state_file" ] && [ ! -L "$state_file" ] || continue
    jq -c --arg session_name "$session_name" '
      select(
        .zellij_session == $session_name
        and (.is_subagent == false)
        and ((.session_id // "") != "")
      )
    ' "$state_file" 2>/dev/null || true
  done
}

CLIENT="claude"
INSPECT_ONLY=false
RESTORE_SESSION=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --client)
      [ -n "${2:-}" ] || exit 0
      CLIENT=$2
      shift 2
      ;;
    --inspect)
      INSPECT_ONLY=true
      shift
      ;;
    --restore)
      [ -n "${2:-}" ] || exit 0
      RESTORE_SESSION=$2
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

if [ -n "$RESTORE_SESSION" ]; then
  restore_cached_states "$RESTORE_SESSION"
  exit 0
fi

case "$CLIENT" in
  codex) CLIENT_LABEL="Codex" ;;
  *) CLIENT="claude"; CLIENT_LABEL="Claude Code" ;;
esac

# Read hook JSON from stdin before checking Zellij. Codex Stop hooks require
# a JSON response even when there is no pane event to forward.
INPUT=$(cat)
HOOK_EVENT=$(echo "$INPUT" | jq -r '.hook_event_name // empty')

finish() {
  # Codex requires JSON stdout from these two hooks. An empty object leaves
  # the agent flow unchanged.
  if [ "$CLIENT" = "codex" ] &&
     { [ "$HOOK_EVENT" = "Stop" ] || [ "$HOOK_EVENT" = "SubagentStop" ]; }; then
    printf '{}\n'
  fi
  exit "${1:-0}"
}

# Exit silently if not running inside Zellij
[ -z "${ZELLIJ_SESSION_NAME:-}" ] && finish 0
[ -z "${ZELLIJ_PANE_ID:-}" ] && finish 0
[ -z "$HOOK_EVENT" ] && finish 0

# Capture send-time immediately so the plugin can order events
# that race through parallel hook subprocesses.
TS_MS=$(jq -nc 'now * 1000 | floor')

# The agent process that fired this hook: the nearest claude/codex ancestor
# (hooks run as descendants of the client). Recorded so tools reading the
# state cache (zellmv) get exact kill/inspect targets instead of inferring
# them from argv, which process-title rewrites make unreliable. The host
# matters on shared homes, where every pod sees this cache.
find_agent_pid() {
  local pid comm
  pid=$PPID
  while [ -n "$pid" ] && [ "$pid" -gt 1 ] 2>/dev/null; do
    comm=$(ps -o comm= -p "$pid" 2>/dev/null | tr -d '[:space:]')
    case "$comm" in
      claude*|codex*) printf '%s\n' "$pid"; return 0 ;;
    esac
    pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d '[:space:]')
  done
  return 1
}
AGENT_PID=$(find_agent_pid) || AGENT_PID=""
AGENT_HOST=$(hostname 2>/dev/null) || AGENT_HOST=""

# Extract fields with jq (required dependency)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty')
CWD=$(echo "$INPUT" | jq -r '.cwd // empty')
TRANSCRIPT_PATH=$(echo "$INPUT" | jq -r '.transcript_path // empty')
TURN_ID=$(echo "$INPUT" | jq -r '.turn_id // empty')
AGENT_ID=$(echo "$INPUT" | jq -r '.agent_id // empty')
IS_SUBAGENT=false
case "$HOOK_EVENT" in
  SubagentStart|SubagentStop) IS_SUBAGENT=true ;;
esac
if [ "$IS_SUBAGENT" = false ] && [ -n "$AGENT_ID" ]; then
  IS_SUBAGENT=true
fi
if [ "$IS_SUBAGENT" = false ] &&
   [ "$CLIENT" = "codex" ] &&
   [ -r "$TRANSCRIPT_PATH" ]; then
  TRANSCRIPT_IS_SUBAGENT=$(
    head -n 16 "$TRANSCRIPT_PATH" 2>/dev/null |
      jq -Rnr '
        [
          inputs
          | fromjson?
          | select(.type == "session_meta")
          | .payload.source?
          | select((type == "object") and has("subagent"))
        ]
        | length > 0
      ' 2>/dev/null || printf 'false'
  )
  [ "$TRANSCRIPT_IS_SUBAGENT" = true ] && IS_SUBAGENT=true
fi
MAX_TRANSCRIPT_BYTES=2097152

parse_codex_transcript_effort() {
  local requested_turn_id=$1

  jq -Rnr --arg turn_id "$requested_turn_id" '
    [
        inputs
        | fromjson?
        | select(.type == "turn_context")
        | {
            turn_id: (.payload.turn_id? // ""),
            effort: (
              .payload.effort?
              // .payload.reasoning_effort?
              // empty
            )
          }
        | select(.effort | type == "string")
      ] as $contexts
    | (
        if $turn_id == "" then
          $contexts | last
        else
          [
            $contexts[]
            | select(.turn_id == $turn_id)
          ]
          | last
        end
      ) // {}
    | .effort // empty
    | ascii_downcase
  ' 2>/dev/null
}

detect_codex_rainbow() {
  local direct_effort transcript_effort

  # Prefer a direct field if a future Codex hook schema adds one.
  direct_effort=$(echo "$INPUT" | jq -r '
    [
      .reasoning_effort?,
      .model_reasoning_effort?,
      (if (.effort? | type) == "object" then .effort.level? else .effort? end)
    ]
    | map(select((type == "string") and length > 0))
    | first // empty
    | ascii_downcase
  ')
  if [ -n "$direct_effort" ]; then
    [ "$direct_effort" = "ultra" ] && printf 'true' || printf 'false'
    return
  fi

  # Codex 0.146 does not put reasoning effort in hook stdin. Its current
  # turn_context is written to the supplied JSONL transcript before hooks run.
  [ -r "$TRANSCRIPT_PATH" ] || { printf 'null'; return; }
  transcript_effort=""

  # Long-running sessions can append thousands of lines after the root turn
  # while background agents finish. Locate an exact turn anywhere in the file
  # before falling back to the bounded recent window.
  if [ -n "$TURN_ID" ]; then
    transcript_effort=$(
      grep -F -- "$TURN_ID" "$TRANSCRIPT_PATH" 2>/dev/null |
        parse_codex_transcript_effort "$TURN_ID"
    )
  fi

  if [ -z "$transcript_effort" ]; then
    transcript_effort=$(
      tail -c "$MAX_TRANSCRIPT_BYTES" "$TRANSCRIPT_PATH" 2>/dev/null |
        parse_codex_transcript_effort "$TURN_ID"
    )
  fi

  # A resumed idle session may not supply a turn ID and its latest context can
  # be older than the bounded window. Only pay for a full scan in that rare
  # fallback case, filtering before jq so large tool outputs are not parsed.
  if [ -z "$transcript_effort" ]; then
    transcript_effort=$(
      grep -F 'turn_context' "$TRANSCRIPT_PATH" 2>/dev/null |
        parse_codex_transcript_effort "$TURN_ID"
    )
  fi

  if [ -z "$transcript_effort" ]; then
    printf 'null'
  elif [ "$transcript_effort" = "ultra" ]; then
    printf 'true'
  else
    printf 'false'
  fi
}

parse_claude_effort_commands() {
  jq -Rrs '
    split("\n")
    | map(fromjson?)
    | . as $entries
    | [
        $entries[]
        | select(.type == "user")
        | select((.message.content? | type) == "string")
        | select(
            .message.content
            | contains("<command-name>/effort</command-name>")
          )
        | {
            uuid: (.uuid // ""),
            timestamp: (.timestamp // "")
          }
      ] as $commands
    | ($commands | last // null) as $command
    | if $command == null then
        ["null", "", ""]
      else
        (
          [
            $entries[]
            | select(.type == "user")
            | select(.parentUuid? == $command.uuid)
            | select((.message.content? | type) == "string")
            | select(.message.content | contains("<local-command-stdout>"))
            | {
                content: .message.content,
                timestamp: (.timestamp // "")
              }
          ]
          | last // {}
        ) as $output_entry
        | (($output_entry.content // "")
            | gsub("\u001b\\[[0-9;]*[[:alpha:]]"; "")) as $output
        | (
            if $output | test(
              "(?i)^[[:space:]]*<local-command-stdout>[[:space:]]*(current effort level:|effort level set to|set effort level to|effort level:)[[:space:]]*ultracode"
            ) then
              "true"
            elif $output | test(
              "(?i)^[[:space:]]*<local-command-stdout>[[:space:]]*(current effort level:|effort level set to|set effort level to|effort level:)[[:space:]]*(low|medium|high|xhigh|max|auto|unset)"
            ) then
              "false"
            else
              "null"
            end
          ) as $state
        | [
            $state,
            (if $state == "null" then "" else $command.uuid end),
            (
              if $state == "null"
              then ""
              elif (($output_entry.timestamp // "") | length) > 0
              then $output_entry.timestamp
              else ($command.timestamp // "")
              end
            )
          ]
      end
    | @tsv
  ' 2>/dev/null
}

iso_timestamp_ms() {
  printf '%s' "$1" |
    jq -Rr '
      try (
        capture(
          "^(?<seconds>[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\\.(?<fraction>[0-9]+))?Z$"
        ) as $parts
        | (($parts.seconds + "Z") | fromdateiso8601) * 1000
          + (
              (($parts.fraction // "") + "000")[0:3]
              | tonumber
            )
      ) catch empty
    ' 2>/dev/null
}

claude_process_requested_mode() {
  local process_id parent_id process_args depth
  process_id=$PPID
  depth=0

  while [ "$process_id" -gt 1 ] 2>/dev/null && [ "$depth" -lt 8 ]; do
    process_args=$(ps -o args= -p "$process_id" 2>/dev/null)
    case "$process_args" in
      *"--effort ultracode"*|*"--effort=ultracode"*)
        printf 'ultracode'
        return
        ;;
      *"--settings"*"\"ultracode\":true"*|\
      *"--settings"*"\"ultracode\": true"*)
        printf 'ultracode'
        return
        ;;
      *"--settings"*"\"ultracode\":false"*|\
      *"--settings"*"\"ultracode\": false"*)
        printf 'standard'
        return
        ;;
      *"--effort low"*|*"--effort=low"*|\
      *"--effort medium"*|*"--effort=medium"*|\
      *"--effort high"*|*"--effort=high"*|\
      *"--effort xhigh"*|*"--effort=xhigh"*|\
      *"--effort max"*|*"--effort=max"*|\
      *"--effort auto"*|*"--effort=auto"*)
        printf 'standard'
        return
        ;;
    esac

    parent_id=$(ps -o ppid= -p "$process_id" 2>/dev/null | tr -d ' ')
    case "$parent_id" in
      ''|*[!0-9]*)
        printf 'unknown'
        return
        ;;
    esac
    process_id=$parent_id
    depth=$((depth + 1))
  done

  printf 'unknown'
}

detect_claude_rainbow() {
  local explicit_state transcript_result transcript_state transcript_marker
  local transcript_timestamp transcript_timestamp_ms session_started_at_ms
  local launch_state launch_effort effort_level configured_effort launch_mode tail_lines

  # Accept an explicit field if Claude exposes one in a future hook version
  # (or an SDK host supplies it today).
  explicit_state=$(echo "$INPUT" | jq -r '
    def as_state:
      if . == true
        or ((type == "string") and ((ascii_downcase == "true") or (ascii_downcase == "ultracode")))
      then "true"
      else "false"
      end;

    if has("ultracode") and .ultracode != null then
      .ultracode | as_state
    elif ((.effort? | type) == "object")
      and (.effort | has("ultracode"))
      and .effort.ultracode != null then
      .effort.ultracode | as_state
    elif ((.effort.level? // "") | ascii_downcase) == "ultracode" then
      "true"
    else
      empty
    end
  ')
  if [ -n "$explicit_state" ]; then
    printf '%s\t' "$explicit_state"
    return
  fi

  # Claude reports ultracode's model effort as xhigh, so recover the separate
  # session setting from the latest successful /effort command when available.
  transcript_result=$'null\t\t'
  if [ -r "$TRANSCRIPT_PATH" ]; then
    case "$HOOK_EVENT" in
      SessionStart) tail_lines=4096 ;;
      UserPromptSubmit) tail_lines=1024 ;;
      *) tail_lines=512 ;;
    esac
    transcript_result=$(
      tail -n "$tail_lines" "$TRANSCRIPT_PATH" 2>/dev/null |
        tail -c "$MAX_TRANSCRIPT_BYTES" |
        parse_claude_effort_commands
    )
    if [ "${transcript_result%%$'\t'*}" = "null" ]; then
      transcript_result=$(
        grep -F -e '<command-name>/effort</command-name>' \
          -e '<local-command-stdout>' "$TRANSCRIPT_PATH" 2>/dev/null |
          parse_claude_effort_commands
      )
    fi
  fi
  IFS=$'\t' read -r transcript_state transcript_marker transcript_timestamp \
    <<< "$transcript_result"
  transcript_state=${transcript_state:-null}
  transcript_marker=${transcript_marker:-}
  transcript_timestamp=${transcript_timestamp:-}

  # Attach recovery reconstructs launch state from the target process rather
  # than treating it as a current hook field. A successful /effort command
  # issued after that process started is newer and therefore wins.
  if [ "$HOOK_EVENT" = "SessionRestore" ]; then
    launch_state=$(echo "$INPUT" | jq -r '
      def as_state:
        if . == true
          or ((type == "string") and ((ascii_downcase == "true") or (ascii_downcase == "ultracode")))
        then "true"
        else "false"
        end;

      if has("launch_ultracode") and .launch_ultracode != null then
        .launch_ultracode | as_state
      else
        (.launch_effort_level? // "" | ascii_downcase) as $effort
        | if $effort == "ultracode" then
            "true"
          elif ($effort | IN("low", "medium", "high", "xhigh", "max", "auto", "unset")) then
            "false"
          else
            empty
          end
      end
    ')
    session_started_at_ms=$(echo "$INPUT" | jq -r '
      .session_started_at_ms?
      | select(type == "number" and . > 0 and floor == .)
      // empty
    ')
    transcript_timestamp_ms=""
    if [ -n "$transcript_timestamp" ]; then
      transcript_timestamp_ms=$(iso_timestamp_ms "$transcript_timestamp")
    fi

    if [ -n "$launch_state" ]; then
      if [ "$transcript_state" != "null" ] &&
         [ -n "$session_started_at_ms" ] &&
         [ -n "$transcript_timestamp_ms" ] &&
         [ "$transcript_timestamp_ms" -ge "$session_started_at_ms" ]; then
        printf '%s\t%s' "$transcript_state" "$transcript_marker"
      else
        printf '%s\t%s' "$launch_state" "$transcript_marker"
      fi
      return
    fi
  fi

  configured_effort=$(printf '%s' "${CLAUDE_CODE_EFFORT_LEVEL:-}" |
    tr '[:upper:]' '[:lower:]')
  case "$configured_effort" in
    low|medium|high|max|auto|unset)
      printf 'false\t'
      return
      ;;
  esac

  # A launch flag is newer than any history in a resumed transcript. Include
  # the historical command marker as a baseline so later hooks do not replay it.
  if [ "$HOOK_EVENT" = "SessionStart" ]; then
    if [ "${ZELLAUDE_CLAUDE_MODE:-}" = "ultracode" ]; then
      printf 'true\t%s' "$transcript_marker"
      return
    fi
    launch_mode=$(claude_process_requested_mode)
    case "$launch_mode" in
      ultracode)
        printf 'true\t%s' "$transcript_marker"
        return
        ;;
      standard)
        printf 'false\t%s' "$transcript_marker"
        return
        ;;
    esac
  fi

  effort_level=$(echo "$INPUT" | jq -r '.effort.level? // empty | ascii_downcase')
  [ -n "$effort_level" ] || effort_level="${CLAUDE_EFFORT:-}"
  case "$effort_level" in
    low|medium|high|max)
      printf 'false\t'
      return
      ;;
  esac

  if [ "$transcript_state" != "null" ]; then
    printf '%s\t%s' "$transcript_state" "$transcript_marker"
    return
  fi

  # If hooks were installed after SessionStart, the first submitted prompt can
  # still seed launch-time state when no /effort command is available.
  if [ "$HOOK_EVENT" = "UserPromptSubmit" ]; then
    if [ "${ZELLAUDE_CLAUDE_MODE:-}" = "ultracode" ]; then
      printf 'true\t'
      return
    fi
    launch_mode=$(claude_process_requested_mode)
    case "$launch_mode" in
      ultracode) printf 'true\t'; return ;;
      standard) printf 'false\t'; return ;;
    esac
  fi

  case "$effort_level" in
    "")
      printf 'null\t'
      ;;
    xhigh)
      # xhigh is ambiguous: it can be ordinary xhigh or ultracode. Preserve
      # the plugin's last known state instead of creating a false positive.
      printf 'null\t'
      ;;
    *)
      printf 'false\t'
      ;;
  esac
}

if [ "$IS_SUBAGENT" = true ]; then
  # Child agents inherit the root terminal's Zellij pane but have a different
  # session ID and may use a different effort. Keep their activity updates
  # without letting them replace the root session's tab mode.
  SESSION_ID=""
  RAINBOW_NAME=null
  RAINBOW_MODE_MARKER=""
elif [ "$CLIENT" = "codex" ]; then
  RAINBOW_NAME=$(detect_codex_rainbow)
  RAINBOW_MODE_MARKER=""
else
  MODE_RESULT=$(detect_claude_rainbow)
  IFS=$'\t' read -r RAINBOW_NAME RAINBOW_MODE_MARKER <<< "$MODE_RESULT"
  RAINBOW_NAME=${RAINBOW_NAME:-null}
  RAINBOW_MODE_MARKER=${RAINBOW_MODE_MARKER:-}
fi
case "$RAINBOW_NAME" in
  true|false|null) ;;
  *) RAINBOW_NAME="null" ;;
esac

# Build compact JSON payload
PAYLOAD=$(jq -nc \
  --arg pane_id "$ZELLIJ_PANE_ID" \
  --arg session_id "$SESSION_ID" \
  --arg hook_event "$HOOK_EVENT" \
  --arg tool_name "$TOOL_NAME" \
  --arg cwd "$CWD" \
  --arg zellij_session "$ZELLIJ_SESSION_NAME" \
  --arg term_program "${TERM_PROGRAM:-}" \
  --arg client "$CLIENT" \
  --arg ts_ms "$TS_MS" \
  --arg agent_pid "$AGENT_PID" \
  --arg host "$AGENT_HOST" \
  --argjson rainbow_name "$RAINBOW_NAME" \
  --arg rainbow_mode_marker "$RAINBOW_MODE_MARKER" \
  --argjson is_subagent "$IS_SUBAGENT" \
  '{
    pane_id: ($pane_id | tonumber),
    session_id: $session_id,
    agent_pid: (if $agent_pid == "" then null else ($agent_pid | tonumber) end),
    host: (if $host == "" then null else $host end),
    hook_event: $hook_event,
    tool_name: (if $tool_name == "" then null else $tool_name end),
    cwd: (if $cwd == "" then null else $cwd end),
    zellij_session: $zellij_session,
    term_program: (if $term_program == "" then null else $term_program end),
    client: $client,
    ts_ms: ($ts_ms | tonumber),
    is_subagent: $is_subagent,
    rainbow_name: $rainbow_name,
    rainbow_mode_ts_ms: (
      if $rainbow_name == null
      then null
      else ($ts_ms | tonumber)
      end
    ),
    rainbow_mode_marker: (
      if $rainbow_mode_marker == ""
      then null
      else $rainbow_mode_marker
      end
    )
  }')

persist_root_state() {
  local cache_dir session_key cache_path lock_dir cache_tmp cache_payload
  local merged_payload cached_owner cached_ts
  [ "$IS_SUBAGENT" = false ] || return 0
  [ -n "$SESSION_ID" ] || return 0

  cache_dir=$(state_cache_dir) || return 0
  session_key=$(state_cache_key "$ZELLIJ_SESSION_NAME") || return 0
  cache_path="$cache_dir/$session_key.$ZELLIJ_PANE_ID.json"
  lock_dir="$cache_path.lock"

  umask 077
  acquire_cache_lock "$lock_dir" || return 0

  if [ -e "$cache_path" ] || [ -L "$cache_path" ]; then
    if [ ! -f "$cache_path" ] || [ -L "$cache_path" ]; then
      release_cache_lock "$lock_dir"
      return 0
    fi
  fi

  if [ "$HOOK_EVENT" = "SessionEnd" ]; then
    if [ -f "$cache_path" ]; then
      cached_owner=$(jq -r '.session_id // empty' "$cache_path" 2>/dev/null)
      cached_ts=$(jq -r '.ts_ms // 0' "$cache_path" 2>/dev/null)
      case "$cached_ts" in
        ''|*[!0-9]*) cached_ts=0 ;;
      esac
      if [ "$cached_owner" = "$SESSION_ID" ] &&
         [ "$TS_MS" -ge "$cached_ts" ]; then
        rm -f "$cache_path"
      fi
    fi
    release_cache_lock "$lock_dir"
    return 0
  fi

  cache_payload=$PAYLOAD

  if [ -f "$cache_path" ]; then
    cached_ts=$(jq -r '.ts_ms // 0' "$cache_path" 2>/dev/null)
    case "$cached_ts" in
      ''|*[!0-9]*) cached_ts=0 ;;
    esac
    if [ "$cached_ts" -gt "$TS_MS" ]; then
      release_cache_lock "$lock_dir"
      return 0
    fi

    merged_payload=$(
      jq -c --argjson current "$PAYLOAD" '
        . as $previous
        | if (
            $previous.session_id == $current.session_id
            and $current.hook_event != "SessionStart"
          ) then
            $current
            | (
                $current.rainbow_mode_marker != null
                and (
                  $current.rainbow_mode_marker
                  == $previous.rainbow_mode_marker
                )
              ) as $same_marker
            | .rainbow_name = (
                if $current.rainbow_name == null or $same_marker
                then $previous.rainbow_name
                else $current.rainbow_name
                end
              )
            | .rainbow_mode_ts_ms = (
                if $current.rainbow_name == null or $same_marker
                then (
                  $previous.rainbow_mode_ts_ms
                  // $previous.ts_ms
                  // null
                )
                else $current.rainbow_mode_ts_ms
                end
              )
            | .rainbow_mode_marker = (
                if (
                  $current.rainbow_name == null
                  or $current.rainbow_mode_marker == null
                  or $same_marker
                )
                then $previous.rainbow_mode_marker
                else $current.rainbow_mode_marker
                end
              )
          else
            $current
          end
      ' "$cache_path" 2>/dev/null
    ) || merged_payload=""
    [ -z "$merged_payload" ] || cache_payload=$merged_payload
  fi

  cache_tmp=$(mktemp "$cache_dir/.state.XXXXXX" 2>/dev/null) || {
    release_cache_lock "$lock_dir"
    return 0
  }
  if printf '%s\n' "$cache_payload" > "$cache_tmp"; then
    mv "$cache_tmp" "$cache_path" 2>/dev/null || rm -f "$cache_tmp"
  else
    rm -f "$cache_tmp"
  fi
  release_cache_lock "$lock_dir"
}

if [ "$INSPECT_ONLY" = true ]; then
  printf '%s\n' "$PAYLOAD"
  exit 0
fi

persist_root_state

# Permission request: bell + desktop notification
if [ "$HOOK_EVENT" = "PermissionRequest" ]; then
  printf '\a' > /dev/tty 2>/dev/null || true

  # Read notification setting (default: Always)
  SETTINGS_FILE="$HOME/.config/zellij/plugins/zellaude.json"
  NOTIFY_MODE="Always"
  if [ -f "$SETTINGS_FILE" ]; then
    NOTIFY_MODE=$(jq -r '.notifications // "Always"' "$SETTINGS_FILE" 2>/dev/null)
  fi

  # For "Unfocused" mode, check if the terminal app is frontmost
  SHOULD_NOTIFY=false
  case "$NOTIFY_MODE" in
    Always) SHOULD_NOTIFY=true ;;
    Unfocused)
      TERM_FOCUSED=false
      case "$(uname)" in
        Darwin)
          # Map TERM_PROGRAM to macOS process name
          EXPECTED="${TERM_PROGRAM:-}"
          case "$EXPECTED" in
            Apple_Terminal) EXPECTED="Terminal" ;;
            iTerm.app)     EXPECTED="iTerm2" ;;
          esac
          FRONT_APP=$(osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true' 2>/dev/null)
          [ "$FRONT_APP" = "$EXPECTED" ] && TERM_FOCUSED=true
          ;;
        Linux)
          # X11: check if focused window belongs to our terminal
          if command -v xdotool >/dev/null 2>&1; then
            ACTIVE_PID=$(xdotool getactivewindow getwindowpid 2>/dev/null)
            if [ -n "$ACTIVE_PID" ]; then
              # Walk up the process tree from our shell to see if the
              # focused window's process is an ancestor (i.e. our terminal)
              PID=$$
              while [ "$PID" -gt 1 ] 2>/dev/null; do
                [ "$PID" = "$ACTIVE_PID" ] && { TERM_FOCUSED=true; break; }
                PID=$(ps -o ppid= -p "$PID" 2>/dev/null | tr -d ' ')
              done
            fi
          fi
          # Wayland: no standard way to check; fall through to not-focused
          ;;
      esac
      [ "$TERM_FOCUSED" = false ] && SHOULD_NOTIFY=true
      ;;
  esac

  if [ "$SHOULD_NOTIFY" = true ]; then
    TOOL_SUFFIX=""
    [ -n "$TOOL_NAME" ] && TOOL_SUFFIX=" — $TOOL_NAME"
    TITLE="⚠ ${CLIENT_LABEL}"
    MESSAGE="Permission requested${TOOL_SUFFIX}"

    # Rate-limit: one notification per pane per 10 seconds
    LOCK="/tmp/zellaude-notify-${ZELLIJ_PANE_ID}"
    NOW=$(date +%s)
    LAST=0
    [ -f "$LOCK" ] && LAST=$(cat "$LOCK" 2>/dev/null)
    if [ $((NOW - LAST)) -ge 10 ]; then
      echo "$NOW" > "$LOCK"

      # Click callback: activate terminal + focus the pane
      ZELLIJ_BIN=$(command -v zellij)
      FOCUS_CMD="${ZELLIJ_BIN} -s '${ZELLIJ_SESSION_NAME}' pipe --name zellaude:focus -- ${ZELLIJ_PANE_ID}"

      case "$(uname)" in
        Darwin)
          [ -n "${TERM_PROGRAM:-}" ] && FOCUS_CMD="open -a '${TERM_PROGRAM}' && ${FOCUS_CMD}"
          if command -v terminal-notifier >/dev/null 2>&1; then
            terminal-notifier \
              -title "$TITLE" \
              -message "$MESSAGE" \
              -execute "$FOCUS_CMD" >/dev/null 2>&1 &
          else
            osascript -e "display notification \"$MESSAGE\" with title \"$TITLE\"" \
              >/dev/null 2>&1 &
          fi
          ;;
        Linux)
          if command -v notify-send >/dev/null 2>&1; then
            notify-send "$TITLE" "$MESSAGE" >/dev/null 2>&1 &
          fi
          ;;
      esac
    fi
  fi
fi

# Forwarding is best-effort: a missing Zellij session/plugin should never fail
# the agent's own hook. Redirect output so Codex sees only the JSON it expects.
# A dying session accepts the pipe but never answers it (SessionEnd races the
# server shutdown), so a watchdog bounds the wait — unbounded, these hooks pile
# up forever. Hand-rolled instead of timeout(1), which stock macOS lacks.
zellij pipe --name "zellaude" -- "$PAYLOAD" >/dev/null 2>&1 &
PIPE_PID=$!
( sleep 10; kill "$PIPE_PID" 2>/dev/null ) &
WATCHDOG_PID=$!
wait "$PIPE_PID" 2>/dev/null || true
kill "$WATCHDOG_PID" 2>/dev/null || true
wait "$WATCHDOG_PID" 2>/dev/null || true
finish 0
