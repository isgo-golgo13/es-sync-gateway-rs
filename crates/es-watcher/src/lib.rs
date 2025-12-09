//! # ES Watcher
//!
//! Watches Elasticsearch for changes and publishes to NATS JetStream.
//!
//! ## Strategies
//!
//! - `PollingSource`: Polls ES with sequence tracking
//! - `ScrollSource`: Uses scroll API for initial sync
//!
//! ## Usage
//!
//! ```rust,ignore
//! let source = PollingSource::new(config);
//! let sink = NatsJetStreamSink::new(nats_config);
//!
//! let watcher = Watcher::new(source, sink);
//! watcher.run().await?;
//! ```

pub mod checkpoint;
pub mod es_client;
pub mod publisher;
pub mod source;

pub use checkpoint::*;
pub use es_client::*;
pub use publisher::*;
pub use source::*;

use es_gateway_core::prelude::*;
use es_gateway_core::{EnvelopeStream, WatcherMetrics};
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// Main watcher orchestrator
pub struct Watcher<S, P>
where
    S: ChangeSource,
    P: MessageSink,
{
    source: Arc<S>,
    sink: Arc<P>,
    metrics: WatcherMetrics,
    running: AtomicBool,
    processed: AtomicU64,
}

impl<S, P> Watcher<S, P>
where
    S: ChangeSource + 'static,
    P: MessageSink + 'static,
{
    /// Create new watcher
    pub fn new(source: S, sink: P) -> Self {
        Self {
            source: Arc::new(source),
            sink: Arc::new(sink),
            metrics: WatcherMetrics::new("es_watcher"),
            running: AtomicBool::new(false),
            processed: AtomicU64::new(0),
        }
    }

    /// Run the watcher loop
    pub async fn run(&self) -> Result<()> {
        info!("Starting ES watcher");

        // Start components
        self.source.start().await?;
        self.sink.start().await?;
        self.running.store(true, Ordering::SeqCst);

        // Get change stream
        let mut stream = self.source.changes().await?;

        // Process changes
        while self.running.load(Ordering::SeqCst) {
            tokio::select! {
                Some(result) = stream.next() => {
                    match result {
                        Ok(envelope) => {
                            let seq = envelope.metadata.sequence;
                            let index = envelope.event.index.clone();
                            let op = envelope.event.operation.to_string();

                            // Publish to sink
                            if let Err(e) = self.sink.publish(envelope).await {
                                error!(error = %e, "Failed to publish envelope");
                                self.metrics.record_error("publish_failed");
                                continue;
                            }

                            // Commit checkpoint
                            if let Err(e) = self.source.commit(seq).await {
                                warn!(error = %e, seq, "Failed to commit checkpoint");
                            }

                            // Record metrics
                            self.metrics.record_change(&index, &op);
                            self.processed.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            error!(error = %e, "Error in change stream");
                            self.metrics.record_error("stream_error");
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        // Cleanup
        self.sink.flush().await?;
        self.sink.stop().await?;
        self.source.stop().await?;

        info!(
            processed = self.processed.load(Ordering::Relaxed),
            "ES watcher stopped"
        );
        Ok(())
    }

    /// Stop the watcher
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
}
