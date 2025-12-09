//! # ES Writer
//!
//! Consumes from NATS JetStream and writes to Elasticsearch using bulk API.
//!
//! ## Strategies
//!
//! - `BulkWriter`: Batched bulk writes with retry
//! - `BufferedWriter`: Time/size triggered batching
//!
//! ## Features
//!
//! - Automatic batching with configurable thresholds
//! - Exponential backoff retry with circuit breaker
//! - Dead letter queue for failed documents
//! - Exactly-once semantics via idempotent writes

pub mod batcher;
pub mod bulk_writer;
pub mod consumer;
pub mod dlq;
pub mod retry;

pub use batcher::*;
pub use bulk_writer::*;
pub use consumer::*;
pub use dlq::*;
pub use retry::*;

use es_gateway_core::prelude::*;
use es_gateway_core::WriterMetrics;
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Main writer orchestrator
pub struct EsWriter<S, W>
where
    S: MessageSource,
    W: Writer,
{
    source: Arc<S>,
    writer: Arc<W>,
    batcher: Batcher,
    dlq: Option<Arc<DeadLetterQueue>>,
    metrics: WriterMetrics,
    running: AtomicBool,
    processed: AtomicU64,
    failed: AtomicU64,
}

impl<S, W> EsWriter<S, W>
where
    S: MessageSource + 'static,
    W: Writer + 'static,
{
    /// Create new writer
    pub fn new(source: S, writer: W, config: WriterOrchestratorConfig) -> Self {
        let dlq = if config.dlq_enabled {
            Some(Arc::new(DeadLetterQueue::new(&config.dlq_path, config.dlq_max_size)))
        } else {
            None
        };

        Self {
            source: Arc::new(source),
            writer: Arc::new(writer),
            batcher: Batcher::new(config.batch_size, config.batch_max_bytes, config.batch_timeout),
            dlq,
            metrics: WriterMetrics::new("es_writer"),
            running: AtomicBool::new(false),
            processed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }
    }

    /// Run the writer loop
    pub async fn run(&self) -> Result<()> {
        info!("Starting ES writer");

        // Start components
        self.source.start().await?;
        self.writer.start().await?;
        self.running.store(true, Ordering::SeqCst);

        // Get message stream
        let mut stream = self.source.messages().await?;

        // Batch flush timer
        let mut flush_interval = interval(Duration::from_millis(100));

        while self.running.load(Ordering::SeqCst) {
            tokio::select! {
                Some(result) = stream.next() => {
                    match result {
                        Ok(envelope) => {
                            self.batcher.add(envelope).await;

                            // Check if batch is ready
                            if self.batcher.should_flush().await {
                                self.flush_batch().await?;
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "Error in message stream");
                            self.metrics.record_docs_failed(1, "stream_error");
                        }
                    }
                }
                _ = flush_interval.tick() => {
                    // Time-based flush
                    if self.batcher.len().await > 0 {
                        self.flush_batch().await?;
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        // Final flush
        if self.batcher.len().await > 0 {
            self.flush_batch().await?;
        }

        self.writer.flush().await?;
        self.writer.stop().await?;
        self.source.stop().await?;

        info!(
            processed = self.processed.load(Ordering::Relaxed),
            failed = self.failed.load(Ordering::Relaxed),
            "ES writer stopped"
        );
        Ok(())
    }

    /// Flush current batch
    async fn flush_batch(&self) -> Result<()> {
        let batch = self.batcher.drain().await;
        if batch.is_empty() {
            return Ok(());
        }

        let count = batch.len();
        debug!(count, "Flushing batch");

        match self.writer.write_batch(&batch).await {
            Ok(result) => {
                self.processed.fetch_add(result.success_count as u64, Ordering::Relaxed);
                self.failed.fetch_add(result.failure_count as u64, Ordering::Relaxed);

                self.metrics.record_docs_indexed(result.success_count as u64, "batch");
                self.metrics.record_bulk_latency(Duration::from_millis(result.duration_ms));

                if result.failure_count > 0 {
                    self.metrics.record_docs_failed(result.failure_count as u64, "bulk_error");

                    // Send failures to DLQ
                    if let Some(ref dlq) = self.dlq {
                        for (doc_id, error) in &result.failures {
                            warn!(doc_id, error, "Document failed, sending to DLQ");
                            // Find envelope and add to DLQ
                            for envelope in batch.iter() {
                                if envelope.event.doc_id == *doc_id {
                                    dlq.add(envelope.clone(), error.clone()).await?;
                                    break;
                                }
                            }
                        }
                        self.metrics.set_dlq_size(dlq.len().await);
                    }
                }

                // Acknowledge all messages
                for envelope in batch.iter() {
                    if let Err(e) = self.source.ack(envelope).await {
                        warn!(error = %e, "Failed to ack message");
                    }
                }

                Ok(())
            }
            Err(e) => {
                error!(error = %e, count, "Batch write failed");
                self.metrics.record_docs_failed(count as u64, "batch_error");

                // Nack all for retry
                for envelope in batch.iter() {
                    if let Err(e) = self.source.nack(envelope).await {
                        warn!(error = %e, "Failed to nack message");
                    }
                }

                Err(e)
            }
        }
    }

    /// Stop the writer
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get processed count
    pub fn processed_count(&self) -> u64 {
        self.processed.load(Ordering::Relaxed)
    }

    /// Get failed count
    pub fn failed_count(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }
}

/// Writer orchestrator configuration
#[derive(Debug, Clone)]
pub struct WriterOrchestratorConfig {
    pub batch_size: usize,
    pub batch_max_bytes: usize,
    pub batch_timeout: Duration,
    pub dlq_enabled: bool,
    pub dlq_path: String,
    pub dlq_max_size: usize,
}

impl Default for WriterOrchestratorConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            batch_max_bytes: 5 * 1024 * 1024,
            batch_timeout: Duration::from_millis(100),
            dlq_enabled: true,
            dlq_path: "/data/dlq".to_string(),
            dlq_max_size: 100_000,
        }
    }
}
