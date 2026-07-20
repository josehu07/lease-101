# Lease Message Simulator

The `lease_sim` Rust crate: a multi-node, message-passing lease simulator. It powers real-time WASM animations on the web (the engine runs live in-browser, not pre-rendered) and, via a headless-browser capture of that same web canvas, the pre-generated GIFs for the blog post (see [GIF capture](#gif-capture)). See the algorithms it models in [algorithm.md](algorithm.md).

## Goals

- Clean, friendly API: a caller specifies a **scenario** (how many nodes, who grants to whom) and runs it.
- Everything configurable: per-node behavior (response slowness, tendency to initiate, failure likelihood, clock offset/drift) and per-link behavior (delay shape, drop probability, partitions).
- Continually simulate message passing, tracking state of all nodes, links, messages, and logical lease status.
- Output a stream of timestamped **events**, and turn them into lightweight browser animations.
- **Drivable deterministically**: scripted or interactive [commands](#driving-commands) (initiate / revoke / fail / recover / write) let a caller cause specific events, not just watch stochastic ones.

## Scope: one primitive, many patterns

The engine models exactly **one** thing: the one-to-one lease primitive (guard → renew → revoke / passive expiry / failure), run over arbitrary directed grantor → grantee pairs. That is deliberately all it models.

Per [algorithm.md](algorithm.md), the higher levels are **not new mechanisms** — leader, quorum, and roster leases are this same primitive replicated over a different *pattern* of pairs, plus a **majority-counting interpretation** (`≥ ⌊n/2⌋+1` grants ⇒ stable). The engine therefore:

- provides the patterns as thin scenario helpers (`all_to_one`, `all_to_many`, `all_to_all`) that just declare the lease pairs;
- exposes each party's lease status separately (see `LeaseBar`) so the counting rule is a trivial derived view in the consumer.

Mostly **out of scope** (these belong to a layer *above* a lease-message simulator): the Paxos/consensus log, real objects/values, roster ballots and slot-threshold safety, and optimistic holding. Keeping them out is what keeps the core minimal.

One deliberate exception is a **stylized write path** (the `Write`/`WriteReply`/`Commit` messages), enough to *illustrate* how writes interact with leases without a real consensus log. It comes in two modes — **disruptive** (quorum-lease coupling: a write suspends the reads each node holds until it commits) and **non-disruptive** (Bodega-style: leases are untouched). See [Write path](#write-path).

## Architecture

Discrete-event simulation (DES) core with a continuous view layer on top.

```text
scenario  ──build──▶  Engine (DES, event heap)  ──▶  Event stream (timestamped)
                            │                              │
                            └──▶ Frame geometry (interpolated)
                                        │
                                WASM + SVG/DOM (live canvas)
                                        │
                            headless-browser frame capture ──▶ GIF
```

### Layering

- **`sim` (core)** — owns all state; advances via a min-heap event queue keyed on integer virtual time. Pure logic, no layout/drawing. Deterministic.
- **`scenario`** — builder API producing the initial `Engine` state.
- **frame geometry** — Rust computes layout + interpolated drawables into a `Frame`; the web frontend (thin SVG/DOM via Dioxus RSX) paints `Frame`s. The GIFs are captured *from* that same rendered canvas (see [GIF capture](#gif-capture)), so there is a single drawing path — no separate native renderer to keep in sync. Maximizes logic reuse, minimizes frontend code.

### Determinism & no-std-ish footprint

- Seedable hand-rolled PRNG (`xoshiro256**`, seeded via SplitMix64). **No `getrandom`/`rand`** so the WASM build stays tiny and fully reproducible.
- Integer virtual time (call it ticks) as the DES clock.

### Clock model (skew & drift are first-class)

Each node has its own clock: `local = offset + drift · global`. This makes clock skew and drift *visible*, which is the whole point of a lease demo (see the skew-vs-drift reasoning in [algorithm.md](algorithm.md)).

## Driving model (pull-based)

The frontend drives the engine by virtual time:

- `engine.advance_to(t) -> Vec<Event>` — advance the DES, return newly produced timestamped events **in ascending time order**.
- `engine.frame_at(t) -> Frame` — renderable state at time `t`.

In-flight messages and shrinking lease-timer bars are **interpolated** between discrete events, so each browser `requestAnimationFrame` is a cheap query at the current virtual time. The same events/frames feed both the live demo and the GIF generator.

The event stream is guaranteed sorted by time: message *sends* emit immediately, while their delivery/drop and any reply are scheduled on the heap, so nothing is emitted with a timestamp behind `now`.

## Driving commands

Beyond stochastic behavior, a caller can inject deterministic actions via a `Command`:

- `Initiate(LeaseId)` — an idle grantor opens (guards) the lease.
- `Revoke(LeaseId)` — a grantor proactively revokes; it stops renewing and notifies the grantee (safe whether or not the grantee is reached — the grantor's `D'` still bounds it). The grantee drops its hold and returns a `RevokeReply`; **on that ack the grantor leaves the granting state at once** rather than waiting for `D'` to lapse. This stays safe: the ack is a round-trip *after* the grantee expired, so the grantee's expiry already precedes the grantor's, preserving `grantor expiry ≥ grantee expiry`. (Without the ack the grantor simply lets `D'` lapse; the ack just ends it promptly.)
- `FailNode(NodeId)` / `RecoverNode(NodeId)` — crash or restore a node.
- `Write` — the leader serves one write round now (the scripted one-shot form of a `WriteTick`; disruptive or not per `write_disruptive`). Lets a scenario inject a single write at a chosen tick without arming a periodic cadence (`writes(None, true)`).

Two ways in, both reproducible per seed:

- **Scripted** on the scenario: `Scenario::command(at, cmd)` fires at global time `at`.
- **Live** on the engine: `engine.command(cmd)` (now) and `engine.schedule_command(at, cmd)` — for interactive clicks in the playground.

Reply latency is modeled by scheduling the reply-send after the responder's think time (not by mutating `now`), so a node that crashes mid-processing correctly never emits its reply.

## Configuration surface

- **Distributions** via a small `Dist` enum: `Fixed`, `Uniform`, `Normal`.
- **Per-node:** clock offset, clock drift, response delay (`Dist`), tendency to initiate a first step, failure hazard, recovery hazard. `all_nodes(f)` applies one closure to every node for symmetric patterns.
- **Per-link:** delay distribution (`Dist`), drop probability, partition toggle. Unlisted links fall back to a sensible default, so pattern helpers need only declare *leases*.
- **Per-message-kind:** an extra drop probability per `MsgKind`, set with `kind_drop(kind, p)` and layered on top of the per-link `drop_chance` (combined as one independent probability `1 − (1−link)(1−kind)`). Lets a caller fail a whole *class* of messages — e.g. every `Guard` or every `Renew` — without touching link reliability. `kind_drop_from(kind, p, from)` is the time-gated variant: the drop only applies from global tick `from` onward, so a scenario can establish cleanly and *then* start losing a kind (e.g. drop `RenewReply`s only after the lease is active).
- **Relationships:** intended grantor → grantee lease pairs, declared individually (`lease`) or via a pattern helper (`all_to_one`, `all_to_many`, `all_to_all`).
- **Writes:** `writes(interval, disruptive)` sets the leader's write cadence (`Some(avg_ticks)` with ±20% jitter, or `None` to disable) and whether writes are disruptive (see [Write path](#write-path)).
- **Commands:** scripted `(time, Command)` pairs (see [Driving commands](#driving-commands)).

### Default timings

`LeaseParams::default()` and the default link delay follow one fixed set of relationships, anchored on `t_delta` (`T_Δ`, ticks). All in ticks:

| Constant | Meaning | Default | Relationship |
| --- | --- | --- | --- |
| `t_delta` | max clock-drift budget `T_Δ` | 100 | anchor |
| link delay | one-way message time `T_msg` | `Uniform{120,280}` | `≈ 2·T_delta`, jittered ±40% |
| `renew_interval` | renew cadence `T_renew` | 600 | `≈ 3·T_msg` |
| `t_guard` | guard window `T_guard` | 1500 | `≈ 2.5·renew_interval` |
| `t_lease` | lease lifetime `T_expire` | 1500 | `≈ 2.5·renew_interval` (`≈ t_guard`) |

## Event stream

`EventKind` variants:

- `MessageSent { from, to, kind, sent, arrival, fate }` — emitted the moment a message leaves; carries its eventual `fate` so the view can foreshadow a drop.
- `MessageDelivered { from, to, kind }` / `MessageDropped { from, to, kind }` — at arrival time.
- `GrantorLease { lease, status }` / `GranteeLease { lease, status }` — a party's lease status transition (each side reported independently).
- `NodeFailed { node }` / `NodeRecovered { node }`.
- `WriteStarted { leader }` / `WriteCommitted { leader }` — a write round (either mode) began / committed at the leader (see [Write path](#write-path)).

Message kinds mirror the algorithm — `Guard`, `GuardReply`, `Renew`, `RenewReply`, `Revoke`, `RevokeReply` (the grantee's acknowledgement that it dropped a revoked lease; on receipt the grantor safely leaves the granting state — see [Driving commands](#driving-commands)) — plus the write path: `Write`, `WriteReply`, `Commit`.

## Write path

A stylized write path, enabled by `writes(interval, disruptive)`, illustrates how writes interact with leases. The **leader** is the smallest-id grantee (matching the playground's crowned node). A `WriteTick` fires every `interval` ± 20% jitter (or a scripted `Command::Write` fires one write on demand, for scenarios that want a single write with no periodic cadence); the leader opens a round and broadcasts `Write` to every peer. Each write carries a stable **id** (rides the message's `lease_idx` slot, unused for write messages), so overlapping rounds stay distinct. Both modes share the commit rule: the leader commits round `id` once its reply set (itself counted implicitly) both reaches a `majority = ⌊n/2⌋+1` **and** covers every grantee node, then emits `WriteCommitted` and broadcasts `Commit`.

**Disruptive** — a write **tears down** the leases it touches (models quorum-lease write coupling, where the write is itself the revocation notice), so they must be **re-established from scratch, re-guarding**, once the write commits — the "Lease teardown" cost in [algorithm.md](algorithm.md#write-disrupts-local-reads). One round at a time (a new tick is skipped while a round is outstanding):

1. **Suspend + broadcast.** The leader tears down every lease it takes part in — the reads it holds *as a grantee* (active/guarding → expired) and the grants it makes *as a grantor* (→ inactive, stops renewing) — *freezes* itself (so it won't re-guard or re-activate mid-write), emits `WriteStarted`, and broadcasts `Write`.
2. **Peer suspend + reply.** Each peer receiving `Write` does the same — tears down both sides of its leases, freezes, and replies `WriteReply` after its think time. No `Revoke` is sent: the `Write` *is* the notification.
3. **Commit + re-establish.** On commit the leader thaws and re-establishes locally; each node receiving `Commit` does the same (`thaw_and_reguard`). Recovery is **deterministic and commit-driven**: the moment a grantor learns the write committed, it re-opens a fresh **guard** for each grant the write tore down (tracked by a per-lease `reguard_on_thaw` flag set during teardown) — no waiting on a stochastic re-initiation. Each lease then re-establishes through the full guard → renew handshake, paying the guard round-trips again. (A stuck round that hits `WRITE_ROUND_TIMEOUT` thaws and re-guards the same way, so an aborted write still recovers.)

Both endpoints are torn down as their nodes are notified: the leader in `begin_write`, every peer in `on_write` — so a lease's grantor and grantee each reset when they learn of the write. In an all-to-one topology the leader is the sole grantee; a write suspends its held majority and resets every grantor's grant to it, and the majority visibly rebuilds via a fresh guard round after the `Commit`.

**Non-disruptive** — leases are left entirely untouched (models Bodega's background leases, where writes don't interrupt reads). Rounds may overlap freely:

1. **Broadcast.** The leader emits `WriteStarted` and broadcasts `Write` — without touching any leases or node state.
2. **Peer reply.** Each peer keeps its leases/renews running and simply replies `WriteReply`.
3. **Commit.** On commit the leader emits `WriteCommitted` and broadcasts `Commit`, which every node ignores for lease purposes. The `Write`/`WriteReply`/`Commit` messages sweep and animate, but no lease or node state ever changes.

A round that never reaches its commit condition (a dropped `Write`/`WriteReply`) is abandoned after `WRITE_ROUND_TIMEOUT` (1500 ticks) on the leader's poll. Disruptive: thaw every frozen node and re-guard its torn-down grants, exactly as a commit would. Non-disruptive: dropping the stale round is the only cleanup needed, since no node state was touched.

## GIF capture

The blog-post GIFs are captured from the **real rendered web canvas**, not a separate renderer — so a GIF frame is pixel-identical to what a reader sees on the site, with zero drawing code to keep in sync. The pipeline (Python, `scripts/gifcap.py`):

1. **Capture route.** The web app exposes `/capture/:name` (`web/src/scenarios.rs::Capture`, outside the nav `Shell` layout): a bare, frame-stepped `.pg-stage` with no title/run-bar/grant-bars. It pre-generates the *whole* run offline via `sim_view::generate_frames` — byte-for-byte the frames the live `use_sim_run` loop would record (same `FRAME_TICKS` stepping + `StopWhen` stop logic) — and shows one frame at a time. A `pg-capture` CSS class disables all transitions/animations, so a stepped frame is the exact settled state at its tick, not a mid-tween.
2. **External stepping.** The route installs a `window.__setFrame(n)` hook (over Dioxus's eval JS↔Rust channel), publishes the run length as `window.__frameCount`, and mirrors the shown index into a `data-frame` attribute. The driver sets a frame, waits for `data-frame` to match, then screenshots the stage.
3. **Encode.** [Playwright](https://playwright.dev) drives headless Chromium at desktop width (serving the built `dx` dist with an SPA `index.html` fallback for the deep-link route), and Pillow encodes the frames into a GIF that loops forever with a short static hold at both ends. Frames are subsampled to a ~20 ms GIF delay while preserving the site's wall-clock playback speed, and a single shared 256-color palette avoids inter-frame flicker.
4. **Optimize.** Each GIF is finally shrunk in place with `gifsicle` (`-O3 --colors 64 --lossy=200`) — frame-diff optimization plus a palette+lossy pass that takes ~35% off the dense scenes with no visible quality loss at display size. `gifsicle` is an **external dependency** (install via e.g. `brew install gifsicle`); if it isn't on `PATH` the step is skipped with a warning and the un-optimized GIF is kept.

Output filenames are prefixed `lease-101-` (namespacing them in the blog site's shared asset dir); captured at `--scale 2` (2× device pixels) for crisp text. The blog post itself (`scripts/bloggen.py`) concatenates the walkthrough sections and swaps each `:::figure` for its captured GIF — see [webpage.md](webpage.md#plain-blog-post).

## Packaging

Single crate, `crate-type = ["cdylib", "rlib"]`:

- The lean core is the default build; the `web/` crate depends on it by path and `dx` compiles it to WASM directly (no hand-written `wasm-bindgen` glue — the frontend calls `advance_to`/`frame_at` in-process).
- GIF generation needs no extra Cargo features or native image/encoding deps: it reuses the WASM canvas via headless-browser capture (see [GIF capture](#gif-capture)), keeping the crate build lean. The capture + blog tooling lives in Python (`scripts/`, driven by `uv`).

## Status

- [x] DES core (engine, event heap, virtual clock) — `src/engine.rs`
- [x] Per-node clock model (offset/drift) — `src/clock.rs`
- [x] PRNG + `Dist` distributions — `src/rng.rs`, `src/dist.rs`
- [x] Scenario builder API — `src/scenario.rs`
- [x] Lease state machine (one-to-one, per [algorithm.md](algorithm.md)) — `src/engine.rs`
- [x] Event stream (time-ordered) — `src/event.rs`
- [x] Deterministic driving commands (initiate/revoke/fail/recover/write), scripted + live — `src/engine.rs`
- [x] Pattern helpers (`all_to_one`/`all_to_many`/`all_to_all`) — `src/scenario.rs`
- [x] Frame geometry + interpolation — `src/frame.rs`, `Engine::frame_at`
- [x] Write path — disruptive (lease churn) + non-disruptive (no lease effect) — `src/engine.rs`
- [x] WASM consumption — the `web/` crate uses the core by path and `dx` builds it to WASM (no separate `wasm-bindgen` layer needed)
- [x] GIF capture — headless-browser screenshot of the `/capture/:name` route (`scripts/gifcap.py`), reusing the live canvas; no native renderer needed

### Module map

| Module | Responsibility |
| --- | --- |
| `rng` | Seedable `xoshiro256**` PRNG, no external deps |
| `clock` | `Time` alias + per-node `Clock` (offset/drift) |
| `dist` | `Dist` (Fixed/Uniform/Normal) sampling |
| `event` | `NodeId`, `LeaseId`, `MsgKind`, `LeaseStatus`, `Command`, `Event`/`EventKind` |
| `scenario` | `Scenario` builder, `NodeConfig`, `LinkConfig`, `LeaseParams`, pattern helpers |
| `engine` | DES `Engine`: `advance_to` (event stream) + `frame_at` (geometry) + `command` |
| `frame` | `Frame` geometry: `NodeShape`, `MsgShape`, `LeaseBar` (per-party status), ring layout, lerp |
| `demos` | Fixed-seed demo `Scenario`s for the walkthrough figures (e.g. `one_to_one_success`), natively unit-tested |

### Implementation notes

The lease state machine models the full one-to-one protocol: guard phase (`Guard`/`GuardReply`), renew phase (`Renew`/`RenewReply`), `Revoke`, no-reply safe expiry, and per-party expiry detection. This one primitive is sufficient for all four algorithm levels (see [Scope](#scope-one-primitive-many-patterns)); leader/quorum/roster are patterns + counting on top, done in the consumer.

Three timeouts keep a message loss from stranding a lease (each detailed in the engine's `const` and fn doc-comments):

- **Guard-phase give-up (grantor):** a `Guarding` attempt unanswered for a full guard window `t_guard − t_delta` (same length as the grantee's `A'`) falls back to `Inactive`; re-guarding is then the ordinary per-poll `initiate_chance` path, which begins *from* that idle state — so a dropped `Guard`/`GuardReply` never strands the grantor, and a retry is a whole guard phase away, not a fraction of one. (There is no separate short retry timer — the grantor holds the guard for exactly as long as the grantee would still accept the activating renew.)
- **Guard-window expiry (grantee):** a `Guarding` grantee whose activating first `Renew` never arrives expires once its guard deadline `A'` lapses (checked in `recompute_statuses`), mirroring the grantor-side give-up.
- **One renew in flight + renew-reply timeout (grantor):** the grantor sends the next `Renew` only once the previous one's reply is confirmed (`awaiting_reply`), so a dropped reply *halts* the renew stream rather than sending more un-acked renews. It then waits, and after `RENEW_REPLY_TIMEOUT` (1500 ≈ one `t_lease`) with no confirmation `expire_stale_renews` stops intending the lease (an `awaiting_reply` lease would otherwise hold its `D'`/intent forever), letting `D'` lapse so the lease expires and re-guards.

Countdown bars: a `LeaseBar`'s `*_fill` is the fraction remaining toward each party's **real** expiry — the grantee's `hold_expiry` (`C'`) and the grantor's `grant_expiry` (`D'`). There is no separate display timer; the bar *is* the safety bound, so it can never drift out of sync with the actual gray-out. Each is normalized by the span its bound is set a whole *away* from at (re)arm — the grantee's `C'` by `t_lease − t_delta` (its distance from receipt), the grantor's `D'` by the full provisioned grant span `t_guard + t_lease + 2·t_delta` (its distance from the send, see `send_renew`). That normalization is what keeps the bar **always draining** rather than pegged: the fill hits 1.0 only at the instant of (re)arm and falls continuously from there. So both sides show a clean sawtooth while renewing — the grantor's `D'` maintained by extend-on-send / shorten-on-reply (see [algorithm.md](algorithm.md#phase-2--renew-steady-state-promise-exchanges)) — and once replies stop, `D'` holds at its last value and the bar drains smoothly to 0 exactly at expiry. (Normalizing the over-provisioned `D'` by only `t_lease` would instead peg the bar at full until `local` came within a span of `D'`, then drain — the bug this avoids.)

The *guard-phase* bars drain over the guard-phase length `t_guard − t_delta` on both sides, and read identically. The grantee's is its real acceptance window `A'` (`guard_deadline`), a genuine protocol deadline. The grantor has **no** protocol guard deadline — it just awaits the `GuardReply` — but it holds the guard for the same span before giving up (`expire_stale_guards`), so `grantor_fill` while `Guarding` counts down from `guard_since` over that same window; the bar reaching empty coincides with the grantor falling idle. This is both faithful (the two sides run equal-length guard timers) and consistent on screen (neither guard bar drains faster than the other or than the active bars).

Other invariants: safety (`grantor expiry >= grantee expiry` in real time) is test-checked under perfect and skewed/drifting clocks; `frame_at` is read-only and pull-based (the frontend calls `advance_to(t)` then `frame_at(t)` per animation tick); stochastic hazards, timeouts, and expiry detection are quantized to `POLL_INTERVAL` (50 ticks), while commands/sends/arrivals fire at their exact scheduled tick.
