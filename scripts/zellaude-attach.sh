#!/usr/bin/env bash
# Discover agent sessions that were already running when Zellaude attached.
# Arguments:
#   $1 — Zellij session name
#   $2 — comma-separated pane_id:leader_pid:client records
#   $3 — attach scan start time in milliseconds

SESSION_NAME=${1:-}
PANE_RECORDS=${2:-}
SCAN_STARTED_MS=${3:-}
HOOK_PATH=${ZELLAUDE_ATTACH_HOOK:-"$HOME/.config/zellij/plugins/zellaude-hook.sh"}
PROC_ROOT=${ZELLAUDE_PROC_ROOT:-/proc}

[ -n "$SESSION_NAME" ] || exit 0
[ -x "$HOOK_PATH" ] || exit 0

# Cached hook state preserves launch-only mode choices that cannot always be
# reconstructed from a transcript. It is validated and emitted below.
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

foreground_pid() {
  # /proc/<pid>/stat field 8 is tpgid; after removing pid+comm it is field 6.
  proc_stat_value "$1" 6
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

record_client_for_pane() {
  local wanted_pane=$1 remaining record record_tail pane_id client
  remaining=$PANE_RECORDS
  while [ -n "$remaining" ]; do
    case "$remaining" in
      *,*)
        record=${remaining%%,*}
        remaining=${remaining#*,}
        ;;
      *)
        record=$remaining
        remaining=""
        ;;
    esac
    pane_id=${record%%:*}
    record_tail=${record#*:}
    client=${record_tail#*:}
    if [ "$pane_id" = "$wanted_pane" ]; then
      printf '%s\n' "$client"
      return 0
    fi
  done
  return 1
}

emit_cached_states() {
  local now_ms cache_max_age_ms line metadata pane_id client ts_ms
  local record_client age_ms
  now_ms=$(jq -nr 'now * 1000 | floor' 2>/dev/null) || return 0
  valid_positive_number "$now_ms" || return 0
  cache_max_age_ms=43200000

  while IFS= read -r line; do
    [ -n "$line" ] || continue
    metadata=$(printf '%s\n' "$line" | jq -r '
      [(.pane_id // ""), (.client // ""), (.ts_ms // "")]
      | @tsv
    ' 2>/dev/null) || continue
    IFS=$'\t' read -r pane_id client ts_ms <<< "$metadata"
    valid_nonnegative_number "$pane_id" || continue
    valid_positive_number "$ts_ms" || continue
    case "$client" in
      codex|claude) ;;
      *) continue ;;
    esac

    age_ms=$((now_ms - ts_ms))
    [ "$age_ms" -ge -300000 ] || continue
    [ "$age_ms" -le "$cache_max_age_ms" ] || continue

    if [ -n "$PANE_RECORDS" ]; then
      record_client=$(record_client_for_pane "$pane_id") || continue
      [ "$record_client" = "$client" ] || continue
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
# use the bounded, command-matched hook cache above.
[ "$(uname)" = "Linux" ] || exit 0
[ -d "$PROC_ROOT" ] || exit 0

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

remaining=$PANE_RECORDS
while [ -n "$remaining" ]; do
  case "$remaining" in
    *,*)
      record=${remaining%%,*}
      remaining=${remaining#*,}
      ;;
    *)
      record=$remaining
      remaining=""
      ;;
  esac

  pane_id=${record%%:*}
  record_tail=${record#*:}
  leader_pid=${record_tail%%:*}
  client=${record_tail#*:}
  valid_nonnegative_number "$pane_id" || continue
  valid_positive_number "$leader_pid" || continue
  case "$client" in
    codex|claude|unknown) ;;
    *) continue ;;
  esac

  process_id=$(foreground_pid "$leader_pid")
  valid_positive_number "$process_id" || continue
  depth=0
  while valid_positive_number "$process_id" && [ "$depth" -lt 64 ]; do
    process_name=$(cat "$PROC_ROOT/$process_id/comm" 2>/dev/null || true)
    case "$process_name" in
      codex)
        discover_codex "$pane_id" "$process_id" && break
        ;;
      claude)
        discover_claude "$pane_id" "$process_id" && break
        ;;
    esac

    [ "$process_id" = "$leader_pid" ] && break
    parent_id=$(proc_stat_value "$process_id" 2)
    valid_positive_number "$parent_id" || break
    [ "$parent_id" != "$process_id" ] || break
    process_id=$parent_id
    depth=$((depth + 1))
  done
done
