//! Durable EVM (`eip155`) exact-payment mechanisms for x402 facilitators.
//!
//! Built on the upstream [`x402_chain_eip155`] crate (ERC-3009
//! `transferWithAuthorization`, EIP-712, EOA and deployed EIP-1271 signatures,
//! USDC). Upstream verification is retained, while this crate owns the
//! chain-specific durable-submission and reconciliation mechanisms:
//!
//! - [`prepare`] canonically decodes both supported ERC-3009 overloads, signs
//!   EIP-1559 bytes at a pinned pending account nonce, and strictly revalidates
//!   stored bytes against one settlement identity before recovery trusts them.
//! - [`provider`] enforces an absolute gas-price cap, accounts for Base L1 data
//!   fees, reads durable state from explicit primary and backup RPC endpoints,
//!   and applies each record's persisted confirmation depth.
//! - [`settle`] keeps signature-shape selection aligned with upstream
//!   verification. Counterfactual EIP-6492 signatures are definitively rejected
//!   until atomic wallet deployment is implemented.
//!
//! A production service remains responsible for policy, authentication,
//! sponsorship budgets, persistence, and operator readiness. This crate keeps
//! those boundaries out of reusable chain mechanics.

#![forbid(unsafe_code)]

pub mod prepare;
pub mod provider;
pub mod settle;

// Upstream building blocks are re-exported for consumers that need to assemble
// adjacent verification behavior.

/// The upstream V2 eip155 `exact` scheme handler, reused for verification.
pub use x402_chain_eip155::V2Eip155Exact;

/// The upstream EVM chain provider (RPC access, meta-transaction submission,
/// pending-nonce tracking) that the durable provider wraps for submit/status.
pub use x402_chain_eip155::chain::Eip155ChainProvider;
