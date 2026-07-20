//! The friendly, configurable scenario description that seeds a simulation.
//!
//! A [`Scenario`] is pure configuration data. Build one with [`Scenario::new`]
//! and the chainable helpers, then hand it to the engine to run.

use crate::clock::{Clock, Time};
use crate::dist::Dist;
use crate::event::{Command, LeaseId, MsgKind, NodeId};

/// Lease timing parameters shared by the simulation, in ticks.
///
/// Mirrors the algorithm's `t_guard`, `t_lease`, and `t_delta`. Renewals are
/// re-sent every `renew_interval` while a lease is meant to stay active.
///
/// The defaults follow one fixed set of relationships, anchored on `t_delta`
/// (`T_Δ`) and the link message delay (`T_msg ≈ 2·T_delta`):
/// `t_lease ≈ t_guard ≈ 2.5·renew_interval`, `renew_interval ≈ 3·T_msg`.
#[derive(Debug, Clone, Copy)]
pub struct LeaseParams {
    pub t_guard: Time,
    pub t_lease: Time,
    pub t_delta: Time,
    pub renew_interval: Time,
}

impl LeaseParams {
    /// The guard window `A' = t_guard − t_delta`: how long a grantee accepts the
    /// activating first renew, and equally how long a grantor holds an unanswered
    /// guard before giving up.
    pub fn guard_window(&self) -> Time {
        self.t_guard - self.t_delta
    }

    /// The grantor's provisioned no-reply grant span `B' + (t_lease + t_delta)`
    /// where `B' = send + (t_guard + t_delta)` — i.e. `t_guard + t_lease + 2·t_delta`
    /// past a renew's send. The full length the grantor's `D'` is armed to on send.
    pub fn grant_span(&self) -> Time {
        self.t_guard + self.t_lease + 2 * self.t_delta
    }
}

impl Default for LeaseParams {
    fn default() -> Self {
        Self {
            t_guard: 1500,
            t_lease: 1500,
            t_delta: 100,
            renew_interval: 600,
        }
    }
}

/// Per-node behavioral knobs.
#[derive(Debug, Clone, Copy)]
pub struct NodeConfig {
    /// This node's local clock (skew + drift).
    pub clock: Clock,
    /// How long the node takes to turn a received message into a reply.
    pub response_delay: Dist,
    /// Per-step probability the node spontaneously initiates a first lease step
    /// toward an intended grantee it is not yet promising to.
    pub initiate_chance: f64,
    /// Per-step probability the node crashes while up.
    pub fail_chance: f64,
    /// Per-step probability the node recovers while down.
    pub recover_chance: f64,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            clock: Clock::perfect(),
            response_delay: Dist::Fixed(50),
            initiate_chance: 0.0,
            fail_chance: 0.0,
            recover_chance: 0.0,
        }
    }
}

/// Per-link behavioral knobs for the directed edge `from -> to`.
#[derive(Debug, Clone, Copy)]
pub struct LinkConfig {
    pub from: NodeId,
    pub to: NodeId,
    /// Distribution of one-way message delay.
    pub delay: Dist,
    /// Probability any given message on this link is dropped.
    pub drop_chance: f64,
    /// If true, the link currently delivers nothing (partition).
    pub partitioned: bool,
}

impl LinkConfig {
    /// A reasonable default link between two nodes: one-way delay `T_msg ≈
    /// 2·T_delta = 200` ticks, with random jitter within ±40% of that average,
    /// reliable and connected.
    pub fn new(from: NodeId, to: NodeId) -> Self {
        Self {
            from,
            to,
            delay: Dist::Uniform { lo: 120, hi: 280 },
            drop_chance: 0.0,
            partitioned: false,
        }
    }
}

/// A complete, runnable scenario description.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// RNG seed; identical seeds reproduce identical runs.
    pub seed: u64,
    /// Lease timing parameters.
    pub params: LeaseParams,
    /// One entry per node, indexed by [`NodeId`].
    pub nodes: Vec<NodeConfig>,
    /// Directed link overrides. Links not listed use a default when looked up.
    pub links: Vec<LinkConfig>,
    /// Intended grantor -> grantee lease relationships to drive.
    pub leases: Vec<LeaseId>,
    /// Extra drop probability applied per message *kind*, indexed by
    /// [`MsgKind::index`]. Layered on top of a link's own `drop_chance`, so a
    /// caller can e.g. fail all `Guard`s without touching link reliability.
    pub kind_drop: [f64; MsgKind::COUNT],
    /// Global tick at which each kind's `kind_drop` starts applying. Before it,
    /// that kind is never dropped by `kind_drop` (0 = drop from the start).
    /// Lets a scenario establish cleanly and *then* begin losing a message kind —
    /// e.g. dropping `RenewReply`s only once the lease is already active.
    pub kind_drop_from: [Time; MsgKind::COUNT],
    /// Scripted commands, each paired with the global time it fires at. Run in
    /// addition to any stochastic behavior, and replay identically per seed.
    pub commands: Vec<(Time, Command)>,
    /// How long to run, in global ticks.
    pub duration: Time,
    /// Average interval (global ticks) between leader write requests, or `None`
    /// to never issue writes. Each round waits this long ± a small jitter.
    pub write_interval: Option<Time>,
    /// Whether writes are *disruptive*: if set, a write suspends the read leases
    /// each node holds until it commits, then re-establishes them via a fresh
    /// guard round; if unset, writes leave leases entirely untouched (see the
    /// engine's write path). Ignored when `write_interval` is `None`.
    pub write_disruptive: bool,
}

impl Scenario {
    /// Start a scenario with `n` default nodes and no links or leases.
    pub fn new(n: usize) -> Self {
        Self {
            seed: 0,
            params: LeaseParams::default(),
            nodes: vec![NodeConfig::default(); n],
            links: Vec::new(),
            leases: Vec::new(),
            kind_drop: [0.0; MsgKind::COUNT],
            kind_drop_from: [0; MsgKind::COUNT],
            commands: Vec::new(),
            duration: 20_000,
            write_interval: None,
            write_disruptive: false,
        }
    }

    /// Set the RNG seed.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Set the lease timing parameters.
    pub fn params(mut self, params: LeaseParams) -> Self {
        self.params = params;
        self
    }

    /// Set the run duration in ticks.
    pub fn duration(mut self, duration: Time) -> Self {
        self.duration = duration;
        self
    }

    /// Mutate a single node's configuration in place.
    pub fn node(mut self, id: NodeId, f: impl FnOnce(&mut NodeConfig)) -> Self {
        f(&mut self.nodes[id]);
        self
    }

    /// Mutate every node's configuration in place. Handy for symmetric patterns.
    pub fn all_nodes(mut self, f: impl Fn(&mut NodeConfig)) -> Self {
        for n in &mut self.nodes {
            f(n);
        }
        self
    }

    /// Override (or add) a directed link's configuration.
    pub fn link(mut self, link: LinkConfig) -> Self {
        if let Some(existing) = self
            .links
            .iter_mut()
            .find(|l| l.from == link.from && l.to == link.to)
        {
            *existing = link;
        } else {
            self.links.push(link);
        }
        self
    }

    /// Declare an intended grantor -> grantee lease to drive.
    pub fn lease(mut self, grantor: NodeId, grantee: NodeId) -> Self {
        self.leases.push(LeaseId { grantor, grantee });
        self
    }

    /// Set the extra drop probability for one message `kind`, on top of any
    /// per-link `drop_chance`. `p` is clamped to `[0, 1]`.
    pub fn kind_drop(mut self, kind: MsgKind, p: f64) -> Self {
        self.kind_drop[kind.index()] = p.clamp(0.0, 1.0);
        self
    }

    /// Like [`kind_drop`](Self::kind_drop), but the drop only takes effect from
    /// global tick `from` onward; before that the kind is delivered normally.
    /// Handy for "establish, then start losing this message kind" scenarios.
    pub fn kind_drop_from(mut self, kind: MsgKind, p: f64, from: Time) -> Self {
        self.kind_drop[kind.index()] = p.clamp(0.0, 1.0);
        self.kind_drop_from[kind.index()] = from;
        self
    }

    /// Configure the leader's write cadence: `interval` average global ticks
    /// between writes (`None` to disable), and whether writes are `disruptive`.
    pub fn writes(mut self, interval: Option<Time>, disruptive: bool) -> Self {
        self.write_interval = interval;
        self.write_disruptive = disruptive;
        self
    }

    /// Declare the all-to-one pattern of leader leases: every node grants to
    /// `leader`. The leader's own grant is implicit (counted as +1 by the
    /// consumer, per the majority rule), so the self-lease is skipped here.
    pub fn all_to_one(mut self, leader: NodeId) -> Self {
        for g in 0..self.nodes.len() {
            if g != leader {
                self.leases.push(LeaseId {
                    grantor: g,
                    grantee: leader,
                });
            }
        }
        self
    }

    /// Declare the all-to-many pattern of quorum read leases: every node grants
    /// to each holder in `holders` (a holder's grant to itself is implicit and
    /// skipped). The holders are the configurable subset of local readers.
    pub fn all_to_many(mut self, holders: &[NodeId]) -> Self {
        for &h in holders {
            for g in 0..self.nodes.len() {
                if g != h {
                    self.leases.push(LeaseId {
                        grantor: g,
                        grantee: h,
                    });
                }
            }
        }
        self
    }

    /// Declare the all-to-all pattern of roster leases: every ordered pair of
    /// distinct nodes gets a lease. Each node thus grants to, and holds from,
    /// every peer.
    pub fn all_to_all(mut self) -> Self {
        for g in 0..self.nodes.len() {
            for h in 0..self.nodes.len() {
                if g != h {
                    self.leases.push(LeaseId {
                        grantor: g,
                        grantee: h,
                    });
                }
            }
        }
        self
    }

    /// Script a command to fire at global time `at`. Deterministic per seed.
    pub fn command(mut self, at: Time, cmd: Command) -> Self {
        self.commands.push((at, cmd));
        self
    }

    /// Number of nodes in the scenario.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Look up the configured link for `from -> to`, or a default if unset.
    pub fn link_config(&self, from: NodeId, to: NodeId) -> LinkConfig {
        self.links
            .iter()
            .find(|l| l.from == from && l.to == to)
            .copied()
            .unwrap_or_else(|| LinkConfig::new(from, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_assembles_scenario() {
        let s = Scenario::new(3)
            .seed(7)
            .duration(5000)
            .node(0, |n| n.initiate_chance = 0.5)
            .link(LinkConfig {
                drop_chance: 0.1,
                ..LinkConfig::new(0, 1)
            })
            .lease(0, 1)
            .lease(0, 2);

        assert_eq!(s.node_count(), 3);
        assert_eq!(s.seed, 7);
        assert_eq!(s.duration, 5000);
        assert_eq!(s.nodes[0].initiate_chance, 0.5);
        assert_eq!(s.leases.len(), 2);
        assert_eq!(s.link_config(0, 1).drop_chance, 0.1);
    }

    #[test]
    fn unset_link_falls_back_to_default() {
        let s = Scenario::new(2);
        let l = s.link_config(0, 1);
        assert_eq!(l.from, 0);
        assert_eq!(l.to, 1);
        assert!(!l.partitioned);
    }

    #[test]
    fn patterns_declare_expected_lease_counts() {
        // Leader: every non-leader grants to the leader (self-grant implicit).
        assert_eq!(Scenario::new(5).all_to_one(2).leases.len(), 4);
        // Quorum: |holders| * (n - 1) leases, self-grants skipped.
        assert_eq!(Scenario::new(5).all_to_many(&[1, 3]).leases.len(), 8);
        // Roster: every ordered distinct pair, n * (n - 1).
        assert_eq!(Scenario::new(4).all_to_all().leases.len(), 12);
    }

    #[test]
    fn command_is_recorded_with_time() {
        let id = LeaseId {
            grantor: 0,
            grantee: 1,
        };
        let s = Scenario::new(2).command(500, Command::Initiate(id));
        assert_eq!(s.commands, vec![(500, Command::Initiate(id))]);
    }

    #[test]
    fn link_override_replaces_existing() {
        let s = Scenario::new(2)
            .link(LinkConfig {
                drop_chance: 0.2,
                ..LinkConfig::new(0, 1)
            })
            .link(LinkConfig {
                drop_chance: 0.5,
                ..LinkConfig::new(0, 1)
            });
        assert_eq!(s.links.len(), 1);
        assert_eq!(s.link_config(0, 1).drop_chance, 0.5);
    }
}
