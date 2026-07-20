+++
id = "lease-manager"
kind = "algo"
step = "02"
pattern = "one-to-many"
title = "The Lease Manager Pattern"
+++

A common way to deploy the primitive in a distributed system is to have a dedicated *lease manager* entity that holds the authority to grant leases, handing them out to nodes as they request them. Each grant is just an independent instance of the one-to-one primitive.

:::figure lease-manager-handover

Distributed lock managers are a common example of application of this pattern. But, what about for consensus, how can leases help there?
