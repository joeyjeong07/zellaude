#!/usr/bin/env bash
# Discover agent sessions that were already running when Zellaude attached.
# Arguments:
#   $1 — Zellij session name
#   $2 — attach scan start time in milliseconds

SESSION_NAME=${1:-}
SCAN_STARTED_MS=${2:-}
HOOK_PATH=${ZELLAUDE_ATTACH_HOOK:-"$HOME/.config/zellij/plugins/zellaude-hook.sh"}
PROC_ROOT=${ZELLAUDE_PROC_ROOT:-/proc}

[ -n "$SESSION_NAME" ] || exit 0
[ -x "$HOOK_PATH" ] || exit 0

# Cached hook state preserves launch-only mode choices that cannot always be
# reconstructed from a transcript.
CACHED_STATES=$("$HOOK_PATH" --restore "$SESSION_NAME" 2>/dev/null || true)

proc_env_value() {
  local process_id=$1 key=$2
  tr '\0' '\n' < "$PROC_ROOT/$process_id/environ" 2>/dev/null |
    sed -n "s/^${key}=//p" |
    head -n 1
}

proc_stat_value() {
  local process_id=$1 field_after_comm=$2 stat_line stat_rest
  stat_line=$(cat "$PROC_ROOT/$process_id/stat" 2>/dev/null) || return 1
  stat_rest=${stat_line##*) }
  printf '%s\n' "$stat_rest" |
    awk -v field="$field_after_comm" '{ print $field }'
}

valid_nonnegative_number() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
    *) return 0 ;;
  esac
}

valid_positive_number() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
    0) return 1 ;;
    *) return 0 ;;
  esac
}

valid_session_id() {
  case "$1" in
    ''|*[!A-Za-z0-9-]*) return 1 ;;
    *) return 0 ;;
  esac
}

LOCAL_HOST=$(hostname 2>/dev/null) || LOCAL_HOST=""

# True ONLY on positive evidence that the cached entry's agent is gone: ps
# exited 1 with no output, or the pid is alive running something else. Every
# other outcome keeps the entry. Decide on exit status, never on empty output:
# a ps that cannot run prints nothing too, and it fails for every entry in the
# same pass, so that reading would blank the whole restore rather than lose one
# row. A foreign or absent hostname is evidence about nothing local, so validate
# only when both hostnames are non-empty and equal.
cached_agent_is_gone() {
  local agent_pid=$1 client=$2 entry_host=$3 reported_comm status
  valid_positive_number "$agent_pid" || return 1
  [ -n "$entry_host" ] && [ -n "$LOCAL_HOST" ] || return 1
  [ "$entry_host" = "$LOCAL_HOST" ] || return 1

  reported_comm=$(ps -o comm= -p "$agent_pid" 2>/dev/null)
  status=$?
  # macOS `ps -o comm=` returns a full path for claude but a bare name for
  # fish, so basename it — no uname-keyed strip covers both. Never `ucomm`:
  # it returns the rewritten process title (measured "2.1.191", 2026-09-03).
  reported_comm=$(printf '%s' "$reported_comm" | tr -d '[:space:]')
  reported_comm=${reported_comm##*/}

  if [ "$status" -eq 1 ]; then
    [ -z "$reported_comm" ] && return 0
    return 1
  fi
  [ "$status" -eq 0 ] || return 1
  [ -n "$reported_comm" ] || return 1

  case "$reported_comm" in
    "$client"*) return 1 ;;
    *) return 0 ;;
  esac
}

emit_cached_states() {
  local now_ms cache_max_age_ms line metadata pane_id client ts_ms
  local agent_pid entry_host age_ms
  now_ms=$(jq -nr 'now * 1000 | floor' 2>/dev/null) || return 0
  valid_positive_number "$now_ms" || return 0
  cache_max_age_ms=43200000

  while IFS= read -r line; do
    [ -n "$line" ] || continue
    # agent_pid defaults to 0, not "": tab is IFS whitespace, so an empty field
    # ahead of `host` would collapse and shift it into agent_pid. 0 is no pid.
    metadata=$(printf '%s\n' "$line" | jq -r '
      [(.pane_id // ""), (.client // ""), (.ts_ms // ""),
       (.agent_pid // 0), (.host // "")]
      | @tsv
    ' 2>/dev/null) || continue
    IFS=$'\t' read -r pane_id client ts_ms agent_pid entry_host <<< "$metadata"
    valid_nonnegative_number "$pane_id" || continue
    valid_positive_number "$ts_ms" || continue
    case "$client" in
      codex|claude) ;;
      *) continue ;;
    esac

    age_ms=$((now_ms - ts_ms))
    [ "$age_ms" -ge -300000 ] || continue
    [ "$age_ms" -le "$cache_max_age_ms" ] || continue

    if cached_agent_is_gone "$agent_pid" "$client" "$entry_host"; then
      continue
    fi
    printf '%s\n' "$line"
  done <<< "$CACHED_STATES"
}

emit_live_payload() {
  if valid_positive_number "$SCAN_STARTED_MS"; then
    jq -c --arg scan_started_ms "$SCAN_STARTED_MS" \
      '
        .ts_ms = ($scan_started_ms | tonumber)
        | if .rainbow_name == null then
            .
          else
            .rainbow_mode_ts_ms = ($scan_started_ms | tonumber)
          end
      '
  else
    cat
  fi
}

emit_cached_states

# Exact process discovery currently relies on Linux procfs. Other platforms
# use the bounded, liveness-validated hook cache above.
[ "$(uname)" = "Linux" ] || exit 0
[ -d "$PROC_ROOT" ] || exit 0

# Every claude/codex process of this session, as "pane_id<TAB>pid<TAB>client",
# ascending by pane then pid. printf is a builtin, so the globs never reach an
# exec: past ARG_MAX execve fails outright, grep never starts, and discovery
# returns nothing — silently, since the plugin discards the stderr bash writes.
# -s covers unreadable entries, not that. xargs chunks the exec, and no process
# is read individually: 20.3 s against 22 ms for 528 processes.
list_pane_processes() {
  local chunk=()
  # Chunk size, set by tests to force more than one xargs invocation at fixture
  # scale. Not harmless tuning: at 1 the walk becomes one exec per file, the
  # shape it exists to avoid.
  [ -z "${ZELLAUDE_PROC_SCAN_MAX_ARGS:-}" ] ||
    chunk=(-n "$ZELLAUDE_PROC_SCAN_MAX_ARGS")
  {
    printf '%s\0' "$PROC_ROOT"/*/comm |
      xargs -0 "${chunk[@]}" grep -sxH -e claude -e codex
    printf '%s\0' "$PROC_ROOT"/*/environ |
      xargs -0 "${chunk[@]}" grep -szH \
        -e '^ZELLIJ_SESSION_NAME=' -e '^ZELLIJ_PANE_ID=' |
      tr '\0' '\n'
  } | awk -v session="$SESSION_NAME" '
    {
      separator = index($0, ":")
      if (separator == 0) next
      path = substr($0, 1, separator - 1)
      record = substr($0, separator + 1)
      depth = split(path, components, "/")
      process_id = components[depth - 1]
      if (components[depth] == "comm") {
        client[process_id] = record
      } else if (record ~ /^ZELLIJ_SESSION_NAME=/) {
        process_session[process_id] = substr(record, 21)
      } else {
        pane[process_id] = substr(record, 16)
      }
    }
    END {
      for (process_id in client) {
        if (process_id !~ /^[0-9]+$/) continue
        if (process_session[process_id] != session) continue
        if (pane[process_id] == "") continue
        printf "%s\t%s\t%s\n", pane[process_id], process_id, client[process_id]
      }
    }
  ' | sort -k1,1n -k2,2n
}

process_matches_pane() {
  local process_id=$1 pane_id=$2 process_session process_pane
  process_session=$(proc_env_value "$process_id" ZELLIJ_SESSION_NAME)
  process_pane=$(proc_env_value "$process_id" ZELLIJ_PANE_ID)
  [ "$process_session" = "$SESSION_NAME" ] && [ "$process_pane" = "$pane_id" ]
}

discover_codex() {
  local pane_id=$1 process_id=$2 codex_home sessions_root process_cwd
  local fd_path fd_number transcript transcript_real position size metadata
  local session_id transcript_cwd transcript_cwd_real candidate_count candidate payload
  local -A candidate_seen=()

  [ "$(cat "$PROC_ROOT/$process_id/comm" 2>/dev/null)" = "codex" ] || return 1
  process_matches_pane "$process_id" "$pane_id" || return 1

  codex_home=$(proc_env_value "$process_id" CODEX_HOME)
  [ -n "$codex_home" ] || codex_home="$HOME/.codex"
  sessions_root=$(cd "$codex_home/sessions" 2>/dev/null && pwd -P) || return 1
  process_cwd=$(readlink -f "$PROC_ROOT/$process_id/cwd" 2>/dev/null) || return 1

  candidate_count=0
  candidate=""
  for fd_path in "$PROC_ROOT/$process_id"/fd/*; do
    [ -L "$fd_path" ] || continue
    transcript=$(readlink "$fd_path" 2>/dev/null) || continue
    transcript_real=$(readlink -f "$transcript" 2>/dev/null) || continue
    case "$transcript_real" in
      "$sessions_root"/*.jsonl) ;;
      *) continue ;;
    esac
    [ -z "${candidate_seen[$transcript_real]+present}" ] || continue

    fd_number=${fd_path##*/}
    position=$(
      sed -n 's/^pos:[[:space:]]*//p' \
        "$PROC_ROOT/$process_id/fdinfo/$fd_number" 2>/dev/null
    )
    size=$(stat -c '%s' "$transcript_real" 2>/dev/null) || continue
    [ "$position" = "$size" ] || continue

    metadata=$(
      head -n 16 "$transcript_real" 2>/dev/null |
        jq -Rnr '
          [
            inputs
            | fromjson?
            | select(.type == "session_meta")
            | .payload
          ]
          | .[0] // {}
          | select(.source == "cli")
          | [(.id // ""), (.cwd // "")]
          | @tsv
        ' 2>/dev/null
    )
    [ -n "$metadata" ] || continue
    IFS=$'\t' read -r session_id transcript_cwd <<< "$metadata"
    valid_session_id "$session_id" || continue
    transcript_cwd_real=$(readlink -f "$transcript_cwd" 2>/dev/null) || continue
    [ "$transcript_cwd_real" = "$process_cwd" ] || continue

    candidate_seen["$transcript_real"]=1
    candidate_count=$((candidate_count + 1))
    candidate=$transcript_real
    [ "$candidate_count" -le 1 ] || return 1
  done

  [ "$candidate_count" -eq 1 ] || return 1
  session_id=$(
    head -n 16 "$candidate" 2>/dev/null |
      jq -Rnr '
        [
          inputs
          | fromjson?
          | select(.type == "session_meta" and .payload.source == "cli")
          | .payload.id
        ]
        | .[0] // empty
      ' 2>/dev/null
  )
  valid_session_id "$session_id" || return 1

  payload=$(
    jq -nc \
      --arg session_id "$session_id" \
      --arg transcript_path "$candidate" \
      --arg cwd "$process_cwd" \
      '{
        session_id: $session_id,
        hook_event_name: "SessionRestore",
        transcript_path: $transcript_path,
        cwd: $cwd
      }' |
      ZELLIJ_SESSION_NAME="$SESSION_NAME" \
        ZELLIJ_PANE_ID="$pane_id" \
        "$HOOK_PATH" --client codex --inspect 2>/dev/null
  ) || return 1
  [ -n "$payload" ] || return 1
  printf '%s\n' "$payload" | emit_live_payload
}

claude_settings_mode() {
  printf '%s' "$1" | jq -Rr '
    fromjson?
    | select(type == "object" and has("ultracode"))
    | if .ultracode == true then
        "true"
      elif .ultracode == false then
        "false"
      else
        empty
      end
  ' 2>/dev/null
}

claude_command_line_mode() {
  local process_id=$1 argument pending value settings_mode mode
  pending=""
  mode=null

  while IFS= read -r -d '' argument; do
    if [ -n "$pending" ]; then
      case "$pending" in
        effort)
          value=$argument
          case "$value" in
            ultracode) mode=true ;;
            low|medium|high|xhigh|max|auto|unset) mode=false ;;
          esac
          ;;
        settings)
          settings_mode=$(claude_settings_mode "$argument")
          case "$settings_mode" in
            true|false) mode=$settings_mode ;;
          esac
          ;;
      esac
      pending=""
      continue
    fi

    case "$argument" in
      --effort)
        pending=effort
        ;;
      --effort=*)
        value=${argument#--effort=}
        case "$value" in
          ultracode) mode=true ;;
          low|medium|high|xhigh|max|auto|unset) mode=false ;;
        esac
        ;;
      --settings)
        pending=settings
        ;;
      --settings=*)
        settings_mode=$(claude_settings_mode "${argument#--settings=}")
        case "$settings_mode" in
          true|false) mode=$settings_mode ;;
        esac
        ;;
    esac
  done < "$PROC_ROOT/$process_id/cmdline"

  printf '%s\n' "$mode"
}

discover_claude() {
  local pane_id=$1 process_id=$2 claude_home registry proc_start registry_data
  local session_id registry_cwd process_cwd transcript transcript_count candidate
  local explicit_mode effort_level requested_mode started_at payload

  [ "$(cat "$PROC_ROOT/$process_id/comm" 2>/dev/null)" = "claude" ] || return 1
  process_matches_pane "$process_id" "$pane_id" || return 1

  claude_home=$(proc_env_value "$process_id" CLAUDE_CONFIG_DIR)
  [ -n "$claude_home" ] || claude_home="$HOME/.claude"
  registry="$claude_home/sessions/$process_id.json"
  [ -f "$registry" ] || return 1

  proc_start=$(proc_stat_value "$process_id" 20)
  valid_positive_number "$proc_start" || return 1
  registry_data=$(
    jq -r \
      --argjson process_id "$process_id" \
      --arg proc_start "$proc_start" '
        select(
          .pid == $process_id
          and ((.procStart // "") | tostring) == $proc_start
          and .kind == "interactive"
          and .entrypoint == "cli"
        )
        | [(.sessionId // ""), (.cwd // ""), ((.startedAt // 0) | tostring)]
        | @tsv
      ' "$registry" 2>/dev/null
  )
  [ -n "$registry_data" ] || return 1
  IFS=$'\t' read -r session_id registry_cwd started_at <<< "$registry_data"
  valid_session_id "$session_id" || return 1

  process_cwd=$(readlink -f "$PROC_ROOT/$process_id/cwd" 2>/dev/null) || return 1
  registry_cwd=$(readlink -f "$registry_cwd" 2>/dev/null) || return 1
  [ "$registry_cwd" = "$process_cwd" ] || return 1

  transcript_count=0
  transcript=""
  for candidate in "$claude_home"/projects/*/"$session_id.jsonl"; do
    [ -f "$candidate" ] || continue
    transcript_count=$((transcript_count + 1))
    transcript=$candidate
  done
  [ "$transcript_count" -eq 1 ] || return 1

  explicit_mode=null
  requested_mode=$(proc_env_value "$process_id" ZELLAUDE_CLAUDE_MODE)
  case "$requested_mode" in
    ultracode)
      explicit_mode=true
      ;;
    *)
      explicit_mode=$(claude_command_line_mode "$process_id")
      ;;
  esac
  effort_level=$(proc_env_value "$process_id" CLAUDE_CODE_EFFORT_LEVEL)

  payload=$(
    jq -nc \
      --arg session_id "$session_id" \
      --arg transcript_path "$transcript" \
      --arg cwd "$process_cwd" \
      --argjson launch_ultracode "$explicit_mode" \
      --arg launch_effort_level "$effort_level" \
      --arg started_at "$started_at" \
      '{
        session_id: $session_id,
        hook_event_name: "SessionRestore",
        transcript_path: $transcript_path,
        cwd: $cwd,
        launch_ultracode: $launch_ultracode,
        session_started_at_ms: (
          if ($started_at | test("^[1-9][0-9]*$"))
          then ($started_at | tonumber)
          else null
          end
        ),
        launch_effort_level: (
          if $launch_effort_level == ""
          then null
          else $launch_effort_level
          end
        )
      }
      | with_entries(select(.value != null))' |
      env -u CLAUDE_EFFORT \
        -u CLAUDE_CODE_EFFORT_LEVEL \
        -u ZELLAUDE_CLAUDE_MODE \
        ZELLIJ_SESSION_NAME="$SESSION_NAME" \
        ZELLIJ_PANE_ID="$pane_id" \
        "$HOOK_PATH" --inspect 2>/dev/null
  ) || return 1
  [ -n "$payload" ] || return 1
  printf '%s\n' "$payload" | emit_live_payload
}

PANE_PROCESSES=$(list_pane_processes)

# A pane is claimed by the lowest-pid candidate that discovers a session; later
# members of the same set are skipped whether or not they would discover too.
declare -A pane_discovered=()
while IFS=$'\t' read -r pane_id process_id client; do
  valid_nonnegative_number "$pane_id" || continue
  valid_positive_number "$process_id" || continue
  [ -z "${pane_discovered[$pane_id]+present}" ] || continue

  case "$client" in
    codex) discover_codex "$pane_id" "$process_id" || continue ;;
    claude) discover_claude "$pane_id" "$process_id" || continue ;;
    *) continue ;;
  esac
  pane_discovered["$pane_id"]=1
done <<< "$PANE_PROCESSES"
