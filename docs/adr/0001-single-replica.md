# Run catsed as a single instance

catsed keeps live Transfers only in the memory of the process holding the Sender's connection, and has no shared store. With several instances, a Receiver could land on one that doesn't know the Transfer Code. We deliberately run exactly one instance, a single container behind a reverse proxy, so every Transfer is findable locally, choosing simplicity over high availability and horizontal scaling.

## Considered Options

- **Peer lookup + internal proxy**: the instance that receives the Receiver's request asks its peers which one holds the Transfer, then relays through it. Rejected for v1: membership, code collisions across instances, and double internal bandwidth add complexity we don't need yet.
- **Owner instance encoded in the Transfer Code**: uses up the short code space and leaks how many instances there are.
- **HTTP redirect to a per-instance hostname**: breaks plain `curl <url>`, which doesn't follow redirects without `-L`.
- **Shared rendezvous store** (e.g. Redis): rejected because catsed is stateless, meaning no external datastore.

## Consequences

- A restart or crash ends every live Transfer.
- Deploys replace the one container in place: there is never a second instance, so a deploy means a short outage. Waiting Transfers are closed immediately; Claimed ones get a 120s grace period.
- Throughput and concurrent Transfers are capped by one process; scaling means vertical sizing.
- Transfer Codes need only be unique within the one process, so collision checks are local.
- Going multi-instance later (e.g. moving to Kubernetes) means picking one of the options above. Codes don't encode any instance, so peer lookup stays open as an option.
