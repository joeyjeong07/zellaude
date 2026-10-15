# Review guide — optimize-hook

## Read in this order

## Already checked

## Decisions with user

### `dump-layout` result accepted, further work parked as optimize-speed
- `dump-layout` on a busy 8-tab session: the branch never returned empty in 280 diagnostic calls (`develop`: 9–37 of every 40), and its slowest call was 0.97 s (`develop` ~1.05 s). It missed the PLAN's p95 < 0.5 s in 3 of 7 blocks (0.62–0.80 s, with plugin-exec near its ceiling), and an earlier run had 2 unexplained fast empties.
- Options, ranked: 1. (rec) accept, and park the per-event update fan-out as a seed · 2. investigate the fan-out in this branch first.
- Selected: 1. The user, verbatim: "1. And I want further optimizations from this seed to be feature/optimize-speed instead, to have not only the that thing but to continuously optimize the whole system, because I think there's still more to optimize." Parked as `feature/optimize-speed`, a standing line holding the fan-out, slow blocks, fast empties, the `SessionUpdate` rate, pull delivery and the 8 × 8 deadlock.

### Throttling arm with the user's idle processes on km-kim-4
- T12's preflight wanted no other user process on km-kim-4. The user's idle `asterorank_trainer-4` session, `nvitop` and shells were there; km-kim-1/2/3 were unusable.
- Options, ranked: 1. (rec) run it with the idle processes present · 2. (rec) the user frees the pod first · 3. skip the arm, so keystroke latency under throttling goes unmeasured.
- The user's answer, verbatim: "Currently km-kim-4 is doing nothing. But I tell you I might use it again though." That is a fact and a caveat, not an option number.
- Rev 12: both runs aborted on short-lived children of the user's idle `asterorank_trainer-4` server: its deployed plugin's `run_command` work, the second time with a direct parent chain. Options: (1, rec) count such children as idle baseline unless they use ≥ 0.2 CPU-s/s together or one lives > 2 s · (2) ask the user to close that session for the window · (3) report the arm as not run in band. Selected: 1. It keeps the user's meaning, yield on use, and those children are not use.
- Read as: run while idle, yield on use. The arm starts only while the user's processes (anything not descended from the probe) and the GPUs are idle, a 1 s monitor kills the burner within 2 s of any user activity, the whole arm is capped at 40 minutes, and the user is warned before it starts.

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
- Again at T16: the same plan doc's step at :1234 anchored on `maybe_install_runtime_bindings`, which T16 removes. Edited inside T16, sentence-only, to name `maybe_retarget_runtime_bindings(BindingsCheck::HostFocus)`.

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

### Binding target tracked by broadcast (rev 5)
- impl2 confirmed T16's cause differs from the suspicion: zellij 0.44.3 erases `KeybindPipe`'s fields, `plugin_id` included, in the `InitialKeybinds` snapshot, so no check can read the target from it.
- Options, ranked (impl2's, plus one addition):
  1. (rec) Broadcast the target (`zellaude:bindings-target`), record it per instance, and reconfigure once, joined, only when the record is not this instance or zellaude's keys are missing — 0 reloads for repeated updates on one tab, 1 per visit / a new pipe name and field, and a multi-client ping-pong no worse than today.
  2. Reset only when another tab is seen active — no new pipe / a quick away-and-back inside one `SessionUpdate` interval can silently misroute keys.
  3. Keep the reset and only join the snippets — smallest change / halves the reloads but misses the ≤ 2 bound.
- Selected: 1. The missing-keys clause is my addition: it keeps today's self-repair after a user config reload, which option 1 alone would lose.
- Pinned at checkpoint 4 (round 2): a received target message re-runs the check, so a quick A→B→A switch cannot leave tab A's keys on B.
- Pinned at checkpoint 4 (round 3): the target is recorded per client (`PluginIds.client_id` in the payload), and a received message triggers at most one retarget per activation, so two attached clients cannot retarget each other forever. Verification adds a two-client idle check.

### "Active" from the host for receipt-triggered retargets (rev 6)
- impl2 found, from zellij 0.44.3 source, that `TabUpdate` reaches only plugins on each client's active tab. A just-hidden instance keeps believing it is active, so rev 5's receipt-triggered retarget could fire from up to k−1 hidden instances and leave the keys on a hidden tab.
- Options, ranked (impl2's):
  1. (rec) Ask the host (`get_focused_pane_info`) only for receipt- and snapshot-triggered checks; cache "not here" until the next `TabUpdate`; Err or timeout = not here, never retried — truthful, about one query per visit / a blocking host call on a hidden instance, bounded at 100 ms.
  2. The sender's activation time in the payload — no host call / changes the pinned payload, and plugin-thread processing order can invert under load, recreating the A→B→A race.
  3. Drop receipt- and snapshot-triggered retargets — simplest / A→B→A leaves the keys on B until A's next `TabUpdate`.
- Selected: 1. "Never retried, never on a timer" is mine, from the August livelock: that was a retried host call with a 100 ms deadline.

### Host-judged "active" on every trigger but TabUpdate (rev 7)
- impl3 measured that today the split-three key and the custom-state prompt key do nothing on a fresh 8-tab session's starting tab, fail on tabs 1 and 4 after a re-attach, and fail in both tabs with a second client. impl2 traced it from source: a fresh session makes every tab briefly active, the grant reaches all 8 instances, all install, and a hidden one wins. Rev 6 host-judged only receipts and snapshots, so it would not fix this, and in a detached session `PaneUpdate` (reaching every instance) would trigger a reconfigure storm.
- Options, ranked:
  1. (rec) Host-judge every trigger except `TabUpdate`, with rev 6's rules (cheap checks first, "not here"/Err/timeout cached until the next `TabUpdate`, never re-asked) — fixes all three cases and keeps detached sessions quiet / up to one query per instance per attach.
  2. Also host-judge `TabUpdate` — closes the late-`TabUpdate` residual / a timed-out query is re-asked on the same visit's next `TabUpdate`.
  3. Leave rev 6 — fewer queries / the starting-tab failure and the detached storm remain.
- Selected: 1. The residual is repaired by the focused instance's receipt, once per activation.

### Keys were less reliable mid-run, before T16
- impl3 measured first visits after a re-attach: the branch at 7585d2e (before T16) failed 7 of 24 split-three and 6 of 24 prompt visits, against `develop`'s 3 of 24. An earlier row made the keys less reliable for a while; which one is not traced. T16 replaces the whole binding mechanism, and rev 8's pass condition (0 failures over ≥3 repeats of 8 visits) tells the fix apart from both baselines: chance alone would pass it about 4 % of the time at `develop`'s rate.
- Options: trace which earlier row worsened it / rely on T16's replacement plus the pass condition. Selected: rely on T16 — the mechanism that failed no longer exists once T16 lands. If T16's pass condition fails, the trace comes back.

### Split-three key failures were the test harness's load
- T12's first keys run showed 5 of 24 first-visit failures, and the fresh starting tab failed 2 of 3. It polled `zellij action list-panes --all` ten times a second, and the split-three state machine needs that same server. With a lighter harness (5 polls/s, 20 s window): the branch passed 0 failures over 3 repeats, fresh tab included; `develop` had 1 slow visit. Under the heavy harness the branch failed 1 of 16 against `develop`'s 7 of 16.
- Options: suspect T17 and trace the state machine / re-measure with a lighter harness first. Selected: re-measure — no stuck case on the branch, so T17's timer change is cleared. Split-three's sensitivity to server load is pre-existing, and the branch improves it.

### Two-client keys become a stated residual (rev 10)
- T12 under the lighter harness: with a second client attached, the first client's split-three and prompt keys never acted — branch stuck 3 of 3, `develop` 2 of 2. The second client's acted sometimes on both builds. The branch's own target, no reload storm with two clients, passed: 0 `InitialKeybinds` in all 8 instances, 3 of 3 runs.
- Options, ranked:
  1. (rec) Keep the reload check as the pass and state two-client keys as a residual — the branch matches `develop` there, and multi-client key routing is a feature bug, not this branch's CPU goal / the keys stay broken with two clients.
  2. Trace and fix multi-client routing here — fixes it / depends on how zellij scopes focus and actions per client, unknown, and outside the user's stated goal.
- Selected: 1, with the fix proposed to the user as a seed.

### Keys on zellij 0.44.0, and the two-client prompt
- On the 0.44.0 release (T16 changed its legacy `ModeUpdate` path): the branch's prompt key was on time 27 of 27, and split-three was on time 15, slow 11, stuck once. On `develop`, 3 of 4 runs ended early on a stuck prompt. Selected: old behaviour, not a regression — split-three is slow on 0.44.0 for both, and the branch's prompt is better there.
- Two clients: the prompt opened visibly in client A's bar every time (4 runs). A's next keystrokes went into that visible prompt, as an open prompt should; no hidden prompt swallowed keys. The earlier "stuck" prompt followed A's own split attempt, likely the split still running (unverified).

### E2E pass 1 settles, pass 6 pools (rev 11)
- T12's rev-2 E2E (two runs, final HEAD 28bbe2b): every stream invariant and passes 2–5 held. Pass 6 per block: 7 of 8 under 0.5 s, one at 0.518 s (n = 16). Run 2 pooled: n = 69, p95 0.498 s. Pass 1: one read caught a bar's first-visit blank frame, which predates the branch and is shorter on it (numbers in the PLAN).
- Options, ranked:
  1. (rec) Pool pass 6 per run, and read pass 1 at least 1 s after the switch — sound statistics and a fair read / pass 6 still sits at its bar, not inside it.
  2. Keep per-block p95 and re-run until a set passes — no rule change / tests the noise in a 16-sample p95, not the branch.
  3. Raise pass 6's bar — passes easily / moves the goalpost.
- Pooling alone did not rescue run 1 (p95 0.518 s). The tail is one event: the `SessionStart` that `/clear` fires 20 ms after its `SessionEnd`, waiting behind a running tool pipe and then behind its own `SessionEnd`, as the design orders on purpose. Without the `/clear` pairs, p95 is 0.430 s. An urgent lane (urgent events never wait behind a coalesced pipe) would cut only the tool-pipe part (estimate p95 0.457 s), at the price of two pipes in flight per session.
- Selected: pool per run; give the `/clear` `SessionStart` its own bound, p95 ≤ 1.0 s (two pipe-times), so that path stays gated (checkpoint 4 round 2: ungated would let a lock-ordering regression pass); no urgent lane. `develop`'s baseline under the same burst (p50 18.2 s, p95 26.8 s) shows the bar is about the branch's own tail, not the user's experience. The user is told.
