# Webpage Design

The Dioxus static website for the Distributed Lease 101 walkthrough. It renders entirely client-side (WASM) and runs the `lease_sim` engine live in the browser to drive animations. See [simulator.md](simulator.md) for the engine and [algorithm.md](algorithm.md) for the algorithms being illustrated.

Two client-side routes share one banner: the home walkthrough (`/`) and a standalone simulator playground (`/sim`). Each route sets the browser-tab title via `document::Title` — the base `"Bodega Consensus"` on `/`, and `"Bodega Consensus — Lease Sim Playground"` on `/sim`.

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
    1-one-to-one.md     # pattern levels 01–04, in ladder order
    2-leader-leases.md
    3-quorum-leases.md
    4-roster-leases.md
    5-bodega.md         # level 05: roster leases co-designed with consensus
  src/
    main.rs         # entry point: launch Router + global assets (Root component)
    components.rs   # routes (Home, Sim), Shell layout, Nav, section templates, widgets
    content.rs      # walkthrough data shapes; includes the build-generated SECTIONS
  assets/
    main.css        # global stylesheet (loaded via asset!() macro)
```

The web app depends on the `lease_sim` core via a path dependency, so the same engine that powers native GIF generation powers the in-browser animations.

## Walkthrough content

The home walkthrough's prose is **not** hand-written in RSX — it lives in `web/content/*.md`, one file per section, so the writing can be edited as plain markdown without touching Rust or CSS. Files are numbered by reading order (`0-intro`, `1-one-to-one`, `2-leader-leases`, `3-quorum-leases`, `4-roster-leases`, `5-bodega`); `build.rs` reads them in that fixed order. Each file is:

- a **TOML front-matter** header, `+++ … +++`, carrying the section's structured chrome: `id`, `kind` (`intro` or `algo`), `title`, and — for algo sections — `step`, `pattern`, `figure_caption`, `tradeoff_pro`/`tradeoff_con`, and (Bodega) a `recap_lead` plus a `[[recap]]` array of `{ trait, seen, new }` rows.
- a **markdown body** of prose. Inline math variables keep their exact `<span class="var">…</span>` markup (pulldown-cmark passes inline HTML through), so the `.var` styling is unchanged. `*em*` / `**strong**`, blockquotes (`>` → the safety invariant), and ordered lists (the intro ladder, the guard/renew steps) render to plain `<p>`/`<blockquote>`/`<ol>`, styled via `.prose`-scoped CSS rules.

Where a widget sits *within* the prose flow, a marker line — `:::figure`, `:::tradeoff`, or `:::recap` — splits the body into ordered blocks so the component can slot the `SimFigure` / `Tradeoff` / `RecapTable` back in at that point (e.g. the Bodega section's figure, then recap-lead paragraph, then table).

`web/build.rs` runs at compile time: it reads each file, splits front-matter from body, renders the body (and each inter-marker chunk) to HTML with `pulldown-cmark`, and emits the whole set as a `SECTIONS: &[Section]` data literal into `$OUT_DIR/content_gen.rs`, which `src/content.rs` `include!`s. **Rendering at build time keeps the markdown parser out of the shipped WASM bundle** — only the finished HTML strings ship. Editing any `content/*.md` (or the `content/` dir) triggers a rebuild via `cargo:rerun-if-changed`.

## Page structure

A sticky dark-blue top banner (shared across routes) over a vertical stack of sections inside a centered column (`max-width ~820px`):

1. **Banner** — "Bodega Consensus" brand on the left (an internal `Link` home); external links (Paper, TLA+, Summerset, Web) plus the internal `Sim*` route link on the right. Full-bleed background, content capped to the body width.
2. **Home (`/`)** — a progressive, blog-post-style walkthrough of the lease algorithms, mirroring [algorithm.md](algorithm.md). An intro establishes the lease primitive (grantor/grantee, time-bounded promise) and previews the *ladder*, then **one section per level** climbs the four leasing *patterns* — one-to-one → leader → quorum → roster — each motivated by the limitation of the rung beneath it, and a final **Bodega** section co-designs the roster lease with consensus (when local reads are/aren't enabled, how writes stay off the lease path, the safety threshold, optimistic holding). Every algorithm section carries bespoke prose (an opening blurb, a safety-invariant blockquote, a numbered step list, a `Tradeoff` pro/con motivating the next rung) plus a `SimFigure` — a captioned canvas placeholder describing the live animation to be wired to `lease_sim` next. The Bodega section closes with a `RecapTable` showing how each accumulated trait lands in Bodega. All of this prose lives in `content/*.md`; see [Walkthrough content](#walkthrough-content).
3. **Sim (`/sim`)** — a standalone simulator playground. Scenario-setup controls (preset, node count, grantor/grantee selection) sit over a canvas, with playback controls and a scrubbable timeline below. See [Simulator playground](#simulator-playground).

The walkthrough prose is authored per level in markdown (see [Walkthrough content](#walkthrough-content)), so each section reads as its own smooth passage rather than a uniform template — the components are thin templates that slot each section's prose and widgets into place.

## Components

- `Root` — top-level; injects the global stylesheet asset, then mounts `Router::<Route>`.
- `Route` — the routable enum: `Home {}` at `/` and `Sim {}` at `/sim`, both nested under the `Shell` layout.
- `Shell` — layout wrapping every route: renders `Nav`, then the active route via `Outlet`.
- `Home` — the walkthrough page: iterates `content::SECTIONS` (see [Walkthrough content](#walkthrough-content)), rendering each via `WalkthroughSection`.
- `WalkthroughSection { section }` — renders one authored section: the plain heading (`kind == "intro"`) or the algo head — a small muted ordinal (`01`–`05`) and a small-caps pattern tag (`one-to-one` … `all-to-all`, `co-design`) — wrapping that section's ordered blocks.
- `SectionBlock { section, block }` — renders one body block in document order: a prose chunk (`Block::Html`, injected via `dangerous_inner_html` into a `.prose` wrapper), or a widget slotted from the section's fields (`Block::Figure` → `SimFigure`, `Block::Tradeoff` → `Tradeoff`, `Block::Recap` → the recap-lead paragraph + `RecapTable`).
- `Sim` — the standalone simulator playground page; hosts `Playground`.
- `Playground` — the interactive scenario builder + live simulation (in `playground.rs`). See [Simulator playground](#simulator-playground).
- `Nav` — sticky dark-blue banner: "Bodega Consensus" brand on the left, external links (Paper, TLA+, Summerset, Web) and the `Sim*` route link on the right.
- `Tradeoff { pro, con }` — the pro/con pair that motivates the next rung.
- `SimFigure { caption }` — a placeholder `.sim-placeholder` div (a stand-in for the WASM-driven simulation canvas, to be wired to `lease_sim` later) under a `figcaption` describing the live animation to come.
- `RecapTable { rows }` — the closing table mapping each accumulated trait to the rung it first appeared on; `rows` come from the Bodega section's front-matter (`new` rows highlighted).

All external links open in a new tab (`target="_blank"` with `rel="noopener noreferrer"`).

## Simulator playground

The `Playground` component (`web/src/playground.rs`) is a self-contained scenario builder and live animation over the `lease_sim` engine, driven by a four-state `Phase`:

- **`Idle`** (editing) — the default. The controls (preset pills, node-count slider, grantor/grantee toggles, Guard/Renew failure switches, write-load switches) define a *scenario shape*, drawn statically on the canvas as light-gray directed grantor → grantee arrows. Changing any knob returns to this state. Each control-row label (Preset, Nodes, Grantors, Grantees, Msg drop, Writes) and the grant-bar "Grants?" header is a `.pg-hint` — a help-cursor marker that shows a dark tooltip explaining that knob on hover (same bubble style as the caption's `.pg-tconst`).
- **`Generating`** — entered by pressing **Play** (or **Resume**). The current knobs build a `Scenario` and the engine is advanced *live*, animating the messages and lease timers, until the user pauses it or it reaches the cap. There is no automatic stop condition.
- **`Paused`** — the user pressed **Pause** mid-generation. The engine and bookkeeping are kept, so **Resume** continues the run from where it left off. Scrubbable while paused.
- **`Capped`** — generation reached the `MAX_TICKS` cap; generation stops, the engine is dropped, and the recorded frames stay put for free scrubbing.

### Live generation model

The run is generated **incrementally on a wall-clock loop**, not computed all at once — advancing the engine live is what makes the run *visible*, and keeping every frame is what lets the user scrub freely afterward. A single `use_future` loop ticks every `RENDER_MS` (12 ms); while the phase is `Generating`, each tick advances a batch of `FRAMES_PER_STEP` (1) frames — or `FRAMES_PER_STEP · FF_MULT` (3) while fast-forward is on — and for each frame it:

1. advances the shared `Engine` to the next global time `t` (`+FRAME_TICKS` = 5 ticks per frame — a fine resolution for smooth motion and scrubbing) and snapshots `frame_at(t)` into a `Vec<Frame>`;
2. advances the frame clock, or — once `t` reaches the `MAX_TICKS` (60000) cap — transitions to `Capped` and drops the engine, leaving the frames.

The loop keeps the engine on pause (so **Resume** can pick it back up) and decouples time *resolution* (`FRAME_TICKS`) from wall-clock *pace* by batching frames per repaint.

`build_scenario` produces a `Scenario` from the knobs: every node initiates the leases it grants on a per-poll chance (`initiate_chance = 0.5`, so guarding starts at a staggered random time), no baseline link loss, no crashes — plus the two **message-failure switches** applied as per-kind drops, and the **write switches** as the leader's cadence/mode. Each Play draws a **fresh random seed** from the browser's `Math.random()` (`fresh_seed`), so every run generates new randomness (different drop timing, jitter, staggered initiation) rather than replaying an identical scripted run; the engine itself stays deterministic *given* that seed. Per-run bookkeeping (just the next frame time `t`) lives in a `GenState` signal.

### Message-failure switches

A "Msg drop" control row holds two segmented switches, **Guard** and **Renew**, each offering `Off` / `1%` / `10%` / `30%` / `100%` (a `FailRate` enum → drop probability). The chosen rate is fed to `build_scenario` as a `kind_drop(MsgKind, p)` on that message kind, so the engine drops that fraction of `Guard`s (resp. `Renew`s) on top of the (currently zero) link loss. This makes the guard/renew phases' fragility visible: dropping all `Guard`s stalls establishment entirely; dropping all `Renew`s lets a lease guard but never activate (and any active lease lapses). Unlike the shape knobs, a failure switch **keeps** the selected preset — failure is a property of the run, not the scenario shape.

### Write-load switches

A "Writes" control row holds two more segmented switches (same pill styling). **Every** (`WriteEvery` enum) sets how often the leader — the crowned smallest-id grantee — serves a write: `Never` / `3000 ticks` / `1000 ticks` / `300 ticks`, fed to `build_scenario` as `writes(interval, disruptive)`. **Disruptive** (`Yes` / `No`, a `bool`) picks the write behavior; either way `Write`/`WriteReply`/`Commit` messages sweep the cluster (purple write glyphs, a dark-gray commit checkmark). See [simulator.md](simulator.md#write-path).

- **Disruptive** — each node receiving the write suspends the read leases it *holds* until the write commits, so on the canvas you see held grants collapse and then snap back on the commit (the grantors never stop renewing, so no re-guarding is needed): a visible model of quorum-lease write disruption.
- **Non-disruptive** — leases are left entirely untouched; the write's messages sweep and commit but never change any lease. The grant bars stay steady green throughout — the Bodega model where writes don't interrupt background leases.

Write switches, like failure switches, keep the selected preset. Conversely, **selecting a preset sets all four switches** (via `Preset::switches`) alongside the topology, so each preset opens with a representative configuration: every preset uses a 1% Guard/Renew drop; **One-to-One** and **Lease Manager** have writes off; **Leader** and **Roster** run non-disruptive writes every 3000 ticks; **Quorum** runs disruptive writes every 3000 ticks. The switches remain freely adjustable afterward without clearing the preset.

### Run bar + timeline

The **run bar** is the shared `[control | track | status]` grid:

- The **control cell** holds the **primary button** and the clock glyph. The primary button cycles by phase: **Play** (idle/capped) rebuilds the scenario and starts a fresh live generation — disabled when the scenario declares no lease (no grantor/grantee pair); while generating it becomes a red **Pause** button; while paused it becomes **Resume**. The clock glyph hugs the slider's left edge.
- The **timeline slider** fills the track column. It is *inert* while editing and generating (disabled), and becomes freely scrubbable — bound to the frame `cursor` — while paused or once capped. During generation the cursor auto-follows the newest frame.
- A **status** area on the right: a spinner + "sim running" while generating, a gray "run stopped" while paused, a red "✗ ticks limit" if capped, or an editing hint while idle.

Below the run bar, a compact **time axis** (`.pg-timeaxis`) reuses the same grid: its track cell centers the current scrub time (`t = N`) with the run's end (`max N ticks`) at the right. Its **control cell holds the ghost Restart button** — directly under the primary button, so no separate row is wasted on it. Restart discards the run and returns to static editing (so the next Play starts fresh); it is gray/disabled until a run exists, then clickable while playing, paused, or capped. Its **status cell holds a "3x ⏩" fast-forward toggle** (`.pg-ff`, under the run status): while on, the generation loop advances `FRAMES_PER_STEP · FF_MULT` (3) frames per repaint, so the run and its animation play three times faster.

Changing any scenario knob (or any failure/write switch) discards the current run via `reset_sim`, returning to `Idle` so the canvas shows the static scenario again.

### Grant bars

Under the axis, one **grant bar** per node run-length-encodes its history (a `GrantRun` per segment) from the recorded frames. A segment is shaded **green** by how many of that node's possible grants it holds (deeper with more), an empty track when it holds none; a new run begins whenever the grant count changes. `grant_color(grants, max)` computes the hex; each segment tooltips its grant count and duration.

### Canvas rendering

Both modes lay nodes out with `frame::ring_layout`. When frames exist the canvas is driven by the current `Frame`:

- **Topology backdrop** — the same gray grantor → grantee arrows as the static editing view, drawn beneath the live lease edges so the scenario's links are visible from the very start of a run, before any guard link establishes.
- **Lease edges** are directed arrows (like the static view) colored by the grantee's view — green `Active` (opacity tracks remaining lease life for a visible countdown), solid light-blue `Guarding`, faint gray otherwise; each arrowhead matches its stem color. Every stem is pulled back to the arrowhead base and heads are drawn in a second pass, so no stem shows through a translucent head.
- **Message glyphs** at each in-flight message's interpolated `pos`, colored by phase (guard blue / renew green / revoke orange, write purple, commit dark gray, darkened for contrast) via `MsgGlyph`: a shield for guard-phase messages, a circular "renew" arrow for renewals, a dot for revokes, a pencil for writes, a checkmark for commits. Reply kinds (`GuardReply`/`RenewReply`/`WriteReply`) overlay a small thumbs-up badge marking them as acknowledgements and are tinted a touch lighter than their request counterparts. Each glyph sits on a frosted, semi-transparent backing disk so it reads over the edges and nodes beneath it, and fades in over its initial departure and out over its final approach (`msg_opacity` on flight `progress`) so it emerges from the sender node and vanishes just as it reaches the destination node, rather than popping in/out at the borders.
- **Node aura** — a green halo on any grantee currently holding grants, whose size and depth scale with the fraction of its possible grants held (mirroring the grant bars' green shading); set inline per node via `aura_style`. A node holding a majority additionally gets a green border.
- **Node timer bars** — countdown bars beside each node disk (`node_timers` → `TimerBar`), one per lease the node takes part in, arranged in **two short columns**: an **OUT** column (orange, leases it grants as grantor) beside an **IN** column (green, leases it holds as grantee), each capped by an OUT / IN header (a column is omitted if the node plays no such role). Two columns keep the stack short — `max(out, in)` rows rather than `out + in` — so busy nodes don't grow tall. Each bar carries a small `→N` / `←N` endpoint label, so it reads without hovering. When the lease is **active** the bar's fill width is the remaining-life fraction (`grantor_fill` / `grantee_fill`) in the role color, draining **right-to-left** as it counts toward expiry; when it is **not active** (guarding / idle / expired) the bar is instead filled solid **gray** (`.is-inactive`), so it reads as "no live countdown" rather than an almost-drained active bar. A slightly thicker, darker border makes each bar apparent against the surface. The stack is centered on the disk then pushed **radially outward** from the cluster center (`--tx`/`--ty`, per node) into the empty region outside the ring, so it clears the lease arrows running through the interior; each cell also tooltips its full role, endpoint, and status.
  - **Adaptive spacing** keeps the stacks off their disks and inside the clipped stage regardless of bar count. The push distance is per-node and *direction-aware* (`timer_offset_rem`): a side node needs the stack half-width cleared, a top/bottom node its half-height. And in the playback view the whole ring is shrunk toward center by `ring_scale` — the tightest per-node fit (`node_max_scale`, solving each stack box's outward edges against the stage margin), clamped at `MIN_SCALE` so a pathological all-to-all never collapses the nodes onto the center (it clips a little instead). Node bar counts (`(out, in)` from the grantor/grantee sets) are known even while idle; the editing view keeps the full radius and nodes glide between the two on the `.pg-node` position transition. The fit assumes a conservative stage size (`REM_UNIT`, below the real square `.pg-stage`, capped at 580px tall) so it stays inside the box across viewport widths.
  - Message glyphs and drop bursts derive their positions from the **scaled** node positions (`lerp(pts[from], pts[to], progress)`), not the engine's `m.pos` (which rides the unscaled ring), so in-flight messages travel between the disks as actually drawn once the ring shrinks.
- **Leader crown** — the smallest-id grantee is marked with a small gold crown (`Crown`) perched above its disk, in both the static and playback views. This is a fixed topology annotation (recomputed from the grantee set, not the run state): in the all-to-one leader preset it lands on the sole grantee; more generally it just tags the lowest-id local reader.

### Constants

| Const | Meaning | Value |
| --- | --- | --- |
| `FRAME_TICKS` | ticks per recorded frame (resolution + scrub granularity) | 5 |
| `RENDER_MS` | wall-clock ms between generation repaints | 12 |
| `FRAMES_PER_STEP` | frames advanced per repaint while generating | 1 |
| `FF_MULT` | frames-per-repaint multiplier while fast-forward is on | 3 |
| `MAX_TICKS` | cap on run length; hitting it ends the run as `Capped` | 60000 |

`gloo-timers` (feature `futures`) provides the async `sleep` backing the generation loop on WASM.

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
- [ ] Live simulation canvas on the home walkthrough sections (replaces the `SimFigure` placeholder)
- [ ] Failure/recovery and per-link/per-node knobs in the playground
- [ ] Release build + deployment target
