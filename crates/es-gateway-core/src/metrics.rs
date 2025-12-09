//! Metrics for ES Sync Gateway
//!
//! Provides Prometheus-compatible metrics for observability.

use metrics::{counter, gauge, histogram, Counter, Gauge, Histogram, Key, Label};
use std::time::{Duration, Instant};

/// Metric names as constants for consistency
pub mod names {
    // Watcher metrics
    pub const WATCHER_CHANGES_TOTAL: &str = "es_watcher_changes_total";
    pub const WATCHER_PUBLISH_LATENCY: &str = "es_watcher_publish_latency_seconds";
    pub const WATCHER_CHECKPOINT_LAG: &str = "es_watcher_checkpoint_lag";
    pub const WATCHER_ERRORS_TOTAL: &str = "es_watcher_errors_total";
    pub const WATCHER_POLL_DURATION: &str = "es_watcher_poll_duration_seconds";

    // Gateway metrics
    pub const GATEWAY_MESSAGES_RECEIVED: &str = "nats_gateway_messages_received_total";
    pub const GATEWAY_MESSAGES_FILTERED: &str = "nats_gateway_messages_filtered_total";
    pub const GATEWAY_MESSAGES_FORWARDED: &str = "nats_gateway_messages_forwarded_total";
    pub const GATEWAY_FILTER_LATENCY: &str = "nats_gateway_filter_latency_seconds";

    // Writer metrics
    pub const WRITER_BULK_REQUESTS: &str = "es_writer_bulk_requests_total";
    pub const WRITER_DOCS_INDEXED: &str = "es_writer_docs_indexed_total";
    pub const WRITER_DOCS_FAILED: &str = "es_writer_docs_failed_total";
    pub const WRITER_BULK_LATENCY: &str = "es_writer_bulk_latency_seconds";
    pub const WRITER_RETRY_COUNT: &str = "es_writer_retry_count_total";
    pub const WRITER_DLQ_SIZE: &str = "es_writer_dlq_size";
    pub const WRITER_BUFFER_SIZE: &str = "es_writer_buffer_size";

    // Connection metrics
    pub const CONNECTION_STATE: &str = "connection_state";
    pub const CONNECTION_RECONNECTS: &str = "connection_reconnects_total";
}

/// Labels for metrics
pub mod labels {
    pub const COMPONENT: &str = "component";
    pub const INDEX: &str = "index";
    pub const OPERATION: &str = "operation";
    pub const FILTER: &str = "filter";
    pub const ERROR_TYPE: &str = "error_type";
    pub const CONNECTION: &str = "connection";
    pub const STATUS: &str = "status";
}

/// Watcher metrics
#[derive(Clone)]
pub struct WatcherMetrics {
    component: String,
}

impl WatcherMetrics {
    pub fn new(component: impl Into<String>) -> Self {
        Self {
            component: component.into(),
        }
    }

    /// Record a change event
    pub fn record_change(&self, index: &str, operation: &str) {
        counter!(
            names::WATCHER_CHANGES_TOTAL,
            labels::COMPONENT => self.component.clone(),
            labels::INDEX => index.to_string(),
            labels::OPERATION => operation.to_string(),
        )
        .increment(1);
    }

    /// Record publish latency
    pub fn record_publish_latency(&self, duration: Duration) {
        histogram!(
            names::WATCHER_PUBLISH_LATENCY,
            labels::COMPONENT => self.component.clone(),
        )
        .record(duration.as_secs_f64());
    }

    /// Update checkpoint lag
    pub fn set_checkpoint_lag(&self, lag: u64) {
        gauge!(
            names::WATCHER_CHECKPOINT_LAG,
            labels::COMPONENT => self.component.clone(),
        )
        .set(lag as f64);
    }

    /// Record an error
    pub fn record_error(&self, error_type: &str) {
        counter!(
            names::WATCHER_ERRORS_TOTAL,
            labels::COMPONENT => self.component.clone(),
            labels::ERROR_TYPE => error_type.to_string(),
        )
        .increment(1);
    }

    /// Record poll duration
    pub fn record_poll_duration(&self, duration: Duration) {
        histogram!(
            names::WATCHER_POLL_DURATION,
            labels::COMPONENT => self.component.clone(),
        )
        .record(duration.as_secs_f64());
    }
}

/// Gateway metrics
#[derive(Clone)]
pub struct GatewayMetrics {
    component: String,
}

impl GatewayMetrics {
    pub fn new(component: impl Into<String>) -> Self {
        Self {
            component: component.into(),
        }
    }

    /// Record message received
    pub fn record_received(&self, subject: &str) {
        counter!(
            names::GATEWAY_MESSAGES_RECEIVED,
            labels::COMPONENT => self.component.clone(),
        )
        .increment(1);
    }

    /// Record message filtered
    pub fn record_filtered(&self, filter: &str) {
        counter!(
            names::GATEWAY_MESSAGES_FILTERED,
            labels::COMPONENT => self.component.clone(),
            labels::FILTER => filter.to_string(),
        )
        .increment(1);
    }

    /// Record message forwarded
    pub fn record_forwarded(&self) {
        counter!(
            names::GATEWAY_MESSAGES_FORWARDED,
            labels::COMPONENT => self.component.clone(),
        )
        .increment(1);
    }

    /// Record filter latency
    pub fn record_filter_latency(&self, duration: Duration) {
        histogram!(
            names::GATEWAY_FILTER_LATENCY,
            labels::COMPONENT => self.component.clone(),
        )
        .record(duration.as_secs_f64());
    }
}

/// Writer metrics
#[derive(Clone)]
pub struct WriterMetrics {
    component: String,
}

impl WriterMetrics {
    pub fn new(component: impl Into<String>) -> Self {
        Self {
            component: component.into(),
        }
    }

    /// Record bulk request
    pub fn record_bulk_request(&self, status: &str) {
        counter!(
            names::WRITER_BULK_REQUESTS,
            labels::COMPONENT => self.component.clone(),
            labels::STATUS => status.to_string(),
        )
        .increment(1);
    }

    /// Record documents indexed
    pub fn record_docs_indexed(&self, count: u64, index: &str) {
        counter!(
            names::WRITER_DOCS_INDEXED,
            labels::COMPONENT => self.component.clone(),
            labels::INDEX => index.to_string(),
        )
        .increment(count);
    }

    /// Record documents failed
    pub fn record_docs_failed(&self, count: u64, index: &str) {
        counter!(
            names::WRITER_DOCS_FAILED,
            labels::COMPONENT => self.component.clone(),
            labels::INDEX => index.to_string(),
        )
        .increment(count);
    }

    /// Record bulk latency
    pub fn record_bulk_latency(&self, duration: Duration) {
        histogram!(
            names::WRITER_BULK_LATENCY,
            labels::COMPONENT => self.component.clone(),
        )
        .record(duration.as_secs_f64());
    }

    /// Record retry
    pub fn record_retry(&self) {
        counter!(
            names::WRITER_RETRY_COUNT,
            labels::COMPONENT => self.component.clone(),
        )
        .increment(1);
    }

    /// Set DLQ size
    pub fn set_dlq_size(&self, size: usize) {
        gauge!(
            names::WRITER_DLQ_SIZE,
            labels::COMPONENT => self.component.clone(),
        )
        .set(size as f64);
    }

    /// Set buffer size
    pub fn set_buffer_size(&self, size: usize) {
        gauge!(
            names::WRITER_BUFFER_SIZE,
            labels::COMPONENT => self.component.clone(),
        )
        .set(size as f64);
    }
}

/// Connection metrics
#[derive(Clone)]
pub struct ConnectionMetrics {
    connection_name: String,
}

impl ConnectionMetrics {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            connection_name: name.into(),
        }
    }

    /// Set connection state (1 = connected, 0 = disconnected)
    pub fn set_connected(&self, connected: bool) {
        gauge!(
            names::CONNECTION_STATE,
            labels::CONNECTION => self.connection_name.clone(),
        )
        .set(if connected { 1.0 } else { 0.0 });
    }

    /// Record reconnection
    pub fn record_reconnect(&self) {
        counter!(
            names::CONNECTION_RECONNECTS,
            labels::CONNECTION => self.connection_name.clone(),
        )
        .increment(1);
    }
}

/// Timer guard for automatic latency recording
pub struct LatencyTimer<F>
where
    F: FnOnce(Duration),
{
    start: Instant,
    on_drop: Option<F>,
}

impl<F> LatencyTimer<F>
where
    F: FnOnce(Duration),
{
    /// Start a new timer
    pub fn start(on_drop: F) -> Self {
        Self {
            start: Instant::now(),
            on_drop: Some(on_drop),
        }
    }

    /// Get elapsed time without stopping
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Stop timer and record
    pub fn stop(mut self) -> Duration {
        let elapsed = self.start.elapsed();
        if let Some(f) = self.on_drop.take() {
            f(elapsed);
        }
        elapsed
    }
}

impl<F> Drop for LatencyTimer<F>
where
    F: FnOnce(Duration),
{
    fn drop(&mut self) {
        if let Some(f) = self.on_drop.take() {
            f(self.start.elapsed());
        }
    }
}

/// Convenience macro for timing operations
#[macro_export]
macro_rules! time_operation {
    ($metrics:expr, $method:ident, $op:expr) => {{
        let _timer = $crate::metrics::LatencyTimer::start(|d| $metrics.$method(d));
        $op
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_timer() {
        let mut recorded = None;
        {
            let timer = LatencyTimer::start(|d| recorded = Some(d));
            std::thread::sleep(Duration::from_millis(10));
            timer.stop();
        }
        assert!(recorded.is_some());
        assert!(recorded.unwrap() >= Duration::from_millis(10));
    }
}
