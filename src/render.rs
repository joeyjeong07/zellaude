use crate::custom_layouts::Prompt as CustomLayoutPrompt;
use crate::rainbow;
use crate::session_selection::{session_to_display, session_to_focus};
use crate::tool_symbol::tool_symbol;
use crate::state::{
    unix_now, unix_now_ms, Activity, ClickRegion, FlashMode, MenuAction, MenuClickRegion,
    NotifyMode, SessionInfo, SettingKey, State, ViewMode,
};
use crate::theme::{self, BarTheme, SegmentStyle};
use std::fmt::Write;
use std::io::Write as IoWrite;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::{InputMode, PaletteColor, TabInfo};

#[derive(Clone, Copy)]
enum ActivityColor {
    Base,
    Emphasis(usize),
    Success,
    Error,
    Warning,
}

struct ActivityGlyph {
    symbol: &'static str,
    color: ActivityColor,
}

fn activity_glyph(activity: &Activity) -> ActivityGlyph {
    match activity {
        Activity::Init => ActivityGlyph {
            symbol: "◆",
            color: ActivityColor::Base,
        },
        Activity::Thinking => ActivityGlyph {
            symbol: "●",
            color: ActivityColor::Emphasis(3),
        },
        Activity::Tool(name) => ActivityGlyph {
            symbol: tool_symbol(name),
            color: ActivityColor::Emphasis(0),
        },
        Activity::Prompting => ActivityGlyph {
            symbol: "▶",
            color: ActivityColor::Success,
        },
        Activity::Waiting => ActivityGlyph {
            symbol: "⚠",
            color: ActivityColor::Error,
        },
        Activity::Notification => ActivityGlyph {
            symbol: "◇",
            color: ActivityColor::Warning,
        },
        Activity::Done | Activity::AgentDone => ActivityGlyph {
            symbol: "✓",
            color: ActivityColor::Success,
        },
        Activity::Idle => ActivityGlyph {
            symbol: "○",
            color: ActivityColor::Base,
        },
    }
}

fn activity_color(glyph: &ActivityGlyph, tab_style: SegmentStyle, theme: BarTheme) -> PaletteColor {
    match glyph.color {
        ActivityColor::Base => tab_style.base,
        ActivityColor::Emphasis(index) => tab_style.emphasis(index),
        ActivityColor::Success => theme.success.emphasis(0),
        ActivityColor::Error => theme.error.base,
        ActivityColor::Warning => theme.error.emphasis(0),
    }
}

fn fg(color: PaletteColor) -> String {
    theme::foreground(color)
}

fn bg(color: PaletteColor) -> String {
    theme::background(color)
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn take_display_width(value: &str, max_width: usize) -> String {
    let mut width = 0;
    value
        .chars()
        .take_while(|character| {
            let character_width = character.width().unwrap_or(0);
            if width + character_width > max_width {
                return false;
            }
            width += character_width;
            true
        })
        .collect()
}

fn take_display_width_from_end(value: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut characters: Vec<char> = value
        .chars()
        .rev()
        .take_while(|character| {
            let character_width = character.width().unwrap_or(0);
            if width + character_width > max_width {
                return false;
            }
            width += character_width;
            true
        })
        .collect();
    characters.reverse();
    characters.into_iter().collect()
}

fn sanitize_tab_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .collect()
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const NORMAL_INTENSITY: &str = "\x1b[22m";
const ELAPSED_THRESHOLD: u64 = 30;
const SEPARATOR: &str = "\u{e0b0}";

/// Write a powerline arrow: fg=from_bg, bg=to_bg, then separator char.
fn arrow(buf: &mut String, col: &mut usize, from: PaletteColor, to: PaletteColor) {
    let _ = write!(buf, "{}{}{SEPARATOR}", fg(from), bg(to));
    *col += 1;
}

fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

pub(crate) fn elapsed_suffix(session: &SessionInfo, now_s: u64) -> Option<String> {
    // A placeholder has no observed event to measure from.
    if session.placeholder {
        return None;
    }
    let elapsed = now_s.saturating_sub(session.last_event_ts);
    (elapsed >= ELAPSED_THRESHOLD).then(|| format_elapsed(elapsed))
}

/// Permission notices, widest first.
///
/// Every one of them names the click, because that is the only recovery that
/// needs no focus — telling a user to "press y" while their shell has focus
/// sends the keystroke to whatever is running there. Only the widest names the
/// keys, and it names Zellij's defaults: the keybinds live on `ModeUpdate`,
/// which Zellij gates behind the very permission that was just denied, so the
/// plugin cannot know a remapped binding in this state.
///
/// ASCII only so the permission notice renders consistently across terminals,
/// including terminals that choose different widths for ambiguous glyphs.
/// Every variant keeps the word "permission" — naming what is missing is the
/// part that turns a strange-looking bar into something the user can act on.
const PERMISSION_NOTICES: [&str; 4] = [
    " Zellaude needs permission - click here, or Ctrl p then Up, then y ",
    " Zellaude needs permission - click here ",
    " permission needed - click ",
    " perms: click ",
];

/// The widest notice fitting `available` columns, truncated when even the
/// narrowest does not fit. Never empty for a non-zero width: a denied plugin
/// that renders an ordinary-looking bar is the exact failure this prevents.
fn permission_notice(available: usize) -> String {
    if available == 0 {
        return String::new();
    }
    PERMISSION_NOTICES
        .iter()
        .find(|notice| display_width(notice) <= available)
        .map(|notice| (*notice).to_string())
        .unwrap_or_else(|| {
            PERMISSION_NOTICES[PERMISSION_NOTICES.len() - 1]
                .chars()
                .take(available)
                .collect()
        })
}

fn render_permission_notice(
    buf: &mut String,
    col: &mut usize,
    cols: usize,
    prefix_bg: PaletteColor,
    theme: BarTheme,
) {
    // The arrow costs a column; skip it rather than spend the last one on
    // decoration when that would leave no room for the message itself.
    if *col + 1 < cols {
        arrow(buf, col, prefix_bg, theme.flash.background);
    }
    let text = permission_notice(cols.saturating_sub(*col));
    if text.is_empty() {
        return;
    }
    let _ = write!(
        buf,
        "{}{}{BOLD}{text}{RESET}",
        bg(theme.flash.background),
        fg(theme.flash.base),
    );
    *col += display_width(&text);
}

fn mode_style(mode: InputMode, theme: BarTheme) -> (SegmentStyle, &'static str) {
    match mode {
        InputMode::Normal => (theme.settings_prefix, "NORMAL"),
        InputMode::Locked => (theme.error, "LOCKED"),
        InputMode::Pane => (theme.frame_selected, "PANE"),
        InputMode::Tab => (theme.active_tab, "TAB"),
        InputMode::Resize => (theme.highlight, "RESIZE"),
        InputMode::Move => (theme.highlight, "MOVE"),
        InputMode::Scroll => (theme.highlight, "SCROLL"),
        InputMode::EnterSearch => (theme.highlight, "SEARCH"),
        InputMode::Search => (theme.highlight, "SEARCH"),
        InputMode::RenameTab => (theme.highlight, "RENAME"),
        InputMode::RenamePane => (theme.highlight, "RENAME"),
        InputMode::Session => (theme.active_tab, "SESSION"),
        InputMode::Prompt => (theme.settings_prefix, "PROMPT"),
        InputMode::Tmux => (theme.settings_prefix, "TMUX"),
    }
}

pub fn render_status_bar(state: &mut State, rows: usize, cols: usize) {
    let buf = build_status_bar(state, rows, cols);
    print!("{buf}");
    let _ = std::io::stdout().flush();
}

pub(crate) fn build_status_bar(state: &mut State, _rows: usize, cols: usize) -> String {
    state.click_regions.clear();
    state.menu_click_regions.clear();
    state.prefix_click_region = None;

    let theme = state
        .zellij_styling
        .as_ref()
        .map(BarTheme::from_styling)
        .unwrap_or_default();
    let mut buf = String::with_capacity(cols * 4);
    // Terminal setup for a 1-row status bar:
    //  \x1b[H     — cursor home (prevent scroll from cursor at end-of-line)
    //  \x1b[?7l   — disable auto-wrap (clip overflow instead of scroll)
    //  \x1b[?25l  — hide cursor
    buf.push_str("\x1b[H\x1b[?7l\x1b[?25l");
    let bar_bg_str = bg(theme.surface.background);

    if let Some(prompt) = state.custom_layout_prompt.as_ref() {
        render_custom_layout_prompt(&mut buf, prompt, cols, theme);
        return buf;
    }

    // Bail early if terminal is too narrow
    if cols < 5 {
        let _ = write!(buf, "{bar_bg_str}{:width$}{RESET}", "", width = cols);
        return buf;
    }

    let prefix_style = if state.view_mode == ViewMode::Settings {
        theme.settings_prefix
    } else {
        theme.prefix
    };

    // Build prefix: " Zellaude (session) MODE "
    let (mode_style, mode_text) = mode_style(state.input_mode, theme);
    // An inert bar has no mode worth reporting and no session worth naming, and
    // on a narrow split the prefix is what crowds the notice off the row. Give
    // the width to the message instead.
    let show_mode = state.settings.mode_indicator && !state.permissions_denied;
    let session_part = match state.zellij_session_name.as_deref() {
        Some(name) if !state.permissions_denied => format!(" ({name})"),
        _ => String::new(),
    };
    let prefix_text = format!(" Zellaude{session_part} ");
    let prefix_width = display_width(&prefix_text);
    let mode_pill_width = if show_mode {
        1 + mode_text.len() + 1
    } else {
        0
    };
    let total_prefix_width = prefix_width + mode_pill_width;

    // Render prefix segment (truncate if wider than cols)
    let mut col;
    if total_prefix_width <= cols {
        let _ = write!(
            buf,
            "{}{}{BOLD}{prefix_text}{RESET}",
            bg(prefix_style.background),
            fg(prefix_style.base),
        );
        if show_mode {
            let _ = write!(
                buf,
                "{}{}{BOLD} {mode_text} {RESET}",
                bg(mode_style.background),
                fg(mode_style.base),
            );
        }
        col = total_prefix_width;
    } else if prefix_width <= cols {
        // Fit the name part but skip mode pill
        let _ = write!(
            buf,
            "{}{}{BOLD}{prefix_text}{RESET}",
            bg(prefix_style.background),
            fg(prefix_style.base),
        );
        col = prefix_width;
    } else {
        // Even name doesn't fit — just show what we can
        let avail = cols.saturating_sub(2); // leave room for fill
        let short = take_display_width(&prefix_text, avail);
        let _ = write!(
            buf,
            "{}{}{BOLD}{short}{RESET}",
            bg(prefix_style.background),
            fg(prefix_style.base),
        );
        col = display_width(&short);
    }
    state.prefix_click_region = Some((0, col));

    let last_prefix_bg = if show_mode && total_prefix_width <= cols {
        mode_style.background
    } else {
        prefix_style.background
    };
    let prefix_used = col;

    if col < cols {
        // A denied permission leaves the plugin unable to do anything: no hooks,
        // no config, no activity. Say so instead of rendering a bar that looks
        // healthy. This replaces the tabs rather than sharing the row because
        // the whole point is that it must not be mistaken for normal output.
        if state.permissions_denied {
            render_permission_notice(&mut buf, &mut col, cols, last_prefix_bg, theme);
        } else {
            match state.view_mode {
                ViewMode::Normal => {
                    render_tabs(
                        state,
                        &mut buf,
                        &mut col,
                        cols,
                        last_prefix_bg,
                        prefix_used,
                        theme,
                    );
                }
                ViewMode::Settings => {
                    arrow(&mut buf, &mut col, last_prefix_bg, theme.surface.background);
                    let _ = write!(buf, "{bar_bg_str}");
                    render_settings_menu(state, &mut buf, &mut col, theme);
                }
            }
        }
    }

    // Fill remaining width with bar background — never exceed cols
    if col < cols {
        let remaining = cols - col;
        let _ = write!(buf, "{bar_bg_str}{:width$}", "", width = remaining);
    }
    let _ = write!(buf, "{RESET}");

    buf
}

fn render_custom_layout_prompt(
    buf: &mut String,
    prompt: &CustomLayoutPrompt,
    cols: usize,
    theme: BarTheme,
) {
    let full_label = " Custom state ";
    let label = if cols >= display_width(full_label) + 2 {
        full_label
    } else {
        ""
    };
    let mut col = display_width(label);
    let _ = write!(
        buf,
        "{}{}{BOLD}{label}{RESET}",
        bg(theme.settings_prefix.background),
        fg(theme.settings_prefix.base),
    );

    if col > 0 && col < cols {
        arrow(
            buf,
            &mut col,
            theme.settings_prefix.background,
            theme.surface.background,
        );
    }

    let available = cols.saturating_sub(col);
    if available > 0 {
        let hint = prompt
            .error
            .as_deref()
            .unwrap_or("Enter: open | Esc: cancel");
        let hint_budget = if available >= 30 { available / 2 } else { 0 };
        let input_budget = available.saturating_sub(hint_budget);
        let cursor = prompt.cursor.min(prompt.input.chars().count());
        let cursor_byte = prompt
            .input
            .char_indices()
            .nth(cursor)
            .map(|(byte, _)| byte)
            .unwrap_or(prompt.input.len());
        let (before_cursor, after_cursor) = prompt.input.split_at(cursor_byte);
        let padded = input_budget >= 3;
        let around_cursor = input_budget.saturating_sub(if padded { 3 } else { 1 });
        let before_budget = around_cursor * 2 / 3;
        let after_budget = around_cursor.saturating_sub(before_budget);
        let before = take_display_width_from_end(before_cursor, before_budget);
        let after = take_display_width(after_cursor, after_budget);
        let input = if padded {
            format!(" {before}▌{after} ")
        } else {
            format!("{before}▌{after}")
        };
        let input = take_display_width(&input, input_budget);
        let _ = write!(
            buf,
            "{}{}{}",
            bg(theme.surface.background),
            fg(theme.active_tab.base),
            input,
        );
        col += display_width(&input);

        if hint_budget > 0 && col < cols {
            let hint = format!(" {hint}");
            let hint = take_display_width(&hint, cols - col);
            let hint_color = if prompt.error.is_some() {
                theme.error.base
            } else {
                theme.surface.base
            };
            let _ = write!(buf, "{}{hint}", fg(hint_color));
            col += display_width(&hint);
        }
    }

    if col < cols {
        let _ = write!(
            buf,
            "{}{:width$}",
            bg(theme.surface.background),
            "",
            width = cols - col
        );
    }
    let _ = write!(buf, "{RESET}");
}

fn render_tabs(
    state: &mut State,
    buf: &mut String,
    col: &mut usize,
    cols: usize,
    prefix_bg: PaletteColor,
    prefix_width: usize,
    theme: BarTheme,
) {
    let now_s = unix_now();
    let now_ms = unix_now_ms();

    // Sort tabs by position
    let mut tabs: Vec<&TabInfo> = state.tabs.iter().collect();
    tabs.sort_by_key(|t| t.position);

    let count = tabs.len();
    if count == 0 {
        arrow(buf, col, prefix_bg, theme.surface.background);
        return;
    }

    // For each tab, find the best session to display and the pane to focus.
    let best_sessions: Vec<Option<&SessionInfo>> = tabs
        .iter()
        .map(|tab| {
            session_to_display(
                state
                    .sessions
                    .values()
                    .filter(|s| s.tab_index == Some(tab.position)),
            )
        })
        .collect();
    let focus_pane_ids: Vec<Option<u32>> = tabs
        .iter()
        .map(|tab| {
            session_to_focus(
                state
                    .sessions
                    .values()
                    .filter(|s| s.tab_index == Some(tab.position)),
            )
            .map(|session| session.pane_id)
        })
        .collect();

    // Pre-compute elapsed strings (only for agent-aware tabs)
    let elapsed_strs: Vec<Option<String>> = best_sessions
        .iter()
        .map(|session: &Option<&SessionInfo>| {
            if !state.settings.elapsed_time {
                return None;
            }
            session.and_then(|s| elapsed_suffix(s, now_s))
        })
        .collect();

    // Compute overhead: varies per tab type
    let total_elapsed_width: usize = elapsed_strs
        .iter()
        .map(|e: &Option<String>| e.as_ref().map_or(0, |s| s.len() + 1))
        .sum();
    let per_tab_overhead: usize = best_sessions
        .iter()
        .map(|s: &Option<&SessionInfo>| if s.is_some() { 4 } else { 2 })
        .sum();
    let overhead = prefix_width + 2 * count + per_tab_overhead + total_elapsed_width;
    let max_name_len = if overhead < cols {
        ((cols - overhead) / count).min(20)
    } else {
        0
    };

    let mut prev_bg = prefix_bg;

    for (i, tab) in tabs.iter().enumerate() {
        // Stop if we'd overflow — need room for at least arrow + closing arrow
        let arrows_needed = if prev_bg == prefix_bg { 1 } else { 2 };
        if *col + arrows_needed + 3 > cols {
            break;
        }

        let session = best_sessions[i];
        let is_agent = session.is_some();
        let tab_name = sanitize_tab_name(&tab.name);

        // Truncate name
        let name_width = display_width(&tab_name);
        let truncated = if max_name_len == 0 {
            String::new()
        } else if name_width > max_name_len {
            let s = take_display_width(&tab_name, max_name_len.saturating_sub(1));
            format!("{s}…")
        } else {
            tab_name
        };

        // Check flash for any session in this tab
        let is_flash_bright = state
            .sessions
            .values()
            .filter(|s| s.tab_index == Some(tab.position))
            .any(|s| {
                state
                    .flash_deadlines
                    .get(&s.pane_id)
                    .map(|&deadline| now_ms < deadline && (now_ms / 250) % 2 == 0)
                    .unwrap_or(false)
            });

        let is_active = tab.active;
        let has_rainbow_name = state
            .sessions
            .values()
            .filter(|s| s.tab_index == Some(tab.position))
            .any(|s| s.rainbow_name);

        // Pick the complete theme declaration for this tab state so its
        // foreground and background remain a deliberate pair.
        let tab_style = if is_flash_bright {
            theme.flash
        } else if is_active {
            theme.active_tab
        } else {
            theme.inactive_tab
        };
        let tab_bg = tab_style.background;

        // Arrow: close previous segment, then open this tab
        if prev_bg == prefix_bg {
            arrow(buf, col, prev_bg, tab_bg);
        } else {
            arrow(buf, col, prev_bg, theme.surface.background);
            arrow(buf, col, theme.surface.background, tab_bg);
        }

        let tab_bg_str = bg(tab_bg);
        let region_start = *col;

        if is_agent {
            let s = session.unwrap();
            let glyph = activity_glyph(&s.activity);
            let symbol_color = if is_flash_bright {
                tab_style.base
            } else {
                activity_color(&glyph, tab_style, theme)
            };
            let sym_fg = fg(symbol_color);
            let name_fg = fg(tab_style.base);
            let name_bold = is_flash_bright || is_active;

            // Leading space
            let _ = write!(buf, "{tab_bg_str} ");
            *col += 1;

            // Symbol
            let _ = write!(buf, "{sym_fg}{}", glyph.symbol);
            *col += display_width(glyph.symbol);

            // Space + name
            if !truncated.is_empty() {
                let bold_str = if name_bold || has_rainbow_name {
                    BOLD
                } else {
                    ""
                };
                let _ = write!(buf, " {bold_str}");
                if has_rainbow_name {
                    let _ = rainbow::write_rainbow(
                        buf,
                        &truncated,
                        now_ms,
                        tab.position,
                        !is_active,
                        theme::exact_rgb(tab_style.background),
                        theme::exact_rgb(tab_style.base),
                    );
                } else {
                    let _ = write!(buf, "{name_fg}{truncated}");
                }
                let _ = write!(buf, "{RESET}{tab_bg_str}");
                *col += 1 + display_width(&truncated);
            }

            // Elapsed suffix
            if let Some(ref es) = elapsed_strs[i] {
                if *col + 1 + es.len() + 1 < cols {
                    let _ = write!(buf, " {}{es}", fg(tab_style.base));
                    *col += 1 + es.len();
                }
            }

            // Fullscreen indicator
            if tab.is_fullscreen_active && *col + 3 < cols {
                let _ = write!(buf, " {}F{RESET}{tab_bg_str}", fg(theme.error.emphasis(0)));
                *col += 2;
            }

            // Trailing space
            let _ = write!(buf, " ");
            *col += 1;

            state.click_regions.push(ClickRegion {
                start_col: region_start,
                end_col: *col,
                tab_index: tab.position,
                focus_pane_id: focus_pane_ids[i],
            });
        } else {
            // Non-agent tab: no activity symbol.
            let name_fg = fg(tab_style.base);
            let name_bold = is_active;

            // Leading space
            let _ = write!(buf, "{tab_bg_str} ");
            *col += 1;

            // Name only (no symbol)
            if !truncated.is_empty() {
                let bold_str = if name_bold { BOLD } else { "" };
                let _ = write!(buf, "{bold_str}{name_fg}{truncated}{RESET}{tab_bg_str}");
                *col += display_width(&truncated);
            }

            // Fullscreen indicator
            if tab.is_fullscreen_active && *col + 3 < cols {
                let _ = write!(buf, " {}F{RESET}{tab_bg_str}", fg(theme.error.emphasis(0)));
                *col += 2;
            }

            // Trailing space
            let _ = write!(buf, " ");
            *col += 1;

            state.click_regions.push(ClickRegion {
                start_col: region_start,
                end_col: *col,
                tab_index: tab.position,
                focus_pane_id: None,
            });
        }

        prev_bg = tab_bg;
    }

    // Arrow from last tab → bar background (only if we rendered any tabs)
    if prev_bg != prefix_bg || count > 0 {
        arrow(buf, col, prev_bg, theme.surface.background);
    }
}

struct SettingLabel {
    symbol: &'static str,
    label: &'static str,
    symbol_color: PaletteColor,
    label_color: PaletteColor,
    dimmed: bool,
}

fn notify_mode_label(mode: NotifyMode, surface: SegmentStyle) -> SettingLabel {
    match mode {
        NotifyMode::Always => SettingLabel {
            symbol: "●",
            label: "Notify: always",
            symbol_color: surface.emphasis(1),
            label_color: surface.base,
            dimmed: false,
        },
        NotifyMode::Unfocused => SettingLabel {
            symbol: "◐",
            label: "Notify: unfocused",
            symbol_color: surface.emphasis(0),
            label_color: surface.base,
            dimmed: false,
        },
        NotifyMode::Never => SettingLabel {
            symbol: "○",
            label: "Notify: off",
            symbol_color: surface.base,
            label_color: surface.base,
            dimmed: true,
        },
    }
}

fn flash_mode_label(mode: FlashMode, surface: SegmentStyle) -> SettingLabel {
    match mode {
        FlashMode::Persist => SettingLabel {
            symbol: "●",
            label: "Flash: persist",
            symbol_color: surface.emphasis(1),
            label_color: surface.base,
            dimmed: false,
        },
        FlashMode::Once => SettingLabel {
            symbol: "◐",
            label: "Flash: brief",
            symbol_color: surface.emphasis(0),
            label_color: surface.base,
            dimmed: false,
        },
        FlashMode::Off => SettingLabel {
            symbol: "○",
            label: "Flash: off",
            symbol_color: surface.base,
            label_color: surface.base,
            dimmed: true,
        },
    }
}

/// Render a three-state toggle and register its click region.
/// Assumes the caller has already set the desired background color.
fn render_tristate(
    buf: &mut String,
    col: &mut usize,
    state_regions: &mut Vec<MenuClickRegion>,
    key: SettingKey,
    appearance: SettingLabel,
) {
    let region_start = *col;
    let width = display_width(appearance.symbol) + 1 + appearance.label.len();
    *col += width;

    state_regions.push(MenuClickRegion {
        start_col: region_start,
        end_col: *col,
        action: MenuAction::ToggleSetting(key),
    });

    let intensity = if appearance.dimmed {
        DIM
    } else {
        NORMAL_INTENSITY
    };
    let _ = write!(
        buf,
        "{intensity}{}{} {}{}{NORMAL_INTENSITY}",
        fg(appearance.symbol_color),
        appearance.symbol,
        fg(appearance.label_color),
        appearance.label,
    );
}

fn render_settings_menu(state: &mut State, buf: &mut String, col: &mut usize, theme: BarTheme) {
    // Leading space after arrow
    let _ = write!(buf, " ");
    *col += 1;

    // --- Notifications (three-state) ---
    {
        let appearance = notify_mode_label(state.settings.notifications, theme.surface);
        render_tristate(
            buf,
            col,
            &mut state.menu_click_regions,
            SettingKey::Notifications,
            appearance,
        );
    }

    // --- Flash (three-state) ---
    {
        let _ = write!(buf, "  ");
        *col += 2;
        let appearance = flash_mode_label(state.settings.flash, theme.surface);
        render_tristate(
            buf,
            col,
            &mut state.menu_click_regions,
            SettingKey::Flash,
            appearance,
        );
    }

    // --- Elapsed time (bool) ---
    {
        let _ = write!(buf, "  ");
        *col += 2;
        let enabled = state.settings.elapsed_time;
        let appearance = if enabled {
            SettingLabel {
                symbol: "●",
                label: "Elapsed time: on",
                symbol_color: theme.surface.emphasis(1),
                label_color: theme.surface.base,
                dimmed: false,
            }
        } else {
            SettingLabel {
                symbol: "○",
                label: "Elapsed time: off",
                symbol_color: theme.surface.base,
                label_color: theme.surface.base,
                dimmed: true,
            }
        };
        render_tristate(
            buf,
            col,
            &mut state.menu_click_regions,
            SettingKey::ElapsedTime,
            appearance,
        );
    }

    // --- Mode indicator (bool) ---
    {
        let _ = write!(buf, "  ");
        *col += 2;
        let enabled = state.settings.mode_indicator;
        let appearance = if enabled {
            SettingLabel {
                symbol: "●",
                label: "Mode indicator: on",
                symbol_color: theme.surface.emphasis(1),
                label_color: theme.surface.base,
                dimmed: false,
            }
        } else {
            SettingLabel {
                symbol: "○",
                label: "Mode indicator: off",
                symbol_color: theme.surface.base,
                label_color: theme.surface.base,
                dimmed: true,
            }
        };
        render_tristate(
            buf,
            col,
            &mut state.menu_click_regions,
            SettingKey::ModeIndicator,
            appearance,
        );
    }

    // Close button
    let _ = write!(buf, "  ");
    *col += 2;
    let close_start = *col;
    let _ = write!(buf, "{}×", fg(theme.error.base));
    *col += 1;

    state.menu_click_regions.push(MenuClickRegion {
        start_col: close_start,
        end_col: *col,
        action: MenuAction::CloseMenu,
    });
}
