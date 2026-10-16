#!/usr/bin/env bash
# zellaude-hook.sh — agent hook → zellij action pipe bridge
# Forwards Claude Code and Codex hook events to the zellaude Zellij plugin.
#
# Usage:
#   Claude Code: "/path/to/zellaude-hook.sh"
#   Codex:       "/path/to/zellaude-hook.sh --client codex"

# Prints "<owner uid> <octal permission bits>" from one stat.
path_owner_and_mode() {
  stat -c '%u %a' "$1" 2>/dev/null ||
    stat -f '%u %Lp' "$1" 2>/dev/null
}

is_owned_private_parent() {
  local path=$1 expected_owner=$2 owner_and_mode mode
  [ -d "$path" ] && [ ! -L "$path" ] || return 1
  owner_and_mode=$(path_owner_and_mode "$path") || return 1
  [ "${owner_and_mode%% *}" = "$expected_owner" ] || return 1
  mode=${owner_and_mode#* }
  case "$mode" in
    ''|*[!0-7]*) return 1 ;;
  esac
  while [ "${#mode}" -gt 3 ]; do
    mode=${mode#?}
  done
  [ $((8#$mode & 8#022)) -eq 0 ]
}

ensure_private_cache_dir() {
  local path=$1 expected_owner=$2 owner_and_mode
  if [ -e "$path" ] || [ -L "$path" ]; then
    [ -d "$path" ] && [ ! -L "$path" ] || return 1
  else
    (umask 077 && mkdir "$path") 2>/dev/null || return 1
  fi

  owner_and_mode=$(path_owner_and_mode "$path") || return 1
  [ "$owner_and_mode" = "$expected_owner 700" ] && return 0
  [ "${owner_and_mode%% *}" = "$expected_owner" ] || return 1
  chmod 700 "$path" 2>/dev/null || return 1
  is_owned_private_parent "$path" "$expected_owner"
}

state_cache_dir() {
  local runtime_root cache_root cache_dir user_id home_cache
  user_id=$EUID

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
      # Attach-time consumers read the cache files directly; keeping launch_env
      # out of this transit path costs them nothing.
      | del(.launch_env)
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

SETTINGS_FILE="${HOME:-}/.config/zellij/plugins/zellaude.json"
# By path, not argv (128 KiB cap). On a path it cannot open, jq never reads stdin.
if [ ! -f "$SETTINGS_FILE" ]; then
  SETTINGS_JQ_ARGS=(--arg settings_json '{}')
elif [ -r "$SETTINGS_FILE" ]; then
  SETTINGS_JQ_ARGS=(--rawfile settings_json "$SETTINGS_FILE")
else
  SETTINGS_JQ_ARGS=(--arg settings_json "")
fi

# Reads the hook input on stdin; prints shell assignments for eval.
parse_hook_input() {
  jq -r \
    --arg claude_effort "${CLAUDE_EFFORT:-}" \
    --arg configured_effort "${CLAUDE_CODE_EFFORT_LEVEL:-}" \
    "${SETTINGS_JQ_ARGS[@]}" '
      def assign($name; value):
        "\($name)=\(value // "" | if type == "string" then . else tojson end | @sh)";
      def lowercase_text: if type == "string" then ascii_downcase else "" end;
      def as_state:
        if . == true
          or ((type == "string") and ((ascii_downcase == "true") or (ascii_downcase == "ultracode")))
        then "true"
        else "false"
        end;

      assign("HOOK_EVENT"; .hook_event_name),
      # Send time: the plugin orders racing hook subprocesses by it.
      assign("TS_MS"; now * 1000 | floor),
      assign("SESSION_ID"; .session_id),
      assign("TOOL_NAME"; .tool_name),
      assign("CWD"; .cwd),
      assign("TRANSCRIPT_PATH"; .transcript_path),
      assign("TURN_ID"; .turn_id),
      assign("AGENT_ID"; .agent_id),
      # Not every event carries .effort; CLAUDE_EFFORT covers SessionStart.
      assign("EFFORT_LEVEL";
        (.effort.level? // "" | lowercase_text) as $level
        | if $level != "" then $level else $claude_effort | ascii_downcase end),
      assign("CONFIGURED_EFFORT_LEVEL"; $configured_effort | ascii_downcase),
      # For a future Claude hook field, or an SDK host that sends one today.
      assign("EXPLICIT_RAINBOW_STATE";
        if has("ultracode") and .ultracode != null then
          .ultracode | as_state
        elif ((.effort? | type) == "object")
          and (.effort | has("ultracode"))
          and .effort.ultracode != null then
          .effort.ultracode | as_state
        elif (.effort.level? // "" | lowercase_text) == "ultracode" then
          "true"
        else
          ""
        end),
      # Prefer a direct field if a future Codex hook schema adds one.
      assign("CODEX_DIRECT_EFFORT";
        [
          .reasoning_effort?,
          .model_reasoning_effort?,
          (if (.effort? | type) == "object" then .effort.level? else .effort? end)
        ]
        | map(select((type == "string") and length > 0))
        | first // ""
        | ascii_downcase),
      assign("RESTORE_LAUNCH_STATE";
        if has("launch_ultracode") and .launch_ultracode != null then
          .launch_ultracode | as_state
        else
          (.launch_effort_level? // "" | lowercase_text) as $effort
          | if $effort == "ultracode" then
              "true"
            elif ($effort | IN("low", "medium", "high", "xhigh", "max", "auto", "unset")) then
              "false"
            else
              ""
            end
        end),
      assign("RESTORE_STARTED_AT_MS";
        .session_started_at_ms?
        | select(type == "number" and . > 0 and floor == .)),
      assign("RAINBOW_ENABLED";
        if (try ($settings_json | fromjson | .rainbow) catch null) == false
        then "false"
        else "true"
        end),
      assign("NOTIFY_MODE";
        try ($settings_json | fromjson | .notifications // "Always") catch "")
    ' 2>/dev/null
}

# Read hook JSON from stdin before checking Zellij. Codex Stop hooks require
# a JSON response even when there is no pane event to forward.
eval "$(parse_hook_input)"

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

# Not PROC_ROOT: the launch-env harness plants only <pid>/environ there.
TREE_PROC_ROOT=${ZELLAUDE_TREE_PROC_ROOT:-/proc}
TREE_HAS_PROCFS=false
[ -r "$TREE_PROC_ROOT/$PPID/stat" ] && TREE_HAS_PROCFS=true

# Basenamed: macOS ps prints claude's comm as a full path, fish's bare.
is_agent_comm() {
  case "${1##*/}" in
    claude*|codex*) return 0 ;;
  esac
  return 1
}

# read_process_<fact> PID VAR sets VAR: procfs builtins, else one ps.
read_process_comm() {
  local process_comm=""
  if [ "$TREE_HAS_PROCFS" = true ]; then
    read -r process_comm 2>/dev/null < "$TREE_PROC_ROOT/$1/comm"
  else
    # comm, never ucomm: ucomm holds the rewritten process title.
    process_comm=$(ps -o comm= -p "$1" 2>/dev/null | tr -d '[:space:]')
  fi
  printf -v "$2" '%s' "$process_comm"
}

# Joined by spaces, as `ps -o args=` prints it.
read_process_args() {
  local joined="" arg
  if [ "$TREE_HAS_PROCFS" = true ]; then
    while IFS= read -r -d '' arg || [ -n "$arg" ]; do
      joined="$joined${joined:+ }$arg"
    done 2>/dev/null < "$TREE_PROC_ROOT/$1/cmdline"
  else
    joined=$(ps -o args= -p "$1" 2>/dev/null)
  fi
  printf -v "$2" '%s' "$joined"
}

read_process_ppid() {
  local parent=""
  if [ "$TREE_HAS_PROCFS" = true ]; then
    read -r parent 2>/dev/null < "$TREE_PROC_ROOT/$1/stat"
    # comm sits in parens and may itself hold spaces or parens.
    parent=${parent##*) }
    parent=${parent#* }
    parent=${parent%% *}
  else
    parent=$(ps -o ppid= -p "$1" 2>/dev/null | tr -d '[:space:]')
  fi
  printf -v "$2" '%s' "$parent"
}

# The pane record's agent pid; never on SessionStart: a resume reuses the id.
memo_agent_pid() {
  local record memo_pid memo_comm
  local session_pattern='"session_id":"([^"\]*)"'
  local host_pattern='"host":"([^"\]*)"'
  local agent_pid_pattern='"agent_pid":([0-9]+)'
  [ "$HOOK_EVENT" != "SessionStart" ] && [ -n "$PANE_STATE_PATH" ] || return 1
  IFS= read -r record 2>/dev/null < "$PANE_STATE_PATH" || return 1
  [[ $record =~ $session_pattern ]] &&
    [ "${BASH_REMATCH[1]}" = "$SESSION_ID" ] || return 1
  [[ $record =~ $host_pattern ]] &&
    [ "${BASH_REMATCH[1]}" = "$AGENT_HOST" ] || return 1
  [[ $record =~ $agent_pid_pattern ]] || return 1
  memo_pid=${BASH_REMATCH[1]}
  if [ "$TREE_HAS_PROCFS" = true ]; then
    read_process_comm "$memo_pid" memo_comm
    is_agent_comm "$memo_comm" || return 1
  fi
  printf '%s\n' "$memo_pid"
}

# Climbs a `ps -A` table by the same rule as is_agent_comm.
find_agent_pid_in_ps_table() {
  awk -v start="$PPID" '
    {
      comm = $0
      sub(/^[ \t]*[0-9]+[ \t]+[0-9]+[ \t]+/, "", comm)
      sub(/.*\//, "", comm)
      parent[$1] = $2
      name[$1] = comm
    }
    END {
      for (pid = start; pid > 1 && (pid in parent); pid = parent[pid])
        if (name[pid] ~ /^(claude|codex)/) { print pid; exit 0 }
      exit 1
    }
  ' <<< "$1"
}

climb_to_agent_pid() {
  local pid=$PPID comm
  while [ "$pid" -gt 1 ] 2>/dev/null; do
    read_process_comm "$pid" comm
    if is_agent_comm "$comm"; then
      printf '%s\n' "$pid"
      return 0
    fi
    read_process_ppid "$pid" pid
  done
  return 1
}

# The agent process that fired this hook: the nearest claude/codex ancestor
# (hooks run as descendants of the client). Recorded so tools reading the
# state cache (zellmv) get exact kill/inspect targets instead of inferring
# them from argv, which process-title rewrites make unreliable. The host
# matters on shared homes, where every pod sees this cache.
find_agent_pid() {
  local process_table
  memo_agent_pid && return 0
  # Empty means ps failed: a working one lists init, the agent and this hook.
  if [ "$TREE_HAS_PROCFS" = false ] &&
     process_table=$(ps -A -o pid= -o ppid= -o comm= 2>/dev/null) &&
     [ -n "$process_table" ]; then
    find_agent_pid_in_ps_table "$process_table"
    return
  fi
  climb_to_agent_pid
}

# Launch-time knobs a relaunch must reproduce: backend, auth, model, agent
# behavior. Exact names, one list for both clients — no prefix matching and no
# denylist, so a variable the client injects is excluded by not appearing here.
# In order: claude endpoint and provider, claude identity and config home,
# claude model and limits, zellaude, codex.
LAUNCH_ENV_CONFIG_NAMES="ANTHROPIC_BASE_URL \
CLAUDE_CODE_USE_BEDROCK ANTHROPIC_BEDROCK_BASE_URL \
ANTHROPIC_BEDROCK_REGION_PREFIX ANTHROPIC_SMALL_FAST_MODEL_AWS_REGION \
CLAUDE_CODE_SKIP_BEDROCK_AUTH \
CLAUDE_CODE_USE_MANTLE CLAUDE_CODE_SKIP_MANTLE_AUTH \
CLAUDE_CODE_USE_VERTEX ANTHROPIC_VERTEX_BASE_URL ANTHROPIC_VERTEX_PROJECT_ID \
CLAUDE_CODE_SKIP_VERTEX_AUTH \
CLAUDE_CODE_USE_FOUNDRY ANTHROPIC_FOUNDRY_BASE_URL ANTHROPIC_FOUNDRY_RESOURCE \
CLAUDE_CODE_SKIP_FOUNDRY_AUTH \
CLAUDE_CODE_USE_ANTHROPIC_AWS ANTHROPIC_AWS_BASE_URL \
ANTHROPIC_AWS_WORKSPACE_ID CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH \
ANTHROPIC_PROFILE ANTHROPIC_FEDERATION_RULE_ID ANTHROPIC_ORGANIZATION_ID \
ANTHROPIC_WORKSPACE_ID CLAUDE_CODE_OAUTH_SCOPES \
CLAUDE_CONFIG_DIR CLAUDE_CODE_PROJECT_DIR_NAME \
ANTHROPIC_MODEL ANTHROPIC_DEFAULT_MODEL ANTHROPIC_DEFAULT_OPUS_MODEL \
ANTHROPIC_DEFAULT_SONNET_MODEL ANTHROPIC_DEFAULT_HAIKU_MODEL \
ANTHROPIC_DEFAULT_FABLE_MODEL ANTHROPIC_SMALL_FAST_MODEL \
ANTHROPIC_CUSTOM_MODEL_OPTION CLAUDE_CODE_SUBAGENT_MODEL \
CLAUDE_CODE_SUBAGENT_MODEL_FORCE CLAUDE_CODE_EFFORT_LEVEL \
CLAUDE_CODE_MAX_CONTEXT_TOKENS CLAUDE_CODE_MAX_OUTPUT_TOKENS \
CLAUDE_CODE_AUTO_COMPACT_WINDOW CLAUDE_CODE_DISABLE_1M_CONTEXT \
ZELLAUDE_CLAUDE_MODE \
CODEX_HOME CODEX_SQLITE_HOME CODEX_CA_CERTIFICATE \
CODEX_OSS_BASE_URL CODEX_OSS_PORT OPENAI_BASE_URL \
OPENAI_ORGANIZATION OPENAI_PROJECT OPENAI_FEDERATION_RULE_ID \
OPENAI_IDENTITY_TOKEN_FILE OPENAI_WORKLOAD_IDENTITY_CONTEXT"
# ANTHROPIC_SMALL_FAST_MODEL: deprecated upstream in favour of
# ANTHROPIC_DEFAULT_HAIKU_MODEL; claude 2.1.258 still reads both.
# CODEX_SQLITE_HOME: single-source, never confirmed against a running codex.
# OPENAI_BASE_URL: codex ignores it today (openai/codex#16719). Captured anyway
# — a replay reproduces sameness, not effectiveness, so the relaunched session
# must ignore it too.
# OPENAI_ORGANIZATION, OPENAI_PROJECT, CODEX_OSS_*: read by codex source
# (model-provider-info, `--oss`), absent from its env-var docs.
LAUNCH_ENV_SECRET_NAMES="ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY \
CLAUDE_CODE_OAUTH_TOKEN CLAUDE_CODE_OAUTH_REFRESH_TOKEN \
ANTHROPIC_FOUNDRY_API_KEY ANTHROPIC_FOUNDRY_AUTH_TOKEN ANTHROPIC_AWS_API_KEY \
CODEX_API_KEY CODEX_ACCESS_TOKEN OPENAI_API_KEY"
# OPENAI_API_KEY: current codex reads it only through a provider env_key or
# `codex login --with-api-key`; kept for older releases and custom providers.
# A literal set, never a pattern: a pattern would eventually match a real
# credential. It lets local-proxy sessions replay without a re-source contract.
LAUNCH_ENV_SAFE_SECRET_VALUE="local"
LAUNCH_ENV_REDACTED_MARKER="<set>"

PROC_ROOT=${ZELLAUDE_PROC_ROOT:-/proc}

# Emits the launch_env object, or nothing when the environ cannot be read
# (no /proc, refused read, no agent pid). Never falls back to this hook's own
# environment: under --inspect that belongs to zellaude-attach.sh, not the agent.
# jq splits the raw NUL-separated blob so a value containing a newline survives.
read_launch_env() {
  local agent_pid=$1 environ_path
  [ -n "$agent_pid" ] || return 1
  environ_path="$PROC_ROOT/$agent_pid/environ"
  [ -r "$environ_path" ] || return 1

  jq -Rsc \
    --arg config_names "$LAUNCH_ENV_CONFIG_NAMES" \
    --arg secret_names "$LAUNCH_ENV_SECRET_NAMES" \
    "${SETTINGS_JQ_ARGS[@]}" \
    --arg safe_secret_value "$LAUNCH_ENV_SAFE_SECRET_VALUE" \
    --arg redacted_marker "$LAUNCH_ENV_REDACTED_MARKER" '
      # No apostrophes in these comments: one would end the shell string.

      # Built-ins plus user additions; a missing or malformed file adds none.
      def launch_env_names:
        ($settings_json | fromjson? // {}) as $user
        | ($config_names | split(" ")) as $config
        | ($secret_names | split(" ")) as $secret
        | ($config + $secret) as $predefined
        # "verbatim" is the config tier: a name added there keeps its value.
        | def added($tier):
            (try ($user.launch_env_names[$tier]) catch null)
            | if type == "array"
              then map(select(type == "string" and . != ""))
              else []
              end;
          # Only additions: a predefined name re-tiered to secret is dropped.
          ((added("secret") - $predefined) | unique) as $user_secret
        # A name listed in both tiers is a secret: fail closed.
        | ((added("verbatim") - $predefined - $user_secret) | unique) as $user_config
        | {
            config: ($config + $user_config),
            secret: ($secret + $user_secret)
          };

      launch_env_names as $names
      | $names.config as $config
      | $names.secret as $secret
      | [ split("\u0000")[]
          | (index("=")) as $eq
          | select($eq != null)
          | { name: .[:$eq], value: .[$eq + 1:] }
          | select(.name | IN($config[], $secret[]))
          | {
              key: .name,
              value: (
                if (.name | IN($secret[])) and .value != $safe_secret_value
                then $redacted_marker
                else .value
                end
              )
            }
        ]
      | from_entries
    ' "$environ_path" 2>/dev/null
}

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
  local transcript_effort

  if [ -n "$CODEX_DIRECT_EFFORT" ]; then
    [ "$CODEX_DIRECT_EFFORT" = "ultra" ] && printf 'true' || printf 'false'
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
    read_process_args "$process_id" process_args
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

    read_process_ppid "$process_id" parent_id
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
  local transcript_result transcript_state transcript_marker
  local transcript_timestamp transcript_timestamp_ms launch_mode tail_lines

  if [ -n "$EXPLICIT_RAINBOW_STATE" ]; then
    printf '%s\t' "$EXPLICIT_RAINBOW_STATE"
    return
  fi

  # /effort cannot change mid-turn: only turn boundaries and recovery scan.
  case "$HOOK_EVENT" in
    SessionStart) tail_lines=4096 ;;
    UserPromptSubmit) tail_lines=1024 ;;
    SessionRestore) tail_lines=512 ;;
    *) tail_lines="" ;;
  esac
  # Claude reports ultracode as xhigh, so the transcript's /effort recovers it.
  transcript_result=$'null\t\t'
  if [ -n "$tail_lines" ] && [ -r "$TRANSCRIPT_PATH" ]; then
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
    transcript_timestamp_ms=""
    if [ -n "$transcript_timestamp" ]; then
      transcript_timestamp_ms=$(iso_timestamp_ms "$transcript_timestamp")
    fi

    if [ -n "$RESTORE_LAUNCH_STATE" ]; then
      if [ "$transcript_state" != "null" ] &&
         [ -n "$RESTORE_STARTED_AT_MS" ] &&
         [ -n "$transcript_timestamp_ms" ] &&
         [ "$transcript_timestamp_ms" -ge "$RESTORE_STARTED_AT_MS" ]; then
        printf '%s\t%s' "$transcript_state" "$transcript_marker"
      else
        printf '%s\t%s' "$RESTORE_LAUNCH_STATE" "$transcript_marker"
      fi
      return
    fi
  fi

  case "$CONFIGURED_EFFORT_LEVEL" in
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

  case "$EFFORT_LEVEL" in
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

  case "$EFFORT_LEVEL" in
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
elif [ "$RAINBOW_ENABLED" = false ]; then
  # null is "not looked", so the cache keeps its last value instead of false.
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

# Prefix of this pane's cache files: the state record and the stream markers.
PANE_CACHE_PREFIX=""
PANE_STATE_PATH=""
if [ "$INSPECT_ONLY" = false ] &&
   STATE_CACHE_DIR=$(state_cache_dir) &&
   STATE_CACHE_KEY=$(state_cache_key "$ZELLIJ_SESSION_NAME"); then
  PANE_CACHE_PREFIX="$STATE_CACHE_DIR/$STATE_CACHE_KEY.$ZELLIJ_PANE_ID"
  if [ "$IS_SUBAGENT" = false ] && [ -n "$SESSION_ID" ]; then
    PANE_STATE_PATH="$PANE_CACHE_PREFIX.json"
  fi
fi
AGENT_HOST=$(hostname 2>/dev/null) || AGENT_HOST=""
AGENT_PID=$(find_agent_pid) || AGENT_PID=""

if [ "$INSPECT_ONLY" = true ]; then
  # --inspect runs outside the agent's process tree: the environment here is
  # zellaude-attach.sh's, not the agent's, and substituting it would be a lie.
  LAUNCH_ENV=null
else
  LAUNCH_ENV=$(read_launch_env "$AGENT_PID") || LAUNCH_ENV=null
fi
if [ "$CLIENT" = "codex" ]; then
  # Under codex, EFFORT_LEVEL may be a Claude session's inherited CLAUDE_EFFORT.
  CURRENT_EFFORT_LEVEL=""
else
  CURRENT_EFFORT_LEVEL=$EFFORT_LEVEL
fi

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
  --argjson launch_env "$LAUNCH_ENV" \
  --arg current_effort_level "$CURRENT_EFFORT_LEVEL" \
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
    ),
    launch_env: $launch_env,
    current_effort_level: (
      if $current_effort_level == ""
      then null
      else $current_effort_level
      end
    )
  }')

persist_root_state() {
  local cache_path lock_dir cache_tmp cache_payload
  local merged_payload cached_owner cached_ts
  [ -n "$PANE_STATE_PATH" ] || return 0

  cache_path=$PANE_STATE_PATH
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
      # ts first: tab is IFS whitespace, so an empty first field would vanish.
      IFS=$'\t' read -r cached_ts cached_owner <<< "$(
        jq -r '[.ts_ms // 0, .session_id // ""] | @tsv' "$cache_path" 2>/dev/null
      )"
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

  # Empty: the cache is newer. A record jq cannot read fails, and is replaced.
  if [ -f "$cache_path" ] &&
     merged_payload=$(
      jq -nc --argjson current "$PAYLOAD" '
        input as $previous
        | ($previous.ts_ms // 0 | tostring) as $cached_ts
        | if ($cached_ts | test("^[0-9]+$"))
            and ($cached_ts | tonumber) > $current.ts_ms
          then empty
          elif (
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
        # These two keep $previous on any event, SessionStart included: a
        # SessionStart is the authoritative new signal for rainbow_name, but an
        # environ is fixed at exec, so within one session a null is only ever a
        # failed read — never a newer, better answer.
        | if $previous.session_id == $current.session_id then
            .launch_env = (
              if $current.launch_env == null
              then $previous.launch_env
              else $current.launch_env
              end
            )
            | .current_effort_level = (
                if $current.current_effort_level == null
                then $previous.current_effort_level
                else $current.current_effort_level
                end
              )
          else
            .
          end
      ' "$cache_path" 2>/dev/null
    ); then
    if [ -z "$merged_payload" ]; then
      release_cache_lock "$lock_dir"
      return 0
    fi
    cache_payload=$merged_payload
  fi

  cache_tmp=$(mktemp "$STATE_CACHE_DIR/.state.XXXXXX" 2>/dev/null) || {
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

DELIVERY_COALESCE_SECONDS=${ZELLAUDE_DELIVERY_COALESCE_SECONDS:-0.2}
DELIVERY_REAPER_SECONDS=${ZELLAUDE_DELIVERY_REAPER_SECONDS:-5}

# True only on connected_clients 0, so missing or odd metadata means deliver.
session_is_clientless() {
  local zellij_cache metadata newline=$'\n'
  case "$OSTYPE" in
    darwin*) zellij_cache="${HOME:-}/Library/Caches/org.Zellij-Contributors.Zellij" ;;
    *) zellij_cache="${XDG_CACHE_HOME:-${HOME:-}/.cache}/zellij" ;;
  esac
  IFS= read -r -d '' metadata 2>/dev/null \
    < "$zellij_cache/contract_version_1/session_info/$ZELLIJ_SESSION_NAME/session-metadata.kdl"
  case "$metadata" in
    *"${newline}connected_clients 0${newline}"*) return 0 ;;
  esac
  return 1
}

# Reaped after DELIVERY_REAPER_SECONDS: a dying session never answers, and
# macOS has no timeout(1).
pipe_payload() {
  local client_pid reaper_pid
  zellij action pipe --name zellaude -- "$PAYLOAD" &
  client_pid=$!
  # Its own process group, so one kill also takes its sleep.
  set -m
  ( sleep "$DELIVERY_REAPER_SECONDS"; kill "$client_pid" 2>/dev/null ) &
  reaper_pid=$!
  set +m
  wait "$client_pid"
  kill -- "-$reaper_pid" 2>/dev/null
}

# A token written just before another deliverer's claim is found in the
# claimed marker.
is_stream_newest() {
  local newest
  if [ -e "$STREAM_PENDING" ]; then
    read -r newest 2>/dev/null < "$STREAM_PENDING"
  else
    read -r newest 2>/dev/null < "$STREAM_CLAIMED"
  fi
  [ "$newest" = "$1" ]
}

# Whether token $1 still has something to pipe. SessionEnd always does; a
# Notification only while its stream has no event pending.
is_event_live() {
  case "$HOOK_EVENT" in
    SessionEnd) return 0 ;;
    Notification) [ ! -e "$STREAM_PENDING" ] ;;
    *) is_stream_newest "$1" ;;
  esac
}

claim_stream_event() {
  case "$HOOK_EVENT" in
    SessionEnd|Notification) return 0 ;;
  esac
  is_stream_newest "$1" || return 1
  [ ! -e "$STREAM_PENDING" ] || mv -f "$STREAM_PENDING" "$STREAM_CLAIMED" 2>/dev/null
  is_stream_newest "$1"
}

# The session pipe lock holds "<epoch seconds> <token>". The clock is
# TS_MS plus $SECONDS: fork-free, bash 3.2, 1 s resolution.
take_pipe_lock() {
  local taken
  set -C
  { printf '%s %s\n' "$((TS_MS / 1000 + SECONDS))" "$1" > "$PIPE_LOCK"; } 2>/dev/null
  taken=$?
  set +C
  return "$taken"
}

# Stale once older than a pipe can run, judged by age: pids get reused.
break_stale_pipe_lock() {
  local taken_at="" holder max_age=$((DELIVERY_REAPER_SECONDS + 2))
  if [ ! -e "$PIPE_LOCK" ]; then
    UNREADABLE_LOCK_SINCE=""
    return 0
  fi
  read -r taken_at holder 2>/dev/null < "$PIPE_LOCK"
  case "$taken_at" in
    ''|*[!0-9]*)
      # Mid-write, or its writer died there: aged by how long it reads so.
      UNREADABLE_LOCK_SINCE=${UNREADABLE_LOCK_SINCE:-$SECONDS}
      [ $((SECONDS - UNREADABLE_LOCK_SINCE)) -gt "$max_age" ] || return 0
      ;;
    *)
      UNREADABLE_LOCK_SINCE=""
      [ $((TS_MS / 1000 + SECONDS - taken_at)) -gt "$max_age" ] || return 0
      ;;
  esac
  rm -f "$PIPE_LOCK"
  UNREADABLE_LOCK_SINCE=""
}

# False while an urgent event of the session waits ahead of token $1: any,
# for a coalesced event; an older one, for an urgent event.
is_my_turn() {
  local marker other other_ms max_age=$((4 * DELIVERY_REAPER_SECONDS + 2))
  for marker in "$URGENT_PREFIX"*; do
    [ -e "$marker" ] || continue
    other=${marker#"$URGENT_PREFIX"}
    other_ms=${other%%.*}
    case "$other_ms" in
      ''|*[!0-9]*) continue ;;
    esac
    [ "$other" != "$1" ] || continue
    # Outlived any urgent deliverer: its owner is gone.
    if [ $((TS_MS / 1000 + SECONDS - other_ms / 1000)) -gt "$max_age" ]; then
      rm -f "$marker"
      continue
    fi
    [ "$IS_URGENT_EVENT" = true ] || return 1
    if [ "$other_ms" -lt "${1%%.*}" ] ||
       { [ "$other_ms" -eq "${1%%.*}" ] && [ "${other#*.}" -lt "${1#*.}" ]; }; then
      return 1
    fi
  done
  return 0
}

end_turn() {
  local token=$1 holding=$2 taken_at holder done_files=()
  [ "$IS_URGENT_EVENT" = false ] || done_files+=("$URGENT_PREFIX$token")
  if [ "$holding" = true ] &&
     read -r taken_at holder 2>/dev/null < "$PIPE_LOCK" &&
     [ "$holder" = "$token" ]; then
    done_files+=("$PIPE_LOCK")
  fi
  [ "${#done_files[@]}" -eq 0 ] || rm -f "${done_files[@]}"
}

# One pipe per session at a time: urgent events first, oldest first. A newer
# event of the stream takes the turn, except from SessionEnd. After three
# reaper spans of waiting, a coalesced event is dropped and an urgent one
# pipes without the lock.
pipe_in_turn() {
  local token=$1 started=$SECONDS holding=false may_pipe=false
  while is_event_live "$token"; do
    if is_my_turn "$token" && take_pipe_lock "$token"; then
      holding=true
      may_pipe=true
      break
    fi
    break_stale_pipe_lock
    if [ $((SECONDS - started)) -ge $((3 * DELIVERY_REAPER_SECONDS)) ]; then
      may_pipe=$IS_URGENT_EVENT
      break
    fi
    sleep "$DELIVERY_POLL_SECONDS"
  done
  if [ "$may_pipe" = true ] && claim_stream_event "$token"; then
    pipe_payload
  fi
  end_turn "$token" "$holding"
}

deliver_after_window() {
  sleep "$DELIVERY_COALESCE_SECONDS"
  if session_is_clientless; then
    claim_stream_event "$1"
    return 0
  fi
  pipe_in_turn "$1"
}

# Runs "$@" detached: own process group, stdio closed, no agent pipe held.
start_deliverer() {
  set -m
  ( "$@" ) </dev/null >/dev/null 2>&1 &
  set +m
}

# Hands the payload to a detached deliverer and returns at once.
deliver_payload() {
  local host_key token="$TS_MS.$$"
  if [ -z "$PANE_CACHE_PREFIX" ]; then
    start_deliverer pipe_payload
    return
  fi
  host_key=${AGENT_HOST//\//_}
  PIPE_LOCK="$STATE_CACHE_DIR/$STATE_CACHE_KEY.$host_key.pipe-lock"
  URGENT_PREFIX="$STATE_CACHE_DIR/$STATE_CACHE_KEY.$host_key.urgent."
  if [ "$IS_SUBAGENT" = true ]; then
    STREAM_PENDING="$PANE_CACHE_PREFIX.$host_key.subagents.pending"
    STREAM_CLAIMED="$PANE_CACHE_PREFIX.$host_key.subagents.claimed"
  else
    STREAM_PENDING="$PANE_CACHE_PREFIX.$host_key.root.pending"
    STREAM_CLAIMED="$PANE_CACHE_PREFIX.$host_key.root.claimed"
  fi
  case "$HOOK_EVENT" in
    SessionStart|SessionEnd|UserPromptSubmit|PermissionRequest|Stop)
      IS_URGENT_EVENT=true
      DELIVERY_POLL_SECONDS=0.1 ;;
    *)
      IS_URGENT_EVENT=false
      DELIVERY_POLL_SECONDS=0.25 ;;
  esac

  if [ "$HOOK_EVENT" = "SessionEnd" ]; then
    rm -f "$PANE_CACHE_PREFIX.$host_key".root.* \
      "$PANE_CACHE_PREFIX.$host_key".subagents.*
  elif [ "$HOOK_EVENT" = "Notification" ]; then
    # Off the newest marker, so it can never supersede an event mid-claim.
    is_event_live "$token" || return
  elif ! printf '%s\n' "$token" 2>/dev/null > "$STREAM_PENDING"; then
    start_deliverer pipe_payload
    return
  fi

  if [ "$IS_URGENT_EVENT" = true ]; then
    : 2>/dev/null > "$URGENT_PREFIX$token"
    start_deliverer pipe_in_turn "$token"
  else
    start_deliverer deliver_after_window "$token"
  fi
}

# Best-effort: a missing Zellij session or plugin never fails the agent's hook.
deliver_payload
finish 0
