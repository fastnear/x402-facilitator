# Registry submission artifacts

This directory contains target-specific bodies that are useful to keep under
review. It is not an x402 protocol extension or a standardized facilitator
manifest. A running instance's `/supported` response remains authoritative for
its network, scheme, wire version, extensions, and signer.

- [`x402-list-submission.json`](x402-list-submission.json) is a
  submission-ready body for x402-list.com's facilitator API. Review it against
  that service's current OpenAPI immediately before submitting.

The body uses the contact already published by the demo OpenAPI documents.
Confirm that it should receive the registry review outcome immediately before
submission; changing a contact does not change facilitator identity.

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
