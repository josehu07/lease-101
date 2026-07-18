+++
id = "intro"
kind = "intro"
title = "Distributed Lease"
+++

A *lease* is a directional, time-bounded, refreshable promise: one node — the *grantor* —
promises another — the *grantee* — that it will uphold some rule until expiry.

Each rung is driven by a limit of the one below it:

1. [**One-to-one**](#one-to-one) — the primitive: a safe promise between two nodes.
2. [**All-to-one**](#leader) — everyone leases the leader, so the leader reads locally.
3. [**All-to-many**](#quorum) — a chosen subset leases each object and reads it locally.
4. [**All-to-all**](#roster) — lease the cluster roster; a chosen subset reads it locally.

Then [**Bodega**](#bodega) co-designs that roster lease with consensus, so those
local reads hold *anytime* — undisturbed by the write log.
