# Webpage Design

The Dioxus static website for the Distributed Lease 101 walkthrough. It renders entirely client-side (WASM) and runs the `lease_sim` engine live in the browser to drive animations. See [simulator.md](simulator.md) for the engine and [algorithm.md](algorithm.md) for the algorithms being illustrated.

Two client-side routes share one banner: the home walkthrough (`/`) and a standalone simulator playground (`/sim`). Each route sets the browser-tab title via `document::Title` — `"Bodega Consensus"` on `/`, and `"Lease Sim Playground"` on `/sim`.

## Goals

- A single, scrollable page: a friendly walkthrough post, top to bottom.
- Static hosting only — no server, no backend. Just `index.html` + WASM + assets. A `404.html` copy of the shell (added in the deploy workflow) lets client-side deep links like `/sim` survive direct hits and refreshes on GitHub Pages.
- Animations run **live** on the simulation engine (not pre-rendered). The blog post version uses pre-generated GIFs instead; this site does not.
- Lightweight and fast to load.

## Tech stack

- **Dioxus 0.7** (pinned `=0.7.9` to match the installed `dx` CLI; `dx` refuses mismatched dioxus versions).
- Features: `web` (client-side WASM renderer) + `router` (drives the `/` and `/sim` routes).
- Build/serve via the `dx` CLI: `dx serve` (dev) and `dx build --platform web` (static output under `target/dx/lease_web/debug|release/web/public`).

## Repository layout

This repo is a Cargo workspace:

```text
Cargo.toml          # workspace root; also the `lease_sim` core lib package
src/                # lease_sim core (engine, scenario, frame, ...)
web/                # the Dioxus app (package `lease_web`)
  Cargo.toml        # deps: dioxus + lease_sim (path = ".."); build-deps: pulldown-cmark, toml, serde
  Dioxus.toml       # dx app config (title, watch paths)
  index.html        # custom HTML shell (analytics tag + Google Fonts; see Analytics below)
  build.rs          # renders content/*.md → HTML at build time (see Walkthrough content)
  content/          # the walkthrough prose, one markdown file per section (source of truth)
    0-intro.md          # the intro (front-matter + prose)
    1-one-to-one.md     # pattern levels 01–05, in ladder order
    2-lease-manager.md  # one grantor fanning the primitive out to many grantees
    3-leader-leases.md
    4-quorum-leases.md
    5-roster-leases.md
    6-bodega.md         # level 06: roster leases co-designed with consensus
  src/
    main.rs         # entry point: launch Router + global assets (Root component)
    components.rs   # routes (Home, Sim), Shell layout, Nav, section templates, widgets
    content.rs      # walkthrough data shapes; includes the build-generated SECTIONS
    sim_view.rs     # shared canvas view layer: SimRun run loop, Topology, SimStage, RunBar, GrantBars
    scenarios.rs    # hardcoded walkthrough scenario canvases (ScenarioCanvas + the name→spec registry)
    playground.rs   # the /sim scenario builder + live sim (scenario-editing controls over sim_view)
  assets/
    main.css        # global stylesheet (loaded via asset!() macro)
```

The web app depends on the `lease_sim` core via a path dependency, so the same engine that powers native GIF generation powers the in-browser animations.

## Walkthrough content

The home walkthrough's prose is **not** hand-written in RSX — it lives in `web/content/*.md`, one file per section, so the writing can be edited as plain markdown without touching Rust or CSS. Files are numbered by reading order (`0-intro`, `1-one-to-one`, `2-lease-manager`, `3-leader-leases`, `4-quorum-leases`, `5-roster-leases`, `6-bodega`); `build.rs` reads them in that fixed order. Each file is:

- a **TOML front-matter** header, `+++ … +++`, carrying the section's structured chrome: `id`, `kind` (`intro` or `algo`), `title`, and — for algo sections — `step`, `pattern`, `tradeoff_pro`/`tradeoff_con`, and (Bodega) a `recap_lead` plus a `[[recap]]` array of `{ trait, seen, new }` rows.
- a **markdown body** of prose. Inline math variables keep their exact `<span class="var">…</span>` markup (pulldown-cmark passes inline HTML through), so the `.var` styling is unchanged. `*em*` / `**strong**`, blockquotes (`>` → the safety invariant), and ordered lists (the intro ladder, the guard/renew steps) render to plain `<p>`/`<blockquote>`/`<ol>`, styled via `.prose`-scoped CSS rules.

Where a widget sits *within* the prose flow, a marker line — `:::figure`, `:::tradeoff`, or `:::recap` — splits the body into ordered blocks so the component can slot the scenario canvas / `Tradeoff` / `RecapTable` back in at that point (e.g. the Bodega section's figure, then recap-lead paragraph, then table). A `:::figure` marker may carry a trailing **scenario name** (`:::figure one-to-one-success`) naming the hardcoded scenario that figure runs (see [Scenario canvases](#scenario-canvases)); a bare `:::figure` renders the plain placeholder for a figure not yet wired up.

`web/build.rs` runs at compile time: it reads each file, splits front-matter from body, renders the body (and each inter-marker chunk) to HTML with `pulldown-cmark`, and emits the whole set as a `SECTIONS: &[Section]` data literal into `$OUT_DIR/content_gen.rs`, which `src/content.rs` `include!`s. **Rendering at build time keeps the markdown parser out of the shipped WASM bundle** — only the finished HTML strings ship. Editing any `content/*.md` (or the `content/` dir) triggers a rebuild via `cargo:rerun-if-changed`.

## Page structure

A sticky dark-blue top banner (shared across routes) over a vertical stack of sections inside a centered column (`max-width ~820px`):

1. **Banner** — "Bodega Consensus" brand on the left (an internal `Link` home); external links (Paper, TLA+, Summerset, Web) plus the internal `Sim*` route link on the right. Full-bleed background, content capped to the body width.
2. **Home (`/`)** — a progressive, blog-post-style walkthrough of the lease algorithms, mirroring [algorithm.md](algorithm.md). An intro establishes the lease primitive (grantor/grantee, time-bounded promise) and previews the *ladder*, then **one section per level** climbs the leasing *patterns* — one-to-one → lease manager → leader → quorum → roster — each motivated by the limitation of the rung beneath it (the lease manager is a brief interlude showing the primitive fanned out one-grantor-to-many before the ladder proper resumes), and a final **Bodega** section co-designs the roster lease with consensus (when local reads are/aren't enabled, how writes stay off the lease path, the safety threshold, optimistic holding). Every algorithm section carries bespoke prose (an opening blurb, a safety-invariant blockquote, a numbered step list, a `Tradeoff` pro/con motivating the next rung) plus one or more **scenario canvases** — live `lease_sim` animations of a hardcoded scenario (see [Scenario canvases](#scenario-canvases)); figures not yet wired to a scenario fall back to a plain placeholder. The Bodega section closes with a `RecapTable` showing how each accumulated trait lands in Bodega. All of this prose lives in `content/*.md`; see [Walkthrough content](#walkthrough-content).
3. **Sim (`/sim`)** — a standalone simulator playground. Scenario-setup controls (preset, node count, grantor/grantee selection) sit over a canvas, with playback controls and a scrubbable timeline below. See [Simulator playground](#simulator-playground).

The walkthrough prose is authored per level in markdown (see [Walkthrough content](#walkthrough-content)), so each section reads as its own smooth passage rather than a uniform template — the components are thin templates that slot each section's prose and widgets into place.

## Components

- `Root` — top-level; injects the global stylesheet asset, then mounts `Router::<Route>`.
- `Route` — the routable enum: `Home {}` at `/` and `Sim {}` at `/sim`, both nested under the `Shell` layout.
- `Shell` — layout wrapping every route: renders `Nav`, then the active route via `Outlet`.
- `Home` — the walkthrough page: iterates `content::SECTIONS` (see [Walkthrough content](#walkthrough-content)), rendering each via `WalkthroughSection`.
- `WalkthroughSection { section }` — renders one authored section: the plain heading (`kind == "intro"`) or the algo head — a small muted ordinal (`01`–`06`) and a small-caps pattern tag (`one-to-one`, `one-to-many` … `all-to-all`, `co-design`) — wrapping that section's ordered blocks.
- `SectionBlock { section, block }` — renders one body block in document order: a prose chunk (`Block::Html`, injected via `dangerous_inner_html` into a `.prose` wrapper), or a widget slotted from the section's fields (`Block::Figure(name)` → a `ScenarioCanvas` for `name`, `Block::Tradeoff` → `Tradeoff`, `Block::Recap` → the recap-lead paragraph + `RecapTable`).
- `Sim` — the standalone simulator playground page; hosts `Playground`.
- `Playground` — the interactive scenario builder + live simulation (in `playground.rs`); owns the editing controls and drives the canvas through the shared `sim_view` widgets. See [Simulator playground](#simulator-playground).
- `ScenarioCanvas { name }` — a self-contained walkthrough figure replaying a hardcoded scenario over the same shared `sim_view` widgets as the playground (in `scenarios.rs`). See [Scenario canvases](#scenario-canvases).
- `SimStage { topology, frame }` / `RunBar { … }` / `GrantBars { … }` — the shared canvas view layer (in `sim_view.rs`): the animated (or static) canvas contents, the `[control | track | status]` run bar + time axis, and the per-node grant bars. Both `Playground` and `ScenarioCanvas` compose these, so the two displays are identical.
- `Nav` — sticky dark-blue banner: "Bodega Consensus" brand on the left, external links (Paper, TLA+, Summerset, Web) and the `Sim*` route link on the right.
- `Tradeoff { pro, con }` — the pro/con pair that motivates the next rung.
- `RecapTable { rows }` — the closing table mapping each accumulated trait to the rung it first appeared on; `rows` come from the Bodega section's front-matter (`new` rows highlighted).

All external links open in a new tab (`target="_blank"` with `rel="noopener noreferrer"`).

## Simulator playground

The `Playground` component (`web/src/playground.rs`) is a self-contained scenario builder and live animation over the `lease_sim` engine. It owns the scenario-*editing* controls; the canvas, run bar, and grant bars are the shared [`sim_view`](#shared-view-layer) widgets, and the run itself is a shared `SimRun` (see below). It is driven by the four-state `RunPhase`:

- **`Idle`** (editing) — the default. The controls (preset pills, node-count slider, grantor/grantee toggles, Guard/Renew failure switches, write-load switches) define a *scenario shape*, drawn statically on the canvas as light-gray directed grantor → grantee arrows. Changing any knob returns to this state. Each control-row label (Preset, Nodes, Grantors, Grantees, Msg drop, Writes) and the grant-bar "Grants?" header is a `.pg-hint` — a help-cursor marker that shows a dark tooltip explaining that knob on hover (same bubble style as the caption's `.pg-tconst`).
- **`Generating`** — entered by pressing **Play** (or **Resume**). The current knobs build a `Scenario` and the engine is advanced *live*, animating the messages and lease timers, until the user pauses it or it reaches the cap.
- **`Paused`** — the user pressed **Pause** mid-generation. The engine and bookkeeping are kept, so **Resume** continues the run from where it left off. Scrubbable while paused.
- **`Stopped`** — generation reached its stop tick (for the playground, the `MAX_TICKS` cap); generation stops, the engine is dropped, and the recorded frames stay put for free scrubbing. Not resumable — a fresh Play/Restart is needed.

### Shared view layer

The animated canvas, the run bar, the grant bars, and the record → scrub → resume machinery are **not** playground-specific: they live in `web/src/sim_view.rs` and are shared verbatim with the walkthrough's [scenario canvases](#scenario-canvases), so the two displays are pixel-identical. It exposes:

- **`Topology { n, grantors, grantees }`** — the grantor → grantee *shape*, with all the derived per-node quantities (bar counts, self-grants, max grants, the crowned leader, whether any lease is declared). Built from the playground's selection sets, or `Topology::from_scenario` for a hardcoded scenario.
- **`SimStage { topology, frame, fit_height? }`** — the `.pg-stage` canvas (it owns the box): the static gray topology when `frame` is `None`, else the live frame (lease edges, message glyphs, node disks with auras/crown/timer stacks). `fit_height` crops the box to the content's vertical band (isotropically). All the geometry/color helpers (edge insets, the adaptive ring shrink, the fit-height band, aura/grant colors, message glyphs) are private to this module.
- **`RunBar { phase, …, show_ff?, lock_when_stopped?, status }`** — the `[control | track | status]` run bar plus the time axis beneath it (Play/Pause/Resume + clock, timeline slider, status cell, Restart + `t = N`/`max N ticks` + the 3× fast-forward toggle). The caller supplies the phase, the callbacks, and the status-cell content; `show_ff` (default true) hides the fast-forward toggle; `lock_when_stopped` (default false) disables Play once the run has `Stopped`, so the only way to run again is Restart (the walkthrough canvases set this). A disabled button (Play locked/unrunnable, or Restart with no run) renders pure white — visibly distinct from its clickable state.
- **`GrantBars { topology, bars, cursor_frac, rec_len, grantees_only?, draggable? }`** — the per-node grant-status bars on the same shared grid. `grantees_only` (default false) limits the rows to grantee nodes; `draggable` (default false) pins a small "you can drag" cue just right of the bars (mirroring the "Grants?" header), shown while the timeline slider is scrubbable.
- **`SimRun` / `use_sim_run()` / `StopWhen`** — the run state (recorded frames, lifecycle `RunPhase`, scrub cursor, fast-forward, stop condition) *and* the wall-clock generation loop that fills it, repainting every `RENDER_MS`. `use_sim_run` installs the loop as a `use_future` and returns a `Copy` handle with `start(scenario, stop)` / `pause` / `resume` / `reset` and the derived read accessors (`frame`, `grant_bars`, `now_ticks`, …). `StopWhen` is the run's stop condition: `AtTick(t)` (a fixed tick — the playground's `MAX_TICKS`) or `OnNthEvent { n, pred, after, cap }` (stop `after` ticks past the `n`-th event matching `pred`, or at the `cap` tick — a safety net so a run can never generate forever if its event never fires). The `after` lead-out keeps a milestone's aftermath on screen (e.g. stopping a bit *after* a dropped guard ack, so the grantor is seen giving up); `after: 0` stops right on the event.

### Live generation model

The run is generated **incrementally on a wall-clock loop** (the `use_sim_run` loop), not computed all at once — advancing the engine live is what makes the run *visible*, and keeping every frame is what lets the user scrub freely afterward. The loop ticks every `RENDER_MS` (7 ms, ~143 fps — fast enough to stay smooth on 120/144 Hz displays); while the phase is `Generating`, each tick advances a batch of `FRAMES_PER_STEP` (1) frames — or `FRAMES_PER_STEP · FF_MULT` (3) while fast-forward is on — and for each frame it:

1. advances the shared `Engine` to the next global time `t` (`+FRAME_TICKS` = 3 ticks per frame — a fine resolution for smooth motion and scrubbing) and snapshots `frame_at(t)` into a `Vec<Frame>`;
2. advances the frame clock, or — once the run's `StopWhen` fires (the playground passes `AtTick(MAX_TICKS = 60000)`; a scenario may pass `OnNthEvent`, scanning the events returned by `advance_to` for its milestone) — transitions to `Stopped` and drops the engine, leaving the frames.

The loop keeps the engine on pause (so **Resume** can pick it back up) and decouples time *resolution* (`FRAME_TICKS`) from wall-clock *pace* by batching frames per repaint.

`build_scenario` produces a `Scenario` from the knobs: every node initiates the leases it grants on a per-poll chance (`initiate_chance = 0.5`, so guarding starts at a staggered random time), no baseline link loss, no crashes — plus the two **message-failure switches** applied as per-kind drops, and the **write switches** as the leader's cadence/mode. Each Play draws a **fresh random seed** from the browser's `Math.random()` (`fresh_seed`), so every run generates new randomness (different drop timing, jitter, staggered initiation) rather than replaying an identical scripted run; the engine itself stays deterministic *given* that seed. Per-run bookkeeping (the next frame time and stop tick) lives inside the `SimRun`.

### Message-failure switches

A "Msg drop" control row holds two vertical **stacks** — one for **Guard** and one for **Renew** — each pairing the request switch with its reply switch (labeled just **Reply**) directly beneath it: four switches in all (`Guard`, its `GuardReply`; `Renew`, its `RenewReply`). Each offers `Off` / `1%` / `10%` / `30%` / `100%` (a `FailRate` enum → drop probability), fed to `build_scenario` as a `kind_drop(MsgKind, p)` on that message kind, so the engine drops that fraction on top of the (currently zero) link loss. This makes each phase's fragility visible from either direction: dropping all `Guard`s stalls establishment; dropping all `GuardReply`s lets the grantee ack but leaves the grantor unactivated (it re-guards after its window); dropping all `Renew`s or `RenewReply`s lets a lease lapse. Unlike the shape knobs, a failure switch **keeps** the selected preset — failure is a property of the run, not the scenario shape (all four switches default to `1%`; reply drops aren't part of a preset, so they reset to `1%` on preset apply).

### Write-load switches

A "Writes" control row holds two more segmented switches (same pill styling). **Every** (`WriteEvery` enum) sets how often the leader — the crowned smallest-id grantee — serves a write: `Never` / `3000 ticks` / `1000 ticks` / `300 ticks`, fed to `build_scenario` as `writes(interval, disruptive)`. **Disruptive** (`Yes` / `No`, a `bool`) picks the write behavior; either way `Write`/`WriteReply`/`Commit` messages sweep the cluster (purple write glyphs, a dark-gray commit checkmark). See [simulator.md](simulator.md#write-path).

- **Disruptive** — each node receiving the write tears down the leases it takes part in (the reads it holds *and* the grants it makes) until the write commits, so on the canvas you see held grants collapse and then, after the commit, **re-establish through a fresh guard round** (guard → renew, not an instant snap-back): a visible model of quorum-lease write disruption, where a write's revocation forces holders to re-guard from scratch.
- **Non-disruptive** — leases are left entirely untouched; the write's messages sweep and commit but never change any lease. The grant bars stay steady green throughout — the Bodega model where writes don't interrupt background leases.

Write switches, like failure switches, keep the selected preset. Conversely, **selecting a preset sets all four switches** (via `Preset::switches`) alongside the topology, so each preset opens with a representative configuration: every preset uses a 1% Guard/Renew drop; **One-to-One** and **Lease Manager** have writes off; **Leader** and **Roster** run non-disruptive writes every 3000 ticks; **Quorum** runs disruptive writes every 3000 ticks. The switches remain freely adjustable afterward without clearing the preset.

### Run bar + timeline

The **run bar** is the shared `[control | track | status]` grid (the `RunBar` view, so the playground and scenario canvases share it verbatim):

- The **control cell** holds the **primary button** and the clock glyph. The primary button cycles by phase: **Play** (idle/stopped) rebuilds the scenario and starts a fresh live generation — disabled (and shown white) when the scenario declares no lease (no grantor/grantee pair); while generating it becomes a red **Pause** button; while paused it becomes **Resume**. In the playground Play stays available after a stop (starting a new run); a scenario canvas passes `lock_when_stopped`, so once it halts at its condition Play goes white/disabled and Restart is the way back. The clock glyph hugs the slider's left edge.
- The **timeline slider** fills the track column. It is *inert* while editing and generating (disabled), and becomes freely scrubbable — bound to the frame `cursor` — while paused or once stopped. During generation the cursor auto-follows the newest frame.
- A **status** area on the right (supplied by the caller): in the playground, a spinner + "sim running" while generating, a gray "run stopped" while paused, a red "✗ ticks limit" if it hits the cap, or an editing hint while idle.

Below the run bar, a compact **time axis** (`.pg-timeaxis`) reuses the same grid: its track cell centers the current scrub time (`t = N`) with the run's end (`max N ticks`) at the right. Its **control cell holds the ghost Restart button** — directly under the primary button, so no separate row is wasted on it. Restart discards the run and returns to static editing (so the next Play starts fresh); it is white/disabled until a run exists, then a clickable gray while playing, paused, or stopped. Its **status cell holds a "3x ⏩" fast-forward toggle** (`.pg-ff`, under the run status): while on, the generation loop advances `FRAMES_PER_STEP · FF_MULT` (3) frames per repaint, so the run and its animation play three times faster.

Changing any scenario knob (or any failure/write switch) discards the current run via `reset_sim`, returning to `Idle` so the canvas shows the static scenario again.

### Grant bars

Under the axis, one **grant bar** per node run-length-encodes its history (a `GrantRun` per segment) from the recorded frames. A segment is shaded **green** by how many of that node's possible grants it holds (deeper with more), an empty track when it holds none; a new run begins whenever the grant count changes. `grant_color(grants, max)` computes the hex; each segment tooltips its grant count and duration. While the timeline slider is scrubbable (paused/stopped), a faint **"you can drag"** cue is pinned just right of the bars, cueing that the timeline can be dragged.

### Canvas rendering

Both modes lay nodes out with `frame::ring_layout`. When frames exist the canvas is driven by the current `Frame`:

- **Topology backdrop** — the same gray grantor → grantee arrows as the static editing view, drawn beneath the live lease edges so the scenario's links are visible from the very start of a run, before any guard link establishes.
- **Lease edges** are directed arrows (like the static view) colored by the grantee's view — green `Active` (opacity tracks remaining lease life for a visible countdown), solid light-blue `Guarding`, faint gray otherwise; each arrowhead matches its stem color. Every stem is pulled back to the arrowhead base and heads are drawn in a second pass, so no stem shows through a translucent head.
- **Message glyphs** at each in-flight message's interpolated `pos`, colored by phase (guard blue / renew green / revoke orange, write purple, commit dark gray, darkened for contrast) via `MsgGlyph`: a shield for guard-phase messages, a circular "renew" arrow for renewals, a slashed ring (a "prohibition" sign) for revokes, a pencil for writes, a checkmark for commits. Reply kinds (`GuardReply`/`RenewReply`/`RevokeReply`/`WriteReply`) overlay a small thumbs-up badge marking them as acknowledgements and are tinted a touch lighter than their request counterparts. Each glyph sits on a frosted, semi-transparent backing disk so it reads over the edges and nodes beneath it, and fades in over its initial departure and out over its final approach (`msg_opacity` on flight `progress`) so it emerges from the sender node and vanishes just as it reaches the destination node, rather than popping in/out at the borders.
- **Node aura** — a green halo on any grantee currently holding grants, whose size and depth scale with the fraction of its possible grants held (mirroring the grant bars' green shading); set inline per node via `aura_style`. A node holding a majority additionally gets a green border.
- **Node timer bars** — countdown bars beside each node disk (`node_timers` → `TimerBar`), one per lease the node takes part in, arranged in **two short columns**: an **OUT** column (orange, leases it grants as grantor) beside an **IN** column (green, leases it holds as grantee), each capped by an OUT / IN header (a column is omitted if the node plays no such role). Two columns keep the stack short — `max(out, in)` rows rather than `out + in` — so busy nodes don't grow tall. Each bar carries a small `→N` / `←N` endpoint label, so it reads without hovering. When the lease is **active** the bar's fill width is the remaining-life fraction (`grantor_fill` / `grantee_fill`) in the role color, draining **right-to-left** as it counts toward expiry; when it is **not active** (guarding / idle / expired) the bar is instead filled solid **gray** (`.is-inactive`), so it reads as "no live countdown" rather than an almost-drained active bar. A slightly thicker, darker border makes each bar apparent against the surface. The stack is centered on the disk then pushed **radially outward** from the cluster center (`--tx`/`--ty`, per node) into the empty region outside the ring, so it clears the lease arrows running through the interior; each cell also tooltips its full role, endpoint, and status.
  - **Adaptive spacing** keeps the stacks off their disks and inside the clipped stage regardless of bar count. The push distance is per-node and *direction-aware* (`timer_offset_rem`): a side node needs the stack half-width cleared, a top/bottom node its half-height. And in the playback view the whole ring is shrunk toward center by `ring_scale` — the tightest per-node fit (`node_max_scale`, solving each stack box's outward edges against the stage margin), clamped at `MIN_SCALE` so a pathological all-to-all never collapses the nodes onto the center (it clips a little instead). Node bar counts (`(out, in)` from the grantor/grantee sets) are known even while idle; the editing view keeps the full radius and nodes glide between the two on the `.pg-node` position transition. The fit assumes a conservative stage size (`REM_UNIT` = 17/520, below the real square `.pg-stage`, capped at 530px tall) so it stays inside the box across viewport widths.
  - Message glyphs and drop bursts derive their positions from the **scaled** node positions (`lerp(pts[from], pts[to], progress)`), not the engine's `m.pos` (which rides the unscaled ring), so in-flight messages travel between the disks as actually drawn once the ring shrinks.
- **Leader crown** — the smallest-id grantee is marked with a small gold crown (`Crown`) perched above its disk, in both the static and playback views. This is a fixed topology annotation (recomputed from the grantee set, not the run state): in the all-to-one leader preset it lands on the sole grantee; more generally it just tags the lowest-id local reader.

### Constants

| Const | Meaning | Value |
| --- | --- | --- |
| `FRAME_TICKS` | ticks per recorded frame (resolution + scrub granularity) | 3 |
| `RENDER_MS` | wall-clock ms between generation repaints (~143 fps) | 7 |
| `FRAMES_PER_STEP` | frames advanced per repaint while generating | 1 |
| `FF_MULT` | frames-per-repaint multiplier while fast-forward is on | 3 |
| `MAX_TICKS` | playground cap on run length; hitting it ends the run as `Stopped` | 60000 |

`FRAME_TICKS` / `RENDER_MS` / `FRAMES_PER_STEP` / `FF_MULT` live in `sim_view` (shared by both consumers); `MAX_TICKS` is the playground's own stop tick, in `playground.rs`. Scenario canvases play at the same `RENDER_MS` pace as the playground. The *playback speed* is the ratio `FRAME_TICKS / RENDER_MS` (sim-ticks advanced per real ms, ≈ 0.43); raising the display framerate keeps that ratio roughly fixed by dropping `FRAME_TICKS` alongside `RENDER_MS`, so smoother motion doesn't speed up the simulation. `gloo-timers` (feature `futures`) provides the async `sleep` backing the generation loop on WASM.

## Scenario canvases

The walkthrough's figures are **hardcoded scenario canvases** (`web/src/scenarios.rs`), each replaying one deterministic `lease_sim` run as a self-contained illustration. They render through the exact same [shared view layer](#shared-view-layer) as the playground — the same `SimStage` canvas, `RunBar`, and `GrantBars` — so a figure looks identical to the `/sim` display, minus the scenario-editing controls. The whole unit — a title, the canvas, the run bar, and the grant bars — is boxed in one very light gray panel (`.sc-root`, lighter than the canvas's own gray) with the title as its heading above the canvas, so it reads as a single scenario animation. Figures also differ from the playground in a few deliberately reader-tuned ways (below): a shorter canvas, no fast-forward, and a leaner grant-bar list.

- **Naming.** A `:::figure <name>` marker in `content/*.md` carries the scenario name into `Block::Figure(name)`; `SectionBlock` renders `ScenarioCanvas { name }`. `scenarios::lookup(name)` maps the name to a `ScenarioSpec { title, scenario, stop, fit_pad }`. An unknown (or empty) name falls back to the plain `.sim-placeholder` div, so a not-yet-wired figure still renders.
- **Determinism.** Unlike the playground (fresh `Math.random()` seed per Play), a scenario's `Scenario` carries a **fixed seed**, so the run replays identically every time. The scenario builders live in the **core crate** (`src/demos.rs`), not the web layer, so their outcomes are natively unit-testable (the WASM view is not) — e.g. `demos::one_to_one_success` is test-checked to establish and stay `Active` on both sides through its stop tick. Guarding is driven by a scripted `Command::Initiate` at a known tick rather than the stochastic per-poll chance, so establishment happens at a stable time.
- **Stop condition.** Each spec's `StopWhen` is passed to `SimRun::start`; the shared generation loop transitions the run to `Stopped` when it fires. A scenario can stop on a *meaningful protocol milestone* rather than a hand-tuned tick — e.g. one-to-one stops on the grantor's 3rd renew reply (`OnNthEvent`), so it always halts once the lease is established and visibly holding, whatever the exact timing. The `cap` tick keeps it well within the scenario's own `duration` as a safety net.
- **Topology.** Derived once from the scenario's declared leases via `Topology::from_scenario`, the single source of truth for both the run and the static shape drawn before Play.
- **Boxed unit + title.** `ScenarioCanvas` wraps the title, canvas, run bar, and grant bars in a padded light-gray `.sc-root` panel (its own background/border/radius), so the figure reads as one self-contained animation. The title (`.sc-title`) is a plain heading *above* the canvas — not a chip inside it — and the canvas is a white sub-panel within the gray box.
- **Fit-height canvas.** `SimStage` is rendered with `fit_height: true`, so the `.pg-stage` box is cropped to the layout's actual vertical extent rather than kept square — a horizontal 2-node scenario gets a short, wide canvas instead of a tall one with empty bands. First, for a cluster larger than 2 the ring layout is deliberately flattened into a short/fat ellipse (`FIT_Y_SQUISH = 0.52`) so the blog canvas stays compact; the *crop* is then *isotropic*: `SimStage` computes the (squished) content's vertical band (`content_y_band` — node disks + auras, a leader's crown, timer half-heights), sets the box `aspect-ratio` inline to `1 : band`, remaps the SVG `viewBox` and every node/message `top%` through the same band, so x and y keep equal pixel scale (the viewBox remap squashes nothing; only the node layout is intentionally squished). Fit-height mode also uses a higher ring-scale floor `FIT_MIN_SCALE = 0.78` (vs the playground's `MIN_SCALE`), and each spec carries a `fit_pad` (0 for most; the roster mesh uses a little to keep its off-axis timer stacks from clipping) added as extra vertical breathing room. `.pg-stage.is-fit` drops the square default and the 530px cap so the inline ratio governs.
- **No fast-forward.** The canvas plays at the same `RENDER_MS` pace as the playground (via `use_sim_run`). `RunBar` is passed `show_ff: false`, hiding the 3× fast-forward toggle (a fixed-pace illustration has no use for it).
- **Grantees-only grant bars.** `GrantBars` is passed `grantees_only: true`, so only grantee nodes get a row — grants are held *as a grantee*, so a non-grantee's row would be an always-empty track. (The playground shows every node.)

Wired scenarios:

| Name | Section | Shows |
| --- | --- | --- |
| `one-to-one-success` | 01 one-to-one | node 0 grants to node 1 over a reliable link: guards once, then renews steadily and stays held (stops on the grantor's 3rd renew reply; capped at 13000 ticks) |
| `one-to-one-guard-reply-lost` | 01 one-to-one | node 1 acks the guard but the `GuardReply` is dropped; the grantor times out and falls back to idle, never establishing (stops just after both guard timers expire — the grantee's guard-window lapse, which trails the grantor's own guard give-up; capped at 4000 ticks) |
| `one-to-one-revoked` | 01 one-to-one | establishes, exchanges two renews, then the grantor proactively `Revoke`s; the grantee drops its hold and acks with `RevokeReply`, on which the grantor also leaves the granting state (its OUT bar grays) (stops when that ack reaches the grantor; capped at 5000 ticks) |
| `one-to-one-renew-replies-lost` | 01 one-to-one | establishes and renews once, then every `RenewReply` is dropped (`kind_drop_from`); the grantor gives up renewing and both sides lapse (stops when both have expired — the 2nd `Expired`; capped at 9000 ticks) |
| `lease-manager-handover` | 02 lease manager | manager (node 0) grants to 1 and 2, then hands 1's slot to 3: revokes 1 (2 keeps renewing), and guards 3 once 1's revoke ack lands (stops once node 3 has renewed twice, the handover visibly complete; capped at 8000 ticks) |
| `leader-leases` | 03 leader leases | 5 nodes, all-to-one: followers 1..4 each grant to leader 0 (stops once the leader has been renewed by all followers through 3 rounds — the 12th `Renew` to reach it; capped at 8000 ticks) |
| `quorum-leases` | 04 quorum leases | 5 nodes, all-to-many: holders 0 and 2 are each granted by the other four, no writes (stops once both holders have exchanged three renews with everyone — the 24th `Renew` to reach a holder; capped at 8000 ticks) |
| `quorum-leases-write-disruption` | 04 quorum leases | same two-holder setup, then a disruptive write at t=2000 tears every lease down; on commit each grantor re-guards deterministically and renewing resumes (stops a lead-out past the commit, after ~3 fresh renew rounds; capped at 9000 ticks) |
| `roster-leases` | 05 roster leases | 5 nodes, all-to-all: 20 leases (every ordered pair) (stops once every lease has renewed at least three rounds — the 60th `Renew` delivered; capped at 8000 ticks) |

## Styling

- Single global `assets/main.css`, loaded via the `asset!()` macro so the build injects a content-hashed URL automatically.
- Light page theme with a dark-blue banner (its own `--nav-*` palette). CSS custom properties for the rest of the palette (`--grantor` orange, `--grantee` green to mirror Figure 2 in the paper). Responsive.
- Body font is **Source Sans 3** (weights 400/600/700), loaded from Google Fonts in the HTML shell `<head>` and applied via a `--font-sans` custom property with a `system-ui` fallback stack. Math-styled variables (`.var`, `.pg-tconst`) keep a serif (`Cambria Math`) stack.
- Walkthrough prose is wrapped in `.prose` (see [Walkthrough content](#walkthrough-content)). Since the markdown renders to plain elements, the prose styling is **element-scoped**: `.prose p`/`ol`/`blockquote`/`a`, all body paragraphs at one size (`.prose > p:first-child` only drops its top margin), and the blockquote styled as the green-accented safety invariant. There are no per-block prose classes.

## Analytics

The site is measured with Google Analytics (gtag.js, measurement ID `G-N2T5220LS6`).

**Requirement.** Every served page must load the standard gtag.js snippet.

**Invariant.** The gtag.js snippet appears **exactly once** per served document — never zero, never duplicated.

The snippet lives in the `<head>` of the custom HTML shell `web/index.html`, *not* in a Dioxus component. This is what upholds the invariant:

- `dx` auto-detects `web/index.html` at the web crate root as the shell for the whole app; it injects the WASM loader and head resources but passes the rest of `<head>` through verbatim (only a `<div id="main">` mount point is required).
- The SPA is a single shell document. Client-side routing swaps the mounted `App` subtree, never reloading the shell — so the tag is loaded once at boot and covers every current and future subpage/route, with no risk of a component re-render injecting it twice.
- Placing it in a component would be wrong: components can mount/re-render on navigation (duplicating the tag) and cannot reliably emit the inline init block into `<head>` before WASM boots.

When adding pages/routes, do nothing analytics-specific — they inherit the tag from the shell automatically. Do **not** add gtag.js anywhere else.

## Status

- [x] Workspace + `web/` Dioxus crate scaffolded
- [x] Static build verified (`dx build --platform web` produces index.html + wasm)
- [x] Page shell: light theme + dark-blue banner + walkthrough sections
- [x] Sticky nav + one skeleton section per algorithm level (data-driven)
- [x] Simulator playground: scenario-setup controls + static scenario canvas
- [x] Live simulation on the playground canvas (record → playback)
- [x] Playback controls (Play/Pause/Resume/Restart) + scrubbable timeline
- [x] Shared canvas view layer (`sim_view`) factored out of the playground
- [x] Live simulation canvas on the home walkthrough sections (scenario canvases): all walkthrough figures wired (§01–§05)
- [ ] Node failure/recovery and per-link/per-node knobs in the playground (message-drop failure injection shipped)
- [x] Release build + deployment (`.github/workflows/deploy.yml`: `dx build --release` → GitHub Pages, with CNAME + 404.html fallback)
