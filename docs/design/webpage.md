# Webpage Design

The Dioxus single-page static website for the Distributed Lease 101 walkthrough. It renders entirely client-side (WASM) and runs the `lease_sim` engine live in the browser to drive animations. See [simulator.md](simulator.md) for the engine and [algorithm.md](algorithm.md) for the algorithms being illustrated.

## Goals

- A single, scrollable page: a friendly walkthrough post, top to bottom.
- Static hosting only — no server, no backend. Just `index.html` + WASM + assets.
- Animations run **live** on the simulation engine (not pre-rendered). The blog post version uses pre-generated GIFs instead; this site does not.
- Lightweight and fast to load.

## Tech stack

- **Dioxus 0.7** (pinned `=0.7.3` to match the installed `dx` CLI; `dx` refuses mismatched dioxus versions).
- Features: `web` (client-side WASM renderer) + `router`.
- Build/serve via the `dx` CLI: `dx serve` (dev) and `dx build --platform web` (static output under `target/dx/lease_web/debug|release/web/public`).

## Repository layout

This repo is a Cargo workspace:

```text
Cargo.toml          # workspace root; also the `lease_sim` core lib package
src/                # lease_sim core (engine, scenario, frame, ...)
web/                # the Dioxus app (package `lease_web`)
  Cargo.toml        # depends on dioxus + lease_sim (path = "..")
  Dioxus.toml       # dx app config (title, watch paths)
  src/
    main.rs         # entry point: launch + global assets (Root component)
    components.rs    # page components (App, Hero, Section, sim placeholder)
  assets/
    main.css        # global stylesheet (loaded via asset!() macro)
```

The web app depends on the `lease_sim` core via a path dependency, so the same engine that powers native GIF generation powers the in-browser animations.

## Page structure

A sticky dark-blue top banner over a vertical stack of sections inside a centered column (`max-width ~820px`):

1. **Banner** — "Bodega Consensus" brand on the left; external links (Paper, Summerset, GitHub) on the right. Full-bleed background, content capped to the body width.
2. **Intro** — what a lease is (grantor/grantee, time-bounded promise).
3. **One algorithm section per level**, mirroring [algorithm.md](algorithm.md): one-to-one, leader, quorum, and roster leases. Each has a blurb and a live simulation animation (currently a `SimPlaceholder`; wired to `lease_sim` next).

The section list is data-driven (a `SectionMeta` slice), so the sections stay in sync from one source.

## Components

- `Root` — top-level; injects the global stylesheet asset, then renders `App`.
- `App` — composes the page: `Nav`, the intro, then one `Section` per entry in `ALGO_SECTIONS`.
- `Nav` — sticky dark-blue banner: "Bodega Consensus" brand on the left, external links (Paper, Summerset, GitHub) on the right.
- `Section { id, title, children }` — reusable anchored section with a heading.
- `SimPlaceholder` — stand-in for the WASM-driven simulation canvas.
- `ALGO_SECTIONS` — a `SectionMeta` slice (id, title, blurb) that drives the section bodies.

All external links open in a new tab (`target="_blank"` with `rel="noopener noreferrer"`).

## Styling

- Single global `assets/main.css`, loaded via the `asset!()` macro so the build injects a content-hashed URL automatically.
- Light page theme with a dark-blue banner (its own `--nav-*` palette). CSS custom properties for the rest of the palette (`--grantor` orange, `--grantee` green to mirror Figure 2 in the paper). Responsive, system font stack.

## Status

- [x] Workspace + `web/` Dioxus crate scaffolded
- [x] Static build verified (`dx build --platform web` produces index.html + wasm)
- [x] Page shell: light theme + dark-blue banner + walkthrough sections
- [x] Sticky nav + one skeleton section per algorithm level (data-driven)
- [ ] Live simulation canvas wired to `lease_sim` (replaces `SimPlaceholder`)
- [ ] Per-scenario interactive controls (nodes, links, knobs)
- [ ] Release build + deployment target
