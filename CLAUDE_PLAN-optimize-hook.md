## Plan: Spend less of the pod's CPU quota

zellaude spends CPU in four places it controls: the hook's forks per event, zellij's per-tab broadcast of every event, a cubic sync burst on every attach, and plugin timers left running by a leaked flash or by rainbow. On a CPU-capped pod that spend throttles everything at once, keystrokes and bar clicks included. The shipped `zellij pipe` also loses hook events intermittently, leaving stale glyphs. This branch fixes that and cuts all four costs: a fork-lean hook; a detached, coalescing deliverer that uses `zellij action pipe` (lost none in every flushed block) and skips sessions with no client; a quiet attach; the Persist-flash fix and a rainbow toggle.

**Context**
- **Problem.** The user's fleet pod has a 16-CPU cgroup quota (`cpu.max 1600000/100000`) and throttles in 6–14 % of 100 ms periods; a throttled period stalls every thread in the pod together. In the user's words: "I don't need everything to be very fast — but want the display of top bar & bottom bar & the moving operations, drag operations, keyboard input + basically user interactions to be very fast. Because if this laggy, I have to wait."
- **Cost today:**
  - Hook: 53 execs per tool event and 68 per `SessionStart`, counted on km-kim-0-0 with `mkdir`/`rmdir`/`chmod` shimmed.
  - Measured on the idle 32-CPU pod km-kim-4, isolated:
    - Delivery: one hook event costs zellij 0.6–1.7 server CPU-s at 8 tabs, across blocks and both pipe commands: ~0.74 CPU-s per 100 ms, about 45 % of one period's 1.6 CPU-s quota (interpolated).
    - Attach: ~10 pod CPU-s within ~1.3 s at 8 tabs.
  - Persist flash: a pane that ends while flashing in Persist mode keeps a 0.25 s re-render ticker alive in every instance, forever.
- **A symptom that breaks a tool:**
  - Reported from my-terminal-setup on 2026-10-07: `zellij action dump-layout` passes through the plugin stage, so it waits behind zellaude's per-event work. Past zellij's 1 s action timeout (`ACTION_COMPLETION_TIMEOUT`, route.rs, 0.44.3) it returns EMPTY output with exit 0, which crashed `zellmv restart`.
  - On km-kim-0-0's my-terminal-setup-0 (6 bars, 34 panes, a few active claudes): plugin-exec threads at 280–470 % CPU in every 1 s window of a 20 s run; `dump-layout` 0.31–0.47 s, up to 1.03 s. A fresh session takes 0.06–0.09 s, and `query-tab-names`, which skips the plugin stage, 24 ms.
  - Reproduced on km-kim-4 (8 tabs, 2 tool pairs/s, 40 calls per block, 2 blocks): today's delivery gave p95 1.04 s and 0.56 s, with 5 empty outputs in one block. Simulated branch delivery (each pair's trailing Post piped 0.2 s later via `action pipe`, no branch hook involved) gave p95 0.24 s and 0.14 s, with none empty. Measured on the branch itself: see *Decisions › With user › `dump-layout` result accepted*.
  - Scope: the branch removes zellaude's share of the queue `dump-layout` waits behind, not all of it. Plugin-exec and wasm threads also burn CPU with no plugin loaded, driven by terminal redraw (A/B, 1 tab). So much of the 280–470 % on a 34-pane session is likely zellij's own terminal work; the km-kim-4 repro had little redraw and isolates zellaude's share.
  - Under throttling plus terminal load, `dump-layout` can still pass 1 s and return empty with exit 0. zellmv must keep treating empty output as an error (its retry guard in my-terminal-setup); that fix lives outside this branch.
  - The user's `zellaude.json` has `flash: Persist`, so the flash leak below applies to them.
- **Biggest lever, outside this branch:** the pod's CPU limit; the user has been told.
- **Branch:** `feature/optimize-hook`, from the parked seed of that name; the finalize chain merges `develop`.

Mode: ask. The user delegated in-scope design choices ("simply do all good and meaningful optimization in this branch that's related to my issue"), recorded as *Without user*. PLAN approval and archival stay theirs.

**Decisions**
- *With user:*
  - **Goal:** interactive latency under throttling comes first: the bar drawing and answering clicks, moves, drags and keystrokes. Asked whether "the top bar must be very fast" also covers how quickly a tool glyph updates, the user answered only drawing and clicks, so the in-turn tool glyph may lag a little.
  - **`dump-layout` result accepted (2026-10-16):** the user accepted the branch's measured `dump-layout` and parked further work as `feature/optimize-speed`. That is a standing optimization line for the whole system, starting with the per-event update fan-out. Measured on km-kim-4 at 2 tool pairs/s, 8 tabs:
    - empty output: `develop` 9–37 of every 40 calls; the branch 0 of 280 diagnostic calls, with 2 fast empties in one earlier run, unexplained;
    - slowest call: `develop` ~1.05 s; the branch 0.97 s;
    - p95: the branch 0.25–0.48 s in most blocks. Over the arm's kept pairs, 2 of 6 missed (0.661, 0.752 s); across all 13 branch blocks, 5 missed (0.62–0.80 s), each with plugin-exec near its ceiling. So the PLAN's p95 < 0.5 s pass condition is missed in those blocks, and the user accepted that.
  - **Coalescing window:** 0.2 s, the user's pick from the measured curve: 0.05 s cuts 8 % of deliveries, 0.1 s 12 %, 0.2 s 18 %, 0.3 s 20 %, 0.6 s 31 %, 0.9 s 41 % (49 live events, quiet window, indicative only).
  - **Hook scope (Q1):** optimize the hook script and add `async: true` to the Codex hook entry; event lists stay untrimmed (trimming was the rejected option).
  - **Agent-pid lookup (Q2):** memoize the agent pid. On a miss, Linux climbs `/proc` with shell builtins and macOS uses one batched `ps` + `awk` (the user: "I want the macOS to get optimized hook too").
  - **Transcript scan (Q3):** scan Claude transcripts only where ultracode can have changed.
  - **Rainbow toggle:** a setting to turn rainbow display off for users who don't need it; when on, it still skips scans per Q3.
  - **Delivery cost in branch (QC):** yes, plus the delegation quoted in Context.
- *Without user:*
  - **`zellij action pipe` replaces `zellij pipe`:**
    - Delivery: 40/40 events in all three flushed arms, where `zellij pipe` delivered 33, 36 and 38 of 40 in the same block: it loses events intermittently on an idle box.
    - Cost: measured ~40 % less server CPU per event in early 2-pair blocks. T12's gated 5-block re-measure at 8 tabs found about the same per delivered urgent event (median 1.53 vs 1.65 CPU-s, overlapping ranges). So the branch's saving does not come from a cheaper pipe. It comes from fewer pipes: in the E2E's overload block the branch piped 18–24 times for ~232 hook events, where `develop` pipes every event. The server-cost arm sent urgent events only, one pipe each on both builds (30 per block, alternating `UserPromptSubmit` and `Stop`), so there per urgent event and per pipe are the same measure.
    - Reach: every tab's instance with a client attached; into a client-less session it delivers the same as `zellij pipe`. Every re-attach rebuilds plugin instances anyway, so no per-session command choice is needed.
    - Availability: documented in zellij's CLI-actions reference and measured on 0.44.3. Pipes date from 0.40.0, but `action pipe` at the README's 0.44 floor is unverified; Verification checks it.
  - **Urgent vs coalesced delivery:**
    - *Urgent events* (`SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PermissionRequest`, `Stop`) set the states the user waits on; they deliver at once and supersede a pending event of their stream.
    - *Coalesced events:* everything else, per stream. A stream is one pane's root agent or its subagents, so a subagent event can never displace the root event that owns the pane.
      - *Last writer delivers:* within `DELIVERY_COALESCE_SECONDS` (0.2 s, With user), only a burst's trailing event is piped; dropping it instead would leave a stale glyph.
      - *Notification* only refreshes a timestamp: it never supersedes, and it is dropped while its stream has an event pending.
    - *Live sample:* the 0.2 s window cut deliveries by 18 % (curve under *With user*). That count treated every event as coalesced, so exempting urgent events lowers it.
  - **Skip sessions nobody is looking at:**
    - After its window, the deliverer pipes a coalesced event only if zellij's session metadata (`<zellij cache>/contract_version_1/session_info/<session>/session-metadata.kdl`) reads `connected_clients` > 0. Missing or unparseable data means deliver.
    - Why: a client-less session still spends 0.7–1.1 server CPU-s per event at 8 tabs, and the next attach throws that work away. The hook still writes every root event to the state cache, which the fresh instance's attach scan reads.
    - Lag: the count updates within ≤1 s. Urgent events skip the check, so the attach lag can only leave a mid-turn tool glyph stale until that pane's next event.
    - Paths: Linux `${XDG_CACHE_HOME:-$HOME/.cache}/zellij`; macOS `~/Library/Caches/org.Zellij-Contributors.Zellij`, unverified (fail-open means a wrong guess only loses the skip).
    - No staleness guard. zellij rewrites the file on change, so an idle session's mtime just ages and an age check would deliver into every idle detached session. `rename-session` moves the directory and the socket together. A dead server's socket is gone, so its pipe would fail anyway.
  - **No explicit pipe release:**
    - Proposed: call `unblock_cli_pipe_input` to stop each `zellij pipe` waiting until the 1 s limit.
    - Measured: a build that calls it in `pipe()` gave a median of 0.905 s per pipe, against 0.925 s shipped (8 tabs, km-kim-0-0, same block). On idle km-kim-4 both commands release in 0.17–0.19 s without it.
    - Inferred from the queue behaviour, not measured directly: a pipe completes when every instance has processed it, so the wait, and the "CliPipe did not complete within 1s timeout" log lines, are queue time. Fewer and cheaper deliveries shorten that queue. The hand-off below means the hook never waits on it.
  - **Detached hand-off:**
    - The hook returns once it hands off. Today it waits up to 1 s for zellij: Codex's synchronous hook stalls the agent that long, and once the entry is async, each such hook would hold one of Codex's 8 background slots.
    - A detached deliverer does the rest (own process group, stdio closed per commit bfda51f's lesson, a reaper for a session that never answers) and must leave no orphans: today's watchdog kills its subshell but leaves the `sleep` reparented to init (measured).
  - **One pipe in flight per session, 5 s reaper** (rev 2, from T12's E2E):
    - Failure measured with the original 1 s reaper (km-kim-4, 8 tabs): once the server's pipe queue passed 1 s, every client was reaped. A client killed while its pipe still waits behind others loses the event for every instance not yet reached (on an idle queue a client killed at 50 ms still delivered 40/40), and triples the server's cost (4.54 vs 1.49 CPUs over 20 pipes at 2/s), so the queue never recovered, even at 0.5 pipes/s. Bars ended with 67–73 of 76 urgent events staggered and 44–54 under heavy load (`develop`: 51–52 and 25–61). The same branch with a 5 s reaper passed every staggered check.
    - Design: a per-session delivery lock, fork-free and kept beside T4's stream markers, lets one pipe run at a time. A deliverer that takes it re-checks that it is still its stream's newest, so events that pile up behind a running pipe collapse to each stream's latest.
    - Urgent events skip the window and take the lock ahead of coalesced waiters: a coalesced waiter yields while any urgent event of the session is waiting. A newer event of its own stream may supersede a waiting urgent event, except `SessionEnd`, which always pipes. It sets the plugin's tombstone and drops the pane's flash deadline (T6), and `/clear` fires `SessionEnd` then `SessionStart` back to back in one pane.
    - Bounds: the running pipe is reaped at `DELIVERY_REAPER_SECONDS=5`. A lock older than the reaper plus 2 s is stale and broken, judged by age, never pid (pids get reused). A waiter gives up after 3 × the reaper, longer than any legitimate holder; a coalesced event is then dropped, and an urgent one pipes without the lock.
    - Why over only raising the reaper: concurrency is the cause. One pipe per session keeps the server's queue near empty, bounds live pipe clients to one per session (no pile-up like the 8 × 8 deadlock seen earlier), and under overload degrades to each stream's latest state instead of dropping arbitrary events. zellij tops out near 6–7 pipes/s at 8 tabs, so only fewer pipes help there.
    - Cost: an urgent event waits behind the running pipe and any urgent events ahead of it: ~0.1–0.2 s per pipe idle, ~0.7–1.1 s per pipe on the throttled pod at 8 tabs. A non-`SessionEnd` urgent event superseded while waiting never pipes.
  - **Quiet attach:**
    - *Sync:* only an instance holding a non-placeholder session answers a `zellaude:request`. Today every fresh instance answers every request with a broadcast to all: 64 syncs reach each instance at 8 tabs (N³). Removing the grant-time sync alone halved an 8-tab re-attach (10.2 → 5.4 pod CPU-s).
    - *Attach work once:*
      - The active tab's instance runs the session-wide work: the installer, the attach scan and the session-template compile.
      - Only when all three succeed does it broadcast a one-shot `zellaude:attach-done`. Each receiver marks all three done for its lifetime.
      - Today every instance installs and compiles templates on every attach (per-instance flags, no tab gate), and every first visit to a tab after an attach runs another scan.
      - If any of the three fails, nothing is broadcast. Hidden instances then do the work on first visit, as today, so a failed scan or install still gets retried.
    - *Known trade:* dropping the first-visit scan also drops a quiet repair.
      - Case: a coalesced event skipped during the ≤1 s attach lag that landed after the scan. The next tab's scan used to pick it up; now only that pane's next event repairs it.
      - Urgent events always deliver, so this stays bounded to mid-turn tool glyphs.
    - *Config load* stays per instance, because every instance needs settings. What it cost before was its per-instance template compile, which now runs once.
  - **Two older plugin costs found by T12, fixed here** (rev 4):
    - Measured on km-kim-4: a first tab visit after a re-attach costs 5.1 pod CPU-s with zellaude's bar (7.1 on `develop`), against 0.17 with zellij's own tab-bar and 0.13 with no bar. 0.7–1.4 s of it is plugin code; the rest is zellij delivering events to the 8 instances. Switching itself still shows in ~0.05 s.
    - *Keybinding reloads per visit:* every `TabUpdate` on the active instance resets the runtime-binding flags, so each visit reconfigures the client's keybindings again. All 8 instances got ~14 `InitialKeybinds` each per visit, about 7 reconfigures where retargeting needs at most 2. A probe that skipped the reset cut a visit from 6.3 to 2.2 pod CPU-s. The bindings name one instance (`MessagePluginId`), so a visit must still retarget them to the newly active instance. Cause, confirmed by impl2 from source (rev 5): zellij 0.44.3 encodes `KeybindPipe` with every field dropped, `plugin_id` included (`zellij-utils` `plugin_api/action.rs`), so the `InitialKeybinds` snapshot never shows which instance a binding targets. `split_three::available_bindings` therefore always reports zellaude's keys as available, `custom_layouts` refreshes on purpose, and every `TabUpdate` on the active instance issues two reconfigures.
    - Fix: track the target by broadcast. After it reconfigures, the installing instance pipes `zellaude:bindings-target <plugin_id>` to every instance (no render), and each records it. The active instance reconfigures, once, with both snippets joined, only when the record is not its own id, or when the snapshot shows zellaude's keys missing altogether. That second case keeps today's self-repair after a user config reload. A received target message also triggers the check: an active instance that hears a target other than itself retargets. That closes the race where a quick A→B→A switch lets B's late reconfigure win while A is already active again.
    - Runtime bindings are client-scoped, so the message carries the client id (`PluginIds.client_id`) with the plugin id, and an instance compares only targets for its own client. A target from another client's instance never triggers a retarget. As a second guard, a received message triggers at most one retarget per activation, so two attached clients can never retarget each other in a loop. "Active" comes from the host for every trigger except `TabUpdate` (rev 6 for receipts and snapshots, rev 7 adding the grant path and `PaneUpdate`, impl2). Rev 7's why: a fresh session makes every tab briefly active, so all 8 instances hold active-looking tabs; the grant reaches all of them, all 8 install, and a hidden one wins. That is why today the keys fail on a fresh session's starting tab, after re-attach on some tabs, and with a second client. And in a detached session `PaneUpdate` reaches every instance, so trusting cached tabs there would turn each title change into a reconfigure storm. zellij sends `TabUpdate` only to plugins on each client's active tab, so a just-hidden instance keeps believing it is active, and after k visits k−1 hidden instances would all retarget on the next broadcast. So the instance asks `get_focused_pane_info()` and retargets only if the focused pane is in its own tab. A "not here" answer is cached until its next `TabUpdate`, so there is about one query per visit. Err or timeout counts as not here and is never retried, and the query never runs on a timer: the August livelock came from a retried host call with a 100 ms deadline. `TabUpdate`-triggered checks keep using cached tabs, since `TabUpdate` reaches only the active tab's instances. Cheap checks run first (the record, the snapshot), and there is no query without a pane manifest. In a detached session each instance's first query errs, caches "not here", and stays quiet until a client returns. Residual: a `TabUpdate` processed after the client already moved on can still install on a stale instance; the focused instance repairs it on the receipt, once per activation. Cost of failing closed: the truly active instance needs the same query to repair the A→B→A race. On a slow, throttled server that query is the one likely to time out, and then the keys stay on the stale target until the next visit. That is the right trade against a livelock. Cost of that bound: after a triple quick switch A→B→C→A, A can spend its one retarget on B's stale target. If C's reconfigure lands after A's, the keys stay on C until A's next activation. That is rare, and the A→B→A test does not cover it. The per-`TabUpdate` flag reset goes. Repeated `TabUpdate`s on one tab then reconfigure nothing, a visit to another tab once, and a rebuilt instance, which has no record, once on first activation.
    - Rejected: resetting only when another tab is seen active, which can silently misroute keys after a quick away-and-back; and only joining the two snippets, which halves the reloads but misses the bound.
    - *Timer chains leak, one per re-attach:* a rebuilt instance keeps the plugin id, so the previous instance's pending timer fires into it, and every `Timer` re-arms. Idle `Timer` events per instance went 1.0 → 2.0 → 3.0 → 4.1 over three re-attaches, at 1, 4 and 8 tabs, without bound. Every tick's work and render multiplies with session age. Plausibly part of the user's 280–470 % steady plugin-exec CPU; unverified there. Fix: a tick earlier than its due time by more than half an interval re-arms nothing, so a leaked chain dies on its next tick. A tick at or past its due time re-arms from now however late it is: on a throttled pod late ticks are ordinary, and rejecting one would end the instance's only chain and stop its cleanup, flash expiry and elapsed display for good.
    - Left as is: during visits, `SessionUpdate` reaches every instance at ~13/s whatever the plugin does (a no-work `Timer` did not change it). Its source is untraced.
  - **Persist-flash leak fix:**
    - Bug: `FlashMode::Persist` stores a `u64::MAX` deadline. `SessionEnd` removes the session but not the deadline, and `clear_flashes_on_tab` reaches only panes that still have sessions, so the deadline lives forever and keeps the 0.25 s ticker running.
    - Fix: `SessionEnd` drops the pane's deadline, and the Timer arm prunes deadlines whose pane has no session.
  - **Rainbow toggle shape:**
    - `"rainbow": true|false` in `zellaude.json`, default on, with a settings-menu row; the plugin gates rainbow rendering and the 0.25 s ticker on it.
    - Off: the hook skips rainbow detection and sends `rainbow_name: null` ("not looked"), not `false`, so the state cache that `zellaude-attach.sh` reads never records a false fact.
    - Cost of `null`, accepted: if the user leaves ultracode while rainbow is off, then turns rainbow back on, the old rainbow shows until the next `SessionStart` or `UserPromptSubmit` scan. A `null` keeps the cached `true` through the merge.
    - Off renders a name exactly as a non-rainbow session: no rainbow colors and no rainbow-driven bold. Active and flash bold stay.
  - **Q3 scan rule:** Claude's transcript scan runs on `SessionStart` and `UserPromptSubmit` only: `/effort` is a local command, so it can't change mid-turn. `SessionRestore`, the `--inspect` event `zellaude-attach.sh` sends for attach recovery, also scans: it is not a Claude hook event and has no earlier scan to lean on.
    - Other events still resolve from `.effort.level`, else send `null`, which keeps the previous value. Codex detection is untouched.
    - Measured −42 ms per event (15 %) with the suite green.
  - **Hook micro-cuts:**
    - one `jq` pass reads every input field and stamps `ts_ms`, kept millisecond-precise because the plugin orders by it;
    - `$EUID` replaces `id -u`; `hostname` stays, because `attach_detection.sh` compares its output;
    - one read of `zellaude.json` serves the launch-env names, the rainbow flag and the notification mode.
  - **Cache write subset:** one `stat` per path for owner and mode, no `chmod` when already 700, and one `jq` that reads the cached `ts_ms` and merges.
  - **Constraints on every hook edit:**
    - `state_cache_key()` stays self-contained (no helper calls, no outside variables) and `set -eu` clean: `src/manifest.rs` lifts it out of the hook by text search and runs it under `set -eu`.
    - The hook keeps its trailing newline, because `src/installer.rs` substitutes `"__HOOK_SCRIPT__\n"`.
    - The hook stays bash-3.2 clean.
  - **Memo and its safety:**
    - The memo is the pane's existing state record (session id, host, `agent_pid`), written atomically by mktemp+mv.
    - Only root events whose session id and host match read it; on Linux a fork-free `/proc/<pid>/comm` check confirms it.
    - `SessionStart` never reads the memo and always climbs, because resume reuses a session id.
    - The per-event environ re-read stays: it is the redaction and narrowing path.
    - `--inspect` never reads the memo. It runs outside the agent's tree and keeps today's climb, so its output stays byte-identical (`tests/attach_detection.sh` pins it). `zellaude-attach.sh` is untouched.
  - **macOS fallback:** an empty batched `ps` result or a nonzero exit falls back to today's per-process climb. A working call can never be empty: it lists at least init, the agent and the hook.
  - **Test seams:**
    - `ZELLAUDE_TREE_PROC_ROOT` roots tree reads; `ZELLAUDE_PROC_ROOT` stays environ-only, because the launch-env harness plants only `<pid>/environ` there.
    - `ZELLAUDE_DELIVERY_COALESCE_SECONDS` lets tests shorten the window.
    - Exec counts, never wall-clock, pin regressions.
  - **Rejected:**
    - a relay instance: measured 15–35× the CLI broadcast per event, its launch included;
    - pull delivery: needs FullHdAccess, which re-prompts every user; seed candidate;
    - a Rust hook binary: needs a second build target, release artifacts and an install path, and would invalidate the 977-line suite;
    - dropped after measuring, and the user was told: a plugin "changed" flag to skip re-renders (no effect on latency or routing; noparse ≈ shipped), and removing the cache lock (it trades a security property).

**Naming & signatures**
```bash
DELIVERY_COALESCE_SECONDS=0.2        # hook constant; env ZELLAUDE_DELIVERY_COALESCE_SECONDS overrides (tests)
ZELLAUDE_TREE_PROC_ROOT              # env, default /proc: process-tree reads only
find_agent_pid()                     # unchanged name: memo → /proc climb | batched ps → per-process ps
deliver_payload()                    # urgent: detached deliverer now; coalesced: after the window; either way one pipe per session at a time; never waits
DELIVERY_REAPER_SECONDS=5            # a running pipe client is killed after this
session_is_clientless()              # true only when connected_clients reads 0; unknown → false (deliver)
# zellaude.json: "rainbow": bool (absent = true) — written by the plugin, read by the hook
```
```rust
pub struct Settings { /* … */ pub rainbow: bool }   // serde default true
pub enum SettingKey { /* … */ Rainbow }
fn should_animate_rainbow(&self) -> bool;           // settings.rainbow && has_rainbow_sessions(); has_rainbow_sessions stays truthful
fn prune_orphan_flashes(&mut self) -> bool;         // Timer arm: drop deadlines of panes with no session
fn holds_shareable_sessions(&self) -> bool;         // gate for answering zellaude:request
const BINDINGS_TARGET_PIPE: &str = "zellaude:bindings-target"; // payload: "<client_id> <plugin_id>" — that client's runtime bindings now target that instance
bindings_target_plugin_id: Option<u32>              // State field: last target heard for this instance's own client; None in a rebuilt instance
fn arm_tick(&mut self, seconds: f64);              // set_timeout + record next_tick_due_ms; the Timer arm skips re-arming only for an early tick
const ATTACH_DONE_PIPE: &str = "zellaude:attach-done"; // one-shot, sent only after install + scan + compile all succeed
```

**Verification**
- **Tests and builds:**
  - `cargo test --target x86_64-unknown-linux-gnu --features zellij-utils/vendored_curl`
  - `cargo build --release --target wasm32-wasip1`
  - `bash tests/hook_mode_detection.sh`, `tests/attach_detection.sh`, `tests/install_*.sh`; `bash -n` on every touched script.
  - Standalone `rustfmt` on touched `.rs` files, before vs after (repo-wide `cargo fmt --check` misreports).
- **New tests:**
  - Hook:
    - exec counts per event for each path;
    - the batched-`ps` stub branch for macOS emits full paths in `comm`, as macOS does and as the existing per-process stub does, so an awk that forgets to basename fails; plus its empty-output fallback;
    - `--inspect` output unchanged.
  - Delivery, with the harness waiting on the deliverer:
    - the last event per stream always lands, superseded events never land, and urgent events land at once;
    - the Notification rule holds;
    - presence skip on 0 and fail-open on a missing file;
    - no orphaned `sleep`.
  - Plugin:
    - a Persist deadline is gone after `SessionEnd`, and the timer returns to 1 Hz;
    - rainbow off gates render and ticker; `Settings` round-trips `rainbow`, including the absent-key default;
    - the sync answer gate;
    - attach work once: `zellaude:attach-done` is sent only on success, receivers skip the work, and with no attach-done the first-visit fallback runs;
    - update the `Settings` mirrors in `tests/event_handler.rs` and `tests/render_theme.rs`.
- **bash 3.2** for the hook suite if obtainable on the box; otherwise say so.
- **macOS: not verifiable here.** No macOS host exists; the `ps` stub makes the assumption explicit but does not verify it.
- **Load-independent cost**, interleaved A/B against `develop` on km-kim-4:
  - hook CPU-s and forks per event;
  - server CPU-s per delivered event at 1/4/8 tabs;
  - 8-tab re-attach plus one visit to every tab, in the same block:
    - pod CPU-s ≤ 0.6× `develop`;
    - no installer, attach-scan or template-compile spawn during the 8 first visits, logged by an `sh` shim on the server's `PATH` (manifest writes, which follow tab changes, are excluded by signature);
    - first-visit switch time no worse than `develop`;
    - a failed-install arm, where hidden instances still do the work on first visit;
    - `InitialKeybinds` per instance per visit ≤ 2, and split-three and custom-layout keys still act in every visited tab;
    - two clients attached to the 8-tab session on different tabs, idle 10 s: the pass is no `InitialKeybinds` after the initial ones. The keys acting in either client's tab is a stated residual, not a pass condition: with two clients, the first client's split-three and prompt keys never act on the branch (stuck 3 of 3) or on `develop` (2 of 2), and the second client's act only sometimes on both. This predates the branch (T12, harness A);
    - the keys act on a fresh 8-tab session's starting tab, and on every tab's first visit after a re-attach, with 0 failures over ≥3 repeats. Baselines: `develop` fails the fresh starting tab every time, and fails intermittently on first visits after a re-attach (3 of 24; the branch before T16, 7 of 24 split-three and 6 of 24 prompt visits).
  - Idle `Timer` events per instance after 3 re-attaches stay at the configured rate (1/s, or 4/s with a live flash or rainbow), at 8 tabs.
  - Late ticks: with a live flash in an 8-tab session, stop the zellij server (`SIGSTOP`) for 2 s, resume it, and confirm every instance's `Timer` rate returns to 4/s. If the throttling arm runs, repeat the rate check after it.
- **E2E** on km-kim-4, isolated: `HOME`, `XDG_*` and `ZELLIJ_SOCKET_DIR` under `/tmp/optimize-hook`.
  - Setup:
    - zellij 0.44.3 with the branch's plugin wasm, 8 tabs; the plugin installs the branch hook itself.
    - One fake agent per tab (a shebang script named `claude`, so the climb finds it) replays a scripted burst of real-schema events, including Pre/Post pairs 50 ms apart, ending in `Stop`.
    - Hundreds of events across ≥3 blocks, at a realistic rate (agents staggered, about 2 pipes/s), plus one overload block with all 8 agents at once.
  - Pass when all hold:
    1. Every tab's bar, read per bar at least 1 s after switching to its tab, shows each pane's final state from its last event. Reported on its own line, against `develop`: how long a first-visit bar stays without session glyphs (the share of visits blank at all, the median and max blank time, and the count still blank at 4 s). On a first visit a bar draws once without session glyphs about 60 ms after the switch, then fills. That predates the branch, and it is shorter on the branch: 14 of 16 visits, blank for a median 0.18 s, max 0.42 s, none still blank at 4 s. `develop`: 6 of 16 still blank at 4 s.
    2. A counter build sees, on every bar, every urgent event that no newer event of its own stream superseded before its turn to pipe, and every `SessionEnd`. Report the count of superseded urgent events per staggered block.
    6. Urgent hook-fire→bar latency (the counter's increment seen in the pty capture), pooled over each run's staggered blocks (n ≥ 60 per run), has p95 ≤ 0.5 s in every run. Per-block p95s are reported but do not gate: at about 16 samples a block, a p95 is roughly the second-largest value.
       - A `SessionStart` that waits behind its own stream's `SessionEnd` (the `/clear` pair) has its own bound: p95 ≤ 1.0 s pooled per run, two pipe-times at the main bound. By design it waits behind at most the running pipe plus its own `SessionEnd`, the deliberate ordering that keeps T6's cleanup. Context only, not a reason to stop gating: the burst fires `/clear` once per 30 events, likely far denser than real use. Measured on the final code: all urgent events p95 0.498–0.518 s per run; without the `/clear` pairs, p95 0.430 s and max 0.498 s. `develop` under the same burst: p50 18.2 s, p95 26.8 s, and most urgent events never reached the bar.
  - Passes 2–6 apply to the staggered blocks. The overload block passes on its own list: pass 1, no deadlock, a drained backlog, and no hook, `sleep` or `zellij` client process alive 7 s after the drain.
    3. A client-less run (ctrl-o d first) pipes urgent events only, counted by a `zellij` shim on `PATH` that logs each invocation before exec'ing the real binary. The same log shows superseded events never pipe.
    4. Re-attach restores the sessions from the cache.
    5. No zellaude-hook, `sleep` or `zellij` client process is alive 7 s after the last event (the 5 s reaper plus 2 s).
  - **Preflight:** for every arm except the throttling arm, km-kim-4's `nr_throttled` must not rise during the run; ≥8 idle CPUs, scratch socket dir empty, and the user's default `zellij list-sessions` answering (the guard). Never touch the user's real `~/.cache` or sessions.
  - **Artifacts:** `/tmp/optimize-hook/` holds pty captures, cost tables and the burst script.
- **The goal itself: interactive latency under throttling**, on km-kim-2 (rev 17, the user's choice after four km-kim-4 runs stopped early, the last on their own activity there; km-kim-2 has the fleet pod's 16-CPU quota), `develop` vs branch, interleaved, ≥3 blocks each.
  - Throttling regime: the fleet's, not a saturated pod. Closed loop (rev 14): in each `develop` block a controller adjusts the number of `timeout`-capped busy threads every second, judged on a 5 s moving average and stepped at most once every 2 s (rev 17: per-second steps swung the pod 0–80 % on km-kim-4), +1 under 6 %, −1 above 14 %, starting at half the pod's quota and bounded 0 to the quota, and records that count trace. The paired branch block replays `develop`'s trace second by second, so both builds get the same background load and the branch's lower throttle is the measured effect. Why closed loop: a fixed count cannot hold `develop` in band, because its own load swings the pod from 0 % to 57 % throttled at one count (three earlier runs).
  - Report the throttled fraction of periods per block. Discard a pair whose `develop` block falls outside the 6–14 % band. The branch block is never band-checked: throttling below 6 % under `develop`'s trace is the effect being measured. Both blocks start the burst script on the same second as the trace, so load peaks meet the same phase of the event replay. Keep a pty capture per block, and re-read the shell line 5 s after the block, so a missed keystroke echo shows as lost or late.
  - Measure, under that load and the burst:
    - keystroke→echo latency, one key every 200 ms into a terminal pane, read from the pty capture;
    - bar-click→settings-menu render latency.
  - Pass: the branch's p50 and p95 are no worse than `develop`'s. Report both distributions; the expected effect is small, since a click almost never finds a plugin queue and the quota stall hits at any depth.
  - Preflight for this arm only: the burner bounded by `timeout`, and the guard on. The user's idle processes on the arm's pod may stay (on km-kim-4 these were `asterorank_trainer-4`, `nvitop` and shells; user, 2026-10-14: "Currently km-kim-4 is doing nothing. But I tell you I might use it again"). A user process is any process not descended from the probe's root PID; snapshot those PIDs at start. Start only while they are idle (<0.2 CPU and GPUs at 0 % over 30 s). A monitor checks every 1 s: if a user process turns active, a new one appears, or a GPU turns active, it kills the burner within 2 s. Short-lived children of the user's own zellij servers belong to the idle baseline: their idle plugin's `run_command` work, such as manifest writes, config loads and the attach scan's `ps`. They abort the arm only if together they use ≥ 0.2 CPU-s/s, or one lives longer than 2 s (rev 12: both earlier runs tripped on exactly these) and the arm reports the blocks finished, plus the abort's detection and kill times. The whole arm has a 40-minute wall budget. The user launches the arm, since impl3's session is not allowed to start the burner. The 1 s monitor, its kill and the 40-minute budget all run inside that one launched script, detached from the launching terminal (`setsid`, stdio closed), so closing the terminal cannot leave the burner running without its guard. Before it starts, impl3 tells the planner, who tells the user it will hold the arm's pod near its quota for up to 40 minutes and stop on their activity. Never touch the user's processes.
- **`dump-layout` under hook load**, on km-kim-4, `develop` vs branch, ≥2 blocks of 40 calls each:
  - Setup: an attached 8-tab session; the E2E's fake agents replay 2 tool pairs per second; `dump-layout` every 0.5 s.
  - Load validity: run `develop` and the branch as paired blocks. Keep a pair only if `develop` in it shows the symptom (at least one call ≥ 1 s or one empty output); discard the rest. Run pairs until 2 are kept, at most 8; if fewer than 2 qualify, raise the rate and say so. Report the kept-pair count.
  - Pass, over kept pairs: the branch's p95 < 0.5 s, max < 1 s, and no empty output. Outcome: max < 1 s held; p95 missed in 2 of 6 kept pairs (5 of all 13 branch blocks); no empty output in the final 280 calls, but 2 unexplained fast empties earlier. The user accepted the result (*Decisions › With user*).
  - Log each send's client wall time, `zellij pipe` in `develop` and `zellij action pipe` in the branch, in the attached session. That answers directly whether `action pipe` waits the way `zellij pipe` does.
  - The E2E preflight applies: isolation, the guard, and flat `nr_throttled`.
  - Report `develop`'s numbers, and both arms' count of "CliPipe did not complete within 1s timeout" lines in the zellij log during the load window.
  - The throttling arm also records `dump-layout`: no empty output, and no worse than `develop`.
- **Not measurable before merge:** the after-number on km-kim-0-0. Installing the branch hook there writes the user's shared GPFS home, which reaches every pod. Supporting evidence before the change: km-kim-0-0's throttled fraction per hook event, read-only. After deploy, the user re-reads the same number; the PR text gives the command.
- **`action pipe` floor:** run one delivery against a zellij 0.44.0 release binary unpacked under `/tmp/optimize-hook`. If no binary can be fetched, say the floor is unverified.

**Approach**
- Hook, impl1:
  - (T1) process facts: memo, `/proc` climb, batched `ps` with fallback, tree seam, `$EUID`;
  - (T2) one-pass parse, Q3 scan rule, rainbow-off skip, single settings read;
  - (T3) cache write subset;
  - (T4) delivery: urgent/coalesced, detached deliverer, presence skip, `action pipe`;
  - (T5) Codex `async` entry.
- Plugin, impl2, in parallel: (T6) flash fix, (T7) rainbow toggle, (T8) quiet attach; after T12's findings (T16) one reconfigure per visit and (T17) one timer chain per instance.
- Docs, impl2: (T9) README, after T4, T5 and T7.
- Finalize, impl3: (T10–T13) merge `develop`, compact comments, verification with E2E, archive.

**Relevant files**
- Hook and installer:
  - `scripts/zellaude-hook.sh`:
    - `find_agent_pid`, `claude_process_requested_mode` (`/proc` reads);
    - field parse, `detect_claude_rainbow` call site, settings read;
    - `state_cache_dir` / `ensure_private_cache_dir` / `persist_root_state`;
    - delivery tail.
  - `scripts/install-hooks.sh`, `src/installer.rs`: the Codex entry gains `"async": true` (timeout stays 3: SessionEnd's maximum is 3 s). `HOOK_SCRIPT` embeds the hook; a byte difference reinstalls it.
  - `tests/hook_mode_detection.sh`: the fake `zellij`, which captures the last argument and must now be awaited; the launch-env harness; the macOS `ps` stub.
  - `tests/install_*.sh`: Codex entry shape.
- Plugin:
  - `src/main.rs`:
    - Timer arm (flash prune, `should_animate_rainbow`);
    - `on_command_permissions_granted`, `maybe_start_attach_scan` and `maybe_compile_session_templates` (attach-work gate);
    - `zellaude:request` arm and the new `zellaude:attach-done` arm;
    - `has_rainbow_sessions`, `clear_flashes_on_tab`, settings menu actions.
  - `src/event_handler.rs`: `SessionEnd` branch drops the flash deadline.
  - `src/state.rs`: `Settings.rainbow`, `SettingKey::Rainbow`.
  - `src/render.rs`: rainbow name gate, settings-menu row.
  - `tests/event_handler.rs`, `tests/render_theme.rs`: `Settings` mirrors and new cases.
- `README.md`: delivery description (`zellij pipe` → `zellij action pipe`, :577, :581) and a Rainbow row in the Settings table (:337).
