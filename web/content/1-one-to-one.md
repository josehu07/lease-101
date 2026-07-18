+++
id = "one-to-one"
kind = "algo"
step = "01"
pattern = "one-to-one"
title = "Standard One-to-One Leasing"
figure_caption = "One grantor → one grantee: guard handshake, renew loop, and the two expiry timers ticking with their ±t_Δ offset."
+++

The base primitive: one grantor promises one grantee.

Leases need only *bounded drift*, never synchronized clocks. Each node checks
expiry against its own clock, never comparing timestamps — so a constant skew
cancels out. Only drift *within* one period matters, and a small budget
<span class="var">t<sub>Δ</sub></span> covers it.

One invariant holds across every renewal:

> The grantor expires the lease no *earlier* than the grantee does.

So the grantee never acts on a lease the grantor has dropped. Asymmetric slack
(±<span class="var">t<sub>Δ</sub></span>) keeps it true: the grantee expires a
touch early, the grantor a touch late, so the grantor's window always covers the
grantee's. Two phases:

1. **Guard** (once). Tames the *unknown* delay of the first message — the grantee
   accepts the first renewal only inside a self-imposed window, letting the
   grantor bound expiry before any reply.
2. **Renew** (steady state). The lease starts on the first renewal; a renew loop
   at sub-timeout intervals keeps it fresh, off the critical path — free in the
   common case.

To end a lease, the grantor either *revokes* it or just withholds renewals and
waits out the expiry — the same path a failure takes, with no external oracle.

:::figure
