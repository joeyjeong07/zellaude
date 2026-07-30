#!/usr/bin/env bash
# zellaude-hook.sh — agent hook → zellij pipe bridge
# Forwards Claude Code and Codex hook events to the zellaude Zellij plugin.
#
# Usage:
#   Claude Code: "/path/to/zellaude-hook.sh"
#   Codex:       "/path/to/zellaude-hook.sh --client codex"

CLIENT="claude"
if [ "${1:-}" = "--client" ] && [ -n "${2:-}" ]; then
  CLIENT="$2"
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
  transcript_effort=$(
    tail -n 512 "$TRANSCRIPT_PATH" 2>/dev/null |
      tail -c "$MAX_TRANSCRIPT_BYTES" |
      jq -Rnr --arg turn_id "$TURN_ID" '
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
  )

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
        | .uuid
      ] as $commands
    | ($commands | last // null) as $command
    | if $command == null then
        ["null", ""]
      else
        (
          [
            $entries[]
            | select(.type == "user")
            | select(.parentUuid? == $command)
            | select((.message.content? | type) == "string")
            | .message.content
            | select(contains("<local-command-stdout>"))
          ]
          | last // ""
          | gsub("\u001b\\[[0-9;]*[[:alpha:]]"; "")
        ) as $output
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
        | [$state, (if $state == "null" then "" else $command end)]
      end
    | @tsv
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
  local effort_level configured_effort launch_mode tail_lines

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
  transcript_result=$'null\t'
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
  fi
  IFS=$'\t' read -r transcript_state transcript_marker <<< "$transcript_result"
  transcript_state=${transcript_state:-null}
  transcript_marker=${transcript_marker:-}

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
  --arg ts_ms "$TS_MS" \
  --argjson rainbow_name "$RAINBOW_NAME" \
  --arg rainbow_mode_marker "$RAINBOW_MODE_MARKER" \
  --argjson is_subagent "$IS_SUBAGENT" \
  '{
    pane_id: ($pane_id | tonumber),
    session_id: $session_id,
    hook_event: $hook_event,
    tool_name: (if $tool_name == "" then null else $tool_name end),
    cwd: (if $cwd == "" then null else $cwd end),
    zellij_session: $zellij_session,
    term_program: (if $term_program == "" then null else $term_program end),
    ts_ms: ($ts_ms | tonumber),
    is_subagent: $is_subagent,
    rainbow_name: $rainbow_name,
    rainbow_mode_marker: (
      if $rainbow_mode_marker == ""
      then null
      else $rainbow_mode_marker
      end
    )
  }')

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
zellij pipe --name "zellaude" -- "$PAYLOAD" >/dev/null 2>&1 || true
finish 0
