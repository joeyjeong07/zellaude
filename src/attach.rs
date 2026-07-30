use crate::state::unix_now_ms;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use zellij_tile::prelude::*;

const ATTACH_SCRIPT: &str = include_str!("../scripts/zellaude-attach.sh");

fn supports_pane_introspection(version: &str) -> bool {
    let mut parts = version.trim_start_matches('v').split('.');
    let major = parts.next().and_then(|part| part.parse::<u64>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u64>().ok());
    matches!((major, minor), (Some(major), _) if major >= 1)
        || matches!((major, minor), (Some(0), Some(minor)) if minor >= 44)
}

fn client_for_command(command: &[String]) -> Option<&'static str> {
    let executable = command.first()?;
    let name = Path::new(executable).file_name()?.to_str()?;
    match name {
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        _ => None,
    }
}

pub fn is_active_instance(manifest: &PaneManifest, tabs: &[TabInfo]) -> bool {
    let current_id = get_plugin_ids().plugin_id;
    let current_tab = manifest
        .panes
        .iter()
        .find_map(|(tab_index, panes)| {
            panes
                .iter()
                .any(|pane| pane.is_plugin && pane.id == current_id)
                .then_some(*tab_index)
        });
    let active_tab = tabs.iter().find(|tab| tab.active).map(|tab| tab.position);

    match (current_tab, active_tab) {
        (Some(current_tab), Some(active_tab)) => current_tab == active_tab,
        // Do not make discovery impossible if an older Zellij omits the
        // current plugin from its manifest.
        _ => true,
    }
}

pub fn run(session_name: &str, pane_to_tab: &HashMap<u32, (usize, String)>) -> bool {
    let mut pane_ids: Vec<u32> = pane_to_tab.keys().copied().collect();
    pane_ids.sort_unstable();

    let scan_started_ms = unix_now_ms();
    let supports_introspection = supports_pane_introspection(&get_zellij_version());
    let mut records = Vec::new();
    let mut pane_leaders = Vec::new();
    if supports_introspection {
        for pane_id in &pane_ids {
            let zellij_pane = PaneId::Terminal(*pane_id);
            let Ok(leader_pid) = get_pane_pid(zellij_pane) else {
                continue;
            };
            if leader_pid <= 0 {
                continue;
            }

            let client = get_pane_running_command(zellij_pane)
                .ok()
                .as_deref()
                .and_then(client_for_command)
                .unwrap_or("unknown");
            records.push(format!("{pane_id}:{leader_pid}:{client}"));
            pane_leaders.push(format!("{pane_id}:{leader_pid}"));
        }
    }

    let records = records.join(",");
    let allowed_panes = pane_ids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let pane_leaders = pane_leaders.join(",");
    let scan_started_ms_arg = scan_started_ms.to_string();
    let mut context = BTreeMap::new();
    context.insert("type".into(), "attach_scan".into());
    context.insert("pane_ids".into(), allowed_panes);
    context.insert("pane_leaders".into(), pane_leaders);
    context.insert(
        "introspection_supported".into(),
        supports_introspection.to_string(),
    );
    context.insert("scan_started_ms".into(), scan_started_ms_arg.clone());
    run_command(
        &[
            "bash",
            "-c",
            ATTACH_SCRIPT,
            "zellaude-attach",
            session_name,
            &records,
            &scan_started_ms_arg,
        ],
        context,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::{client_for_command, supports_pane_introspection};

    #[test]
    fn pane_introspection_requires_zellij_044() {
        assert!(!supports_pane_introspection("0.43.1"));
        assert!(supports_pane_introspection("0.44.0"));
        assert!(supports_pane_introspection("0.44.3"));
        assert!(supports_pane_introspection("1.0.0"));
        assert!(!supports_pane_introspection("unknown"));
    }

    #[test]
    fn agent_commands_are_classified_by_executable_name() {
        assert_eq!(
            client_for_command(&["/opt/codex/bin/codex".to_string()]),
            Some("codex")
        );
        assert_eq!(
            client_for_command(&["claude".to_string(), "--resume".to_string()]),
            Some("claude")
        );
        assert_eq!(client_for_command(&["bash".to_string()]), None);
        assert_eq!(client_for_command(&[]), None);
    }
}
