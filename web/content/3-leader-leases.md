+++
id = "leader-leases"
kind = "algo"
step = "03"
pattern = "all-to-one"
title = "Classic Leader Leases"
tradeoff_pro = "Leases sit off the critical path of consensus; writes never interrupt stable leadership."
tradeoff_con = "Only leadership is protected, so only the sole stable leader acquires local read power."
+++

Every node acts as a grantor and leases the one node it thinks is leader (which may be self). A *majority* of those promises becomes a cluster-wide guarantee.

The primitive is unchanged -- only the aggregate pattern. Each node grants to the latest leader it knows (after revoking any previous lease). Once a leader holds a majority number of grants (<span class="var">⌈n/2⌉</span>, counting its own), it is provably the only one: the **stable leader**.

Two majorities must overlap, and the shared node never promises two leaders at once. Majority intersection, applied to leases.

:::figure leader-leases

The payoff: the stable leader knows every latest commit, so it can serve reads from its local state directly -- a linearizable read becomes a local check, not a network quorum.

However, this convenience is only at the leader, not at any other replica nodes that clients may be closer to. A big improvement to regular quorum-mandatory consensus reads nonetheless, but not exploiting clients' location affinity fully.

:::tradeoff
