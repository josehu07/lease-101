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
  Cargo.toml        # depends on dioxus + lease_sim (path = "..")
  Dioxus.toml       # dx app config (title, watch paths)
  index.html        # custom HTML shell (analytics tag; see Analytics below)
  src/
    main.rs         # entry point: launch Router + global assets (Root component)
    components.rs    # routes (Home, Sim), Shell layout, Nav, Section, placeholder
  assets/
    main.css        # global stylesheet (loaded via asset!() macro)
```

The web app depends on the `lease_sim` core via a path dependency, so the same engine that powers native GIF generation powers the in-browser animations.

## Page structure

A sticky dark-blue top banner (shared across routes) over a vertical stack of sections inside a centered column (`max-width ~820px`):

1. **Banner** — "Bodega Consensus" brand on the left (an internal `Link` home); external links (Paper, TLA+, Summerset, Web) plus the internal `Sim*` route link on the right. Full-bleed background, content capped to the body width.
2. **Home (`/`)** — a progressive, blog-post-style walkthrough of the lease algorithms, mirroring [algorithm.md](algorithm.md). An intro establishes the lease primitive (grantor/grantee, time-bounded promise) and previews the four-rung *ladder*, then **one section per level** climbs it — one-to-one → leader → quorum → roster — each motivated by the limitation of the rung beneath it. Every algorithm section carries bespoke prose (a blurb, a safety-invariant blockquote, numbered `steps`, a `Tradeoff` pro/con motivating the next rung) plus a `SimFigure` — a captioned `SimPlaceholder` describing the live animation to be wired to `lease_sim` next. The roster section closes with a `RecapTable` showing how each accumulated trait lands in Bodega.
3. **Sim (`/sim`)** — a standalone simulator playground. Scenario-setup controls (preset, node count, grantor/grantee selection) sit over a canvas, with playback controls and a scrubbable timeline below. See [Simulator playground](#simulator-playground).

The walkthrough prose is authored per level (not data-driven), so each section reads as its own smooth passage rather than a uniform template.

## Components

- `Root` — top-level; injects the global stylesheet asset, then mounts `Router::<Route>`.
- `Route` — the routable enum: `Home {}` at `/` and `Sim {}` at `/sim`, both nested under the `Shell` layout.
- `Shell` — layout wrapping every route: renders `Nav`, then the active route via `Outlet`.
- `Home` — the walkthrough page: the intro `Section`, then one `AlgoSection` per algorithm level.
- `Sim` — the standalone simulator playground page; hosts `Playground`.
- `Playground` — the interactive scenario builder + live simulation (in `playground.rs`). See [Simulator playground](#simulator-playground).
- `Nav` — sticky dark-blue banner: "Bodega Consensus" brand on the left, external links (Paper, TLA+, Summerset, Web) and the `Sim*` route link on the right.
- `Section { id, title, children }` — reusable anchored section with a heading (used for the intro).
- `AlgoSection { id, step, pattern, title, children }` — an algorithm-level section: a big faint ordinal (`01`–`04`) and a small-caps pattern tag (`one-to-one` … `all-to-all`) in the heading, over the level's prose and figure.
- `Tradeoff { pro, con }` — the pro/con pair that motivates the next rung.
- `SimFigure { caption }` — a `SimPlaceholder` under a `figcaption` describing the live animation to come.
- `RecapTable` — the closing table mapping each accumulated trait to the rung it first appeared on.
- `SimPlaceholder` — stand-in for the WASM-driven simulation canvas (wrapped by `SimFigure` on the home sections).

All external links open in a new tab (`target="_blank"` with `rel="noopener noreferrer"`).

## Simulator playground

The `Playground` component (`web/src/playground.rs`) is a self-contained scenario builder and live animation over the `lease_sim` engine, driven by a five-state `Phase`:

- **`Idle`** (editing) — the default. The controls (preset pills, node-count slider, grantor/grantee toggles) define a *scenario shape*, drawn statically on the canvas as light-gray directed grantor → grantee arrows. Changing any knob returns to this state.
- **`Generating`** — entered by pressing **Play**. The current knobs build a `Scenario` and the engine is advanced *live*, one frame per wall-clock tick, animating the messages and lease timers as the cluster settles.
- **`Settled`** — the settle condition held; generation stops and the recorded frames stay put for free scrubbing.
- **`Stopped`** — the user pressed **Stop** mid-generation, a manual settle. Scrubbable like `Settled`, just labeled as a user stop.
- **`Capped`** — generation hit the `MAX_TICKS` cap without ever settling (e.g. fewer grantors than the majority threshold, so grantees can never reach it).

### Live generation model

The run is generated **incrementally on a wall-clock loop**, not computed all at once — advancing the engine live is what makes the settling *visible*, and keeping every frame is what lets the user scrub freely afterward. A single `use_future` loop ticks every `RENDER_MS` (18 ms); while the phase is `Generating`, each tick advances a batch of `FRAMES_PER_STEP` (3) frames, and for each frame it:

1. advances the shared `Engine` to the next global time `t` (`+FRAME_TICKS` = 5 ticks per frame — a fine resolution for smooth motion and scrubbing) and snapshots `frame_at(t)` into a `Vec<Frame>`;
2. updates per-node majority-hold tracking (`major_since`): a node holds a majority when its `Active` grants plus its implicit self-grant are `≥ ⌈n/2⌉`, and the timestamp resets whenever it drops below;
3. checks the **settle condition** — every selected grantee has *continuously* held a majority for at least `SETTLE_MULT·T_expire` (`2·T_expire`) — and on success (or hitting the hidden `MAX_TICKS` = 60000 cap) transitions to `Settled`/`Capped` and drops the engine, leaving the frames.

Batching the fine-grained frames per repaint decouples time *resolution* (`FRAME_TICKS`) from wall-clock *pace*: at 5 ticks/frame × 3 frames per 18 ms, a typical run settles in ~4.5 s while recording ~700 frames.

`build_scenario` produces a failure-free `Scenario`: every node eagerly initiates the leases it grants (`initiate_chance = 1.0`), reliable links, no crashes, fixed seed. Per-run bookkeeping (`t`, `major_since`, threshold, settle window, grantee set) lives in a `GenState` signal.

### Run bar + timeline

A single row under the canvas holds the whole run lifecycle:

- **"Run" label** (hugging the button), then the **Play** button, which (re)builds the scenario and starts a fresh live generation. Disabled when the scenario declares no lease (no grantor/grantee pair). While generating it toggles to a red **Stop** button that ends the run at the current frame (`Stopped`).
- The **timeline slider** grows to fill the row. It is *inert* while editing and generating (disabled), and becomes freely scrubbable — bound to the frame `cursor` — once the run finishes. During generation the cursor auto-follows the newest frame.
- A **status** area on the right: a spinner + "settling…" while generating, a gray "✓ run settled" when auto-settled, a gray "✓ run stopped" when stopped by the user, a red "✗ ticks limit" if capped, or an editing hint while idle.

Below the row, a compact **time axis** shows the run's start (`0`), the current scrub time (`t = … ticks`), and the end.

### Canvas rendering

Both modes lay nodes out with `frame::ring_layout`. When frames exist the canvas is driven by the current `Frame`:

- **Topology backdrop** — the same gray grantor → grantee arrows as the static editing view, drawn beneath the live lease edges so the scenario's links are visible from the very start of a run, before any guard link establishes.
- **Lease edges** are directed arrows (like the static view) colored by the grantee's view — green `Active` (opacity tracks remaining lease life for a visible countdown), solid light-blue `Guarding`, faint gray otherwise; each arrowhead matches its stem color. Every stem is pulled back to the arrowhead base and heads are drawn in a second pass, so no stem shows through a translucent head.
- **Message glyphs** at each in-flight message's interpolated `pos`, colored by phase (guard blue / renew green / revoke orange, darkened for contrast) via `MsgGlyph`: a shield for guard-phase messages, a circular "renew" arrow for renewals, a dot for revokes. Reply kinds (`GuardReply`/`RenewReply`) overlay a small thumbs-up badge marking them as acknowledgements and are tinted a touch lighter than their request counterparts. Each glyph sits on a frosted, semi-transparent backing disk so it reads over the edges and nodes beneath it, and fades in over its initial departure and out over its final approach (`msg_opacity` on flight `progress`) so it emerges from the sender node and vanishes just as it reaches the destination node, rather than popping in/out at the borders.
- **Node aura** — a green halo on any grantee currently holding grants, whose size and depth scale with the fraction of its possible grants held (mirroring the grant bars' green shading); set inline per node via `aura_style`. A node holding a majority additionally gets a green border.

### Constants

| Const | Meaning | Value |
| --- | --- | --- |
| `FRAME_TICKS` | ticks per recorded frame (resolution + scrub granularity) | 5 |
| `RENDER_MS` | wall-clock ms between generation repaints | 18 |
| `FRAMES_PER_STEP` | frames advanced per repaint while generating | 3 |
| `MAX_TICKS` | hidden cap on run length if it never settles | 60000 |
| `SETTLE_MULT` | continuous majority-hold window, in `T_expire` | 2 |

`gloo-timers` (feature `futures`) provides the async `sleep` backing the generation loop on WASM.

## Styling

- Single global `assets/main.css`, loaded via the `asset!()` macro so the build injects a content-hashed URL automatically.
- Light page theme with a dark-blue banner (its own `--nav-*` palette). CSS custom properties for the rest of the palette (`--grantor` orange, `--grantee` green to mirror Figure 2 in the paper). Responsive, system font stack.

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
- [x] Live simulation on the playground canvas (record → playback), failure-free
- [x] Playback controls (Start/Pause/Play/Replay) + scrubbable timeline
- [ ] Live simulation canvas on the home walkthrough sections (replaces `SimPlaceholder`)
- [ ] Failure/recovery and per-link/per-node knobs in the playground
- [ ] Release build + deployment target
