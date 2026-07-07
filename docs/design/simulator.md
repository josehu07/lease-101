# Lease Message Simulator

The `lease_sim` Rust crate: a multi-node, message-passing lease simulator. It powers real-time WASM animations on the web (the engine runs live in-browser, not pre-rendered) and pre-generated GIFs for the blog post. See the algorithms it models in [algorithm.md](algorithm.md).

## Goals

- Clean, friendly API: a caller specifies a **scenario** (how many nodes, who grants to whom) and runs it.
- Everything configurable: per-node behavior (response slowness, tendency to initiate, failure likelihood, clock offset/drift) and per-link behavior (delay shape, drop probability, partitions).
- Continually simulate message passing, tracking state of all nodes, links, messages, and logical lease status.
- Output a stream of timestamped **events**, and turn them into lightweight browser animations.
- **Drivable deterministically**: scripted or interactive [commands](#driving-commands) (initiate / revoke / fail / recover) let a caller cause specific events, not just watch stochastic ones.

## Scope: one primitive, many patterns

The engine models exactly **one** thing: the one-to-one lease primitive (guard → renew → revoke / passive expiry / failure), run over arbitrary directed grantor → grantee pairs. That is deliberately all it models.

Per [algorithm.md](algorithm.md), the higher levels are **not new mechanisms** — leader, quorum, and roster leases are this same primitive replicated over a different *pattern* of pairs, plus a **majority-counting interpretation** (`≥ ⌈n/2⌉` grants ⇒ stable). The engine therefore:

- provides the patterns as thin scenario helpers (`all_to_one`, `all_to_many`, `all_to_all`) that just declare the lease pairs;
- exposes each party's lease status separately (see `LeaseBar`) so the counting rule is a trivial derived view in the consumer.

Explicitly **out of scope** (these belong to a layer *above* a lease-message simulator, not in it): the Paxos/consensus log, reads/writes and objects, quorum-lease write-path coupling, roster ballots and slot-threshold safety, and optimistic holding. Keeping them out is what keeps the core minimal.

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

- `engine.advance_to(t) -> Vec<Event>` — advance the DES, return newly produced timestamped events **in ascending time order**.
- `engine.frame_at(t) -> Frame` — renderable state at time `t`.

In-flight messages and shrinking lease-timer bars are **interpolated** between discrete events, so each browser `requestAnimationFrame` is a cheap query at the current virtual time. The same events/frames feed both the live demo and the GIF generator.

The event stream is guaranteed sorted by time: message *sends* emit immediately, while their delivery/drop and any reply are scheduled on the heap, so nothing is emitted with a timestamp behind `now`.

## Driving commands

Beyond stochastic behavior, a caller can inject deterministic actions via a `Command`:

- `Initiate(LeaseId)` — an idle grantor opens (guards) the lease.
- `Revoke(LeaseId)` — a grantor proactively revokes; it stops renewing and notifies the grantee, while letting its own `D'` lapse naturally (safe whether or not the grantee is reached).
- `FailNode(NodeId)` / `RecoverNode(NodeId)` — crash or restore a node.

Two ways in, both reproducible per seed:

- **Scripted** on the scenario: `Scenario::command(at, cmd)` fires at global time `at`.
- **Live** on the engine: `engine.command(cmd)` (now) and `engine.schedule_command(at, cmd)` — for interactive clicks in the playground.

Reply latency is modeled by scheduling the reply-send after the responder's think time (not by mutating `now`), so a node that crashes mid-processing correctly never emits its reply.

## Configuration surface

- **Distributions** via a small `Dist` enum: `Fixed`, `Uniform`, `Normal`.
- **Per-node:** clock offset, clock drift, response delay (`Dist`), tendency to initiate a first step, failure hazard, recovery hazard. `all_nodes(f)` applies one closure to every node for symmetric patterns.
- **Per-link:** delay distribution (`Dist`), drop probability, partition toggle. Unlisted links fall back to a sensible default, so pattern helpers need only declare *leases*.
- **Relationships:** intended grantor → grantee lease pairs, declared individually (`lease`) or via a pattern helper (`all_to_one`, `all_to_many`, `all_to_all`).
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
- [x] Event stream (time-ordered) — `src/event.rs`
- [x] Deterministic driving commands (initiate/revoke/fail/recover), scripted + live — `src/engine.rs`
- [x] Pattern helpers (`all_to_one`/`all_to_many`/`all_to_all`) — `src/scenario.rs`
- [x] Frame geometry + interpolation — `src/frame.rs`, `Engine::frame_at`
- [ ] WASM bindings (`wasm-bindgen`)
- [ ] Native GIF renderer (feature `native-render`)

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

### Notes for next steps

- The lease state machine models the full one-to-one protocol: guard phase (`Guard`/`GuardReply`), renew phase (`Renew`/`RenewReply`), `Revoke`, no-reply safe expiry, and per-party expiry detection. This one primitive is intended to be sufficient for all four algorithm levels (see [Scope](#scope-one-primitive-many-patterns)); the leader/quorum/roster levels are patterns + counting on top, done in the consumer.
- Safety (`grantor expiry >= grantee expiry` in real time) is checked by tests under both perfect and skewed/drifting clocks.
- `frame_at` is read-only and pull-based — the frontend calls `advance_to(t)` then `frame_at(t)` each animation tick.
- Per-node poll granularity is `POLL_INTERVAL` (200 ticks): stochastic hazards and expiry detection are quantized to it. Commands, sends, and arrivals are not — they fire at their exact scheduled tick.
