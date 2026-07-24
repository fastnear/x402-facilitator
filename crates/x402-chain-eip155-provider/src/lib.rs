//! Durable EVM (eip155) exact-payment provider for the x402 NEAR facilitator.
//!
//! Built on the upstream [`x402_chain_eip155`] crate (ERC-3009
//! `transferWithAuthorization`, EIP-712, EIP-1271/6492 smart-wallet signatures,
//! USDC). Verification is reused wholesale; this crate owns the **durable**
//! submission path so an EVM settlement rides the same journal, idempotency, and
//! indeterminate-recovery machinery as NEAR:
//!
//! - `prepare` builds and signs the `transferWithAuthorization` transaction via
//!   `alloy` from a funded EOA at a pinned account nonce; the signed RLP bytes
//!   and their deterministic hash are journaled and never re-signed.
//! - `broadcast` submits and always returns `Pending` — an EVM outcome is never
//!   trusted at one confirmation, so it rides the facilitator's existing
//!   indeterminate `submitted → reconcile` path.
//! - `reconcile_status` applies a confirmation-depth policy: a terminal success
//!   is written only at ≥ N confirmations; a mined transaction that disappears
//!   before N (reorg) stays `submitted` and is re-evaluated. Exactly-once is
//!   anchored on the on-chain EIP-3009 authorization nonce.
//!
//! ## Increment 5 build order (see `docs/evm-v2-design.md`)
//!
//! - **5a:** crate + dependency wiring; the `alloy` tree compiles on the pinned
//!   toolchain. The upstream building blocks are re-exported below.
//! - **5b (in progress):** the durable submit core in [`prepare`] — ERC-3009
//!   calldata encoding and deterministic EIP-1559 transaction signing that
//!   yields journalable RLP + hash — plus (next) the `EvmChainProvider` that
//!   reuses [`V2Eip155Exact`] for `verify` and drives `prepare` / `broadcast`.
//! - **5c:** `reconcile_status` with the confirmation-depth / reorg re-check.
//! - **5d:** wire the `ChainProvider::Evm` variant, `validate_eip155`, the
//!   secp256k1 signer credential, and the EVM readiness branch into the service.
//! - **5e/5f:** `base-sepolia.json`, integration tests, Base Sepolia drills.

#![forbid(unsafe_code)]

pub mod prepare;

// Upstream building blocks the durable provider is assembled from in 5b+. They
// are re-exported so the settlement provider references stable local paths.

/// The upstream V2 eip155 `exact` scheme handler, reused for verification.
pub use x402_chain_eip155::V2Eip155Exact;

/// The upstream EVM chain provider (RPC access, meta-transaction submission,
/// pending-nonce tracking) that the durable provider wraps for submit/status.
pub use x402_chain_eip155::chain::Eip155ChainProvider;
