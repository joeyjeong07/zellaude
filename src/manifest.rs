use serde::Serialize;
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

/// One pane as an external consumer needs it to map a layout slot to a pane
/// id: identity, layer, and the full-frame geometry `list-panes --geometry`
/// reports. Never carries `agent_pid` — consumers globbing the runtime cache
/// use that key to tell pane records from this manifest.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManifestPane {
    pub id: u32,
    pub is_plugin: bool,
    pub is_floating: bool,
    pub title: String,
    pub x: usize,
    pub y: usize,
    pub rows: usize,
    pub cols: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManifestTab {
    pub position: usize,
    pub name: String,
    pub panes: Vec<ManifestPane>,
}

/// Everything in the manifest except the write timestamp, so an unchanged
/// topology compares equal across rebuilds and is never rewritten.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManifestBody {
    pub zellij_session: String,
    pub tabs: Vec<ManifestTab>,
}

pub fn body(session_name: &str, tabs: &[TabInfo], manifest: &PaneManifest) -> ManifestBody {
    let mut manifest_tabs: Vec<ManifestTab> = tabs
        .iter()
        .map(|tab| {
            let mut panes: Vec<ManifestPane> = manifest
                .panes
                .get(&tab.position)
                .into_iter()
                .flatten()
                .map(|pane| ManifestPane {
                    id: pane.id,
                    is_plugin: pane.is_plugin,
                    is_floating: pane.is_floating,
                    title: pane.title.clone(),
                    x: pane.pane_x,
                    y: pane.pane_y,
                    rows: pane.pane_rows,
                    cols: pane.pane_columns,
                })
                .collect();
            // Tiled panes first in spatial order, so the file is stable across
            // rebuilds regardless of the order Zellij delivers panes in.
            panes.sort_by(|a, b| {
                (a.is_floating, a.y, a.x, a.is_plugin, a.id).cmp(&(
                    b.is_floating,
                    b.y,
                    b.x,
                    b.is_plugin,
                    b.id,
                ))
            });
            ManifestTab {
                position: tab.position,
                name: tab.name.clone(),
                panes,
            }
        })
        .collect();
    manifest_tabs.sort_by_key(|tab| tab.position);
    ManifestBody {
        zellij_session: session_name.to_string(),
        tabs: manifest_tabs,
    }
}

pub fn payload_json(body: &ManifestBody, ts_ms: u64) -> String {
    #[derive(Serialize)]
    struct Payload<'a> {
        zellij_session: &'a str,
        ts_ms: u64,
        tabs: &'a [ManifestTab],
    }
    serde_json::to_string(&Payload {
        zellij_session: &body.zellij_session,
        ts_ms,
        tabs: &body.tabs,
    })
    .unwrap_or_default()
}

/// The literal `state_cache_key()` function from the hook script. The manifest
/// filename must match the per-pane state files byte for byte, and external
/// consumers mirror that function — extracting the shell source keeps a single
/// authority instead of a Rust re-implementation that could drift.
fn state_cache_key_shell_fn() -> Option<&'static str> {
    let script = crate::installer::HOOK_SCRIPT;
    let start = script.find("\nstate_cache_key() {")? + 1;
    let len = script[start..].find("\n}")? + 2;
    Some(&script[start..start + len])
}

/// Write one manifest atomically: `$1` manifest JSON (host not yet included),
/// `$2` the session name, `$3` the previous session name ("" when unchanged),
/// whose manifest is removed after a rename. Readers may race the write, so
/// the payload lands in a same-directory temp file first and is renamed over
/// the destination.
const WRITE_SCRIPT_TEMPLATE: &str = r#"
set -eu

__STATE_CACHE_KEY__

cache_dir="$HOME/.cache/zellaude/runtime"
umask 077
mkdir -p "$cache_dir"
key=$(state_cache_key "$2")
tmp=$(mktemp "$cache_dir/.manifest.XXXXXX")
trap 'rm -f "$tmp"' 0 HUP INT TERM
host=$(hostname 2>/dev/null) || host=""
if [ -n "$host" ] && command -v jq >/dev/null 2>&1; then
    printf '%s\n' "$1" | jq -c --arg host "$host" '. + {host: $host}' > "$tmp"
else
    printf '%s\n' "$1" > "$tmp"
fi
mv "$tmp" "$cache_dir/$key.manifest.json"
trap - 0 HUP INT TERM
if [ -n "$3" ] && [ "$3" != "$2" ]; then
    old_key=$(state_cache_key "$3") || exit 0
    rm -f "$cache_dir/$old_key.manifest.json"
fi
"#;

pub fn write(payload: &str, session_name: &str, previous_session_name: Option<&str>) {
    let Some(key_fn) = state_cache_key_shell_fn() else {
        eprintln!("Zellaude could not find state_cache_key in the embedded hook script");
        return;
    };
    let script = WRITE_SCRIPT_TEMPLATE.replace("__STATE_CACHE_KEY__", key_fn);
    let mut ctx = BTreeMap::new();
    ctx.insert("type".into(), "write_manifest".into());
    run_command(
        &[
            "bash",
            "-c",
            &script,
            "zellaude-manifest",
            payload,
            session_name,
            previous_session_name.unwrap_or(""),
        ],
        ctx,
    );
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{body, payload_json, state_cache_key_shell_fn, WRITE_SCRIPT_TEMPLATE};
    use serde_json::Value;
    use std::process::Command;
    use zellij_tile::prelude::{PaneInfo, PaneManifest, TabInfo};

    fn run_state_cache_key(value: &str) -> String {
        let key_fn = state_cache_key_shell_fn().expect("hook script defines state_cache_key");
        let script = format!("{key_fn}\nstate_cache_key \"$1\"");
        let output = Command::new("bash")
            .args(["-c", &script, "state-cache-key", value])
            .output()
            .expect("run extracted state_cache_key");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("key is UTF-8")
            .trim()
            .to_string()
    }

    #[test]
    fn extracted_shell_function_matches_the_hook_key_scheme() {
        let key_fn = state_cache_key_shell_fn().expect("hook script defines state_cache_key");
        assert!(key_fn.starts_with("state_cache_key() {"));
        assert!(key_fn.ends_with("\n}"));

        assert_eq!(run_state_cache_key("dev-session_1.a"), "dev-session_1.a");
        // printf '%s' 'name with spaces' | sha256sum
        assert_eq!(
            run_state_cache_key("name with spaces"),
            "~78162a09cd6e117eb532cfe26d466fb77010641ff189b0a3ccf68574be53d18d"
        );
    }

    #[test]
    fn write_script_only_removes_the_previous_manifest_on_rename() {
        assert!(WRITE_SCRIPT_TEMPLATE.contains(r#"rm -f "$cache_dir/$old_key.manifest.json""#));
    }

    fn pane(id: u32, y: usize, x: usize) -> PaneInfo {
        PaneInfo {
            id,
            title: format!("pane {id}"),
            pane_x: x,
            pane_y: y,
            pane_rows: 10,
            pane_columns: 20,
            ..Default::default()
        }
    }

    fn tab(position: usize, name: &str) -> TabInfo {
        TabInfo {
            position,
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn payload_orders_tabs_by_position_and_panes_spatially() {
        let mut bar = pane(9, 0, 0);
        bar.is_plugin = true;
        let mut floating = pane(7, 0, 0);
        floating.is_floating = true;
        let manifest = PaneManifest {
            panes: [
                (1, vec![pane(3, 1, 40), floating, pane(2, 1, 0), bar]),
                (0, vec![pane(1, 0, 0)]),
            ]
            .into_iter()
            .collect(),
        };
        let body = body("dev", &[tab(1, "work"), tab(0, "main")], &manifest);

        let payload: Value =
            serde_json::from_str(&payload_json(&body, 1234)).expect("payload is JSON");
        assert_eq!(payload["zellij_session"], "dev");
        assert_eq!(payload["ts_ms"], 1234);
        // Consumers skip runtime-cache entries without agent_pid; the manifest
        // must stay recognizable as a non-pane record.
        assert!(payload.get("agent_pid").is_none());

        let tabs = payload["tabs"].as_array().expect("tabs array");
        assert_eq!(tabs[0]["position"], 0);
        assert_eq!(tabs[1]["name"], "work");
        let pane_ids: Vec<u64> = tabs[1]["panes"]
            .as_array()
            .expect("panes array")
            .iter()
            .map(|pane| pane["id"].as_u64().expect("pane id"))
            .collect();
        assert_eq!(pane_ids, vec![9, 2, 3, 7]);
        assert_eq!(tabs[1]["panes"][0]["is_plugin"], true);
        assert_eq!(tabs[1]["panes"][3]["is_floating"], true);
        assert_eq!(tabs[0]["panes"][0]["cols"], 20);
    }
}
