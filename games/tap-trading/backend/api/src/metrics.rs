//! Prometheus counters/histograms. Single shared registry per process.

use prometheus::{
    register_counter_vec_with_registry, register_histogram_vec_with_registry, CounterVec,
    HistogramVec, Registry,
};
use std::sync::Arc;

pub struct Metrics {
    pub registry: Registry,
    pub taps_committed_total: CounterVec,
    pub taps_rejected_total: CounterVec,
    pub tap_handler_duration_seconds: HistogramVec,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        let registry = Registry::new();
        let taps_committed_total = register_counter_vec_with_registry!(
            "taps_committed_total",
            "Total successful tap commits.",
            &["asset"],
            registry
        )
        .unwrap();
        let taps_rejected_total = register_counter_vec_with_registry!(
            "taps_rejected_total",
            "Total rejected tap commits by reason.",
            &["reason"],
            registry
        )
        .unwrap();
        let tap_handler_duration_seconds = register_histogram_vec_with_registry!(
            "tap_handler_duration_seconds",
            "Tap handler wall time in seconds.",
            &["outcome"],
            registry
        )
        .unwrap();
        // Initialize known label sets so the metric families appear in /metrics
        // output immediately, even before any tap is processed.
        for asset in &["BTC", "ETH", "SUI"] {
            taps_committed_total.with_label_values(&[asset]);
        }
        for reason in &[
            "invalid_stake",
            "unknown_asset",
            "lock_window",
            "invalid_cell",
            "stale_quote",
            "drift_exceeded",
            "insufficient_balance",
            "rate_limited",
            "internal",
        ] {
            taps_rejected_total.with_label_values(&[reason]);
        }
        for outcome in &["ok", "err"] {
            tap_handler_duration_seconds.with_label_values(&[outcome]);
        }

        Arc::new(Self {
            registry,
            taps_committed_total,
            taps_rejected_total,
            tap_handler_duration_seconds,
        })
    }
}
