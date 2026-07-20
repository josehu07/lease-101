+++
id = "one-to-one"
kind = "algo"
step = "01"
pattern = "one-to-one"
title = "Standard One-to-One Leasing"
+++

The base primitive: one grantor promises one grantee.

One *invariant* holds at all times: the grantor expires the lease no earlier than the grantee does, so the grantee never believes on a lease the grantor has dropped.

Lease only requires *bounded clock drift* between the two sides, i.e., the speed of time flowing does not differ by more than <span class="var">t<sub>Δ</sub></span>.

How do we hold the invariant true without synchronized timestamps? Via messages. Two phases:

1. **Guard** (once). Tames the unknown delay of the first message -- the grantee accepts the first renewal only inside a self-imposed window, letting the grantor bound expiry before any reply.
2. **Renew** (steady state). The lease starts on the first renewal (if received timely before guard expiry); a renew exchange loop at sub-timeout intervals keeps the lease alive.

:::figure one-to-one-success

:::figure one-to-one-guard-reply-lost

Lease ends at either when the grantor proactively *revokes* it, or when renewals don't arrive before expiry -- the expiration mechanism makes lease failure-resilient.

:::figure one-to-one-revoked

:::figure one-to-one-renew-replies-lost

Notice the grantor-side timer dance: every renewal it sends pessimistically *extends* grantor's expiry (in case the reply never comes back), and upon receiving the reply it *tightens* expiry calculation back to the confirmed receipt.
