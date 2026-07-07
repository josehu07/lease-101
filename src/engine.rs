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
use std::collections::BinaryHeap;

use crate::clock::Time;
use crate::dist::Dist;
use crate::event::{Command, Event, EventKind, LeaseId, LeaseStatus, MsgFate, MsgKind, NodeId};
use crate::frame::{Frame, LeaseBar, MsgShape, NodeShape, NodeViz, lerp, ring_layout};
use crate::scenario::{LeaseParams, Scenario};

/// How often each node wakes to make stochastic decisions and service leases.
/// Per-step probabilities in the scenario are interpreted per poll.
const POLL_INTERVAL: Time = 50;

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
    /// Grantor-local time until which the promise is held (`D'`).
    grant_expiry: Time,
    /// Grantor-local time the next renew is due to be sent.
    next_renew_due: Time,
    /// Whether the grantor currently intends this lease to be active.
    intended: bool,
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
#[derive(Debug, Clone, Copy)]
struct NodeState {
    up: bool,
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
}

impl Engine {
    /// Build an engine from a scenario, scheduling each node's first poll and
    /// any commands scripted on the scenario.
    pub fn new(scenario: Scenario) -> Self {
        let rng = crate::rng::Rng::new(scenario.seed);
        let nodes = (0..scenario.node_count())
            .map(|_| NodeState { up: true })
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
        };
        for node in 0..engine.nodes.len() {
            engine.schedule(0, Scheduled::Poll { node });
        }
        for k in 0..engine.scenario.commands.len() {
            let (at, cmd) = engine.scenario.commands[k];
            engine.schedule(at, Scheduled::Command(cmd));
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

        let leases = self
            .leases
            .iter()
            .map(|l| LeaseBar {
                grantor: l.id.grantor,
                grantee: l.id.grantee,
                grantor_status: l.grantor.status,
                grantee_status: l.grantee.status,
                grantor_fill: self.fill(l.id.grantor, l.grantor.status, l.grantor.grant_expiry, t),
                grantee_fill: self.fill(l.id.grantee, l.grantee.status, l.grantee.hold_expiry, t),
            })
            .collect();

        Frame {
            at: t,
            nodes,
            messages,
            leases,
        }
    }

    /// Fraction of lease life remaining for one party at global time `t`, as a
    /// value in `0.0..1.0`. Zero unless the party considers the lease active.
    fn fill(&self, node: NodeId, status: LeaseStatus, expiry_local: Time, t: Time) -> f64 {
        if status != LeaseStatus::Active {
            return 0.0;
        }
        let local = self.scenario.nodes[node].clock.local(t);
        let remaining = (expiry_local - local) as f64;
        let span = self.params().t_lease as f64;
        (remaining / span).clamp(0.0, 1.0)
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
        }
    }

    /// Send a message `from -> to` now, sampling delay and drop from the link.
    /// The `MessageSent` event fires immediately; the (delivered-or-dropped)
    /// arrival is scheduled so its event lands in time order.
    fn send(&mut self, from: NodeId, to: NodeId, kind: MsgKind, lease_idx: usize) {
        let link = self.scenario.link_config(from, to);
        let sent = self.now;
        let dropped = link.partitioned || self.rng.chance(link.drop_chance);
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

    /// A node may spontaneously start a lease it grants but has not activated.
    fn maybe_initiate(&mut self, node: NodeId) {
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
        self.send(grantor, grantee, MsgKind::Guard, i);
    }

    /// Proactively revoke a lease: stop renewing and notify the grantee. The
    /// grantor keeps its own outstanding `D'` and lets it lapse naturally, so
    /// the safety invariant holds regardless of whether the grantee is reached.
    fn begin_revoke(&mut self, i: usize) {
        let LeaseId { grantor, grantee } = self.leases[i].id;
        self.leases[i].grantor.intended = false;
        self.send(grantor, grantee, MsgKind::Revoke, i);
    }

    /// Send due renews for leases this node grants and currently intends.
    fn service_grants(&mut self, node: NodeId) {
        for i in 0..self.leases.len() {
            if self.leases[i].id.grantor != node || !self.leases[i].grantor.intended {
                continue;
            }
            if self.leases[i].grantor.status != LeaseStatus::Active {
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
        let provisional = b + p.t_guard + p.t_delta + p.t_lease + p.t_delta;
        self.leases[i].grantor.grant_expiry = self.leases[i].grantor.grant_expiry.max(provisional);
        self.leases[i].grantor.next_renew_due = b + p.renew_interval;
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
        }
    }

    /// Grantee receives `Guard`: record the acceptance window `A'` and reply.
    fn on_guard(&mut self, i: usize) {
        let LeaseId { grantor, grantee } = self.leases[i].id;
        let p = self.params();
        let a = self.local(grantee);
        self.leases[i].grantee.guard_deadline = a + p.t_guard - p.t_delta;
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
        self.leases[i].grantor.next_renew_due = self.local(self.leases[i].id.grantor);
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
    /// Safe because the reply proves the grantee's receipt `C` preceded `d`, and
    /// the grantor's `+t_delta` versus the grantee's `-t_delta` keeps `D' > C'`.
    fn on_renew_reply(&mut self, i: usize) {
        if self.leases[i].grantor.status != LeaseStatus::Active {
            return;
        }
        let p = self.params();
        let d = self.local(self.leases[i].id.grantor);
        let tightened = d + p.t_lease + p.t_delta;
        // Only ever reduce toward the tighter bound; never below the grantee's
        // possible expiry, which `tightened` already dominates.
        self.leases[i].grantor.grant_expiry = self.leases[i].grantor.grant_expiry.min(tightened);
    }

    /// Grantee receives `Revoke`: drop the lease immediately.
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
    }

    // ---- Expiry detection -------------------------------------------------

    /// Recompute lease statuses for both roles `node` plays, emitting expiry
    /// transitions when a local clock has passed the relevant deadline.
    fn recompute_statuses(&mut self, node: NodeId) {
        for i in 0..self.leases.len() {
            let id = self.leases[i].id;
            if id.grantor == node && self.leases[i].grantor.status == LeaseStatus::Active {
                let local = self.local(node);
                if local > self.leases[i].grantor.grant_expiry {
                    self.leases[i].grantor.status = LeaseStatus::Expired;
                    self.emit(
                        self.now,
                        EventKind::GrantorLease {
                            lease: id,
                            status: LeaseStatus::Expired,
                        },
                    );
                }
            }
            if id.grantee == node && self.leases[i].grantee.status == LeaseStatus::Active {
                let local = self.local(node);
                if local > self.leases[i].grantee.hold_expiry {
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
}
