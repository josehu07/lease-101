+++
id = "quorum"
kind = "algo"
step = "03"
pattern = "all-to-many"
title = "All-to-Many Quorum Read Leases"
figure_caption = "A subset holding a per-object lease; a write sweeps the quorum and briefly collapses their local-read privilege."
tradeoff_pro = "Local reads expand from one node to a configurable subset, exploiting read locality."
tradeoff_con = "Leases are coupled to the write path, so any write to a leased object disrupts local reads."
+++

One local reader is limiting. Hand the privilege to a *chosen subset* instead —
picked per object, so each object's frequent readers serve it locally.

A quorum lease is a pair <span class="var">(Q, O)</span>: holder replicas
<span class="var">Q</span> over objects <span class="var">O</span>. The promise
is new in kind — not “you are the leader” but *“I won't modify these without
notifying you first.”* A holder reads locally once a majority grant it.

The trick: revocation rides the write path. A Paxos write already needs a majority
quorum; include the holders in it, and their ordinary accept-replies *double as*
the notification — revocation and write in one round, no extra trip.

That is also the catch: since the write carries the revocation, writing a leased
object *suspends* local reads and tears the lease down, forcing the holder to
re-establish it — guard round-trips and all — before reads resume.

:::figure

:::tradeoff
