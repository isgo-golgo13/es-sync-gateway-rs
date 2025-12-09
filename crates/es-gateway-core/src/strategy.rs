//! Strategy Pattern Traits for ES Sync Gateway
//!
//! This module defines the core abstractions that enable pluggable implementations
//! across the entire pipeline. Each trait represents a strategy that can be swapped
//! at runtime or compile time.
//!
//! ## Design Philosophy
//!
//! - **Async-first**: All I/O operations are async
//! - **Stream-based**: Data flows as async streams for backpressure
//! - **Composable**: Strategies can be chained and combined
//! - **Observable**: Built-in hooks for metrics and tracing
//!
//! ## Strategy Hierarchy
//!
//! ```text
//! Lifecycle (start/stop)
//!     │
//!     ├── ChangeSource (produces changes)
//!     │       └── ElasticsearchSource, WebhookSource, ...
//!     │
//!     ├── MessageSource (consumes from transport)
//!     │       └── NatsSource, KafkaSource, ...
//!     │
//!     ├── MessageSink (publishes to transport)
//!     │       └── NatsSink, KafkaSink, ...
//!     │
//!     └── Writer (writes to target)
//!             └── ElasticsearchWriter, StdoutWriter, ...
//! ```

use crate::error::Result;
use crate::message::{Envelope, EnvelopeBatch};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for boxed async streams of envelopes
pub type EnvelopeStream = Pin<Box<dyn Stream<Item = Result<Envelope>> + Send>>;

/// Type alias for boxed async streams of batches
pub type BatchStream = Pin<Box<dyn Stream<Item = Result<EnvelopeBatch>> + Send>>;

// ============================================================================
// Lifecycle Management
// ============================================================================

/// Lifecycle management for components
///
/// Provides graceful startup and shutdown semantics.
#[async_trait]
pub trait Lifecycle: Send + Sync {
    /// Start the component
    ///
    /// Called once before any operations. Should establish connections,
    /// initialize state, and prepare for operation.
    async fn start(&self) -> Result<()>;

    /// Stop the component gracefully
    ///
    /// Called during shutdown. Should flush buffers, close connections,
    /// and release resources. May be called even if start() failed.
    async fn stop(&self) -> Result<()>;

    /// Check if the component is running
    fn is_running(&self) -> bool;
}

/// Health check capability
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Perform health check
    ///
    /// Returns Ok(()) if healthy, Err with details if not.
    async fn health_check(&self) -> Result<()>;

    /// Get component name for health reporting
    fn component_name(&self) -> &'static str;
}

// ============================================================================
// Source Strategies
// ============================================================================

/// Change source strategy - detects changes from Elasticsearch
///
/// Implementations:
/// - `PollingSource`: Periodic polling with sequence tracking
/// - `ChangeStreamSource`: ES _changes API (if available)
/// - `WebhookSource`: Receives changes via HTTP webhook
/// - `DualWriteSource`: Intercepts from dual-write proxy
#[async_trait]
pub trait ChangeSource: Lifecycle + HealthCheck {
    /// Stream changes from the source
    ///
    /// Returns a stream that yields change envelopes. The stream should
    /// handle reconnection and checkpoint resumption internally.
    async fn changes(&self) -> Result<EnvelopeStream>;

    /// Get current checkpoint position
    ///
    /// Returns the last successfully processed sequence number.
    async fn checkpoint(&self) -> Result<u64>;

    /// Commit checkpoint after successful processing
    ///
    /// Called after downstream has acknowledged the message.
    async fn commit(&self, sequence: u64) -> Result<()>;

    /// Seek to a specific position
    ///
    /// Used for replay or recovery scenarios.
    async fn seek(&self, sequence: u64) -> Result<()>;
}

/// Message source strategy - consumes from message transport
///
/// Implementations:
/// - `NatsJetStreamSource`: Consumes from NATS JetStream
/// - `KafkaSource`: Consumes from Kafka
/// - `DirectSource`: Direct memory channel (for testing)
#[async_trait]
pub trait MessageSource: Lifecycle + HealthCheck {
    /// Stream messages from the transport
    ///
    /// Returns a stream of envelopes. Acknowledgment is handled
    /// via the `ack` method after successful processing.
    async fn messages(&self) -> Result<EnvelopeStream>;

    /// Stream messages in batches
    ///
    /// More efficient for bulk operations. Batch size is determined
    /// by the implementation's configuration.
    async fn batches(&self) -> Result<BatchStream>;

    /// Acknowledge successful processing
    ///
    /// Called after the message has been successfully written to target.
    async fn ack(&self, envelope: &Envelope) -> Result<()>;

    /// Negative acknowledge - request redelivery
    ///
    /// Called when processing fails but may succeed on retry.
    async fn nack(&self, envelope: &Envelope) -> Result<()>;

    /// Get pending message count (if available)
    async fn pending_count(&self) -> Result<Option<u64>>;
}

// ============================================================================
// Sink Strategies
// ============================================================================

/// Message sink strategy - publishes to message transport
///
/// Implementations:
/// - `NatsJetStreamSink`: Publishes to NATS JetStream
/// - `KafkaSink`: Publishes to Kafka
/// - `DirectSink`: Direct memory channel (for testing)
#[async_trait]
pub trait MessageSink: Lifecycle + HealthCheck {
    /// Publish a single envelope
    ///
    /// Returns after the message is durably stored (for JetStream)
    /// or sent (for fire-and-forget transports).
    async fn publish(&self, envelope: Envelope) -> Result<()>;

    /// Publish a batch of envelopes
    ///
    /// More efficient than individual publishes. May use pipelining
    /// or batch APIs depending on the transport.
    async fn publish_batch(&self, batch: EnvelopeBatch) -> Result<()>;

    /// Flush any buffered messages
    ///
    /// Ensures all pending publishes are completed.
    async fn flush(&self) -> Result<()>;
}

// ============================================================================
// Writer Strategies
// ============================================================================

/// Write result for tracking success/failure
#[derive(Debug, Clone)]
pub struct WriteResult {
    /// Number of documents successfully written
    pub success_count: usize,
    /// Number of documents that failed
    pub failure_count: usize,
    /// Failed document IDs with error messages
    pub failures: Vec<(String, String)>,
    /// Time taken for the write operation
    pub duration_ms: u64,
}

impl WriteResult {
    /// Create a fully successful result
    pub fn success(count: usize, duration_ms: u64) -> Self {
        Self {
            success_count: count,
            failure_count: 0,
            failures: Vec::new(),
            duration_ms,
        }
    }

    /// Check if all writes succeeded
    pub fn is_complete_success(&self) -> bool {
        self.failure_count == 0
    }

    /// Check if any writes succeeded
    pub fn has_successes(&self) -> bool {
        self.success_count > 0
    }
}

/// Writer strategy - writes to target Elasticsearch
///
/// Implementations:
/// - `BulkWriter`: Uses ES _bulk API for efficient batch writes
/// - `SingleDocWriter`: Individual document writes (for debugging)
/// - `BufferedWriter`: Accumulates and flushes periodically
/// - `StdoutWriter`: Writes to stdout (for debugging)
#[async_trait]
pub trait Writer: Lifecycle + HealthCheck {
    /// Write a single envelope to the target
    async fn write(&self, envelope: &Envelope) -> Result<WriteResult>;

    /// Write a batch of envelopes to the target
    ///
    /// Uses the ES _bulk API for efficiency. Partial failures are
    /// reported in the WriteResult.
    async fn write_batch(&self, batch: &EnvelopeBatch) -> Result<WriteResult>;

    /// Flush any buffered writes
    async fn flush(&self) -> Result<()>;

    /// Get current buffer size (if buffered)
    fn buffer_size(&self) -> usize {
        0
    }
}

// ============================================================================
// Composite Strategies
// ============================================================================

/// Strategy that combines multiple strategies
#[async_trait]
pub trait CompositeStrategy<S>: Send + Sync {
    /// Add a strategy to the composite
    fn add(&mut self, strategy: S);

    /// Remove a strategy by index
    fn remove(&mut self, index: usize) -> Option<S>;

    /// Get all strategies
    fn strategies(&self) -> &[S];
}

/// Fan-out sink that publishes to multiple sinks
pub struct FanOutSink {
    sinks: Vec<Arc<dyn MessageSink>>,
}

impl FanOutSink {
    pub fn new() -> Self {
        Self { sinks: Vec::new() }
    }

    pub fn add_sink(&mut self, sink: Arc<dyn MessageSink>) {
        self.sinks.push(sink);
    }
}

impl Default for FanOutSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Lifecycle for FanOutSink {
    async fn start(&self) -> Result<()> {
        for sink in &self.sinks {
            sink.start().await?;
        }
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        for sink in &self.sinks {
            sink.stop().await?;
        }
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.sinks.iter().all(|s| s.is_running())
    }
}

#[async_trait]
impl HealthCheck for FanOutSink {
    async fn health_check(&self) -> Result<()> {
        for sink in &self.sinks {
            sink.health_check().await?;
        }
        Ok(())
    }

    fn component_name(&self) -> &'static str {
        "fan_out_sink"
    }
}

#[async_trait]
impl MessageSink for FanOutSink {
    async fn publish(&self, envelope: Envelope) -> Result<()> {
        for sink in &self.sinks {
            sink.publish(envelope.clone()).await?;
        }
        Ok(())
    }

    async fn publish_batch(&self, batch: EnvelopeBatch) -> Result<()> {
        for sink in &self.sinks {
            sink.publish_batch(batch.clone()).await?;
        }
        Ok(())
    }

    async fn flush(&self) -> Result<()> {
        for sink in &self.sinks {
            sink.flush().await?;
        }
        Ok(())
    }
}

// ============================================================================
// Strategy Factory
// ============================================================================

/// Factory trait for creating strategies from configuration
#[async_trait]
pub trait StrategyFactory<S, C>: Send + Sync {
    /// Create a strategy from configuration
    async fn create(&self, config: C) -> Result<S>;
}

/// Marker trait for strategies that support dynamic dispatch
pub trait DynStrategy: Send + Sync + 'static {}

impl<T: Lifecycle + HealthCheck + 'static> DynStrategy for T {}
