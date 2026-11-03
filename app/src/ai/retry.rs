//! The single retry owner for the AI layer.
//!
//! Ported from the old supervisor's backoff (`connection.ts`): jittered
//! exponential backoff starting at [`RETRY_BASE`], doubling per attempt,
//! capped at [`RETRY_CAP`]. ONLY transport-class failures
//! ([`AiError::is_transport`]) are retried — auth, bad-response and
//! input failures surface immediately, because no amount of waiting fixes
//! a rejected password or a malformed reply.
//!
//! Request replay safety: a retried generation repeats the prompt
//! server-side (an extra session message or an orphaned temp session at
//! worst) and never touches repository state — read-only with respect to
//! the user's repo. Do NOT reuse this helper for genuinely mutating
//! requests (stage/commit/push): this module must remain the only place
//! that retries, and it retries only what is safe to repeat.

use std::future::Future;
use std::time::Duration;

use super::AiError;

/// First backoff delay (old supervisor's `RETRY_BASE_MS`).
pub const RETRY_BASE: Duration = Duration::from_secs(5);
/// Backoff cap (old supervisor's `RETRY_MAX_MS`: 5 minutes).
pub const RETRY_CAP: Duration = Duration::from_secs(5 * 60);
/// Attempts before giving up (first try + retries). The old supervisor
/// never replayed requests; this budget keeps a down backend from stalling
/// a feature call for minutes while still riding out a blip.
pub const DEFAULT_RETRY_ATTEMPTS: u32 = 3;

/// Delay before retry number `attempt` (1-based): `base * 2^(attempt-1)`
/// capped at [`RETRY_CAP`], jittered ±10% via `jitter()` ∈ [0, 1). Always
/// at least 1ms.
pub fn backoff_delay(attempt: u32, jitter: impl Fn() -> f64) -> Duration {
    let exponent = attempt.saturating_sub(1);
    let base = RETRY_BASE.saturating_mul(2u32.saturating_pow(exponent.min(30)));
    let base = base.min(RETRY_CAP);
    let factor = 1.0 + (jitter().clamp(0.0, 1.0) * 2.0 - 1.0) * 0.1;
    let millis = (base.as_millis() as f64 * factor).max(1.0);
    Duration::from_millis(millis as u64)
}

/// Runs `op`, retrying only transport-class failures up to `max_attempts`
/// with jittered exponential backoff between attempts. `sleep` is
/// injectable (tests pass an instant fake; production passes
/// [`super::sleep_real`]); `jitter` likewise (tests pass `|| 0.5` for a
/// deterministic ±0%).
///
/// Non-transport failures and exhausted budgets return the LAST error.
pub async fn retry_transport<T, F, Fut, J, S, SF>(
    max_attempts: u32,
    jitter: J,
    sleep: S,
    mut op: F,
) -> Result<T, AiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AiError>>,
    J: Fn() -> f64,
    S: Fn(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let mut attempt: u32 = 1;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) if err.is_transport() && attempt < max_attempts => {
                sleep(backoff_delay(attempt, &jitter)).await;
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Production shape of [`retry_transport`]: [`DEFAULT_RETRY_ATTEMPTS`]
/// attempts, the real tokio timer and the crate's default jitter source.
pub async fn with_transport_retries<T, F, Fut>(op: F) -> Result<T, AiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AiError>>,
{
    retry_transport(
        DEFAULT_RETRY_ATTEMPTS,
        super::default_jitter,
        super::sleep_real,
        op,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Fake clock: records every requested delay instead of sleeping.
    #[derive(Default, Clone)]
    struct FakeSleep(Arc<std::sync::Mutex<Vec<Duration>>>);

    impl FakeSleep {
        async fn sleep(&self, delay: Duration) {
            self.0.lock().unwrap().push(delay);
        }
    }

    /// Deterministic jitter: factor exactly 1.0 (no ±10% wobble).
    fn no_jitter() -> f64 {
        0.5
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_delay(1, no_jitter), Duration::from_secs(5));
        assert_eq!(backoff_delay(2, no_jitter), Duration::from_secs(10));
        assert_eq!(backoff_delay(3, no_jitter), Duration::from_secs(20));
        // attempt 20 would be 5s * 2^19 — clamped to the 5min cap.
        assert_eq!(backoff_delay(20, no_jitter), RETRY_CAP);
        assert_eq!(backoff_delay(1_000, no_jitter), RETRY_CAP);
    }

    #[test]
    fn backoff_jitter_stays_within_ten_percent() {
        assert_eq!(
            backoff_delay(1, || 0.0),
            Duration::from_millis(4_500),
            "factor 0.9"
        );
        assert_eq!(
            backoff_delay(1, || 1.0),
            Duration::from_millis(5_500),
            "factor 1.1"
        );
        // Out-of-range jitter sources are clamped into [0, 1] first.
        assert_eq!(backoff_delay(1, || -100.0), Duration::from_millis(4_500));
        assert_eq!(backoff_delay(1, || 100.0), Duration::from_millis(5_500));
    }

    #[tokio::test]
    async fn retries_transport_failures_then_succeeds() {
        let sleep = FakeSleep::default();
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: Result<&str, AiError> = retry_transport(
            DEFAULT_RETRY_ATTEMPTS,
            no_jitter,
            |d| sleep.sleep(d),
            move || {
                let calls = calls_for_op.clone();
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if n < 3 {
                        Err(AiError::Unavailable(format!("attempt {n} down")))
                    } else {
                        Ok("recovered")
                    }
                }
            },
        )
        .await;
        assert_eq!(result.unwrap(), "recovered");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            sleep.0.lock().unwrap().as_slice(),
            &[Duration::from_secs(5), Duration::from_secs(10)],
            "5s then 10s between the three attempts"
        );
    }

    #[tokio::test]
    async fn non_transport_failures_return_immediately() {
        let sleep = FakeSleep::default();
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: Result<(), AiError> = retry_transport(
            DEFAULT_RETRY_ATTEMPTS,
            no_jitter,
            |d| sleep.sleep(d),
            move || {
                let calls = calls_for_op.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(AiError::Unauthenticated("rejected".into()))
                }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no retry for auth failures"
        );
        assert!(sleep.0.lock().unwrap().is_empty(), "never slept");
    }

    #[tokio::test]
    async fn budget_exhaustion_returns_last_error() {
        let sleep = FakeSleep::default();
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: Result<(), AiError> = retry_transport(
            2,
            no_jitter,
            |d| sleep.sleep(d),
            move || {
                let calls = calls_for_op.clone();
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    Err::<(), _>(AiError::Unavailable(format!("down {n}")))
                }
            },
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(
            err.message(),
            "down 2",
            "last error surfaces, not the first"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            sleep.0.lock().unwrap().len(),
            1,
            "one sleep between two attempts"
        );
    }

    #[tokio::test]
    async fn zero_attempts_budget_still_runs_once() {
        let sleep = FakeSleep::default();
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: Result<u32, AiError> = retry_transport(
            0,
            no_jitter,
            |d| sleep.sleep(d),
            move || {
                let calls = calls_for_op.clone();
                async move { Ok(calls.fetch_add(1, Ordering::SeqCst) + 1) }
            },
        )
        .await;
        assert_eq!(result.unwrap(), 1, "the first attempt always runs");
        assert!(sleep.0.lock().unwrap().is_empty());
    }
}
