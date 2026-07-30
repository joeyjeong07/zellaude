/// Symbol for a tool call, keyed on the tool's wire name.
///
/// Claude Code and Codex name the same operations differently, and neither
/// vocabulary is derivable from the other — Codex's shell tool is `exec`, and
/// the string `Bash` does not appear anywhere in the Codex binary. Both sets are
/// therefore listed explicitly, and an unmapped name falls through to the
/// generic symbol rather than being approximated.
pub fn tool_symbol(name: &str) -> &'static str {
    match name {
        // Claude Code
        "Bash" => "⚡",
        "Read" | "Glob" | "Grep" => "◉",
        "Edit" | "Write" => "✎",
        "Task" | "Agent" => "⊜",
        "WebSearch" | "WebFetch" => "◈",
        // Codex CLI. `exec` is the shell tool and by a wide margin the most
        // frequent call of any kind; `exec_command`/`write_stdin`/`wait` are the
        // unified-exec family around it.
        "exec" | "exec_command" | "run" | "write_stdin" | "wait" => "⚡",
        "view_image" => "◉",
        "apply_patch" => "✎",
        "spawn_agent" | "wait_agent" | "send_message" | "list_agents"
        | "interrupt_agent" | "followup_task" => "⊜",
        // Never observed as a call — Codex reports web search as an event
        // rather than a tool — but harmless to map in case that changes.
        "web_search" => "◈",
        _ => "⚙",
    }
}
