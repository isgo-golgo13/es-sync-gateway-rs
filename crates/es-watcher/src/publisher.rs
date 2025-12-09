//! Message publisher strategies
//!
//! Implements MessageSink for publishing to NATS JetStream.

use async_nats::jetstream::{self, context::PublishAckFuture, Context};
use async_trait::async_trait;
use es_gateway_core::prelude::*;
use es_gateway_core::WatcherMetrics;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

/// NATS JetStream sink configuration
#[derive(Debug, Clone)]
pub struct NatsJetStreamSinkConfig {
    /// NATS server URL
    pub url: String,
    /// Stream name
    pub stream: String,
    /// Subject prefix (e.g., "es")
    pub subject_prefix: String,
    /// Tenant for subject generation
    pub tenant: String,
    /// Connection name
    pub connection_name: String,
    /// Max pending publishes before backpressure
    pub max_pending: usize,
}

impl Default for NatsJetStreamSinkConfig {
    fn default() -> Self {
        Self {
            url: "nats://localhost:4222".to_string(),
            stream: "ES_CHANGES".to_string(),
            subject_prefix: "es".to_string(),
            tenant: "default".to_string(),
            connection_name: "es-watcher".to_string(),
            max_pending: 10000,
        }
    }
}

/// NATS JetStream message sink
pub struct NatsJetStreamSink {
    config: NatsJetStreamSinkConfig,
    client: RwLock<Option<async_nats::Client>>,
    jetstream: RwLock<Option<Context>>,
    metrics: WatcherMetrics,
    running: AtomicBool,
    published: AtomicU64,
}

impl NatsJetStreamSink {
    /// Create new NATS JetStream sink
    pub fn new(config: NatsJetStreamSinkConfig) -> Self {
        Self {
            config,
            client: RwLock::new(None),
            jetstream: RwLock::new(None),
            metrics: WatcherMetrics::new("nats_sink"),
            running: AtomicBool::new(false),
            published: AtomicU64::new(0),
        }
    }

    /// Get published count
    pub fn published_count(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    /// Ensure stream exists
    async fn ensure_stream(&self, js: &Context) -> Result<()> {
        let stream_name = &self.config.stream;
        let subjects = format!("{}.>", self.config.subject_prefix);

        match js.get_stream(stream_name).await {
            Ok(_) => {
                debug!(stream = stream_name, "Stream exists");
                Ok(())
            }
            Err(_) => {
                // Create stream
                let config = jetstream::stream::Config {
                    name: stream_name.clone(),
                    subjects: vec![subjects],
                    retention: jetstream::stream::RetentionPolicy::Limits,
                    max_age: Duration::from_secs(24 * 60 * 60), // 24 hours
                    storage: jetstream::stream::StorageType::File,
                    ..Default::default()
                };

                js.create_stream(config).await.map_err(|e| {
                    GatewayError::nats_with_source(format!("Failed to create stream: {}", e), e)
                })?;

                info!(stream = stream_name, "Created stream");
                Ok(())
            }
        }
    }
}

#[async_trait]
impl Lifecycle for NatsJetStreamSink {
    async fn start(&self) -> Result<()> {
        info!(url = %self.config.url, "Connecting to NATS");

        let client = async_nats::ConnectOptions::new()
            .name(&self.config.connection_name)
            .connect(&self.config.url)
            .await
            .map_err(|e| GatewayError::nats_with_source("Failed to connect", e))?;

        let js = jetstream::new(client.clone());

        // Ensure stream exists
        self.ensure_stream(&js).await?;

        *self.client.write().await = Some(client);
        *self.jetstream.write().await = Some(js);
        self.running.store(true, Ordering::SeqCst);

        info!("NATS JetStream sink started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);

        // Flush before closing
        if let Some(client) = self.client.read().await.as_ref() {
            client.flush().await.map_err(|e| {
                GatewayError::nats_with_source("Failed to flush", e)
            })?;
        }

        *self.jetstream.write().await = None;
        *self.client.write().await = None;

        info!(
            published = self.published.load(Ordering::Relaxed),
            "NATS JetStream sink stopped"
        );
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HealthCheck for NatsJetStreamSink {
    async fn health_check(&self) -> Result<()> {
        let client = self.client.read().await;
        match client.as_ref() {
            Some(c) if c.connection_state() == async_nats::connection::State::Connected => Ok(()),
            Some(_) => Err(GatewayError::nats("Not connected")),
            None => Err(GatewayError::nats("Client not initialized")),
        }
    }

    fn component_name(&self) -> &'static str {
        "nats_jetstream_sink"
    }
}

#[async_trait]
impl MessageSink for NatsJetStreamSink {
    async fn publish(&self, envelope: Envelope) -> Result<()> {
        let js = self.jetstream.read().await;
        let js = js
            .as_ref()
            .ok_or_else(|| GatewayError::nats("JetStream not initialized"))?;

        let subject = envelope.subject(&self.config.tenant);
        let payload = envelope.to_bytes().map_err(|e| {
            GatewayError::Serialization {
                message: format!("Failed to serialize envelope: {}", e),
                source: Some(Box::new(e)),
            }
        })?;

        let start = Instant::now();

        let ack = js
            .publish(subject.clone(), payload)
            .await
            .map_err(|e| GatewayError::nats_with_source("Publish failed", e))?;

        // Wait for acknowledgment
        ack.await
            .map_err(|e| GatewayError::nats_with_source("Ack failed", e))?;

        self.metrics.record_publish_latency(start.elapsed());
        self.published.fetch_add(1, Ordering::Relaxed);

        trace!(subject, seq = envelope.metadata.sequence, "Published message");
        Ok(())
    }

    async fn publish_batch(&self, batch: EnvelopeBatch) -> Result<()> {
        let js = self.jetstream.read().await;
        let js = js
            .as_ref()
            .ok_or_else(|| GatewayError::nats("JetStream not initialized"))?;

        let start = Instant::now();
        let mut acks: Vec<PublishAckFuture> = Vec::with_capacity(batch.len());

        // Publish all messages
        for envelope in batch.iter() {
            let subject = envelope.subject(&self.config.tenant);
            let payload = envelope.to_bytes().map_err(|e| {
                GatewayError::Serialization {
                    message: format!("Failed to serialize envelope: {}", e),
                    source: Some(Box::new(e)),
                }
            })?;

            let ack = js
                .publish(subject, payload)
                .await
                .map_err(|e| GatewayError::nats_with_source("Publish failed", e))?;

            acks.push(ack);
        }

        // Wait for all acknowledgments
        for ack in acks {
            ack.await
                .map_err(|e| GatewayError::nats_with_source("Ack failed", e))?;
        }

        let count = batch.len();
        self.metrics.record_publish_latency(start.elapsed());
        self.published.fetch_add(count as u64, Ordering::Relaxed);

        debug!(count, duration_ms = ?start.elapsed().as_millis(), "Published batch");
        Ok(())
    }

    async fn flush(&self) -> Result<()> {
        if let Some(client) = self.client.read().await.as_ref() {
            client
                .flush()
                .await
                .map_err(|e| GatewayError::nats_with_source("Flush failed", e))?;
        }
        Ok(())
    }
}

// ============================================================================
// Mock Sink (for testing)
// ============================================================================

/// Mock message sink for testing
pub struct MockSink {
    messages: RwLock<Vec<Envelope>>,
    running: AtomicBool,
}

impl MockSink {
    pub fn new() -> Self {
        Self {
            messages: RwLock::new(Vec::new()),
            running: AtomicBool::new(false),
        }
    }

    /// Get all published messages
    pub async fn messages(&self) -> Vec<Envelope> {
        self.messages.read().await.clone()
    }

    /// Clear messages
    pub async fn clear(&self) {
        self.messages.write().await.clear();
    }
}

impl Default for MockSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Lifecycle for MockSink {
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
impl HealthCheck for MockSink {
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    fn component_name(&self) -> &'static str {
        "mock_sink"
    }
}

#[async_trait]
impl MessageSink for MockSink {
    async fn publish(&self, envelope: Envelope) -> Result<()> {
        self.messages.write().await.push(envelope);
        Ok(())
    }

    async fn publish_batch(&self, batch: EnvelopeBatch) -> Result<()> {
        self.messages.write().await.extend(batch.into_iter());
        Ok(())
    }

    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_sink() {
        let sink = MockSink::new();
        sink.start().await.unwrap();

        let event = ChangeEvent::index("test", "doc1", serde_json::json!({}));
        let envelope = Envelope::wrap(event, "test", 1);

        sink.publish(envelope).await.unwrap();

        let messages = sink.messages().await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].event.doc_id, "doc1");
    }
}
