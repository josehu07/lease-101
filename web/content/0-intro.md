+++
id = "intro"
kind = "intro"
title = "Distributed Lease 101"
+++

A *lease* is a directional, time-bounded, refreshable promise: one node (the *grantor*) promises another (the *grantee*) that it will uphold some rule until expiry.

Let's walk through how leases work, and how to apply leases to optimize wide-area linearizable read on a replicated consensus cluster:

1. [One-to-one](#one-to-one) -- the primitive: a safe promise between two nodes.
2. [Lease manager](#lease-manager) -- one node distributes a promise out to many.
3. [Leader Leases](#leader-leases) -- everyone leases the leader, producing a stable leader who can read locally.
4. [Quorum Leases](#quorum-leases) -- everyone leases a subset of nodes for local read in the absence of writes.
5. [Roster Leases](#roster-leases) -- everyone exchanges leases on the roster: a free side-channel agreement.
6. [Bodega Protocol](#bodega) -- co-designs roster leases with consensus: local read anywhere, anytime.

Prefer hands-on exploration? Try our [distributed lease simulator playground](/sim).
