# Review guide — optimize-hook

## Read in this order

## Already checked

## Decisions with user

## Decisions without user

### Settings menu width with the Rainbow row
- Reported by impl2 during T7. The menu never clipped to the bar's width; that predates the branch. The new Rainbow row widens it from 116 to 131 visible columns, so on bars narrower than ~131 columns the close × falls off the right edge (before: below 116). Clicking the Zellaude prefix still closes the menu.
- Options, ranked:
  1. (rec) Accept, propose a seed — no scope beyond T7's pinned change; the menu still closes from the prefix / the × is hidden on 116–131-column bars.
  2. Fix now: clip the menu so the × stays visible — removes the regression / a render.rs layout change outside the pinned design.
  3. Shorten the Rainbow label — less growth / cannot win back the 15 columns, and only cosmetic.
- Selected: 1 — the branch's goal is CPU and latency, the lost × has a working alternative, and the root cause (no clip) is older than this branch.

### Session-template docs follow T8
- Raised by impl2-crit1 at T8. Two dated docs say templates compile "once per plugin instance": `docs/superpowers/specs/2026-08-07-session-templates-design.md:165-166` and `docs/superpowers/plans/2026-08-07-session-templates.md:7`. After T8 that is false. The repo treats these docs as live (f6d0ec9 corrected one when behavior changed).
- Options, ranked:
  1. (rec) Fix both sentences inside T8 — the docs stay true, and nobody else owns `docs/` / two files outside the board's T8 list.
  2. Leave them — no scope change / the docs then describe behavior that no longer exists.
- Selected: 1, with impl2's sentence-only wording.

### Q3 scan rule keeps SessionRestore
- Reported by impl1 after T2 (cbae1dd), accepted by impl1-crit1. `SessionRestore` is the event `zellaude-attach.sh` sends through `--inspect` to rebuild a pane after attach. It is not a Claude hook event, so the PLAN's "SessionStart and UserPromptSubmit only" did not name it. Recovery needs the `/effort` scan, and five suite cases pin it.
- Options, ranked:
  1. (rec) Keep scanning on `SessionRestore` and name it in the PLAN and T2 — attach recovery stays correct; one scan per pane per attach / none worth noting.
  2. Skip the scan there too — matches the PLAN's literal text / a recovered pane would lose its ultracode state until the next prompt, and five pinned cases break.
- Selected: 1. The rule's reason ("`/effort` cannot change mid-turn") is about hook events; recovery starts from nothing.

### Coalescing stays a debounce, no max-wait cap
- Raised by impl1-crit1 at T4 (d8b857e). As specced, the window restarts on every event in a stream, so a stream whose gaps stay under 0.2 s pipes nothing until a gap appears. Parallel subagents share one stream per pane, so a heavy fan-out can hold that pane's in-turn glyph stale: the critic's rough Poisson estimate is ~1 s at 8 subagents each firing once a second, ~3 s at 20.
- Options, ranked:
  1. (rec) Keep the debounce — the most pipes saved exactly during a fan-out storm, when CPU matters most; urgent events (`Stop`, `PermissionRequest`, …) still deliver at once / an in-turn glyph can trail by seconds in a storm.
  2. Cap the wait at the window (throttle) — glyph never trails by more than ~0.2 s / up to 5 pipes a second per stream during the storm.
  3. Cap at ~1 s — bounds the trail / a constant the user never chose, between the two.
- Selected: 1. The user put interactions first and accepted a lagging in-turn glyph; done and waiting are never late. The user has been told and can override.

### Hook header comment goes to T11
- `scripts/zellaude-hook.sh:2` still calls the hook a "zellij pipe bridge" after T4. Options: fix it in T5 (T5 never touches the hook) / route it to T11's comment pass. Selected: T11, sent to impl3.
