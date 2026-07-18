+++
id = "leader"
kind = "algo"
step = "02"
pattern = "all-to-one"
title = "Classic All-to-One Leader Leases"
figure_caption = "Five nodes leasing one leader; the leader lights up once its held leases cross the majority line."
tradeoff_pro = "Leases sit off the critical path of consensus; writes never interrupt local reads."
tradeoff_con = "Only leadership is protected — so only the single leader gets local reads."
+++

Run the primitive in parallel: every node leases the one node it thinks is
leader. A majority of those promises becomes a cluster-wide guarantee.

The primitive is unchanged — only the pattern. Each node grants to the latest
leader it knows, *first revoking* any lease to a previous one. Once a leader holds
a majority (<span class="var">⌈n/2⌉</span>, counting its own), it is provably the
*only* one: two majorities must overlap, and the shared node never promises two
leaders at once. Quorum intersection, applied to leases.

The payoff: a **stable leader** knows nothing newer committed without it, so it
reads from local state — a linearizable read becomes a local clock check, not a
network quorum.

:::figure

:::tradeoff
