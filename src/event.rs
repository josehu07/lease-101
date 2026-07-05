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

/// Wire message kinds, mirroring the one-to-one leasing algorithm.
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
}

/// What ultimately happens to a message once it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgFate {
    /// Will be delivered at its arrival time.
    Delivered,
    /// Dropped in flight (lost link, partition); never arrives.
    Dropped,
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
}
