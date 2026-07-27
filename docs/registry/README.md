# Registry submission artifacts

This directory contains target-specific bodies that are useful to keep under
review. It is not an x402 protocol extension or a standardized facilitator
manifest. A running instance's `/supported` response remains authoritative for
its network, scheme, wire version, extensions, and signer.

- [`x402-list-submission.json`](x402-list-submission.json) is the reviewed body
  submitted to x402-list.com's facilitator API on 2026-07-26. Submission
  `925e62da-75e7-49f5-adca-57762b835966` is pending manual review; do not
  resubmit it while that review remains open.

The body uses the contact already published by the demo OpenAPI documents.
Changing a contact does not change facilitator identity, but any future
submission or update must still confirm the intended recipient immediately
before sending.

The stable public name remains **NEAR x402 Facilitator** because the project
launched on NEAR. Descriptions must say that the current engine supports NEAR
and Base; do not create a second Base-only identity.

Before copying any profile:

1. deploy the release whose public behavior is being described;
2. check the target instance's `/`, `/supported`, and `/readyz`;
3. verify every settlement address and first-transaction date from a chain
   explorer or independent RPC;
4. use only authentic traffic when a registry has a transaction-count gate;
   and
5. omit API keys, private keys, signed authorizations, transaction bytes, and
   credentialed URLs.

Shared identity and target status belong in the
[distribution log](../distribution.md), rather than a made-up machine schema
that another implementation might mistake for an x402 standard.
