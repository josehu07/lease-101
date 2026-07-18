+++
id = "roster"
kind = "algo"
step = "04"
pattern = "all-to-all"
title = "All-to-All Roster Leases"
figure_caption = "A full all-to-all mesh: every node leasing every peer, the roster settling as majorities lock in, undisturbed by the write log."
+++

The culminating *pattern*. Decouple *what a lease promises* from *who takes part*,
and lease one piece of cluster metadata instead of leadership or a per-object rule.

That metadata is the **roster**: who is leader *and* who the responders (local
readers) are per key, tagged by a ballot <span class="var">⟨bal, ros⟩</span>. A
lease now says only “we agree on the roster.” Every node is both grantor and
grantee, exchanging the one-to-one primitive with every peer. Once a node's held
leases reach a majority (<span class="var">⌈n/2⌉</span>), that majority agrees on
its <span class="var">⟨bal, ros⟩</span> — so at most one such **stable roster** can
exist, the precondition for any local read.

Being about metadata, the promise ignores the write log entirely: it holds until
the roster itself changes (a failure or a retuning), not until the next write.
Reads can therefore go local *anytime*, not just in write-quiet windows, and
because the messages carry only roster metadata they piggyback on the heartbeats a
cluster already sends — no common-case overhead. A roster change is a synchronous
revoke of the old ballot followed by an initiate of the new one — roughly two
message rounds, absent failures.

:::figure

That is the philosophy. But a lease on the roster is only half the story — the
other half is how it rides *alongside* the consensus log without ever colliding
with it. That co-design is Bodega.
