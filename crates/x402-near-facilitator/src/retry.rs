//! Bounded retry for transient chain-lookup outcomes, mirroring the eip155
//! provider's internal `retry_transient` so both chains get the same
//! resilience (public RPC endpoints throttle under burst; NEAR lookups can
//! blip the same way). The predicate decides what counts as transient — an
//! ambiguous verification response, a throttled lookup — and everything else
//! returns immediately. Broadcast and reconciliation are deliberately never
//! routed through this helper on either chain: submission recovery belongs to
//! the journaled reconcile machinery.

use std::future::Future;
use std::time::Duration;

/// Backoff between retry attempts (initial attempt plus one per entry).
pub(crate) const RPC_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(300), Duration::from_millis(900)];

/// Re-run `call` while `transient` says the outcome is retryable, bounded by
/// [`RPC_RETRY_DELAYS`]. The predicate sees the whole outcome value, so it
/// works for plain `Result`s as well as protocol responses whose ambiguity is
/// encoded in a successful value.
pub(crate) async fn retry_while_transient<T, Fut>(
    mut call: impl FnMut() -> Fut,
    transient: impl Fn(&T) -> bool,
) -> T
where
    Fut: Future<Output = T>,
{
    let mut outcome = call().await;
    for delay in RPC_RETRY_DELAYS {
        if !transient(&outcome) {
            break;
        }
        tokio::time::sleep(delay).await;
        outcome = call().await;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn transient_outcomes_retry_until_stable() {
        let attempts = std::cell::Cell::new(0_u32);
        let outcome = retry_while_transient(
            || {
                attempts.set(attempts.get() + 1);
                let attempt = attempts.get();
                async move { attempt }
            },
            |attempt| *attempt < 3,
        )
        .await;
        assert_eq!(outcome, 3);
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn stable_outcomes_never_retry() {
        let attempts = std::cell::Cell::new(0_u32);
        let outcome: Result<u32, &str> = retry_while_transient(
            || {
                attempts.set(attempts.get() + 1);
                async { Err("invalid_signature") }
            },
            |outcome: &Result<u32, &str>| outcome.is_ok(),
        )
        .await;
        assert_eq!(outcome, Err("invalid_signature"));
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_are_bounded_and_return_the_last_outcome() {
        let attempts = std::cell::Cell::new(0_usize);
        let outcome = retry_while_transient(
            || {
                attempts.set(attempts.get() + 1);
                async { "still-ambiguous" }
            },
            |_| true,
        )
        .await;
        assert_eq!(outcome, "still-ambiguous");
        assert_eq!(attempts.get(), 1 + RPC_RETRY_DELAYS.len());
    }
}
