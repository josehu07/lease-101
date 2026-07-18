//! Message kinds, lease status, and the timestamped event stream.

use crate::clock::Time;

/// Stable index of a node within a simulation.
pub type NodeId = usize;

/// A directed grantor -> grantee lease relationship, identified for tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LeaseId {
    pub grantor: NodeId,
    pub grantee: NodeId,
}

/// Wire message kinds, mirroring the one-to-one leasing algorithm, plus the
/// write-path messages layered on top (leader write broadcast / reply / commit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    /// Grantor -> grantee: open the guard phase.
    Guard,
    /// Grantee -> grantor: acknowledge the guard.
    GuardReply,
    /// Grantor -> grantee: start/extend the promise.
    Renew,
    /// Grantee -> grantor: acknowledge a renewal.
    RenewReply,
    /// Grantor -> grantee: proactively deactivate the lease.
    Revoke,
    /// Leader -> every other node: a write request to be served (disruptive
    /// path: recipients suspend the read leases they hold before replying).
    Write,
    /// Node -> leader: acknowledge a `Write` (its held reads are now suspended).
    WriteReply,
    /// Leader -> every other node: the write committed; suspended read leases
    /// may resume (re-activate on the next renew).
    Commit,
}

impl MsgKind {
    /// Number of distinct message kinds — the length of a per-kind array.
    pub const COUNT: usize = 8;

    /// Stable array index for this kind (`0..COUNT`), matching declaration order.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// What ultimately happens to a message once it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgFate {
    /// Will be delivered at its arrival time.
    Delivered,
    /// Dropped in flight (lost link, partition); never arrives.
    Dropped,
}

/// A driving command: an external, deterministic action applied to a running
/// simulation. Either scripted on a [`Scenario`](crate::Scenario) (reproducible)
/// or injected live via [`Engine::command`](crate::Engine::command).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Grantor proactively opens (guards) a declared lease that is idle.
    Initiate(LeaseId),
    /// Grantor proactively revokes a lease it currently grants.
    Revoke(LeaseId),
    /// Force a node down (crash).
    FailNode(NodeId),
    /// Bring a downed node back up.
    RecoverNode(NodeId),
}

/// Logical lease status from a single party's viewpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseStatus {
    /// No active promise.
    Inactive,
    /// Guard phase in progress (promise not yet counted).
    Guarding,
    /// Promise active (granted by grantor / held by grantee).
    Active,
    /// Promise lapsed by timeout or revocation.
    Expired,
}

/// A timestamped event in global simulation time. This is the stream consumed
/// by both the live animation and the GIF generator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// Global time at which the event occurs.
    pub at: Time,
    pub kind: EventKind,
}

/// The payload of an [`Event`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventKind {
    /// A message left `from` at `sent` and (if delivered) arrives at `arrival`.
    MessageSent {
        from: NodeId,
        to: NodeId,
        kind: MsgKind,
        sent: Time,
        arrival: Time,
        fate: MsgFate,
    },
    /// A message was delivered to its destination.
    MessageDelivered {
        from: NodeId,
        to: NodeId,
        kind: MsgKind,
    },
    /// A message was dropped in flight.
    MessageDropped {
        from: NodeId,
        to: NodeId,
        kind: MsgKind,
    },
    /// Lease status changed from the grantor's viewpoint.
    GrantorLease { lease: LeaseId, status: LeaseStatus },
    /// Lease status changed from the grantee's viewpoint.
    GranteeLease { lease: LeaseId, status: LeaseStatus },
    /// A node crashed.
    NodeFailed { node: NodeId },
    /// A node recovered from a crash.
    NodeRecovered { node: NodeId },
    /// The leader began serving a write (broadcast just went out).
    WriteStarted { leader: NodeId },
    /// The leader's write committed (enough replies gathered).
    WriteCommitted { leader: NodeId },
}
