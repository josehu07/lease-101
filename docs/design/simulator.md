# Lease Message Simulator

The `lease_sim` Rust crate: a multi-node, message-passing lease simulator. It powers real-time WASM animations on the web (the engine runs live in-browser, not pre-rendered) and pre-generated GIFs for the blog post. See the algorithms it models in [algorithm.md](algorithm.md).

## Goals

- Clean, friendly API: a caller specifies a **scenario** (how many nodes, who grants to whom) and runs it.
- Everything configurable: per-node behavior (response slowness, tendency to initiate, failure likelihood, clock offset/drift) and per-link behavior (delay shape, drop probability, partitions).
- Continually simulate message passing, tracking state of all nodes, links, messages, and logical lease status.
- Output a stream of timestamped **events**, and turn them into lightweight browser animations.

## Architecture

Discrete-event simulation (DES) core with a continuous view layer on top.

```text
scenario  ──build──▶  Engine (DES, event heap)  ──▶  Event stream (timestamped)
                            │                              │
                            └──▶ Frame geometry (interpolated)
                                        │
                          ┌─────────────┴─────────────┐
                     WASM + Canvas2D (live)      native GIF (feature-gated)
```

### Layering

- **`sim` (core)** — owns all state; advances via a min-heap event queue keyed on integer virtual time. Pure logic, no layout/drawing. Deterministic.
- **`scenario`** — builder API producing the initial `Engine` state.
- **frame geometry** — Rust computes layout + interpolated drawables into a `Frame`; the frontend (thin Canvas2D) and the native GIF tool both just paint `Frame`s. Maximizes logic reuse, minimizes frontend code.

### Determinism & no-std-ish footprint

- Seedable hand-rolled PRNG (PCG/xorshift). **No `getrandom`/`rand`** so the WASM build stays tiny and fully reproducible.
- Integer virtual time (call it ticks) as the DES clock.

### Clock model (skew & drift are first-class)

Each node has its own clock: `local = offset + drift · global`. This makes clock skew and drift *visible*, which is the whole point of a lease demo (see the skew-vs-drift reasoning in [algorithm.md](algorithm.md)).

## Driving model (pull-based)

The frontend drives the engine by virtual time:

- `engine.advance_to(t) -> &[Event]` — advance the DES, return newly produced timestamped events.
- `engine.frame_at(t) -> Frame` — renderable state at time `t`.

In-flight messages and shrinking lease-timer bars are **interpolated** between discrete events, so each browser `requestAnimationFrame` is a cheap query at the current virtual time. The same events/frames feed both the live demo and the GIF generator.

## Configuration surface

- **Distributions** via a small `Dist` enum: `Fixed`, `Uniform`, `Normal`.
- **Per-node:** clock offset, clock drift, response delay (`Dist`), tendency to initiate a first step, failure hazard, recovery hazard.
- **Per-link:** delay distribution (`Dist`), drop probability, partition toggle.
- **Relationships:** intended grantor → grantee lease pairs.

## Event stream (illustrative)

- `MessageSent { from, to, kind, sent, arrival, fate }`
- `LeaseGranted | LeaseHeld | LeaseRenewed | LeaseExpired | LeaseRevoked`
- `NodeFailed | NodeRecovered`

Message kinds mirror the algorithm: `Guard`, `GuardReply`, `Renew`, `RenewReply`, `Revoke`.

## Packaging

Single crate, `crate-type = ["cdylib", "rlib"]`:

- Core + WASM bindings are the default build (lean).
- Native GIF rendering is **feature-gated** behind `native-render` (off by default), so the WASM build pulls in no heavy image/encoding deps.

## Status

- [x] DES core (engine, event heap, virtual clock) — `src/engine.rs`
- [x] Per-node clock model (offset/drift) — `src/clock.rs`
- [x] PRNG + `Dist` distributions — `src/rng.rs`, `src/dist.rs`
- [x] Scenario builder API — `src/scenario.rs`
- [x] Lease state machine (one-to-one, per [algorithm.md](algorithm.md)) — `src/engine.rs`
- [x] Event stream — `src/event.rs`
- [x] Frame geometry + interpolation — `src/frame.rs`, `Engine::frame_at`
- [ ] WASM bindings (`wasm-bindgen`)
- [ ] Native GIF renderer (feature `native-render`)

### Module map

| Module | Responsibility |
| --- | --- |
| `rng` | Seedable `xoshiro256**` PRNG, no external deps |
| `clock` | `Time` alias + per-node `Clock` (offset/drift) |
| `dist` | `Dist` (Fixed/Uniform/Normal) sampling |
| `event` | `NodeId`, `LeaseId`, `MsgKind`, `LeaseStatus`, `Event`/`EventKind` |
| `scenario` | `Scenario` builder, `NodeConfig`, `LinkConfig`, `LeaseParams` |
| `engine` | DES `Engine`: `advance_to` (event stream) + `frame_at` (geometry) |
| `frame` | `Frame` geometry: `NodeShape`, `MsgShape`, `LeaseBar`, ring layout, lerp |

### Notes for next steps

- The lease state machine currently models the full one-to-one protocol: guard phase (`Guard`/`GuardReply`), renew phase (`Renew`/`RenewReply`), `Revoke`, no-reply safe expiry, and per-party expiry detection.
- Safety (`grantor expiry >= grantee expiry` in real time) is checked by tests under both perfect and skewed/drifting clocks.
- `frame_at` is read-only and pull-based — the frontend calls `advance_to(t)` then `frame_at(t)` each animation tick.
