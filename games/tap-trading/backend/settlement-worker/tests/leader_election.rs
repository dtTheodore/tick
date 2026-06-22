mod common;

use std::time::Duration;

use tap_trading_settlement_worker::leader::LeaderGuard;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_one_worker_acquires_the_lock() {
    let db = common::setup_test_postgres().await;

    let first = LeaderGuard::acquire_or_wait(&db.url).await.expect("first acquires");

    let second = tokio::time::timeout(
        Duration::from_secs(3),
        LeaderGuard::acquire_or_wait(&db.url),
    )
    .await;

    assert!(second.is_err(), "second worker must not acquire while first holds");

    let mut first = first;
    first.release().await.expect("release first");

    let _third = tokio::time::timeout(
        Duration::from_secs(5),
        LeaderGuard::acquire_or_wait(&db.url),
    )
    .await
    .expect("third acquires within timeout")
    .expect("third returns Ok");
}
