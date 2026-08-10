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
LOCK_DIR="$HOME/.config/zellij/plugins/.zellaude-install.lock"
CLAUDE_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh'
CODEX_HOOK_CMD='${HOME}/.config/zellij/plugins/zellaude-hook.sh --client codex'
CLAUDE_SETTINGS="$HOME/.claude/settings.json"
CODEX_CONFIG_DIR="${CODEX_HOME:-$HOME/.codex}"
CODEX_HOOKS="$CODEX_CONFIG_DIR/hooks.json"
CLAUDE_EVENTS='["PreToolUse","PostToolUse","PostToolUseFailure","UserPromptSubmit","PermissionRequest","Notification","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'
CODEX_EVENTS='["PreToolUse","PostToolUse","UserPromptSubmit","PermissionRequest","Stop","SubagentStart","SubagentStop","SessionStart","SessionEnd"]'

# jq is needed for validation as well as updates. Check it before creating the
# lock directory or any temporary/user files so a missing dependency is inert.
if ! command -v jq >/dev/null 2>&1; then
  echo "no_jq"
  exit 1
fi

resolve_file_symlink() {
  path=$1
  symlink_hops=0
  while [ -L "$path" ]; do
    symlink_hops=$((symlink_hops + 1))
    if [ "$symlink_hops" -gt 40 ]; then
      echo "too many symlinks in zellaude hook settings path" >&2
      return 1
    fi
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

# Resolve symlinks so an atomic rename updates their targets rather than
# replacing user-managed links with regular files.
if [ -L "$CLAUDE_SETTINGS" ]; then
  CLAUDE_SETTINGS="$(resolve_file_symlink "$CLAUDE_SETTINGS")"
fi
if [ -L "$CODEX_HOOKS" ]; then
  CODEX_HOOKS="$(resolve_file_symlink "$CODEX_HOOKS")"
fi

LOCK_HELD=false
HOOK_TMP=
CLAUDE_TMP=
CODEX_TMP=

release_lock() {
  if [ "$LOCK_HELD" = true ]; then
    rm -f "$LOCK_DIR/pid"
    rmdir "$LOCK_DIR" 2>/dev/null || true
    LOCK_HELD=false
  fi
}

cleanup_install() {
  [ -n "$HOOK_TMP" ] && rm -f "$HOOK_TMP"
  [ -n "$CLAUDE_TMP" ] && rm -f "$CLAUDE_TMP"
  [ -n "$CODEX_TMP" ] && rm -f "$CODEX_TMP"
  release_lock
}

acquire_lock() {
  attempts=0
  mkdir -p "$(dirname "$LOCK_DIR")"
  while ! mkdir "$LOCK_DIR" 2>/dev/null; do
    owner=
    if [ -r "$LOCK_DIR/pid" ]; then
      owner=$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null || true)
    fi
    stale=false
    case "$owner" in
      ''|*[!0-9]*)
        [ "$attempts" -ge 20 ] && stale=true
        ;;
      *)
        kill -0 "$owner" 2>/dev/null || stale=true
        ;;
    esac
    if [ "$stale" = true ]; then
      current_owner=$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null || true)
      if [ "$current_owner" = "$owner" ]; then
        [ ! -e "$LOCK_DIR/pid" ] || rm -f "$LOCK_DIR/pid"
        if rmdir "$LOCK_DIR" 2>/dev/null; then
          continue
        fi
      fi
    fi
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 200 ]; then
      echo "timed out waiting for Zellaude's installer lock" >&2
      return 1
    fi
    sleep 0.05
  done
  LOCK_HELD=true
  printf '%s\n' "$$" > "$LOCK_DIR/pid"
}

validate_settings_file() {
  file=$1
  [ ! -e "$file" ] && return 0
  if [ ! -f "$file" ] || ! jq -se '
    length == 1 and
    (
      .[0] |
      type == "object" and
      (
        (.hooks? == null) or
        (
          (.hooks | type == "object") and
          all(
            .hooks[]?;
            type == "array" and all(
              .[];
              type == "object" and
              (
                (.hooks? == null) or
                (
                  (.hooks | type == "array") and
                  all(.hooks[]; type == "object")
                )
              )
            )
          )
        )
      )
    )
  ' "$file" >/dev/null; then
    echo "$file contains an invalid hooks configuration" >&2
    return 1
  fi
}

settings_are_current() {
  file=$1
  events=$2
  owned_command=$3
  expected_hook=$4
  [ -f "$file" ] || return 1
  jq -e \
    --argjson events "$events" \
    --arg owned "$owned_command" \
    --argjson expected "$expected_hook" '
      . as $root |
      (([
        $root.hooks[]?[]? | .hooks[]? |
        select((.command // "") == $owned)
      ] | length) == ($events | length)) and
      all(
        $events[];
        . as $event |
        ([
          $root.hooks[$event][]?.hooks[]? |
          select((.command // "") == $owned and . == $expected)
        ] | length) == 1
      )
    ' "$file" >/dev/null 2>&1
}

prepare_settings_update() {
  file=$1
  events=$2
  entry=$3
  owned_command=$4
  file_dir=$(dirname "$file")
  mkdir -p "$file_dir"
  update_tmp=$(mktemp "$file_dir/.zellaude-hooks.XXXXXX")
  if [ -f "$file" ]; then
    input_file=$file
  else
    input_file=$update_tmp.input
    printf '{}\n' > "$input_file"
  fi
  if ! jq \
    --argjson events "$events" \
    --argjson entry "$entry" \
    --arg owned "$owned_command" '
      .hooks //= {} |
      .hooks |= with_entries(
        .value |= [
          .[] | . as $group |
          ($group.hooks // []) as $original |
          ($original | map(select((.command // "") != $owned))) as $filtered |
          if ($original | length) == 0
          then $group
          elif ($filtered | length) > 0
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
    ' "$input_file" > "$update_tmp"; then
    rm -f "$update_tmp"
    if [ "$input_file" != "$file" ]; then
      rm -f "$input_file"
    fi
    return 1
  fi
  if [ "$input_file" != "$file" ]; then
    rm -f "$input_file"
  fi
  printf '%s\n' "$update_tmp"
}

backup_file() {
  file=$1
  if [ -f "$file" ]; then
    backup_tmp=$(mktemp "$(dirname "$file")/.zellaude-backup.XXXXXX")
    cp "$file" "$backup_tmp"
    mv "$backup_tmp" "$file.bak"
  fi
}

trap cleanup_install 0
trap 'exit 1' HUP INT TERM
acquire_lock

# Validate both inputs before changing either one or replacing the bridge.
validate_settings_file "$CLAUDE_SETTINGS"
validate_settings_file "$CODEX_HOOKS"

if [ -e "$HOOK_PATH" ] && [ ! -f "$HOOK_PATH" ]; then
  echo "cannot install Zellaude hook over non-file destination: $HOOK_PATH" >&2
  exit 1
fi

CLAUDE_HOOK=$(jq -nc --arg cmd "$CLAUDE_HOOK_CMD" '{
  "type": "command", "command": $cmd, "timeout": 5, "async": true
}')
CODEX_HOOK=$(jq -nc --arg cmd "$CODEX_HOOK_CMD" '{
  "type": "command", "command": $cmd, "timeout": 3
}')
CLAUDE_ENTRY=$(jq -nc --argjson hook "$CLAUDE_HOOK" '[{"hooks": [$hook]}]')
CODEX_ENTRY=$(jq -nc --argjson hook "$CODEX_HOOK" '[{"hooks": [$hook]}]')

# Materialize the embedded bridge once under the lock. The same file is used
# for exact comparison and, when needed, an atomic same-directory rename.
HOOK_TMP=$(mktemp "$(dirname "$HOOK_PATH")/.zellaude-hook.XXXXXX")
cat > "$HOOK_TMP" << 'ZELLAUDE_HOOK_EOF'
__HOOK_SCRIPT__
ZELLAUDE_HOOK_EOF
chmod +x "$HOOK_TMP"

if [ -x "$HOOK_PATH" ] &&
   cmp -s "$HOOK_TMP" "$HOOK_PATH" &&
   settings_are_current \
     "$CLAUDE_SETTINGS" "$CLAUDE_EVENTS" "$CLAUDE_HOOK_CMD" "$CLAUDE_HOOK" &&
   settings_are_current \
     "$CODEX_HOOKS" "$CODEX_EVENTS" "$CODEX_HOOK_CMD" "$CODEX_HOOK"; then
  echo "current"
  exit 0
fi

# Build both new documents successfully before committing any of the three
# files. Exact command equality preserves unrelated lookalike/wrapper hooks.
CLAUDE_TMP=$(prepare_settings_update \
  "$CLAUDE_SETTINGS" "$CLAUDE_EVENTS" "$CLAUDE_ENTRY" "$CLAUDE_HOOK_CMD")
CODEX_TMP=$(prepare_settings_update \
  "$CODEX_HOOKS" "$CODEX_EVENTS" "$CODEX_ENTRY" "$CODEX_HOOK_CMD")

backup_file "$CLAUDE_SETTINGS"
backup_file "$CODEX_HOOKS"

mv "$CLAUDE_TMP" "$CLAUDE_SETTINGS"
CLAUDE_TMP=
mv "$CODEX_TMP" "$CODEX_HOOKS"
CODEX_TMP=
mv "$HOOK_TMP" "$HOOK_PATH"
HOOK_TMP=

echo "installed"
"##;

fn install_command() -> String {
    INSTALL_TEMPLATE.replace("__HOOK_SCRIPT__\n", &hook_script_content())
}

/// Run the idempotent hook installation command.
/// Checks if hooks are current, writes the hook script, and registers hooks.
pub fn run_install() {
    let cmd = install_command();

    let mut ctx = BTreeMap::new();
    ctx.insert("type".into(), "install_hooks".into());
    run_command(&["sh", "-c", &cmd], ctx);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{hook_script_content, install_command};
    use serde_json::Value;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    const CLAUDE_OWNED: &str = "${HOME}/.config/zellij/plugins/zellaude-hook.sh";
    const CODEX_OWNED: &str = "${HOME}/.config/zellij/plugins/zellaude-hook.sh --client codex";
    const LOOKALIKE: &str = "/bin/wrapper --mentions zellaude-hook.sh";

    struct TempHome(PathBuf);

    impl TempHome {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow Unix epoch")
                .as_nanos();
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "zellaude-installer-{label}-{}-{nonce}-{counter}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temporary home");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn jq_available() -> bool {
        Command::new("jq")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn script_command(script: &str, home: &Path) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .env("HOME", home)
            .env("CODEX_HOME", home.join(".codex"));
        command
    }

    fn run_script(script: &str, home: &Path) -> Output {
        script_command(script, home)
            .output()
            .expect("run embedded installer")
    }

    fn seed_settings(path: &Path, owned: &str) {
        fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings directory");
        let document = serde_json::json!({
            "unrelated_setting": true,
            "hooks": {
                "PreToolUse": [{
                    "hooks": [
                        {"type": "command", "command": LOOKALIKE},
                        {"type": "command", "command": owned},
                        {"type": "command", "command": owned}
                    ]
                }]
            }
        });
        fs::write(path, format!("{document}\n")).expect("seed settings");
    }

    fn command_count(document: &Value, command: &str) -> usize {
        document["hooks"]
            .as_object()
            .into_iter()
            .flat_map(|events| events.values())
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|group| group.get("hooks"))
            .filter_map(Value::as_array)
            .flatten()
            .filter(|hook| hook.get("command").and_then(Value::as_str) == Some(command))
            .count()
    }

    #[test]
    fn missing_jq_does_not_touch_the_home_directory() {
        let home = TempHome::new("no-jq");
        let empty_path = home.path().join("empty-path");
        fs::create_dir(&empty_path).expect("create empty PATH");

        let output = script_command(&install_command(), home.path())
            .env("PATH", &empty_path)
            .output()
            .expect("run embedded installer without jq");

        assert!(!output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "no_jq");
        assert!(!home.path().join(".config").exists());
        assert!(!home.path().join(".claude").exists());
        assert!(!home.path().join(".codex").exists());
    }

    #[test]
    fn install_is_locked_exact_and_content_aware() {
        if !jq_available() {
            return;
        }

        let home = TempHome::new("behavior");
        let claude_settings = home.path().join(".claude/settings.json");
        let codex_hooks = home.path().join(".codex/hooks.json");
        let hook_path = home.path().join(".config/zellij/plugins/zellaude-hook.sh");
        seed_settings(&claude_settings, CLAUDE_OWNED);
        seed_settings(&codex_hooks, CODEX_OWNED);
        let script = install_command();

        let children: Vec<_> = (0..6)
            .map(|_| {
                script_command(&script, home.path())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn concurrent embedded installer")
            })
            .collect();
        let mut installed = 0;
        let mut current = 0;
        for child in children {
            let output = child.wait_with_output().expect("wait for installer");
            assert!(
                output.status.success(),
                "installer failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            match String::from_utf8_lossy(&output.stdout).trim() {
                "installed" => installed += 1,
                "current" => current += 1,
                other => panic!("unexpected installer status: {other:?}"),
            }
        }
        assert_eq!(installed, 1);
        assert_eq!(current, 5);
        assert!(!home
            .path()
            .join(".config/zellij/plugins/.zellaude-install.lock")
            .exists());

        let claude: Value = serde_json::from_slice(
            &fs::read(&claude_settings).expect("read installed Claude settings"),
        )
        .expect("parse installed Claude settings");
        let codex: Value =
            serde_json::from_slice(&fs::read(&codex_hooks).expect("read installed Codex settings"))
                .expect("parse installed Codex settings");
        assert_eq!(command_count(&claude, CLAUDE_OWNED), 11);
        assert_eq!(command_count(&codex, CODEX_OWNED), 9);
        assert_eq!(command_count(&claude, LOOKALIKE), 1);
        assert_eq!(command_count(&codex, LOOKALIKE), 1);
        assert_eq!(
            fs::read_to_string(&hook_path).expect("read installed hook"),
            hook_script_content()
        );

        let output = run_script(&script, home.path());
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "current");

        fs::write(&hook_path, format!("{}# changed\n", hook_script_content()))
            .expect("mutate installed hook");
        let output = run_script(&script, home.path());
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "installed");
        assert_eq!(
            fs::read_to_string(&hook_path).expect("read repaired hook"),
            hook_script_content()
        );
    }

    #[test]
    fn malformed_hooks_fail_before_owned_files_change() {
        if !jq_available() {
            return;
        }

        let home = TempHome::new("malformed");
        let claude_settings = home.path().join(".claude/settings.json");
        let codex_hooks = home.path().join(".codex/hooks.json");
        fs::create_dir_all(claude_settings.parent().expect("Claude settings parent"))
            .expect("create Claude settings directory");
        fs::create_dir_all(codex_hooks.parent().expect("Codex hooks parent"))
            .expect("create Codex hooks directory");
        let invalid = b"{\"hooks\":[]}\n";
        let valid = b"{}\n";
        fs::write(&claude_settings, invalid).expect("write malformed settings");
        fs::write(&codex_hooks, valid).expect("write valid settings");

        let output = run_script(&install_command(), home.path());

        assert!(!output.status.success());
        assert_eq!(
            fs::read(&claude_settings).expect("read malformed settings"),
            invalid
        );
        assert_eq!(fs::read(&codex_hooks).expect("read Codex hooks"), valid);
        assert!(!home
            .path()
            .join(".config/zellij/plugins/zellaude-hook.sh")
            .exists());

        let invalid_nested = br#"{
          "hooks": {
            "PreToolUse": {
              "not-an-array": {
                "hooks": [{
                  "type": "command",
                  "command": "${HOME}/.config/zellij/plugins/zellaude-hook.sh"
                }]
              }
            }
          }
        }
        "#;
        fs::write(&claude_settings, invalid_nested).expect("write malformed nested hooks");
        let output = run_script(&install_command(), home.path());
        assert!(!output.status.success());
        assert_eq!(
            fs::read(&claude_settings).expect("read malformed nested settings"),
            invalid_nested
        );
        assert_eq!(fs::read(&codex_hooks).expect("read Codex hooks"), valid);
        assert!(!home
            .path()
            .join(".config/zellij/plugins/zellaude-hook.sh")
            .exists());
    }

    #[test]
    fn abandoned_locks_without_a_live_pid_recover() {
        if !jq_available() {
            return;
        }

        for (label, pid) in [("missing-pid", None), ("invalid-pid", Some("not-a-pid\n"))] {
            let home = TempHome::new(label);
            let lock = home
                .path()
                .join(".config/zellij/plugins/.zellaude-install.lock");
            fs::create_dir_all(&lock).expect("create abandoned lock");
            if let Some(pid) = pid {
                fs::write(lock.join("pid"), pid).expect("write invalid lock owner");
            }

            let output = run_script(&install_command(), home.path());

            assert!(
                output.status.success(),
                "installer did not recover {label}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "installed");
            assert!(!lock.exists());
        }
    }
}
