# Lease Algorithms

This doc collects the full understanding of the lease algorithms relevant to the project. Source material lives in `docs/references/` (primarily the Bodega paper, `Bodega.pdf`).

Contents:

- [Standard One-to-One Leasing](#standard-one-to-one-leasing)
- [Classic All-to-One Leader Leases](#classic-all-to-one-leader-leases)
- [All-to-Many Quorum Read Leases](#all-to-many-quorum-read-leases)
- [All-to-All Roster Leases (Bodega)](#all-to-all-roster-leases-bodega)

---

## Standard One-to-One Leasing

Source: Bodega paper (`docs/references/Bodega.pdf`), §2.2 "Distributed Lease" and Figure 2. This is the core one-to-one mechanism that roster leases build upon.

### Concept of Lease

A **lease** is a *directional, time-bounded promise* from a **grantor** to a **grantee**. The grantor promises to withhold some conflicting action (granting to another node, voting, etc.) for as long as the grantee might still believe it holds the lease.

Content of lease promise: any abstract information

### Assumption: bounded clock drift, not synchronized clocks

- Over any elapsed physical duration `t_lease`, the two nodes' clocks diverge by at most a small `t_Δ`. (bounded clock drift)
- It does **not** assume synchronized timestamps — clock offset/skew may be arbitrary. (clock skew allowed)

Why skew is irrelevant but drift must be bounded: every expiration is computed as `(local event time) + (duration ± t_Δ)`, checked against the node's clock locally. No node ever compares its timestamp to another node's. So:

- **Skew** (constant offset between clocks) cancels out — each side measures elapsed time on its own clock, independent of the other's absolute value. The one cross-node ordering needed (bootstrapping) is converted into a purely local timer by the guard phase (`C < A'`).
- **Drift** (clocks ticking at different rates) accumulates *within* a lease period, so a `t_lease` measured on the grantee's clock ≠ the same on the grantor's. This directly threatens the `C' < D'` invariant. `t_Δ` is the budget for max drift over one period; the asymmetric `±t_Δ` reserves `2·t_Δ` slack so the grantor's window still contains the grantee's under worst-case drift.

### Safety invariant

> The grantor-side expiration is never earlier than the grantee-side expiration.

If this holds, the grantee never acts on a lease the grantor already considers expired. The phases below maintain it inductively across every renewal round.

### Phase 1 — Guard (one-time, handles unknown message delay)

Bounds *when* the grantee may accept the very first `Renew`, despite an unknown delay of any message, so the grantor can compute a safe expiration even with no reply.

```text
Grantor:  --Guard-->            <--GuardReply--   [B: sends first Renew]
Grantee:  [A: recv Guard]  ...  accept first Renew only if  C < A'
```

- Grantee receives `Guard` at its local time **A**.
- Grantee accepts the first `Renew` only if it arrives before `A' = A + (t_guard − t_Δ)` (condition `C < A'`).
- This self-imposed window lets the grantor bound expiration relative to `A` (which in real time precedes `B`) without knowing the grantee's clock.
- Without a `Guard` round, if grantor sends the first `Renew` but hasn't heard of its reply, it could not know when the grantee receives that activation — it could be arbitrarily late due to arbitrary message delay — there could not determine a safe expiration.

### Phase 2 — Renew (steady-state promise exchanges)

The lease starts on the first `Renew`, **not** on the `Guard`:

- Grantor considers it **granted from B** — the moment it *sends* the first `Renew`.
- Grantee considers it **held from C** — the moment it *receives* the first `Renew`.

Timer durations (note the asymmetric `±t_Δ`):

| Side | Expiration | Slack |
| --- | --- | --- |
| Grantee holds until | `C' = C + (t_lease − t_Δ)` | expires **earlier** |
| Grantor (no RenewReply) grants until | `D' = B' + (t_lease + t_Δ)`, where `B' = B + (t_guard + t_Δ)` | expires **later** |
| Grantor (got RenewReply at D) grants until | `D' = D + (t_lease + t_Δ)` | tighter, anchored on confirmed receipt |

The grantor maintains its single `D'` by an **extend-on-send / shorten-on-reply** rule, so it is always safe yet as tight as confirmations allow:

- **On sending each `Renew`**, it *extends* `D'` to the pessimistic no-reply bound (`max` up to `B' + (t_lease + t_Δ)`), so even if that renew is received but its reply is lost, `D'` still dominates the grantee's `C'`.
- **On receiving a `RenewReply`** (confirming receipt at grantor-local `D`), it *shortens* `D'` back to the tighter `D + (t_lease + t_Δ)` (`min`).

So under healthy renewals `D'` tracks the last confirmed receipt (tight, only a bit past one lease span ahead); when replies stop, `D'` holds at its last pessimistic value and the lease expires there — the grantor never expires earlier than the grantee. This is one bound, not two separate timers: the rows above are just its extended vs. shortened states.

The grantor keeps **at most one renew in flight**: it sends the next `Renew` only after the previous one's `RenewReply` is confirmed. So a lost reply *stops* the renew stream — the grantor waits rather than firing further un-acked renews — and if the silence persists a lease-lifetime (no reply since the last confirmation), the grantor gives up: it stops renewing and lets `D'` lapse, expiring the lease and returning idle to re-guard.

The renewal exchanges continue on repeatedly, until either side decides to proactively terminate the lease, or some failure happens causing lease expiration. Renewal intervals are typically fractional to lease expiration timeouts, to keep the cycles on repeat under healthy situations.

### Deactivation & failure (right half of Figure 2)

The grantor deactivates a lease in two ways:

- **Proactively** via `Revoke`.
- **Passively** by withholding renewals (sometimes unwillingly due to failures) and waiting `t_lease + t_Δ` for safe expiry.

When a `Renew` or `Revoke` at `E` gets no reply (grantee possibly dead):

- Grantee, if alive, sets `E' = E + (t_lease − t_Δ)`.
- Grantor must wait until `F' = D' + (t_lease + t_Δ)` before declaring expiry.
- The induction `C' < D' ⇒ E' < F'` preserves the invariant across rounds. After `F'`, the grantor may safely act elsewhere.

---

## Classic All-to-One Leader Leases

Source: Bodega paper (`docs/references/Bodega.pdf`), §2.3.1 "Classic Protocols & Leader Leases" and Figure 3. This is the first generalization of the one-to-one primitive.

### Concept of Leader Leases

Leader leases are a commonly deployed optimization for establishing **stable leadership** in a leader-based consensus protocol (MultiPaxos, Raft, VR). The pattern is **all-to-one**: every node in the cluster grants a lease to the *one* node it currently believes is leader. It is just the one-to-one primitive run in parallel from all grantors toward a single grantee, plus a majority-counting rule that turns those individual promises into a cluster-wide guarantee.

Content of lease promise: no voting for a different leader

### From one-to-one to all-to-one

Each `(node → leader)` lease is exactly a standard one-to-one lease from the previous section: guard phase, repeated renews, `Revoke`/passive expiry, and the same `C' < D'` safety invariant. Nothing about the primitive changes. What is new is only the *pattern* and its *interpretation*:

| Aspect | One-to-One | All-to-One (Leader Leases) |
| --- | --- | --- |
| Grantors | 1 | every node (all `n`) |
| Grantee | 1 | the current leader (one node when stable) |
| Direction | fixed pair | all nodes → leader |
| Safe count | 1 | leader is stable once it holds `≥ m = ⌊n/2⌋+1` |

The leader is an implicit self-granter: it counts a lease to itself among the `m`.

### The majority rule

A leader `S` collects the leases granted *to* it. Once `S` holds at least a majority `m = ⌊n/2⌋+1` of leases (including its own), it can safely assert it is **the only such leader in the cluster** — the *stable leader*. Two nodes cannot each hold a majority of leases at the same time, because the two majorities would overlap in at least one node, and that node grants to at most one leader at a time (it invalidates any old lease before granting a new one). This is the same quorum-intersection argument that underpins consensus, now applied to leases.

### Why it enables local reads

A stable leader knows no newer values could have committed elsewhere without its involvement: any write must go through it, and no competing leader can exist while it holds a majority of leases. Therefore `S` (and only `S`) may answer read requests **locally** from its latest committed value, skipping the round-trip to a quorum that classic MultiPaxos requires for every read. This is the point — leases convert the cost of a linearizable read from a network quorum into a local clock check.

### Granting and revoking

- **Grant.** Each node grants to the most recent leader it is aware of (which may be itself), first **invalidating any old lease** it had given to a previous leader. This "revoke-then-grant" ordering is what makes the majority rule sound: a grantor never simultaneously promises two leaders.
- **Renew.** Steady state is the one-to-one renew loop from every follower to the leader, keeping the leader's majority fresh with no critical-path cost.
- **Expiry.** If a leader loses renewals from enough followers that it drops below `m` held leases, it can no longer assert stable leadership and must stop serving local reads — falling back to ordinary quorum reads or a leader election. Followers whose leases lapse are then free to grant to a new leader.

### Pro & Con of Leader Leases (that motivated Bodega)

- Pro: Leases are off the critical path of consensus operations and are a strict enhancement to the system; writes do not interrupt local reads.
- Con: Leader leases only protect *leadership*, so only the leader gains local reads.

---

## All-to-Many Quorum Read Leases

Source: Moraru et al., "Paxos Quorum Leases: Fast Reads Without Sacrificing Writes" (SoCC '14, `docs/references/QuorumLeases.pdf`), and the Bodega paper's §2.3.3 summary. This extends "one privileged reader" to "a configurable subset of privileged readers," at the cost of coupling the leases to the write path.

### Concept of Quorum Leases

A **quorum lease** extends local reads to a *chosen subset* of replicas, picked per object so the replicas that read an object most often can serve it locally. The promise it carries is different in kind from a leader lease: it is a **"will not modify without notifying you"** promise on a set of objects, not a "you are the leader" promise. Formally, a quorum lease is a pair `(Q, O)`:

- `Q` — the **lease quorum**: the subset of replicas that hold the lease (any size, even fewer than half). Each `r ∈ Q` is a *lease holder* / local reader.
- `O` — the **granted objects**: the set of objects this lease covers.

Every replica `g` that grants `(Q, O)` to a holder `r` promises two things: (1) it will **notify `r` synchronously before committing** any update to an object in `O` that `g` proposes, and (2) it will only acknowledge `Accept`/`Prepare` for updates to `O` on the condition that the proposer also notifies `r` synchronously first. Local read on a holder becomes **active** once a **majority** `m = ⌊n/2⌋+1` of replicas (the holder counting as an implicit self-grantor) have granted it.

Content of lease promise: no write without notifying

### The trick: revocation rides the write path

This is the "natural fit to Paxos" the paper leans on. A Paxos write must be accepted by a majority quorum anyway. If that write quorum is arranged to **include every current lease holder** of the object being written, then collecting the ordinary `AcceptReply`s from the quorum *also* confirms every holder has been notified — the lease revocation and the Paxos round are one and the same. No extra round-trip for writes in the common case.

### Write disrupts local reads

Because the write path carries the revocation, an in-progress write to a leased object **suspends** local reads of that object at the holders. This disruption is two-fold:

1. **Notification wait** — a holder cannot serve a local read until it learns the write's outcome. This is inherent to strong consistency. (It is a *theoretical* limit that even Bodega is subject to, but handles a little bit better via *optimistic holding*.)
2. **Lease teardown** — revocation *discontinues* the read lease itself. Once a write revokes it, the holder must **re-establish** the lease from scratch, paying the guard round-trips again before it can resume local reads. This cost is *not* fundamental — it is an artifact of coupling the lease's lifetime to individual writes.

### From all-to-one to a configurable set

| Aspect | All-to-One (Leader Leases) | Quorum Leases |
| --- | --- | --- |
| Local readers | leader only | a configurable subset `Q` per object |
| Promise content | "you are the stable leader" | "I won't modify `O` without notifying you" |
| Granularity | whole cluster, one role | per-object `(Q, O)` |
| Grantors for active | `≥ m = ⌊n/2⌋+1` | `≥ m = ⌊n/2⌋+1` |
| Coupling | off the critical path | **on it** — revocation bundled into the write's Paxos quorum |

### Pro & Con of Quorum Leases (that motivated Bodega)

- Pro: Expands local reads from one node to a **configurable subset**, exploiting read locality more usefully than leader leases — the replicas reading an object most can each serve it locally.
- Con: Leases carry **"no write" promises coupled to consensus logic**; because revocation rides the write path, any write to a leased object disrupts the read leases and interrupts local reads at the holders.

---

## All-to-All Roster Leases (Bodega)

Source: Bodega paper (`docs/references/Bodega.pdf`), §3 and Figure 5. The culmination of the progression: local reads at *any* configured subset, *anytime*, with generalized background leases not disrupted by writes.

### Concept of Roster Leases

The key move is to decouple *what the lease promises* from *who participates in leasing actions*. Leader Leases promise "I won't stand for another leader" — a leader-specific promise. Quorum leases promise "I won't write object `O` without notifying you" — a per-object, write-coupled promise. **Roster leases** instead promise agreement on a single piece of cluster **metadata**, the *roster*. The roster is a generalization of "who is leader" into "who is leader **and** who are the responders (local readers) for each key," identified by a ballot: a `⟨bal, ros⟩` pair.

A lease now says only "we agree on `⟨bal, ros⟩`"; it is unaffected by the stream of writes flowing through the log, and stays valid until the roster itself changes (a failure or a retuning). This is the novelty — leases become a generalized background technique. Reads become local **anytime**, not just in write-quiescent windows, and receive minimal disruption from writes.

Content of lease promise: generalized cluster roster metadata

### The all-to-all pattern

Every node is both grantor and grantee, exchanging the same one-to-one primitive with every peer. Each node tracks the guards/renewals it grants (`renewTo`) and holds (`renewBy`). The counting rule mirrors leader leases: a node whose `|renewBy| ≥ m = ⌊n/2⌋+1` knows a **majority agrees on its `⟨bal, ros⟩`**, so at most one such roster can exist — the *stable roster*. This is the precondition for any local read.

| | Leader Leases | Quorum Leases | Roster Leases |
| --- | --- | --- | --- |
| Pattern | all-to-one | all-to-many | **all-to-all** |
| Leased content | leadership | per-object "no write" | the **roster** metadata |
| Local readers | leader only | configurable subset | configurable subset (responders) |
| Coupling to writes | off critical path | **on it** | **off it** in the background |

Because the messages carry only roster metadata, they can nicely **piggyback on the heartbeats** a cluster already sends, so roster leases add no common-case overhead. A proactive roster change is a synchronous `revoke_leases()` of the old ballot followed by `initiate_leases()` for the new one — completing in ~two message rounds absent failures.

### The safety-threshold subtlety

A stable roster says a node's *view* is current, but not that its *log* is. A node that just joined a new roster might be missing recently committed slots. So `Guard` messages carry the grantor's highest accepted slot number; a node may read locally only once it has committed all slots up to the `m`-th smallest such threshold among its lease grantors. This guards against a lagging node serving a stale local read (e.g. S4 in Figure 5, holding a majority of grants but not yet caught up).

### Nice technique: optimistic holding

Even with a stable roster, a responder may hit a read whose latest matching slot is *accepted but not yet committed* — it cannot tell if that value will commit or be overwritten by an impending failure. Rather than reject the read (forcing a client redirect/retry), the responder **optimistically holds** it: parks it against the pending slot and replies the instant the commit notification lands (≤ one RTT in the healthy case). Held reads are released as their slots commit, so even a steady write stream never blocks them indefinitely; a client-side unhold timeout redirects if the leader is slow.

This is how Bodega softens the one disruption it *cannot* eliminate — the notification wait inherent to strong consistency — turning a hard rejection into a brief, bounded hold. An optional *early accept notification* optimization roughly halves the expected hold time under low load.

### Good traits in one algorithm

Bodega combines every desirable trait accumulated across the progression:

- **Weak clock assumption** — needs only bounded drift, never synchronized clocks; build on top of standard one-to-one leasing.
- **Fault tolerance via expiration** — leases self-heal on failure; no external oracle required.
- **Off the critical path** — leasing is background metadata agreement, adding no common-case overhead (heartbeat piggybacking), like Leader Leases.
- **Configurable local readers** — any chosen subset reads locally, exploiting read locality, like Quorum Leases.
- **Anytime local reads** — leases decoupled from writes, unlocking optimistic holding, so reads stay local even under continuous writes.
