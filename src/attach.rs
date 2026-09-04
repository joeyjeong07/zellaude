use crate::state::unix_now_ms;
use std::collections::{BTreeMap, HashMap};
use zellij_tile::prelude::*;

const ATTACH_SCRIPT: &str = include_str!("../scripts/zellaude-attach.sh");

pub fn is_active_instance(manifest: &PaneManifest, tabs: &[TabInfo], current_id: u32) -> bool {
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

    let scan_started_ms = unix_now_ms().to_string();
    let allowed_panes = pane_ids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut context = BTreeMap::new();
    context.insert("type".into(), "attach_scan".into());
    context.insert("pane_ids".into(), allowed_panes);
    context.insert("scan_started_ms".into(), scan_started_ms.clone());
    run_command(
        &[
            "bash",
            "-c",
            ATTACH_SCRIPT,
            "zellaude-attach",
            session_name,
            &scan_started_ms,
        ],
        context,
    );
    true
}
