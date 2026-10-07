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

### One pipe in flight per session (rev 2)
- T12's E2E failed passes 1 and 2 whenever the server's pipe queue passed 1 s: the PLAN's 1 s reaper killed clients mid-pipe, which drops the event for instances not yet reached and triples the server's cost, so the queue never recovered (impl3, km-kim-4; numbers in the PLAN).
- Options, ranked:
  1. (rec) One pipe in flight per session plus a 5 s reaper — removes the cause; bounded live clients; under overload each stream still ends on its latest state / more design in T4's area, and an urgent event can wait behind one pipe or be superseded while waiting.
  2. Only raise the reaper to 5 s — one constant, and staggered load already passed with it / concurrency stays unbounded (up to rate × 5 s live clients, near the earlier 8 × 8 deadlock), and heavy load still lost 14 of 76 urgent events per bar.
  3. Keep 1 s and loosen the pass conditions — no code / the bar stays wrong under load, which is the user's complaint.
- Selected: 1, as a new row T14 on impl1. T12 waits on it for every arm the change touches.
- Pinned at checkpoint 4 (round 2): `SessionEnd` is never superseded (T6's cleanup and the tombstone ride on it, and `/clear` sends it right before `SessionStart`); urgent events jump coalesced waiters; a stale lock is judged by age, not pid; a waiter's bound (3 × the reaper) exceeds the reaper, and on give-up an urgent event pipes anyway. Verification adds urgent hook-fire→bar p95 ≤ 0.5 s and a count of superseded urgent events.

### README follows T14 (rev 3)
- Reported by impl2: T9's README sentence (56e4333, `README.md:578`) says urgent events "go out at once". After T14 they queue behind the session's one running pipe, and a superseded non-`SessionEnd` urgent event never pipes.
- Options, ranked:
  1. (rec) A new row T15 on impl2 after T14, before T12, with impl2's wording — the README stays true, and the author of T9 fixes its own sentence / one more row.
  2. Fold it into T11-style cleanup at the end — no new row / T11 is closed, and the verifier would read a false README.
- Selected: 1.

### T14 review findings left in
- From impl1-crit1 at T14 (31f33ed). Options for each: fix now / accept / defer as a seed.
  - A `UserPromptSubmit` superseded under backlog drops its rainbow scan, since only `SessionStart` and `UserPromptSubmit` scan. A rainbow change then stays unseen until the next prompt. Selected: accept — cosmetic, overload only.
  - A deliverer SIGKILLed mid-pipe leaves its urgent marker live for 22 s, longer than the 15 s give-up. Measured: the next urgent event piped after 14.7 s, and a coalesced event behind it was dropped. Selected: accept — only a single-process kill or OOM shows it, and a pod restart outlasts 22 s.
  - Each waiting deliverer polls with `sleep 0.1`, about 10 execs a second, bounded by the collapse to each stream's newest (~160 execs/s at 16 waiters). Selected: accept, pending T12's CPU numbers.
  - A two-step race could let a stale `UNREADABLE_LOCK_SINCE` clock remove a waiter's fresh lock. Selected: fix now, a one-line follow-up commit to T14 reviewed by impl1-crit1, since T14 had already flipped.

### Fix two older plugin costs in this branch (rev 4)
- Found by T12's re-attach arm and impl3's control and root-cause probes (numbers in the PLAN). Both causes predate the branch: a keybinding reconfigure on every `TabUpdate`, about two thirds of a visit's cost; and a timer chain leaked per re-attach, growing without bound.
- Options, ranked:
  1. (rec) Fix both here as T16 and T17 on impl2, keeping the re-attach bar — they sit on tab switching and steady plugin CPU, the user's named symptoms, and the delegation covers in-scope optimization / two more rows and another verification pass.
  2. Restate the bar to the attach phase and park both as seeds — the run closes sooner / leaves the largest per-visit cost and an unbounded leak in place.
  3. Fix only the timer leak — one small row / leaves two thirds of each visit's cost.
- Selected: 1. The user is told and can push either one to a seed.
