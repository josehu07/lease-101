+++
id = "roster-leases"
kind = "algo"
step = "05"
pattern = "all-to-all"
title = "Roster Leases: New, Simple, and Powerful"
+++

The culminating pattern, simple but powerful. Decouple *who takes part* in leasing from *what a lease protects*. Everyone takes part, exchanging leases in an **all-to-all** pattern, via logical messages.

What the leases protect is a simple piece of cluster metadata -- a **roster**: who is the leader ∧ who are the *responders* (i.e., local readers) per object. Roster is tagged by the ballot number as <span class="var">⟨bal, ros⟩</span>.

A lease now says “we agree on the roster”. Every node is both grantor and grantee, exchanging the primitive with every peer. Once a node holds a majority (<span class="var">⌈n/2⌉</span>), that majority agrees on its <span class="var">⟨bal, ros⟩</span> -- so at most one such **stable roster** can exist, the precondition for local reads *anywhere*.

Being about metadata, the promise isn't disrupted by writes. Reads can therefore go local *anytime*, not just in write-quiet windows.

:::figure roster-leases

That decoupling is the philosophy, combining both pros and avoiding both cons. But roster leases is only half the story -- the other half is how to integrate it with consensus with minimal tension. That co-design is Bodega.
