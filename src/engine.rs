//! Discrete-event simulation engine and the one-to-one lease state machine.
//!
//! Global time is authoritative; each node reads it through its own clock. The
//! engine advances by draining an internal min-heap of scheduled items (message
//! sends, arrivals, per-node polls, and driving commands) up to a requested
//! global time, emitting a time-ordered stream of [`Event`]s as side effects.
//!
//! The engine models only the *one-to-one* lease primitive over arbitrary
//! directed grantor -> grantee pairs. The higher-level algorithms (leader,
//! quorum, roster) are this same primitive run over different patterns plus a
//! majority-counting rule; that counting is a derived view over lease state, not
//! engine logic, and lives in the consumer. See `docs/design/algorithm.md`.

use core::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap};

use crate::clock::Time;
use crate::dist::Dist;
use crate::event::{Command, Event, EventKind, LeaseId, LeaseStatus, MsgFate, MsgKind, NodeId};
use crate::frame::{Frame, LeaseBar, MsgShape, NodeShape, NodeViz, lerp, ring_layout};
use crate::scenario::{LeaseParams, Scenario};

/// How often each node wakes to make stochastic decisions and service leases.
/// Per-step probabilities in the scenario are interpreted per poll.
const POLL_INTERVAL: Time = 50;

/// Grantor-local ticks an `Active` lease may go without any `RenewReply` (nor
/// the activating `GuardReply`) before the grantor gives up renewing it. The
/// grantor keeps at most one renew in flight (`awaiting_reply`), so once a reply
/// is lost it stops sending further renews and simply waits; this timeout, from
/// the last confirmation (`last_reply`), is when it concludes the grantee is
/// unreachable, stops intending the lease (`intended = false`), and lets `D'`
/// lapse. Once expired the lease is idle again and the per-poll Bernoulli trials
/// re-initiate a fresh guard, just as at the start. Sized around one lease
/// lifetime — long enough to ride out a brief silence, short enough to abandon a
/// grantee that has gone quiet.
const RENEW_REPLY_TIMEOUT: Time = 1500;

/// Global ticks a disruptive write round may stay outstanding before the leader
/// gives up on it (a `Write` or `WriteReply` was lost, so the reply set never
/// reached the commit condition). On abort, everyone unfreezes so their
/// torn-down leases can re-establish (re-guard), and the cluster recovers rather
/// than hanging frozen forever.
const WRITE_ROUND_TIMEOUT: Time = 1500;

/// An item scheduled on the engine's internal timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheduled {
    /// A node wakes up to act.
    Poll { node: NodeId },
    /// A message leaves its sender, after any processing delay. Deferring the
    /// send (rather than sending inline) models reply latency and lets a node
    /// that crashes mid-processing correctly never emit the reply.
    Send {
        from: NodeId,
        to: NodeId,
        kind: MsgKind,
        lease_idx: usize,
    },
    /// A message reaches its destination. `fate` decides delivery vs. drop; the
    /// item is scheduled either way so the event stream stays time-ordered.
    Arrival {
        from: NodeId,
        to: NodeId,
        kind: MsgKind,
        lease_idx: usize,
        fate: MsgFate,
    },
    /// An external driving command takes effect.
    Command(Command),
    /// The leader wakes to (maybe) serve a write request. Rescheduled each time
    /// at `write_interval` ± jitter.
    WriteTick,
}

/// A heap entry ordered by ascending time, then a sequence number for a stable,
/// deterministic tie-break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Timed {
    at: Time,
    seq: u64,
    item: Scheduled,
}

impl Ord for Timed {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: BinaryHeap is a max-heap, we want the earliest time first.
        other
            .at
            .cmp(&self.at)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl PartialOrd for Timed {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Grantor-side bookkeeping for one lease.
#[derive(Debug, Clone, Copy)]
struct GrantorState {
    status: LeaseStatus,
    /// Grantor-local time until which the promise is held (`D'`) — the single
    /// safe-expiry bound, and what the countdown bar shows. Maintained by the
    /// extend-on-send / shorten-on-reply rule: each `Renew` sent *extends* it to
    /// the pessimistic no-reply bound (`Engine::send_renew`, a `max`), and each
    /// `RenewReply` received *shortens* it to the tighter confirmed-receipt bound
    /// (`Engine::on_renew_reply`, a `min`). So while replies flow it stays a bit
    /// over one lease span ahead (the bar reads full); once they stop it drains
    /// to the last pessimistic bound and the lease expires there.
    grant_expiry: Time,
    /// Grantor-local time the next renew is due to be sent.
    next_renew_due: Time,
    /// Whether the grantor currently intends this lease to be active.
    intended: bool,
    /// Set when a *disruptive write* tore this grant down (in `suspend_reads`),
    /// while the grantor is frozen. On the `Commit` (or abort) that thaws the
    /// grantor, it deterministically re-opens the guard — so re-establishment is
    /// commit-driven, not left to a stochastic re-initiation. Cleared once the
    /// re-guard fires (or on any fresh guard). A write-torn-down lease is thus
    /// distinguished from a deliberate `Revoke` (which does not re-guard).
    reguard_on_thaw: bool,
    /// Whether a `Renew` is outstanding with no `RenewReply` yet. The grantor
    /// keeps at most one renew in flight: it won't send the next until the
    /// previous is acked (set on send, cleared on reply / activation), so a lost
    /// reply stops the renew stream rather than firing more un-acked renews. The
    /// lease then lapses via `expire_stale_renews` / `D'` (see those).
    awaiting_reply: bool,
    /// Grantor-local time the current guard attempt was opened, while status is
    /// `Guarding` and no reply has arrived. Used to time out an unanswered guard
    /// (a dropped `Guard`/`GuardReply`) after a full guard window `t_guard -
    /// t_delta` — see [`Engine::expire_stale_guards`] — and to render the guard
    /// countdown bar.
    guard_since: Time,
    /// Grantor-local time of the most recent confirmation the grantee is still
    /// reachable — the activating `GuardReply`, then each `RenewReply`. If an
    /// `Active` lease goes [`RENEW_REPLY_TIMEOUT`] without one (every `Renew` or
    /// its reply lost), the grantor gives up renewing rather than renewing into
    /// the void, letting `D'` lapse so a fresh guard can start. See
    /// [`Engine::expire_stale_renews`].
    last_reply: Time,
}

/// Grantee-side bookkeeping for one lease.
#[derive(Debug, Clone, Copy)]
struct GranteeState {
    status: LeaseStatus,
    /// Grantee-local time until which the lease is held (`C'`).
    hold_expiry: Time,
    /// Latest grantee-local time the first renew may be accepted (`A'`), set
    /// when a `Guard` is received. The guard phase bounds first-renew arrival.
    guard_deadline: Time,
}

/// Per-lease combined state.
#[derive(Debug, Clone, Copy)]
struct LeaseState {
    id: LeaseId,
    grantor: GrantorState,
    grantee: GranteeState,
}

/// Per-node runtime state.
#[derive(Debug, Clone)]
struct NodeState {
    up: bool,
    /// While `true`, the node has torn down the leases it takes part in for an
    /// in-progress *disruptive* write (see [`Engine::suspend_reads`]): it ignores
    /// renews (as grantee) and won't re-guard (as grantor) until a `Commit` (or a
    /// safety timeout) clears the flag. This is what makes the disruptive write's
    /// re-establishment genuinely commit-driven.
    write_frozen: bool,
}

/// The leader's in-progress write round (either path).
#[derive(Debug, Clone)]
struct WriteRound {
    /// Stable id for this write, carried on its messages so overlapping
    /// non-disruptive rounds stay distinct.
    id: u64,
    /// Global time the write broadcast went out (for a stuck-round timeout).
    started: Time,
    /// Peers that have replied so far (the leader counts itself implicitly).
    replied: BTreeSet<NodeId>,
    /// Whether the write has already committed (so a late reply is a no-op).
    committed: bool,
}

/// A message currently traveling a link, kept for frame interpolation.
#[derive(Debug, Clone, Copy)]
struct InFlight {
    from: NodeId,
    to: NodeId,
    kind: MsgKind,
    sent: Time,
    arrival: Time,
    fate: MsgFate,
}

/// The simulation engine. Construct via [`Engine::new`], then drive it forward
/// with [`Engine::advance_to`], optionally injecting [`Command`]s.
#[derive(Debug)]
pub struct Engine {
    scenario: Scenario,
    rng: crate::rng::Rng,
    now: Time,
    seq: u64,
    queue: BinaryHeap<Timed>,
    nodes: Vec<NodeState>,
    leases: Vec<LeaseState>,
    /// Messages currently in flight, for frame interpolation. Pruned lazily.
    in_flight: Vec<InFlight>,
    /// Events produced but not yet returned to the caller.
    pending: Vec<Event>,
    /// The leader's write rounds still awaiting commit. Disruptive writes run one
    /// at a time (at most one entry); non-disruptive writes may overlap, so this
    /// holds several. Keyed implicitly by each round's `id`.
    write_rounds: Vec<WriteRound>,
    /// Monotonic source of write ids.
    next_write_id: u64,
}

impl Engine {
    /// Build an engine from a scenario, scheduling each node's first poll and
    /// any commands scripted on the scenario.
    pub fn new(scenario: Scenario) -> Self {
        let rng = crate::rng::Rng::new(scenario.seed);
        let nodes = (0..scenario.node_count())
            .map(|_| NodeState {
                up: true,
                write_frozen: false,
            })
            .collect();
        let leases = scenario
            .leases
            .iter()
            .map(|&id| LeaseState {
                id,
                grantor: GrantorState {
                    status: LeaseStatus::Inactive,
                    grant_expiry: 0,
                    next_renew_due: 0,
                    intended: false,
                    reguard_on_thaw: false,
                    awaiting_reply: false,
                    guard_since: 0,
                    last_reply: 0,
                },
                grantee: GranteeState {
                    status: LeaseStatus::Inactive,
                    hold_expiry: 0,
                    guard_deadline: 0,
                },
            })
            .collect();

        let mut engine = Self {
            scenario,
            rng,
            now: 0,
            seq: 0,
            queue: BinaryHeap::new(),
            nodes,
            leases,
            in_flight: Vec::new(),
            pending: Vec::new(),
            write_rounds: Vec::new(),
            next_write_id: 0,
        };
        for node in 0..engine.nodes.len() {
            engine.schedule(0, Scheduled::Poll { node });
        }
        for k in 0..engine.scenario.commands.len() {
            let (at, cmd) = engine.scenario.commands[k];
            engine.schedule(at, Scheduled::Command(cmd));
        }
        // If writes are enabled, arm the first leader write tick.
        if let Some(iv) = engine.scenario.write_interval {
            let first = engine.jittered_interval(iv);
            engine.schedule(first, Scheduled::WriteTick);
        }
        engine
    }

    /// Current global time.
    pub fn now(&self) -> Time {
        self.now
    }

    /// The configured run duration in global ticks.
    pub fn duration(&self) -> Time {
        self.scenario.duration
    }

    /// Queue a command to take effect at the current time. It is applied on the
    /// next [`advance_to`], whose returned events include its effects.
    ///
    /// [`advance_to`]: Engine::advance_to
    pub fn command(&mut self, cmd: Command) {
        self.schedule_command(self.now, cmd);
    }

    /// Queue a command to take effect at global time `at` (clamped to not be in
    /// the past). Use for scripted, timed interaction.
    pub fn schedule_command(&mut self, at: Time, cmd: Command) {
        self.schedule(at, Scheduled::Command(cmd));
    }

    /// Advance simulation up to and including global time `t`, returning the
    /// events newly produced since the last call, in ascending time order.
    /// Idempotent past `t`.
    pub fn advance_to(&mut self, t: Time) -> Vec<Event> {
        let target = t.min(self.scenario.duration);
        while let Some(top) = self.queue.peek().copied() {
            if top.at > target {
                break;
            }
            self.queue.pop();
            self.now = top.at;
            self.handle(top.item);
        }
        self.now = target;
        // Drop messages that have already arrived (or been dropped) so frame
        // queries only consider currently-traveling messages.
        let now = self.now;
        self.in_flight.retain(|m| m.arrival > now);
        core::mem::take(&mut self.pending)
    }

    /// Build renderable [`Frame`] geometry for the current time. Call after
    /// [`advance_to`] so in-flight messages and expiries reflect time `t`.
    ///
    /// [`advance_to`]: Engine::advance_to
    pub fn frame_at(&self, t: Time) -> Frame {
        let layout = ring_layout(self.nodes.len());

        let nodes = self
            .nodes
            .iter()
            .enumerate()
            .map(|(id, n)| NodeShape {
                id,
                pos: layout[id],
                viz: if n.up { NodeViz::Up } else { NodeViz::Down },
            })
            .collect();

        let messages = self
            .in_flight
            .iter()
            .filter(|m| m.sent <= t && m.arrival > t)
            .map(|m| {
                let span = (m.arrival - m.sent).max(1) as f64;
                let progress = ((t - m.sent) as f64 / span).clamp(0.0, 1.0);
                MsgShape {
                    from: m.from,
                    to: m.to,
                    kind: m.kind,
                    fate: m.fate,
                    progress,
                    pos: lerp(layout[m.from], layout[m.to], progress),
                }
            })
            .collect();

        let p = self.params();
        let leases = self
            .leases
            .iter()
            .map(|l| {
                // Each party's countdown bar reads its real expiry — the grantee's
                // `hold_expiry` (`C'`) and the grantor's `grant_expiry` (`D'`), the
                // very bound that flips it to `Expired` — so a bar reaching empty
                // coincides with that side expiring. Each is normalized by the span
                // that bound is set a whole *away* from, so the fill hits 1.0 only
                // right when it's (re)armed and then drains continuously toward 0:
                // the grantee's `C'` is `t_lease - t_delta` from receipt; the
                // grantor's `D'` is the full provisioned grant span `t_guard +
                // t_lease + 2·t_delta` from its send (see `send_renew`). Using the
                // over-provisioned `D'` with only a `t_lease` span would peg the bar
                // full until `local` came within a span of `D'`, so the bar must
                // normalize by the same span `D'` is provisioned over.
                //
                // Guarding drains over the guard phase's length `t_guard - t_delta`
                // on *both* sides, so the two guard bars read identically: the
                // grantee's is its acceptance window `A'` (`guard_deadline`), the
                // grantor's runs from `guard_since` over the same span — also exactly
                // when it gives up an unanswered guard (`expire_stale_guards`), so
                // that bar empties as the grantor falls idle. Any other status has
                // no live countdown.
                let guard_span = p.guard_window();
                let grant_span = p.grant_span();
                let grantor_fill = match l.grantor.status {
                    LeaseStatus::Active => {
                        self.fill(l.id.grantor, l.grantor.grant_expiry, grant_span, t)
                    }
                    LeaseStatus::Guarding => self.fill(
                        l.id.grantor,
                        l.grantor.guard_since + guard_span,
                        guard_span,
                        t,
                    ),
                    _ => 0.0,
                };
                let grantee_fill = match l.grantee.status {
                    LeaseStatus::Active => {
                        self.fill(l.id.grantee, l.grantee.hold_expiry, p.t_lease, t)
                    }
                    LeaseStatus::Guarding => {
                        self.fill(l.id.grantee, l.grantee.guard_deadline, guard_span, t)
                    }
                    _ => 0.0,
                };
                LeaseBar {
                    grantor: l.id.grantor,
                    grantee: l.id.grantee,
                    grantor_status: l.grantor.status,
                    grantee_status: l.grantee.status,
                    grantor_fill,
                    grantee_fill,
                }
            })
            .collect();

        Frame {
            at: t,
            nodes,
            messages,
            leases,
        }
    }

    /// Fraction of a countdown remaining for one party at global time `t`, as a
    /// value in `0.0..1.0`. `expiry_local` is the party-local tick the countdown
    /// reaches empty at (its real expiry — no separate display timer), and `span`
    /// the full length it drains over: the caller passes the span that expiry is
    /// (re)armed a whole away from, so the bar fills to 1 at arm and drains cleanly
    /// (grantee active `t_lease`; grantor active the full grant span; either guard
    /// bar the guard window — see the call sites in `frame_at`).
    fn fill(&self, node: NodeId, expiry_local: Time, span: Time, t: Time) -> f64 {
        if span <= 0 {
            return 0.0;
        }
        let local = self.scenario.nodes[node].clock.local(t);
        let remaining = (expiry_local - local) as f64;
        (remaining / span as f64).clamp(0.0, 1.0)
    }

    fn params(&self) -> LeaseParams {
        self.scenario.params
    }

    fn schedule(&mut self, at: Time, item: Scheduled) {
        self.seq += 1;
        self.queue.push(Timed {
            at: at.max(self.now),
            seq: self.seq,
            item,
        });
    }

    fn emit(&mut self, at: Time, kind: EventKind) {
        self.pending.push(Event { at, kind });
    }

    fn handle(&mut self, item: Scheduled) {
        match item {
            Scheduled::Poll { node } => self.poll(node),
            Scheduled::Send {
                from,
                to,
                kind,
                lease_idx,
            } => self.try_send(from, to, kind, lease_idx),
            Scheduled::Arrival {
                from,
                to,
                kind,
                lease_idx,
                fate,
            } => self.arrive(from, to, kind, lease_idx, fate),
            Scheduled::Command(cmd) => self.apply(cmd),
            Scheduled::WriteTick => self.write_tick(),
        }
    }

    /// Send a message `from -> to` now, sampling delay and drop from the link.
    /// The `MessageSent` event fires immediately; the (delivered-or-dropped)
    /// arrival is scheduled so its event lands in time order.
    fn send(&mut self, from: NodeId, to: NodeId, kind: MsgKind, lease_idx: usize) {
        let link = self.scenario.link_config(from, to);
        let sent = self.now;
        // A message is lost to a partition, or to either the link's own drop
        // chance or this kind's — combined into one independent probability
        // `1 - (1-a)(1-b)` so a single RNG draw keeps determinism footprint low.
        // A kind's drop only applies from its `kind_drop_from` tick onward, so a
        // scenario can establish cleanly and then begin losing a kind.
        let kind_drop = if sent >= self.scenario.kind_drop_from[kind.index()] {
            self.scenario.kind_drop[kind.index()]
        } else {
            0.0
        };
        let drop_chance = 1.0 - (1.0 - link.drop_chance) * (1.0 - kind_drop);
        let dropped = link.partitioned || self.rng.chance(drop_chance);
        let delay = link.delay.sample(&mut self.rng).max(1);
        let arrival = sent + delay;
        let fate = if dropped {
            MsgFate::Dropped
        } else {
            MsgFate::Delivered
        };
        self.emit(
            sent,
            EventKind::MessageSent {
                from,
                to,
                kind,
                sent,
                arrival,
                fate,
            },
        );
        self.in_flight.push(InFlight {
            from,
            to,
            kind,
            sent,
            arrival,
            fate,
        });
        self.schedule(
            arrival,
            Scheduled::Arrival {
                from,
                to,
                kind,
                lease_idx,
                fate,
            },
        );
    }

    /// A previously deferred send fires. Skipped if the sender is down, modeling
    /// a node that crashed after receiving but before replying.
    fn try_send(&mut self, from: NodeId, to: NodeId, kind: MsgKind, lease_idx: usize) {
        if self.nodes[from].up {
            self.send(from, to, kind, lease_idx);
        }
    }

    /// Schedule `from` to send `kind` after `from`'s own processing delay. Used
    /// for replies, whose latency is the responder's think time.
    fn reply_after_delay(&mut self, from: NodeId, to: NodeId, kind: MsgKind, lease_idx: usize) {
        let delay = self.response_delay(from);
        self.schedule(
            self.now + delay,
            Scheduled::Send {
                from,
                to,
                kind,
                lease_idx,
            },
        );
    }

    /// Local clock reading for a node at the current global time.
    fn local(&self, node: NodeId) -> Time {
        self.scenario.nodes[node].clock.local(self.now)
    }

    /// Response delay for a node, sampled from its config.
    fn response_delay(&mut self, node: NodeId) -> Time {
        let dist: Dist = self.scenario.nodes[node].response_delay;
        dist.sample(&mut self.rng)
    }

    // ---- Driving commands -------------------------------------------------

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Initiate(id) => {
                if let Some(i) = self.lease_index(id)
                    && self.nodes[id.grantor].up
                    && self.grantor_idle(i)
                {
                    self.begin_guard(i);
                }
            }
            Command::Revoke(id) => {
                if let Some(i) = self.lease_index(id)
                    && self.nodes[id.grantor].up
                    && self.grantor_active_or_guarding(i)
                {
                    self.begin_revoke(i);
                }
            }
            Command::FailNode(node) => {
                if self.nodes[node].up {
                    self.fail_node(node);
                }
            }
            Command::RecoverNode(node) => {
                if !self.nodes[node].up {
                    self.recover_node(node);
                }
            }
            Command::Write => self.serve_write(),
        }
    }

    fn lease_index(&self, id: LeaseId) -> Option<usize> {
        self.leases.iter().position(|l| l.id == id)
    }

    fn grantor_idle(&self, i: usize) -> bool {
        matches!(
            self.leases[i].grantor.status,
            LeaseStatus::Inactive | LeaseStatus::Expired
        )
    }

    fn grantor_active_or_guarding(&self, i: usize) -> bool {
        matches!(
            self.leases[i].grantor.status,
            LeaseStatus::Active | LeaseStatus::Guarding
        )
    }

    // ---- Failure & recovery ----------------------------------------------

    fn fail_node(&mut self, node: NodeId) {
        self.nodes[node].up = false;
        self.emit(self.now, EventKind::NodeFailed { node });
        // A failed grantor stops renewing; its leases will lapse safely.
        for l in &mut self.leases {
            if l.id.grantor == node {
                l.grantor.intended = false;
            }
        }
    }

    fn recover_node(&mut self, node: NodeId) {
        self.nodes[node].up = true;
        self.emit(self.now, EventKind::NodeRecovered { node });
    }

    // ---- Periodic per-node poll ------------------------------------------

    fn poll(&mut self, node: NodeId) {
        if self.nodes[node].up {
            if self.rng.chance(self.scenario.nodes[node].fail_chance) {
                self.fail_node(node);
            }
        } else if self.rng.chance(self.scenario.nodes[node].recover_chance) {
            self.recover_node(node);
        }

        if self.nodes[node].up {
            self.abort_stale_writes(node);
            self.expire_stale_guards(node);
            self.expire_stale_renews(node);
            self.maybe_initiate(node);
            self.service_grants(node);
        }
        self.recompute_statuses(node);

        // Reschedule the next poll while the simulation is still running.
        let next = self.now + POLL_INTERVAL;
        if next <= self.scenario.duration {
            self.schedule(next, Scheduled::Poll { node });
        }
    }

    /// Abandon any guard attempt this node opened that has gone unanswered for a
    /// whole guard phase — the sign that its `Guard` or the `GuardReply` was
    /// dropped. The give-up span is the guard window `t_guard - t_delta`, the same
    /// length as the grantee's acceptance window `A'`: the grantor holds the guard
    /// for as long as the grantee would still accept the activating first renew,
    /// and only once that has lapsed does it fall back to `Inactive` (its pre-guard
    /// idle state). Re-initiation ("retry") is then the ordinary per-poll
    /// `initiate_chance` path, which starts fresh *from* that idle state — so a
    /// retry is a full guard phase away, not a fraction of one.
    ///
    /// Only stuck guard phases are timed out; once a lease is `Active` a lost
    /// renew is already handled by the ordinary renew loop, so those are left be.
    fn expire_stale_guards(&mut self, node: NodeId) {
        let local = self.local(node);
        let guard_window = self.params().guard_window();
        for i in 0..self.leases.len() {
            if self.leases[i].id.grantor == node
                && self.leases[i].grantor.status == LeaseStatus::Guarding
                && local - self.leases[i].grantor.guard_since >= guard_window
            {
                self.leases[i].grantor.status = LeaseStatus::Inactive;
                self.leases[i].grantor.intended = false;
            }
        }
    }

    /// Give up renewing an `Active` lease whose grantee has gone silent for
    /// [`RENEW_REPLY_TIMEOUT`] grantor-local ticks — no `RenewReply` since the
    /// last confirmation, the sign the `Renew` (or its reply) was dropped. With
    /// one renew in flight the grantor has already stopped sending renews (it's
    /// `awaiting_reply`); this is when it concludes the grantee is unreachable and
    /// stops *intending* the lease, so `D'` can lapse and the grantor fall idle to
    /// re-guard (without it, an `awaiting_reply` lease would keep its last `D'` and
    /// intent forever, never re-guarding).
    ///
    /// This only stops the renewing (`intended = false`, like a passive revoke);
    /// the grantor keeps its outstanding `D'` and lets it lapse naturally, so the
    /// safety invariant holds even if a renew *was* received and only its reply
    /// lost. Once `D'` lapses the lease expires (via `recompute_statuses`) and is
    /// idle again, so `maybe_initiate` re-opens a fresh guard — the exact restart
    /// the guard-phase timeout gives, one step later in the lifecycle.
    fn expire_stale_renews(&mut self, node: NodeId) {
        let local = self.local(node);
        for i in 0..self.leases.len() {
            if self.leases[i].id.grantor == node
                && self.leases[i].grantor.status == LeaseStatus::Active
                && self.leases[i].grantor.intended
                && local - self.leases[i].grantor.last_reply >= RENEW_REPLY_TIMEOUT
            {
                self.leases[i].grantor.intended = false;
            }
        }
    }

    /// A node may spontaneously start a lease it grants but has not activated
    /// (the stochastic `initiate_chance` path, e.g. the playground's staggered
    /// establishment). A node frozen for an in-progress disruptive write does not
    /// initiate — its torn-down grants are re-guarded deterministically on the
    /// `Commit` that thaws it (see `thaw_and_reguard`), not by this chance path.
    fn maybe_initiate(&mut self, node: NodeId) {
        if self.nodes[node].write_frozen {
            return;
        }
        let p = self.scenario.nodes[node].initiate_chance;
        for i in 0..self.leases.len() {
            if self.leases[i].id.grantor == node && self.grantor_idle(i) && self.rng.chance(p) {
                self.begin_guard(i);
            }
        }
    }

    /// Send the `Guard` that opens a lease's guard phase.
    fn begin_guard(&mut self, i: usize) {
        let LeaseId { grantor, grantee } = self.leases[i].id;
        self.leases[i].grantor.status = LeaseStatus::Guarding;
        self.leases[i].grantor.intended = true;
        // Fresh guard phase: no renew is outstanding yet, and any pending
        // re-guard-on-thaw request is now satisfied.
        self.leases[i].grantor.awaiting_reply = false;
        self.leases[i].grantor.reguard_on_thaw = false;
        self.leases[i].grantor.guard_since = self.local(grantor);
        self.send(grantor, grantee, MsgKind::Guard, i);
    }

    /// Proactively revoke a lease: stop renewing and notify the grantee. If the
    /// grantee's `RevokeReply` comes back the grantor exits granting at once (see
    /// `on_revoke_reply`); otherwise it keeps its outstanding `D'` and lets it
    /// lapse naturally — so the safety invariant holds regardless of whether the
    /// grantee is reached.
    fn begin_revoke(&mut self, i: usize) {
        let LeaseId { grantor, grantee } = self.leases[i].id;
        self.leases[i].grantor.intended = false;
        self.send(grantor, grantee, MsgKind::Revoke, i);
    }

    /// Send due renews for leases this node grants and currently intends. The
    /// grantor keeps at most one renew in flight: a lease still `awaiting_reply`
    /// to its last renew is skipped, so the next renew waits for that ack (or for
    /// `expire_stale_renews` to give up) rather than firing un-acked renews.
    fn service_grants(&mut self, node: NodeId) {
        for i in 0..self.leases.len() {
            if self.leases[i].id.grantor != node || !self.leases[i].grantor.intended {
                continue;
            }
            if self.leases[i].grantor.status != LeaseStatus::Active
                || self.leases[i].grantor.awaiting_reply
            {
                continue;
            }
            let local = self.local(node);
            if local >= self.leases[i].grantor.next_renew_due {
                self.send_renew(i);
            }
        }
    }

    /// Send a `Renew` and (re)arm the grantor's no-reply safe expiry.
    ///
    /// The provisional expiry uses the guard construction `B' = b + t_guard +
    /// t_delta`, then `D' = B' + t_lease + t_delta`. This covers the case where
    /// the renew is received but its reply is lost: the grantee can extend to at
    /// most `(receipt) + t_lease - t_delta`, and the guard window bounds receipt
    /// relative to the grantor's send `b`, so this `D'` still dominates.
    fn send_renew(&mut self, i: usize) {
        let LeaseId { grantor, grantee } = self.leases[i].id;
        let p = self.params();
        let b = self.local(grantor);
        let provisional = b + p.grant_span();
        self.leases[i].grantor.grant_expiry = self.leases[i].grantor.grant_expiry.max(provisional);
        self.leases[i].grantor.next_renew_due = b + p.renew_interval;
        // One renew in flight: block further renews until this one is acked.
        self.leases[i].grantor.awaiting_reply = true;
        self.send(grantor, grantee, MsgKind::Renew, i);
    }

    // ---- Message arrival --------------------------------------------------

    fn arrive(&mut self, from: NodeId, to: NodeId, kind: MsgKind, lease_idx: usize, fate: MsgFate) {
        if fate == MsgFate::Dropped {
            self.emit(self.now, EventKind::MessageDropped { from, to, kind });
            return;
        }
        self.emit(self.now, EventKind::MessageDelivered { from, to, kind });
        // A down node silently ignores everything it receives.
        if !self.nodes[to].up {
            return;
        }
        match kind {
            MsgKind::Guard => self.on_guard(lease_idx),
            MsgKind::GuardReply => self.on_guard_reply(lease_idx),
            MsgKind::Renew => self.on_renew(lease_idx),
            MsgKind::RenewReply => self.on_renew_reply(lease_idx),
            MsgKind::Revoke => self.on_revoke(lease_idx),
            MsgKind::RevokeReply => self.on_revoke_reply(lease_idx),
            // Write-path messages are cluster-wide, not lease-scoped: the
            // `lease_idx` slot carries the write id instead.
            MsgKind::Write => self.on_write(from, to, lease_idx as u64),
            MsgKind::WriteReply => self.on_write_reply(from, lease_idx as u64),
            MsgKind::Commit => self.on_commit(to, lease_idx as u64),
        }
    }

    /// Grantee receives `Guard`: record the acceptance window `A'` and reply.
    fn on_guard(&mut self, i: usize) {
        let LeaseId { grantor, grantee } = self.leases[i].id;
        let p = self.params();
        let a = self.local(grantee);
        self.leases[i].grantee.guard_deadline = a + p.guard_window();
        self.leases[i].grantee.status = LeaseStatus::Guarding;
        self.emit(
            self.now,
            EventKind::GranteeLease {
                lease: self.leases[i].id,
                status: LeaseStatus::Guarding,
            },
        );
        self.reply_after_delay(grantee, grantor, MsgKind::GuardReply, i);
    }

    /// Grantor receives `GuardReply`: become active and send the first renew.
    fn on_guard_reply(&mut self, i: usize) {
        if self.leases[i].grantor.status != LeaseStatus::Guarding {
            return;
        }
        self.leases[i].grantor.status = LeaseStatus::Active;
        // Activation is the first proof the grantee is reachable; arm the
        // renew-reply liveness clock from here. `grant_expiry` (`D'`) is then set
        // by the first `send_renew` below.
        let d = self.local(self.leases[i].id.grantor);
        self.leases[i].grantor.last_reply = d;
        self.leases[i].grantor.next_renew_due = d;
        self.emit(
            self.now,
            EventKind::GrantorLease {
                lease: self.leases[i].id,
                status: LeaseStatus::Active,
            },
        );
        self.send_renew(i);
    }

    /// Grantee receives `Renew`: accept iff within the guard window (first time)
    /// and extend the hold expiry `C' = C + t_lease - t_delta`, then reply.
    fn on_renew(&mut self, i: usize) {
        let LeaseId { grantor, grantee } = self.leases[i].id;
        let p = self.params();
        let c = self.local(grantee);

        // A grantee that suspended its reads for an in-progress disruptive write
        // ignores renews until the `Commit` unfreezes it — this is what holds the
        // read lease down for the duration of the write.
        if self.nodes[grantee].write_frozen {
            return;
        }

        // The very first renew is only welcome inside the guarded window.
        if self.leases[i].grantee.status == LeaseStatus::Guarding
            && c >= self.leases[i].grantee.guard_deadline
        {
            return; // too late; guard failed, ignore
        }
        let was = self.leases[i].grantee.status;
        self.leases[i].grantee.hold_expiry = c + p.t_lease - p.t_delta;
        self.leases[i].grantee.status = LeaseStatus::Active;
        if was != LeaseStatus::Active {
            self.emit(
                self.now,
                EventKind::GranteeLease {
                    lease: self.leases[i].id,
                    status: LeaseStatus::Active,
                },
            );
        }
        self.reply_after_delay(grantee, grantor, MsgKind::RenewReply, i);
    }

    /// Grantor receives `RenewReply`: tighten `D' = d + t_lease + t_delta`.
    ///
    /// Safe because the reply proves the grantee's receipt `C` happened before
    /// this grantor-local `d` (it takes a real round-trip: grantee received the
    /// renew, then replied). So `D' = d + t_lease + t_delta` exceeds the
    /// grantee's `C' = C + t_lease - t_delta` — the later anchor `d > C` plus the
    /// `+t_delta`/`-t_delta` slack both push the same way, keeping `D' > C'`.
    fn on_renew_reply(&mut self, i: usize) {
        if self.leases[i].grantor.status != LeaseStatus::Active {
            return;
        }
        let p = self.params();
        let d = self.local(self.leases[i].id.grantor);
        let tightened = d + p.t_lease + p.t_delta;
        // Shorten `D'` toward the tighter, confirmed-receipt bound (never below the
        // grantee's possible expiry, which `tightened` already dominates). This is
        // what refills the countdown bar — the bar reads `grant_expiry` directly,
        // so tightening on a fresh reply lifts it back up.
        self.leases[i].grantor.grant_expiry = self.leases[i].grantor.grant_expiry.min(tightened);
        // A reply proves the grantee is still reachable; refresh the liveness clock
        // so `expire_stale_renews` only fires on a genuinely silent link.
        self.leases[i].grantor.last_reply = d;
        // This renew is now acked, so the next one may be sent when due.
        self.leases[i].grantor.awaiting_reply = false;
    }

    /// Grantee receives `Revoke`: drop the lease immediately and acknowledge.
    fn on_revoke(&mut self, i: usize) {
        if self.leases[i].grantee.status == LeaseStatus::Active
            || self.leases[i].grantee.status == LeaseStatus::Guarding
        {
            self.leases[i].grantee.status = LeaseStatus::Expired;
            self.leases[i].grantee.hold_expiry = self.local(self.leases[i].id.grantee);
            self.emit(
                self.now,
                EventKind::GranteeLease {
                    lease: self.leases[i].id,
                    status: LeaseStatus::Expired,
                },
            );
        }
        // Acknowledge the revoke back to the grantor. The grantor already stopped
        // renewing when it revoked; the ack lets it leave the granting state
        // *promptly* (see `on_revoke_reply`) instead of waiting out `D'`. Never
        // required for safety — if the ack is lost the grantor just lets `D'`
        // lapse — so it is safe whether or not the grantee is reached.
        let LeaseId { grantor, grantee } = self.leases[i].id;
        self.reply_after_delay(grantee, grantor, MsgKind::RevokeReply, i);
    }

    /// Grantor receives `RevokeReply`: the grantee confirmed it dropped the lease,
    /// so the grantor can leave the granting state at once rather than waiting for
    /// its padded `D'` to lapse. Safe because the ack is a round-trip *after* the
    /// grantee expired its hold, so the grantee's expiry already precedes now — the
    /// invariant `grantor expiry >= grantee expiry` still holds. (Without the ack,
    /// the grantor would simply let `D'` lapse; this just ends it promptly once the
    /// revoke is confirmed.)
    fn on_revoke_reply(&mut self, i: usize) {
        if !self.grantor_active_or_guarding(i) {
            return;
        }
        self.leases[i].grantor.status = LeaseStatus::Expired;
        self.leases[i].grantor.intended = false;
        self.emit(
            self.now,
            EventKind::GrantorLease {
                lease: self.leases[i].id,
                status: LeaseStatus::Expired,
            },
        );
    }

    // ---- Expiry detection -------------------------------------------------

    /// Recompute lease statuses for both roles `node` plays, emitting expiry
    /// transitions when a local clock has passed the relevant deadline.
    fn recompute_statuses(&mut self, node: NodeId) {
        let local = self.local(node);
        for i in 0..self.leases.len() {
            let id = self.leases[i].id;
            if id.grantor == node
                && self.leases[i].grantor.status == LeaseStatus::Active
                && local > self.leases[i].grantor.grant_expiry
            {
                self.leases[i].grantor.status = LeaseStatus::Expired;
                self.emit(
                    self.now,
                    EventKind::GrantorLease {
                        lease: id,
                        status: LeaseStatus::Expired,
                    },
                );
            }
            if id.grantee == node
                && self.leases[i].grantee.status == LeaseStatus::Active
                && local > self.leases[i].grantee.hold_expiry
            {
                self.leases[i].grantee.status = LeaseStatus::Expired;
                self.emit(
                    self.now,
                    EventKind::GranteeLease {
                        lease: id,
                        status: LeaseStatus::Expired,
                    },
                );
            }
            // A grantee whose guard window (`A'`) lapses without an activating
            // first `Renew` — because that `Renew` was dropped or delayed — gives
            // up the guard instead of hanging in `Guarding` forever. This mirrors
            // the grantor-side `expire_stale_guards` timeout on the other end.
            if id.grantee == node
                && self.leases[i].grantee.status == LeaseStatus::Guarding
                && local > self.leases[i].grantee.guard_deadline
            {
                self.leases[i].grantee.status = LeaseStatus::Expired;
                self.emit(
                    self.now,
                    EventKind::GranteeLease {
                        lease: id,
                        status: LeaseStatus::Expired,
                    },
                );
            }
        }
    }

    // ---- Write path ------------------------------------------------------
    //
    // Two flavors, both broadcast by the leader (smallest-id grantee):
    //  * disruptive — a recipient tears down every lease it takes part in (the
    //    reads it holds *and* the grants it makes) and freezes until the `Commit`;
    //    once unfrozen the lease re-establishes from scratch, re-guarding. The
    //    `Write` itself is the revocation notice — no separate `Revoke` traffic.
    //  * non-disruptive — recipients keep their leases running entirely
    //    untouched; the write's `Write`/`WriteReply`/`Commit` messages sweep the
    //    cluster and commit, but touch no lease or node state at all.
    // Write ids ride the messages' `lease_idx` slot (writes aren't lease-scoped),
    // so overlapping non-disruptive rounds stay distinct.

    /// `interval` perturbed by ±20% jitter (deterministic via the PRNG), floored
    /// at 1 tick. Used to space out leader write ticks so they don't land on a
    /// perfectly regular grid.
    fn jittered_interval(&mut self, interval: Time) -> Time {
        let span = (interval / 5).max(1); // ±20%
        (interval + self.rng.next_range(-span, span)).max(1)
    }

    /// The current leader: the smallest-id node that is a grantee of some lease
    /// (matches the playground's crowned leader). `None` if no lease is declared.
    fn leader(&self) -> Option<NodeId> {
        self.leases.iter().map(|l| l.id.grantee).min()
    }

    /// The set of grantee (local-reader) nodes — every distinct lease grantee.
    fn grantee_nodes(&self) -> BTreeSet<NodeId> {
        self.leases.iter().map(|l| l.id.grantee).collect()
    }

    /// The leader wakes to serve a write, then reschedules the next tick. In the
    /// disruptive path a fresh round is skipped while one is already outstanding
    /// (one write at a time); non-disruptive writes may overlap freely.
    fn write_tick(&mut self) {
        // Reschedule the next tick first, so the cadence continues regardless.
        if let Some(iv) = self.scenario.write_interval {
            let next = self.now + self.jittered_interval(iv);
            if next <= self.scenario.duration {
                self.schedule(next, Scheduled::WriteTick);
            }
        }
        self.serve_write();
    }

    /// Have the leader open one write round now (shared by the periodic `WriteTick`
    /// and the scripted `Command::Write`): only if there is a leader that is up,
    /// and — for disruptive writes — only one round at a time.
    fn serve_write(&mut self) {
        let Some(leader) = self.leader() else {
            return;
        };
        if !self.nodes[leader].up {
            return;
        }
        if self.scenario.write_disruptive {
            // One disruptive round at a time.
            if self.write_rounds.is_empty() {
                self.begin_write(leader);
            }
        } else {
            self.begin_write(leader);
        }
    }

    /// Leader opens a write round: allocate an id, record it, emit `WriteStarted`,
    /// and broadcast `Write`. In the disruptive path the leader also suspends the
    /// read leases it holds as a grantee and freezes itself; in the non-disruptive
    /// path its leases are left entirely untouched.
    fn begin_write(&mut self, leader: NodeId) {
        let id = self.next_write_id;
        self.next_write_id += 1;
        self.write_rounds.push(WriteRound {
            id,
            started: self.now,
            replied: BTreeSet::new(),
            committed: false,
        });
        if self.scenario.write_disruptive {
            self.suspend_reads(leader);
        }
        self.emit(self.now, EventKind::WriteStarted { leader });
        for to in 0..self.nodes.len() {
            if to != leader {
                self.send(leader, to, MsgKind::Write, id as usize);
            }
        }
    }

    /// A peer receives the leader's `Write`. Disruptive: drop the read leases it
    /// holds as a grantee and freeze until the commit. Non-disruptive: its leases
    /// are untouched — it just replies. Either way the leader is the reply's `to`.
    fn on_write(&mut self, from: NodeId, to: NodeId, write_id: u64) {
        if self.scenario.write_disruptive {
            self.suspend_reads(to);
        }
        self.reply_after_delay(to, from, MsgKind::WriteReply, write_id as usize);
    }

    /// Leader receives a `WriteReply`: record the replier for that round and try
    /// to commit it.
    fn on_write_reply(&mut self, from: NodeId, write_id: u64) {
        let Some(round) = self.write_rounds.iter_mut().find(|r| r.id == write_id) else {
            return;
        };
        if round.committed {
            return;
        }
        round.replied.insert(from);
        self.try_commit(write_id);
    }

    /// Commit round `write_id` once its reply set (leader implicitly included)
    /// both reaches a majority and covers every grantee node. On commit the
    /// leader emits `WriteCommitted`, broadcasts `Commit`, and — disruptive only —
    /// unfreezes itself so its torn-down leases can re-establish (re-guard).
    fn try_commit(&mut self, write_id: u64) {
        let Some(leader) = self.leader() else {
            return;
        };
        let n = self.nodes.len();
        let maj = n / 2 + 1;
        let grantees = self.grantee_nodes();
        let Some(round) = self.write_rounds.iter().find(|r| r.id == write_id) else {
            return;
        };
        if round.committed {
            return;
        }
        // The leader counts itself among both the majority and the grantee cover.
        let count = round.replied.len() + 1;
        let covered = grantees
            .iter()
            .all(|&g| g == leader || round.replied.contains(&g));
        if count < maj || !covered {
            return;
        }
        self.emit(self.now, EventKind::WriteCommitted { leader });
        for to in 0..self.nodes.len() {
            if to != leader {
                self.send(leader, to, MsgKind::Commit, write_id as usize);
            }
        }
        if self.scenario.write_disruptive {
            // The leader commits locally, so it thaws and re-guards its own
            // torn-down grants here (the same recovery peers do on `Commit`).
            self.thaw_and_reguard(leader);
        }
        self.write_rounds.retain(|r| r.id != write_id);
    }

    /// A node receives a `Commit`. Disruptive: thaw the node and immediately
    /// re-establish the grants the write tore down (see `thaw_and_reguard`).
    /// Non-disruptive: a no-op — leases were never touched.
    fn on_commit(&mut self, node: NodeId, _write_id: u64) {
        if self.scenario.write_disruptive {
            self.thaw_and_reguard(node);
        }
    }

    /// Thaw `node` after a disruptive write and deterministically re-open a fresh
    /// guard for every grant the write tore down (`reguard_on_thaw`). This is what
    /// makes recovery *commit-driven*: the moment a grantor learns the write
    /// committed, it re-grants — no waiting on a stochastic per-poll re-initiation.
    fn thaw_and_reguard(&mut self, node: NodeId) {
        self.nodes[node].write_frozen = false;
        for i in 0..self.leases.len() {
            if self.leases[i].id.grantor == node && self.leases[i].grantor.reguard_on_thaw {
                self.begin_guard(i); // clears `reguard_on_thaw`
            }
        }
    }

    /// Called on the leader's poll: abandon any write round outstanding longer
    /// than [`WRITE_ROUND_TIMEOUT`] (a `Write`/`WriteReply` was dropped so it can
    /// never reach the commit condition), so the cluster recovers rather than
    /// hanging on a stuck round forever.
    fn abort_stale_writes(&mut self, node: NodeId) {
        if self.leader() != Some(node) {
            return;
        }
        let now = self.now;
        let had_stale = self
            .write_rounds
            .iter()
            .any(|r| now - r.started >= WRITE_ROUND_TIMEOUT);
        if !had_stale {
            return;
        }
        self.write_rounds
            .retain(|r| now - r.started < WRITE_ROUND_TIMEOUT);
        // Disruptive writes freeze nodes; thaw every one and re-guard its
        // torn-down grants, exactly as a commit would — so an aborted round still
        // recovers deterministically. Non-disruptive writes touch no node state,
        // so dropping the stale round is all the cleanup needed.
        if self.scenario.write_disruptive {
            for id in 0..self.nodes.len() {
                self.thaw_and_reguard(id);
            }
        }
    }

    /// Suspend, for a disruptive write, every lease `node` takes part in — the
    /// write *is* the revocation, so a torn-down lease must be re-established from
    /// scratch, **paying the guard round-trips again** (per `algorithm.md`, "Write
    /// disrupts local reads → Lease teardown"). Called on each node as it learns of
    /// the write (the leader in `begin_write`, every peer in `on_write`), so both
    /// endpoints of each lease get torn down as their nodes are notified.
    ///
    /// - As **grantee** (a read it holds): drop each active/guarding hold to
    ///   `Expired`; `write_frozen` (set here) then makes it ignore renews until the
    ///   `Commit` (or a timeout) thaws it.
    /// - As **grantor** (a grant it makes): reset each active/guarding grant to
    ///   `Inactive`, stop renewing, and mark it `reguard_on_thaw` so the `Commit`
    ///   re-opens a fresh **guard** deterministically (see `thaw_and_reguard`) —
    ///   rather than a bare renew silently re-activating a still-`Active` grant with
    ///   no guard phase. It keeps its outstanding `D'` and simply stops extending it
    ///   (safe to drop idle).
    fn suspend_reads(&mut self, node: NodeId) {
        self.nodes[node].write_frozen = true;
        let local = self.local(node);
        for i in 0..self.leases.len() {
            let id = self.leases[i].id;
            if id.grantee == node
                && matches!(
                    self.leases[i].grantee.status,
                    LeaseStatus::Active | LeaseStatus::Guarding
                )
            {
                self.leases[i].grantee.status = LeaseStatus::Expired;
                self.leases[i].grantee.hold_expiry = local;
                self.emit(
                    self.now,
                    EventKind::GranteeLease {
                        lease: id,
                        status: LeaseStatus::Expired,
                    },
                );
            }
            if id.grantor == node
                && matches!(
                    self.leases[i].grantor.status,
                    LeaseStatus::Active | LeaseStatus::Guarding
                )
            {
                self.leases[i].grantor.status = LeaseStatus::Inactive;
                self.leases[i].grantor.intended = false;
                // Remember to re-guard this grant once the commit thaws us — the
                // write tore it down, so recovery is deterministic and commit-
                // driven (not a stochastic re-initiation).
                self.leases[i].grantor.reguard_on_thaw = true;
                self.emit(
                    self.now,
                    EventKind::GrantorLease {
                        lease: id,
                        status: LeaseStatus::Inactive,
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;
    use crate::scenario::{LinkConfig, Scenario};

    /// A two-node scenario where node 0 always initiates and grants to node 1,
    /// reliable links, perfect clocks.
    fn basic() -> Scenario {
        Scenario::new(2)
            .seed(1)
            .duration(10_000)
            .node(0, |n| n.initiate_chance = 1.0)
            .link(LinkConfig::new(0, 1))
            .link(LinkConfig::new(1, 0))
            .lease(0, 1)
    }

    /// Same two nodes, but driven only by explicit commands (no stochastics).
    fn scripted() -> Scenario {
        Scenario::new(2)
            .seed(1)
            .duration(10_000)
            .link(LinkConfig::new(0, 1))
            .link(LinkConfig::new(1, 0))
            .lease(0, 1)
    }

    fn grantee_ever_active(events: &[Event]) -> bool {
        events.iter().any(|ev| {
            matches!(
                ev.kind,
                EventKind::GranteeLease {
                    status: LeaseStatus::Active,
                    ..
                }
            )
        })
    }

    #[test]
    fn lease_becomes_active_on_both_sides() {
        let mut e = Engine::new(basic());
        let events = e.advance_to(10_000);
        let grantor_active = events.iter().any(|ev| {
            matches!(
                ev.kind,
                EventKind::GrantorLease {
                    status: LeaseStatus::Active,
                    ..
                }
            )
        });
        assert!(grantor_active, "grantor should activate the lease");
        assert!(
            grantee_ever_active(&events),
            "grantee should hold the lease"
        );
    }

    #[test]
    fn guard_and_renew_messages_flow() {
        let mut e = Engine::new(basic());
        let events = e.advance_to(10_000);
        let kinds: Vec<MsgKind> = events
            .iter()
            .filter_map(|ev| match ev.kind {
                EventKind::MessageSent { kind, .. } => Some(kind),
                _ => None,
            })
            .collect();
        assert!(kinds.contains(&MsgKind::Guard));
        assert!(kinds.contains(&MsgKind::GuardReply));
        assert!(kinds.contains(&MsgKind::Renew));
        assert!(kinds.contains(&MsgKind::RenewReply));
    }

    #[test]
    fn events_are_time_ordered() {
        let mut e = Engine::new(basic());
        let events = e.advance_to(10_000);
        assert!(
            events.windows(2).all(|w| w[0].at <= w[1].at),
            "event stream must be sorted by time"
        );
    }

    #[test]
    fn safety_invariant_grantor_outlasts_grantee() {
        // With perfect clocks and reliable links, at every moment the grantor's
        // expiry must be >= the grantee's expiry once both are active.
        let mut e = Engine::new(basic());
        e.advance_to(10_000);
        for l in &e.leases {
            if l.grantor.status == LeaseStatus::Active && l.grantee.status == LeaseStatus::Active {
                assert!(
                    l.grantor.grant_expiry >= l.grantee.hold_expiry,
                    "invariant violated: D'={} < C'={}",
                    l.grantor.grant_expiry,
                    l.grantee.hold_expiry
                );
            }
        }
    }

    #[test]
    fn partition_prevents_activation() {
        let s = Scenario::new(2)
            .seed(1)
            .duration(10_000)
            .node(0, |n| n.initiate_chance = 1.0)
            .link(LinkConfig {
                partitioned: true,
                ..LinkConfig::new(0, 1)
            })
            .lease(0, 1);
        let mut e = Engine::new(s);
        let events = e.advance_to(10_000);
        assert!(
            !grantee_ever_active(&events),
            "partitioned grantee must never activate"
        );
    }

    #[test]
    fn dropping_all_guards_prevents_activation() {
        // A 100% Guard drop means the guard phase never completes, so the
        // grantee never activates — regardless of link reliability.
        let s = basic().kind_drop(MsgKind::Guard, 1.0);
        let mut e = Engine::new(s);
        let events = e.advance_to(10_000);
        assert!(
            !grantee_ever_active(&events),
            "with every Guard dropped the lease can never come up"
        );
    }

    #[test]
    fn dropping_all_renews_prevents_activation() {
        // Guards get through, but every Renew is lost, so the grantee (which
        // only goes Active on a Renew) never holds the lease.
        let s = basic().kind_drop(MsgKind::Renew, 1.0);
        let mut e = Engine::new(s);
        let events = e.advance_to(10_000);
        assert!(
            !grantee_ever_active(&events),
            "with every Renew dropped the grantee never activates"
        );
    }

    fn renew_sends(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|ev| {
                matches!(
                    ev.kind,
                    EventKind::MessageSent {
                        kind: MsgKind::Renew,
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn stuck_renews_time_out_and_reguard() {
        // Guards get through (grantor goes Active) but every Renew is dropped, so
        // no RenewReply ever confirms the grantee. The grantor must NOT renew into
        // the void forever: after RENEW_REPLY_TIMEOUT it stops renewing, lets `D'`
        // lapse, falls idle, and re-guards — so over a 10k run we see many Guard
        // attempts, not the single guard a no-timeout grantor would send before
        // renewing endlessly. The grantee never activates (all Renews lost).
        let s = basic().kind_drop(MsgKind::Renew, 1.0);
        let mut e = Engine::new(s);
        let events = e.advance_to(10_000);
        assert!(
            !grantee_ever_active(&events),
            "every Renew dropped: grantee never activates"
        );
        assert!(
            guard_sends(&events) > 1,
            "grantor should re-guard after stale renews, not renew forever (guards {})",
            guard_sends(&events)
        );
        // Renews are sent, but boundedly: a handful per active window, not one
        // every renew_interval for the whole run (that would be ~16+ with no
        // timeout). The cap is generous; the point is it is finite per cycle.
        assert!(
            renew_sends(&events) < guard_sends(&events) * 6,
            "renews should be bounded per active window, sent {}",
            renew_sends(&events)
        );
    }

    #[test]
    fn healthy_lease_never_times_out_renews() {
        // With reliable links the RenewReply liveness clock is refreshed every
        // round, so a healthy lease is never abandoned: exactly one guard brings
        // it up and it stays Active the whole run (no re-guarding).
        let mut e = Engine::new(basic());
        let events = e.advance_to(10_000);
        assert!(grantee_ever_active(&events), "healthy lease comes up");
        assert_eq!(
            guard_sends(&events),
            1,
            "a healthy lease guards once and never re-guards (guards {})",
            guard_sends(&events)
        );
    }

    #[test]
    fn grantee_guard_expires_when_first_renew_never_arrives() {
        // The Guard lands (grantee -> Guarding) but every Renew is dropped, so
        // the activating first Renew never arrives. The grantee must not hang in
        // Guarding forever: once its guard window `A'` lapses it expires.
        let s = basic().kind_drop(MsgKind::Renew, 1.0);
        let mut e = Engine::new(s);
        e.advance_to(10_000);
        for l in &e.leases {
            assert_ne!(
                l.grantee.status,
                LeaseStatus::Guarding,
                "grantee must not stay stuck in the guard phase"
            );
        }
        // And the transition was actually emitted as an expiry.
        let mut e2 = Engine::new(basic().kind_drop(MsgKind::Renew, 1.0));
        let events = e2.advance_to(10_000);
        assert!(
            events.iter().any(|ev| matches!(
                ev.kind,
                EventKind::GranteeLease {
                    status: LeaseStatus::Expired,
                    ..
                }
            )),
            "a lapsed guard window should emit a grantee expiry"
        );
    }

    fn guard_sends(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|ev| {
                matches!(
                    ev.kind,
                    EventKind::MessageSent {
                        kind: MsgKind::Guard,
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn stuck_guard_times_out_and_retries() {
        // With every Guard dropped the lease can never come up — but the grantor
        // must not hang in `Guarding` forever: each attempt times out after a full
        // guard window (`t_guard - t_delta`) and, once idle, a fresh Guard is
        // (re-)initiated. Over a 10k run that means several attempts, not the
        // single Guard a no-retry grantor would send and then stall on.
        let s = basic().kind_drop(MsgKind::Guard, 1.0);
        let mut e = Engine::new(s);
        let events = e.advance_to(10_000);
        assert!(
            !grantee_ever_active(&events),
            "every Guard dropped: lease still can't come up"
        );
        assert!(
            guard_sends(&events) > 3,
            "grantor should retry the guard many times, not send just once (sent {})",
            guard_sends(&events)
        );
    }

    #[test]
    fn lossy_guard_lease_still_establishes_via_retry() {
        // A high but < 100% Guard drop: the first attempts likely fail, but the
        // retry path keeps re-guarding until one round gets through, so the lease
        // still establishes. Before the retry timeout existed, a single early
        // drop would strand the grantor in `Guarding` forever.
        let s = basic().kind_drop(MsgKind::Guard, 0.7);
        let mut e = Engine::new(s);
        let events = e.advance_to(40_000);
        assert!(
            grantee_ever_active(&events),
            "with retries a lossy-guard lease should eventually come up"
        );
        assert!(
            guard_sends(&events) > 1,
            "establishing under guard loss should take more than one attempt"
        );
    }

    #[test]
    fn advance_is_monotonic_and_bounded() {
        let mut e = Engine::new(basic());
        e.advance_to(5_000);
        assert_eq!(e.now(), 5_000);
        e.advance_to(50_000);
        assert_eq!(e.now(), 10_000, "must clamp to duration");
    }

    #[test]
    fn frame_geometry_is_well_formed() {
        let mut e = Engine::new(basic());
        e.advance_to(3_000);
        let f = e.frame_at(3_000);
        assert_eq!(f.nodes.len(), 2);
        // Lease bars present for each declared lease; fills in [0, 1].
        assert_eq!(f.leases.len(), 1);
        for bar in &f.leases {
            assert!((0.0..=1.0).contains(&bar.grantor_fill));
            assert!((0.0..=1.0).contains(&bar.grantee_fill));
        }
        // In-flight messages, if any, sit strictly between their endpoints.
        for m in &f.messages {
            assert!((0.0..=1.0).contains(&m.progress));
        }
    }

    #[test]
    fn guarding_bar_has_a_draining_fill() {
        // A grantor that opens a guard whose Guard message is always dropped stays
        // in `Guarding` until its retry window lapses. While guarding, the bar must
        // show a *draining* countdown (fill in (0, 1]), not a flat 0 — the guard
        // timer the frontend colors blue. It should also shrink as time passes.
        let s = scripted().kind_drop(MsgKind::Guard, 1.0);
        let mut e = Engine::new(s);
        e.command(Command::Initiate(LeaseId {
            grantor: 0,
            grantee: 1,
        }));
        // Sample soon after guarding opens, then a little later (both well within
        // the guard window `t_guard - t_delta`, so the guard is still outstanding).
        e.advance_to(100);
        let f0 = e.frame_at(100);
        assert_eq!(f0.leases[0].grantor_status, LeaseStatus::Guarding);
        let early = f0.leases[0].grantor_fill;
        assert!(early > 0.0 && early <= 1.0, "guarding fill was {early}");

        e.advance_to(400);
        let f1 = e.frame_at(400);
        assert_eq!(f1.leases[0].grantor_status, LeaseStatus::Guarding);
        let late = f1.leases[0].grantor_fill;
        assert!(
            late < early,
            "guarding fill should drain: {late} !< {early}"
        );
    }

    #[test]
    fn drifting_clock_still_safe() {
        let s = Scenario::new(2)
            .seed(3)
            .duration(10_000)
            .node(0, |n| {
                n.initiate_chance = 1.0;
                n.clock = Clock::new(0, 1.0);
            })
            // Grantee clock runs fast and is skewed.
            .node(1, |n| n.clock = Clock::new(500, 1.05))
            .link(LinkConfig::new(0, 1))
            .link(LinkConfig::new(1, 0))
            .lease(0, 1);
        let mut e = Engine::new(s);
        e.advance_to(10_000);
        // Compare both expiries in global time to check real-time safety.
        for l in &e.leases {
            if l.grantor.status == LeaseStatus::Active && l.grantee.status == LeaseStatus::Active {
                let grantor_global = e.scenario.nodes[l.id.grantor]
                    .clock
                    .global_for_local(l.grantor.grant_expiry);
                let grantee_global = e.scenario.nodes[l.id.grantee]
                    .clock
                    .global_for_local(l.grantee.hold_expiry);
                assert!(
                    grantor_global >= grantee_global,
                    "real-time invariant violated: grantor {grantor_global} < grantee {grantee_global}"
                );
            }
        }
    }

    #[test]
    fn command_initiate_starts_a_lease() {
        let mut e = Engine::new(scripted());
        // Nothing initiates on its own before the command.
        assert!(!grantee_ever_active(&e.advance_to(1_000)));
        e.command(Command::Initiate(LeaseId {
            grantor: 0,
            grantee: 1,
        }));
        assert!(
            grantee_ever_active(&e.advance_to(10_000)),
            "explicit Initiate should bring the lease up"
        );
    }

    #[test]
    fn command_revoke_drops_the_grantee() {
        let id = LeaseId {
            grantor: 0,
            grantee: 1,
        };
        let mut e = Engine::new(scripted());
        e.command(Command::Initiate(id));
        e.advance_to(4_000);
        e.command(Command::Revoke(id));
        let events = e.advance_to(6_000);
        let grantee_expired = events.iter().any(|ev| {
            matches!(
                ev.kind,
                EventKind::GranteeLease {
                    lease,
                    status: LeaseStatus::Expired,
                } if lease == id
            )
        });
        assert!(grantee_expired, "Revoke should expire the grantee's hold");
    }

    #[test]
    fn scripted_command_is_reproducible() {
        let id = LeaseId {
            grantor: 0,
            grantee: 1,
        };
        let run = || {
            let s = scripted().command(500, Command::Initiate(id));
            Engine::new(s).advance_to(10_000)
        };
        assert_eq!(run(), run(), "same script + seed must replay identically");
    }

    #[test]
    fn failed_grantor_lease_lapses() {
        let id = LeaseId {
            grantor: 0,
            grantee: 1,
        };
        let mut e = Engine::new(scripted());
        e.command(Command::Initiate(id));
        e.advance_to(3_000);
        e.command(Command::FailNode(0));
        let events = e.advance_to(10_000);
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::NodeFailed { node: 0 })),
            "node 0 should be reported failed"
        );
        // With the grantor down and not renewing, the grantee's hold expires.
        assert!(
            e.leases[0].grantee.status == LeaseStatus::Expired,
            "grantee hold must lapse after grantor failure"
        );
    }

    #[test]
    fn all_to_all_leases_are_countable() {
        // Three nodes, all-to-all, all initiating: each node should end up
        // holding a majority (>= 2 of 3, counting itself) of grants.
        let s = Scenario::new(3)
            .seed(5)
            .duration(20_000)
            .all_to_all()
            .all_nodes(|n| n.initiate_chance = 1.0);
        let mut e = Engine::new(s);
        e.advance_to(20_000);
        let f = e.frame_at(e.now());
        for node in 0..3 {
            // Held grants toward `node`, plus 1 for the implicit self-grant.
            let held = f
                .leases
                .iter()
                .filter(|b| b.grantee == node && b.grantee_status == LeaseStatus::Active)
                .count()
                + 1;
            assert!(held >= 2, "node {node} should hold a majority, held {held}");
        }
    }

    // ---- Disruptive write path -------------------------------------------

    /// A 5-node all-to-one leader-lease scenario (everyone grants to node 0),
    /// reliable links, with disruptive writes on a given cadence.
    fn leader_writes(interval: Time) -> Scenario {
        Scenario::new(5)
            .seed(9)
            .duration(30_000)
            .all_to_one(0)
            .all_nodes(|n| n.initiate_chance = 1.0)
            .writes(Some(interval), true)
    }

    fn count_kind(events: &[Event], want: MsgKind) -> usize {
        events
            .iter()
            .filter(|ev| matches!(ev.kind, EventKind::MessageSent { kind, .. } if kind == want))
            .count()
    }

    /// Sample a grantee's held-grant count (active holds toward `node`, plus its
    /// implicit self-grant) at every `step` ticks across a fresh run, returning
    /// the per-sample series. Lets a test see the disrupt→recover swings a
    /// disruptive write drives, rather than snapshotting one fragile final tick.
    fn held_series(mut e: Engine, node: NodeId, step: Time) -> Vec<usize> {
        let mut series = Vec::new();
        let mut t = 0;
        while t <= e.duration() {
            e.advance_to(t);
            let f = e.frame_at(t);
            let held = f
                .leases
                .iter()
                .filter(|b| b.grantee == node && b.grantee_status == LeaseStatus::Active)
                .count()
                + 1;
            series.push(held);
            t += step;
        }
        series
    }

    #[test]
    fn disruptive_write_commits_and_broadcasts() {
        let mut e = Engine::new(leader_writes(2000));
        let events = e.advance_to(30_000);
        // Writes went out, replies came back, and at least one committed.
        assert!(
            count_kind(&events, MsgKind::Write) > 0,
            "leader should write"
        );
        assert!(
            count_kind(&events, MsgKind::WriteReply) > 0,
            "peers should reply to writes"
        );
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::WriteCommitted { leader: 0 })),
            "at least one write should commit at the leader"
        );
        // A commit broadcast followed.
        assert!(
            count_kind(&events, MsgKind::Commit) > 0,
            "a committed write should broadcast Commit"
        );
    }

    #[test]
    fn write_disrupts_then_leases_recover() {
        // A disruptive write tears down the leader's held read leases, then they
        // recover between writes: once the commit unfreezes the cluster the
        // grantors re-guard and re-establish the leader's holds. Sampling across
        // the run we should see both a low point (a write in progress, the leader's
        // reads suspended) and a majority high point (re-established) — rather than
        // snapshotting one fragile final tick.
        let series = held_series(Engine::new(leader_writes(1200)), 0, 25);
        let lo = series.iter().copied().min().unwrap();
        let hi = series.iter().copied().max().unwrap();
        assert!(
            lo <= 1,
            "a disruptive write should suspend the leader's reads (min held {lo})"
        );
        // 5 nodes → majority 3. The leader re-accumulates a majority between
        // writes, proving the commit path lets suspended reads recover.
        assert!(
            hi >= 3,
            "leader should recover a majority between writes (max held {hi})"
        );
    }

    #[test]
    fn no_writes_when_disabled() {
        let s = Scenario::new(5)
            .seed(9)
            .duration(20_000)
            .all_to_one(0)
            .all_nodes(|n| n.initiate_chance = 1.0); // writes default off
        let mut e = Engine::new(s);
        let events = e.advance_to(20_000);
        assert_eq!(
            count_kind(&events, MsgKind::Write),
            0,
            "no writes configured"
        );
    }

    #[test]
    fn stuck_write_round_aborts_and_recovers() {
        // Drop every WriteReply so the round can never reach its commit
        // condition. The leader must abort the stale round and its suspended read
        // leases must recover rather than hanging frozen forever.
        let s = leader_writes(2000).kind_drop(MsgKind::WriteReply, 1.0);
        let events = Engine::new(s.clone()).advance_to(30_000);
        assert!(
            count_kind(&events, MsgKind::Write) > 0,
            "writes still issued"
        );
        // No commit can happen (all replies lost)...
        assert!(
            !events
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::WriteCommitted { .. })),
            "no write can commit with every reply dropped"
        );
        // ...yet the leader recovers a majority between aborts: each stuck round
        // thaws the cluster after the timeout, and the grantors re-guard to
        // re-establish its holds until the next write tears them down again.
        let hi = held_series(Engine::new(s), 0, 25)
            .into_iter()
            .max()
            .unwrap();
        assert!(
            hi >= 3,
            "reads must recover a majority after aborted write rounds (max held {hi})"
        );
    }

    // ---- Non-disruptive write path ---------------------------------------

    /// Like `leader_writes` but non-disruptive.
    fn leader_writes_nondisruptive(interval: Time) -> Scenario {
        Scenario::new(5)
            .seed(9)
            .duration(30_000)
            .all_to_one(0)
            .all_nodes(|n| n.initiate_chance = 1.0)
            .writes(Some(interval), false)
    }

    #[test]
    fn nondisruptive_write_never_revokes_leases() {
        // Under non-disruptive writes the write path must not tear down leases:
        // no Revoke messages should ever be sent (leases only ever come down via
        // expiry here, and with reliable links + always-renew they don't).
        let mut e = Engine::new(leader_writes_nondisruptive(800));
        let events = e.advance_to(30_000);
        assert!(count_kind(&events, MsgKind::Write) > 0, "writes issued");
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::WriteCommitted { .. })),
            "non-disruptive writes still commit"
        );
        assert_eq!(
            count_kind(&events, MsgKind::Revoke),
            0,
            "non-disruptive writes must never revoke/tear down leases"
        );
        // Leases stay healthy throughout: the leader holds a full majority.
        let f = e.frame_at(e.now());
        let held = f
            .leases
            .iter()
            .filter(|b| b.grantee == 0 && b.grantee_status == LeaseStatus::Active)
            .count()
            + 1;
        assert!(
            held >= 3,
            "leader keeps its grants under writes, held {held}"
        );
    }

    #[test]
    fn nondisruptive_write_never_expires_a_held_lease() {
        // Non-disruptive writes must leave leases entirely untouched. With
        // reliable links and always-renew, the only thing that could expire a
        // held grantee lease is the write path — so once a grantee first goes
        // Active, it must never emit an Expired transition for the rest of the
        // run, even as writes flow and commit around it.
        let mut e = Engine::new(leader_writes_nondisruptive(600));
        let events = e.advance_to(30_000);
        assert!(count_kind(&events, MsgKind::Write) > 0, "writes issued");
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::WriteCommitted { .. })),
            "non-disruptive writes still commit"
        );
        // Track each grantee's activation, then assert no expiry follows it.
        let mut active: Vec<LeaseId> = Vec::new();
        for ev in &events {
            if let EventKind::GranteeLease { lease, status } = ev.kind {
                match status {
                    LeaseStatus::Active => {
                        if !active.contains(&lease) {
                            active.push(lease);
                        }
                    }
                    LeaseStatus::Expired => {
                        assert!(
                            !active.contains(&lease),
                            "a held lease {lease:?} expired under non-disruptive writes"
                        );
                    }
                    _ => {}
                }
            }
        }
        assert!(!active.is_empty(), "some lease should have gone active");
    }
}
