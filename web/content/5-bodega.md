+++
id = "bodega"
kind = "algo"
step = "05"
pattern = "co-design"
title = "Bodega: Leases Co-Designed with Consensus"
figure_caption = "Values committing through the log while responders keep answering reads locally; a lagging node is held back by the safety threshold until it catches up, and one read is parked on an uncommitted slot until its commit lands."
recap_lead = "Bodega folds every trait gathered along the climb into one background technique:"

[[recap]]
trait = "Weak clock assumption (bounded drift only)"
seen = "one-to-one"

[[recap]]
trait = "Fault tolerance via self-healing expiry"
seen = "one-to-one"

[[recap]]
trait = "Off the critical path (heartbeat piggyback)"
seen = "leader leases"

[[recap]]
trait = "Configurable set of local readers"
seen = "quorum leases"

[[recap]]
trait = "Anytime local reads, decoupled from writes"
seen = "roster leases"
new = true
+++

Roster leases give agreement on *who* reads locally; consensus gives the ordered
*log* those reads answer from. **Bodega** co-designs the two so a responder serves
reads from its own log with no quorum round-trip — and, unlike quorum leases,
without a write ever tearing the lease down.

A responder may answer a key locally only when two things hold at once: it holds a
**stable roster** (a majority agree on <span class="var">⟨bal, ros⟩</span> and name
it a responder for that key), *and* its log has **caught up** past the safety
threshold below. Miss either — not a designated responder, roster in flux, or log
behind — and the read falls back to the leader as an ordinary quorum read. Nothing
is ever served from a view a majority no longer backs.

Writes are left untouched. They flow through consensus exactly as before — the
leader proposes a slot, a majority accepts, the value commits — and the lease never
enters that path. Because the promise is about roster *metadata*, not about
withholding writes, committing a value neither notifies nor revokes any responder.
The log simply advances beneath a lease that stays valid, so writes run at full
speed *while* reads stay local straight through them — exactly the coupling quorum
leases suffered, now gone.

One subtlety keeps those local reads honest: a current *view* is not a current
*log*. A node that just joined the roster may hold a majority of grants yet still
be missing recently committed slots — reading locally would risk a stale answer. So
each **Guard** carries the grantor's highest accepted slot, and a responder reads
locally only once it has committed every slot up to the
<span class="var">m</span>-th smallest of those thresholds. That ties the lease's
view to a concrete log position and closes the gap.

Even caught up, a read can land on a key whose newest matching slot is *accepted
but not yet committed* — the responder can't yet tell whether that value will commit
or be replaced. Rather than reject it (a client redirect and retry), Bodega
**optimistically holds** the read: it parks against the pending slot and answers the
instant the commit lands, within one round-trip in the healthy case. Held reads
release as their slots commit, so even a steady write stream never blocks them
indefinitely, and a client-side timeout redirects if the leader stalls. This softens
the one disruption strong consistency *cannot* remove — the wait for a value to
commit — turning a hard rejection into a brief, bounded hold.

:::figure

:::recap
