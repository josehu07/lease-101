//! The friendly, configurable scenario description that seeds a simulation.
//!
//! A [`Scenario`] is pure configuration data. Build one with [`Scenario::new`]
//! and the chainable helpers, then hand it to the engine to run.

use crate::clock::{Clock, Time};
use crate::dist::Dist;
use crate::event::{LeaseId, NodeId};

/// Lease timing parameters shared by the simulation, in ticks.
///
/// Mirrors the algorithm's `t_guard`, `t_lease`, and `t_delta`. Renewals are
/// re-sent every `renew_interval` while a lease is meant to stay active.
#[derive(Debug, Clone, Copy)]
pub struct LeaseParams {
    pub t_guard: Time,
    pub t_lease: Time,
    pub t_delta: Time,
    pub renew_interval: Time,
}

impl Default for LeaseParams {
    fn default() -> Self {
        Self {
            t_guard: 1000,
            t_lease: 2000,
            t_delta: 100,
            renew_interval: 1000,
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
    /// A reasonable default link between two nodes.
    pub fn new(from: NodeId, to: NodeId) -> Self {
        Self {
            from,
            to,
            delay: Dist::Uniform { lo: 80, hi: 160 },
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
    /// How long to run, in global ticks.
    pub duration: Time,
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
            duration: 20_000,
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
