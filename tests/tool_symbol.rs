//! The tool-name table is the one place where a wrong guess is invisible: an
//! unrecognised name renders as the generic symbol, which looks like a working
//! integration rather than a missing mapping. These tests pin the Codex names
//! against their Claude equivalents so a gap fails here instead of on the bar.
//!
//! Codex tool names were counted from local Codex CLI session rollouts, taking
//! the `name` of every `function_call` and `custom_tool_call` record:
//! exec 678, exec_command 150, send_message 54, wait_agent 48, spawn_agent 47,
//! list_agents 27, wait 16, followup_task 12, apply_patch 8, update_plan 5,
//! interrupt_agent 5, request_user_input 3, write_stdin 2, run 2, view_image 1.

#[path = "../src/tool_symbol.rs"]
mod tool_symbol;

use tool_symbol::tool_symbol;

#[test]
fn codex_shell_tools_share_the_claude_shell_symbol() {
    // `exec` alone is the majority of all Codex tool calls. Leaving it unmapped
    // means the most common thing a Codex session does renders as the generic
    // gear, so this is the load-bearing case rather than an edge case.
    for name in ["exec", "exec_command", "run", "write_stdin", "wait"] {
        assert_eq!(tool_symbol(name), tool_symbol("Bash"), "{name}");
    }
}

#[test]
fn codex_edit_and_read_tools_match_their_claude_equivalents() {
    assert_eq!(tool_symbol("apply_patch"), tool_symbol("Edit"));
    assert_eq!(tool_symbol("view_image"), tool_symbol("Read"));
}

#[test]
fn every_codex_multi_agent_tool_gets_the_subagent_symbol() {
    for name in [
        "spawn_agent",
        "wait_agent",
        "send_message",
        "list_agents",
        "interrupt_agent",
        "followup_task",
    ] {
        assert_eq!(tool_symbol(name), tool_symbol("Task"), "{name}");
    }
}

#[test]
fn claude_tool_names_are_unchanged() {
    assert_eq!(tool_symbol("Bash"), "⚡");
    assert_eq!(tool_symbol("Read"), "◉");
    assert_eq!(tool_symbol("Glob"), "◉");
    assert_eq!(tool_symbol("Grep"), "◉");
    assert_eq!(tool_symbol("Edit"), "✎");
    assert_eq!(tool_symbol("Write"), "✎");
    assert_eq!(tool_symbol("Task"), "⊜");
    assert_eq!(tool_symbol("WebSearch"), "◈");
    assert_eq!(tool_symbol("WebFetch"), "◈");
}

#[test]
fn an_unknown_tool_falls_through_to_the_generic_symbol() {
    assert_eq!(tool_symbol("some_tool_added_next_release"), "⚙");
}
