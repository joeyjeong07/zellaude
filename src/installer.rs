use std::collections::BTreeMap;
use zellij_tile::prelude::run_command;

const HOOK_VERSION_TAG: &str = concat!("# zellaude v", env!("CARGO_PKG_VERSION"));

/// Generate hook script content with version tag inserted after the shebang.
fn hook_script_content() -> String {
    let original = include_str!("../scripts/zellaude-hook.sh");
    // Insert version tag after the shebang line
    if let Some(pos) = original.find('\n') {
        let (shebang, rest) = original.split_at(pos);
        format!("{shebang}\n{HOOK_VERSION_TAG}{rest}")
    } else {
        original.to_string()
    }
}

const INSTALL_TEMPLATE: &str = r##"set -e
HOOK_PATH="$HOME/.config/zellij/plugins/zellaude-hook.sh"
CLAUDE_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh'
CODEX_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh --client codex'
CLAUDE_SETTINGS="$HOME/.claude/settings.json"
CODEX_CONFIG_DIR="${CODEX_HOME:-$HOME/.codex}"
CODEX_HOOKS="$CODEX_CONFIG_DIR/hooks.json"

resolve_file_symlink() {
  path=$1
  while [ -L "$path" ]; do
    dir=$(cd "$(dirname "$path")" && pwd -P)
    target=$(readlink "$path")
    case "$target" in
      /*) path=$target ;;
      *) path=$dir/$target ;;
    esac
  done
  dir=$(cd "$(dirname "$path")" && pwd -P)
  printf '%s/%s\n' "$dir" "$(basename "$path")"
}

# Resolve symlinks so mv doesn't replace them with regular files
if [ -L "$CLAUDE_SETTINGS" ]; then
  CLAUDE_SETTINGS="$(resolve_file_symlink "$CLAUDE_SETTINGS")"
fi
if [ -L "$CODEX_HOOKS" ]; then
  CODEX_HOOKS="$(resolve_file_symlink "$CODEX_HOOKS")"
fi

# Check if already current
if grep -qF '__VERSION_TAG__' "$HOOK_PATH" 2>/dev/null; then
  if [ -f "$CLAUDE_SETTINGS" ] &&
     grep -qF "$CLAUDE_HOOK_CMD" "$CLAUDE_SETTINGS" 2>/dev/null &&
     [ -f "$CODEX_HOOKS" ] &&
     grep -qF "$CODEX_HOOK_CMD" "$CODEX_HOOKS" 2>/dev/null; then
    echo "current"
    exit 0
  fi
fi

# Write the hook script atomically so concurrent plugin instances cannot expose
# a partially written bridge.
mkdir -p "$(dirname "$HOOK_PATH")"
HOOK_TMP=$(mktemp "$(dirname "$HOOK_PATH")/.zellaude-hook.XXXXXX")
cat > "$HOOK_TMP" << 'ZELLAUDE_HOOK_EOF'
__HOOK_SCRIPT__
ZELLAUDE_HOOK_EOF
chmod +x "$HOOK_TMP"
mv "$HOOK_TMP" "$HOOK_PATH"

# Register hooks (requires jq)
if ! command -v jq >/dev/null 2>&1; then
  echo "no_jq"
  exit 1
fi

ensure_json_file() {
  file=$1
  if [ ! -f "$file" ]; then
    mkdir -p "$(dirname "$file")"
    tmp=$(mktemp "$(dirname "$file")/.zellaude-hooks.XXXXXX")
    printf '{}\n' > "$tmp"
    mv "$tmp" "$file"
  fi
}

remove_zellaude_entries() {
  file=$1
  tmp=$(mktemp)
  jq '
    if .hooks and (.hooks | type == "object") then
      .hooks |= with_entries(
        .value |= [
          .[] | . as $group |
          ($group.hooks // [])
            | map(select(((.command // "") | contains("zellaude-hook.sh")) | not))
            | . as $filtered |
          if length > 0 then ($group | .hooks = $filtered) else empty end
        ]
      ) |
      .hooks |= with_entries(select(.value | length > 0)) |
      if .hooks == {} then del(.hooks) else . end
    else
      .
    end
  ' "$file" > "$tmp" && mv "$tmp" "$file"
}

replace_zellaude_entries() {
  file=$1
  events=$2
  entry=$3
  tmp=$(mktemp "$(dirname "$file")/.zellaude-hooks.XXXXXX")
  jq --argjson events "$events" --argjson entry "$entry" '
    if ((.hooks // {}) | type) == "object" then
      .hooks //= {}
    else
      .hooks = {}
    end |
    .hooks |= with_entries(
      .value |= [
        .[] | . as $group |
        (($group.hooks // [])
          | map(select(((.command // "") | contains("zellaude-hook.sh")) | not))
        ) as $filtered |
        if ($filtered | length) > 0
        then ($group | .hooks = $filtered)
        else empty
        end
      ]
    ) |
    .hooks |= with_entries(select(.value | length > 0)) |
    .hooks //= {} |
    reduce ($events[]) as $event (
      .;
      .hooks[$event] = (.hooks[$event] // []) + $entry
    )
  ' "$file" > "$tmp" && mv "$tmp" "$file"
}

ensure_json_file "$CLAUDE_SETTINGS"
ensure_json_file "$CODEX_HOOKS"

# Back up both user configuration files before modifying them
CLAUDE_BACKUP_TMP=$(mktemp "$(dirname "$CLAUDE_SETTINGS")/.zellaude-backup.XXXXXX")
CODEX_BACKUP_TMP=$(mktemp "$(dirname "$CODEX_HOOKS")/.zellaude-backup.XXXXXX")
cp "$CLAUDE_SETTINGS" "$CLAUDE_BACKUP_TMP"
cp "$CODEX_HOOKS" "$CODEX_BACKUP_TMP"
mv "$CLAUDE_BACKUP_TMP" "$CLAUDE_SETTINGS.bak"
mv "$CODEX_BACKUP_TMP" "$CODEX_HOOKS.bak"

# Add new hook entries with literal ${HOME} to keep settings portable
CLAUDE_EVENTS='["PreToolUse","PostToolUse","PostToolUseFailure","UserPromptSubmit","PermissionRequest","Notification","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
CODEX_EVENTS='["PreToolUse","PostToolUse","UserPromptSubmit","PermissionRequest","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
CLAUDE_ENTRY=$(jq -nc --arg cmd "$CLAUDE_HOOK_CMD" '[{"hooks": [{"type": "command", "command": $cmd, "timeout": 5, "async": true}]}]')
CODEX_ENTRY=$(jq -nc --arg cmd "$CODEX_HOOK_CMD" '[{"hooks": [{"type": "command", "command": $cmd, "timeout": 3}]}]')

# Remove and replace zellaude handlers in one transaction per file. Several
# status-bar instances can reach this installer at the same time after reload.
replace_zellaude_entries "$CLAUDE_SETTINGS" "$CLAUDE_EVENTS" "$CLAUDE_ENTRY"
replace_zellaude_entries "$CODEX_HOOKS" "$CODEX_EVENTS" "$CODEX_ENTRY"

echo "installed"
"##;

/// Run the idempotent hook installation command.
/// Checks if hooks are current, writes the hook script, and registers hooks.
pub fn run_install() {
    let cmd = INSTALL_TEMPLATE
        .replace("__VERSION_TAG__", HOOK_VERSION_TAG)
        .replace("__HOOK_SCRIPT__", &hook_script_content());

    let mut ctx = BTreeMap::new();
    ctx.insert("type".into(), "install_hooks".into());
    run_command(&["sh", "-c", &cmd], ctx);
}
