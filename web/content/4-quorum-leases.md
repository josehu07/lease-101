+++
id = "quorum-leases"
kind = "algo"
step = "04"
pattern = "all-to-many"
title = "Quorum Read Leases"
tradeoff_pro = "Local reads possible at a a configurable subset of nodes, exploiting read locality."
tradeoff_con = "Lease logic is coupled to consensus serving, and any write disrupts local reads."
+++

One local reader is limiting. Hand the privilege to any *chosen subset* of nodes instead -- picked per object, so each object's frequent readers can serve its reads locally.

The promise is new in kind -- not “you are the leader” but “I won't accept writes to this object without notifying you first and revoking” -- a **read lease**. A grantee reads locally once a majority grant it ∧ its latest known write is in committed status; that write's value can be used.

:::figure quorum-leases

What happens upon a write? The trick: lease revocation rides the write's accept messages. A write already needs a majority quorum; we include all the grantees in it as an additional requirement, and their ordinary accept-replies *double as* the lease revocation notification -- no extra trip.

The downside of read lease semantic: any write to a leased object *disrupts* its local reads and tears the leases down, forcing re-establishment and causing local intervals of degradation.

:::figure quorum-leases-write-disruption

:::tradeoff
