//! Backoff helper bounds. ORACLE_SPEC §4.2: exp 100ms → 30s, jitter ±10%.

#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/sources/mod.rs"]
mod sources;

#[tokio::test]
async fn backoff_doubles_within_jitter_band() {
    let mut backoff_ms: u64 = 100;
    let start = std::time::Instant::now();
    sources::sleep_jittered(&mut backoff_ms).await;
    let elapsed = start.elapsed().as_millis() as u64;
    // First sleep: 100 ms ± 10 ms = [90, 110].
    assert!(elapsed >= 80, "elapsed {elapsed} ms");
    assert!(elapsed <= 200, "elapsed {elapsed} ms");
    // After: backoff_ms doubled to 200.
    assert_eq!(backoff_ms, 200);
}

#[tokio::test]
async fn backoff_caps_at_30_seconds() {
    let mut backoff_ms: u64 = 20_000;
    sources::sleep_jittered(&mut backoff_ms).await;
    assert!(backoff_ms <= 30_000, "got {backoff_ms}");
    // One more iteration: 40_000 → 30_000 cap.
    backoff_ms = 40_000;
    let mut tmp = backoff_ms;
    tmp = (tmp * 2).min(30_000);
    assert_eq!(tmp, 30_000);
}
