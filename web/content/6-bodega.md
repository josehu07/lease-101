+++
id = "bodega"
kind = "algo"
step = "06"
pattern = "co-design"
title = "Bodega: Roster Leases and Consensus Co-designed"

[[recap]]
trait = "Bounded clock drift assumption only"
seen = "one-to-one"

[[recap]]
trait = "Fault tolerance via self-healing expiry"
seen = "one-to-one"

[[recap]]
trait = "Off the critical path leases on role (anytime)"
seen = "leader leases"

[[recap]]
trait = "Configurable set of local readers (anywhere)"
seen = "quorum leases"

[[recap]]
trait = "Decoupled leasing and consensus (anywhere, anytime)"
seen = "roster leases"
new = true

[[recap]]
trait = "Efficiency via optimistic holding and more (co-design)"
seen = "bodega"
new = true
+++

Co-designs consensus serving around the roster. A responder serves reads locally if a majority grant it ∧ its latest known write is in committed status. A write requires gathering accepts from responders and broadcasts commit notifications.

Reads stay local through writes. The lease traffic is nearly free too: it *piggybacks on the heartbeats* that consensus replicas already exchange.

Two more subtleties keep local reads honest and efficient:

1. Latest slot not yet committed: Rather than an outright rejection, Bodega **optimistically holds** the read, anchors it on that slot, and answers it the moment it receives the commit notification for that slot (a bounded, often brief wait; the best strategy possible).
2. A newly-joined responder doesn't always have an *up-to-date* consensus log: Let each initial lease message carry the grantor's highest accepted slot, so a responder starts localizing reads when its log has caught up to a sufficient point.

:::recap
