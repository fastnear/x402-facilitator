//! Draft x402 `exact` payment-proof mechanism backed by NEAR Intents 1Click.
//!
//! This crate deliberately contains no production HTTP routes, database code,
//! signer, or refund worker. It isolates the parts of
//! x402-foundation/x402#3370 that are stable enough to implement before merge:
//! the wire contract, authenticated 1Click API boundary, quote binding, proof
//! identity, discovery extension, and terminal-status interpretation.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod discovery;
pub mod one_click;
pub mod settlement;
pub mod signature;
pub mod state;
pub mod wire;

/// Upstream draft revision this implementation was built against.
pub const DRAFT_SPEC_REVISION: &str = "708d660f2f80f966db16caebdb38670e16f0bc4b";

/// x402 `extra.assetTransferMethod` discriminator proposed by the draft.
pub const ASSET_TRANSFER_METHOD: &str = "near-intents";

/// The only payment flow permitted for client-submitted proofs.
pub const PAYMENT_FLOW: &str = "upfront";
