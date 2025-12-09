//! Change source strategies for ES Watcher
//!
//! Implements the ChangeSource strategy trait with multiple backends:
//! - PollingSource: Polls ES using _search with sequence tracking
//! - MockSource: For testing

use async_trait::async_trait;
use es_gateway_core::prelude::*;
use es_gateway_core::{EnvelopeStream, WatcherMetrics};
use futures::{stream, Stream, StreamExt};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;
use tracing::{debug, error, info, trace, warn};

use crate::checkpoint::CheckpointStore;
use crate::es_client::EsClient;

// ============================================================================
// Polling Source Strategy
// ============================================================================

/// Configuration for polling source
#[derive(Debug, Clone)]
pub struct PollingSourceConfig {
    /// Index patterns to watch
    pub index_patterns: Vec<String>,
    /// Poll interval
    pub poll_interval: Duration,
    /// Batch size per poll
    pub batch_size: usize,
    /// Tenant for subject generation
    pub tenant: String,
    /// Source identifier
    pub source_id: String,
}

impl Default for PollingSourceConfig {
    fn default() -> Self {
        Self {
            index_patterns: vec!["*".to_string()],
            poll_interval: Duration::from_millis(100),
            batch_size: 1000,
            tenant: "default".to_string(),
            source_id: "es-watcher".to_string(),
        }
    }
}

/// Polling-based change source
///
/// Uses ES _search API with _seq_no tracking for change detection.
/// This is the most compatible approach, working with all ES versions.
pub struct PollingSource {
    client: Arc<EsClient>,
    checkpoint: Arc<dyn CheckpointStore>,
    config: PollingSourceConfig,
    metrics: WatcherMetrics,
    running: AtomicBool,
    current_seq: AtomicU64,
}

impl PollingSource {
    /// Create new polling source
    pub fn new(
        client: EsClient,
        checkpoint: Arc<dyn CheckpointStore>,
        config: PollingSourceConfig,
    ) -> Self {
        Self {
            client: Arc::new(client),
            checkpoint,
            config,
            metrics: WatcherMetrics::new("polling_source"),
            running: AtomicBool::new(false),
            current_seq: AtomicU64::new(0),
        }
    }

    /// Poll for changes since last checkpoint
    async fn poll_changes(&self) -> Result<Vec<Envelope>> {
        let start = std::time::Instant::now();
        let from_seq = self.current_seq.load(Ordering::SeqCst);

        let mut envelopes = Vec::new();

        for pattern in &self.config.index_patterns {
            let docs = self
                .client
                .fetch_changes(pattern, from_seq, self.config.batch_size)
                .await?;

            for (doc, seq_no, primary_term) in docs {
                let index = doc
                    .get("_index")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                let doc_id = doc
                    .get("_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                let source = doc.get("_source").cloned();

                let event = ChangeEvent {
                    operation: DocumentOperation::Index, // Polling can't distinguish update from index
                    index,
                    doc_id,
                    doc_type: "_doc".to_string(),
                    version: Some(seq_no),
                    routing: None,
                    pipeline: None,
                    source,
                };

                let metadata = Metadata::new(&self.config.source_id, seq_no)
                    .with_primary_term(primary_term);

                envelopes.push(Envelope::new(metadata, event));
            }
        }

        // Update max sequence seen
        if let Some(max_seq) = envelopes.iter().map(|e| e.metadata.sequence).max() {
            self.current_seq.store(max_seq, Ordering::SeqCst);
        }

        self.metrics.record_poll_duration(start.elapsed());
        trace!(count = envelopes.len(), "Polled changes");

        Ok(envelopes)
    }
}

#[async_trait]
impl Lifecycle for PollingSource {
    async fn start(&self) -> Result<()> {
        // Load checkpoint
        let seq = self.checkpoint.load().await?;
        self.current_seq.store(seq, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);

        info!(checkpoint = seq, "Polling source started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);

        // Persist final checkpoint
        let seq = self.current_seq.load(Ordering::SeqCst);
        self.checkpoint.save(seq).await?;

        info!(checkpoint = seq, "Polling source stopped");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HealthCheck for PollingSource {
    async fn health_check(&self) -> Result<()> {
        self.client.ping().await
    }

    fn component_name(&self) -> &'static str {
        "polling_source"
    }
}

#[async_trait]
impl ChangeSource for PollingSource {
    async fn changes(&self) -> Result<EnvelopeStream> {
        let client = self.client.clone();
        let checkpoint = self.checkpoint.clone();
        let config = self.config.clone();
        let metrics = self.metrics.clone();
        let current_seq = self.current_seq.load(Ordering::SeqCst);

        // Create polling stream
        let stream = stream::unfold(
            (client, checkpoint, config, metrics, current_seq, true),
            move |(client, checkpoint, config, metrics, mut seq, first)| async move {
                // Small delay between polls (unless first)
                if !first {
                    tokio::time::sleep(config.poll_interval).await;
                }

                let start = std::time::Instant::now();
                let mut envelopes = Vec::new();

                for pattern in &config.index_patterns {
                    match client.fetch_changes(pattern, seq, config.batch_size).await {
                        Ok(docs) => {
                            for (doc, seq_no, primary_term) in docs {
                                let index = doc
                                    .get("_index")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();

                                let doc_id = doc
                                    .get("_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();

                                let source = doc.get("_source").cloned();

                                let event = ChangeEvent {
                                    operation: DocumentOperation::Index,
                                    index,
                                    doc_id,
                                    doc_type: "_doc".to_string(),
                                    version: Some(seq_no),
                                    routing: None,
                                    pipeline: None,
                                    source,
                                };

                                let metadata = Metadata::new(&config.source_id, seq_no)
                                    .with_primary_term(primary_term);

                                envelopes.push(Ok(Envelope::new(metadata, event)));

                                if seq_no > seq {
                                    seq = seq_no;
                                }
                            }
                        }
                        Err(e) => {
                            envelopes.push(Err(e));
                        }
                    }
                }

                metrics.record_poll_duration(start.elapsed());

                if envelopes.is_empty() {
                    // No changes, continue polling
                    Some((
                        stream::empty().boxed(),
                        (client, checkpoint, config, metrics, seq, false),
                    ))
                } else {
                    Some((
                        stream::iter(envelopes).boxed(),
                        (client, checkpoint, config, metrics, seq, false),
                    ))
                }
            },
        )
        .flatten();

        Ok(Box::pin(stream))
    }

    async fn checkpoint(&self) -> Result<u64> {
        Ok(self.current_seq.load(Ordering::SeqCst))
    }

    async fn commit(&self, sequence: u64) -> Result<()> {
        self.checkpoint.save(sequence).await
    }

    async fn seek(&self, sequence: u64) -> Result<()> {
        self.current_seq.store(sequence, Ordering::SeqCst);
        self.checkpoint.save(sequence).await
    }
}

// ============================================================================
// Mock Source Strategy (for testing)
// ============================================================================

/// Mock change source for testing
pub struct MockSource {
    events: Arc<RwLock<Vec<Envelope>>>,
    checkpoint: Arc<RwLock<u64>>,
    running: AtomicBool,
}

impl MockSource {
    /// Create new mock source
    pub fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            checkpoint: Arc::new(RwLock::new(0)),
            running: AtomicBool::new(false),
        }
    }

    /// Add events to the mock
    pub async fn add_events(&self, events: Vec<Envelope>) {
        self.events.write().await.extend(events);
    }

    /// Add a single event
    pub async fn add_event(&self, event: Envelope) {
        self.events.write().await.push(event);
    }
}

impl Default for MockSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Lifecycle for MockSource {
    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HealthCheck for MockSource {
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    fn component_name(&self) -> &'static str {
        "mock_source"
    }
}

#[async_trait]
impl ChangeSource for MockSource {
    async fn changes(&self) -> Result<EnvelopeStream> {
        let events = self.events.read().await.clone();
        let stream = stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }

    async fn checkpoint(&self) -> Result<u64> {
        Ok(*self.checkpoint.read().await)
    }

    async fn commit(&self, sequence: u64) -> Result<()> {
        *self.checkpoint.write().await = sequence;
        Ok(())
    }

    async fn seek(&self, sequence: u64) -> Result<()> {
        *self.checkpoint.write().await = sequence;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_source() {
        let source = MockSource::new();

        let event = ChangeEvent::index("test", "doc1", serde_json::json!({"foo": "bar"}));
        let envelope = Envelope::wrap(event, "test", 1);

        source.add_event(envelope).await;
        source.start().await.unwrap();

        let mut stream = source.changes().await.unwrap();
        let result = stream.next().await.unwrap().unwrap();

        assert_eq!(result.event.doc_id, "doc1");
        assert_eq!(result.metadata.sequence, 1);
    }
}
