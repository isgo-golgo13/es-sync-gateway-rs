//! NATS JetStream consumer for ES Writer
//!
//! Implements MessageSource strategy for consuming from JetStream.

use async_nats::jetstream::{self, consumer::PullConsumer, Context};
use async_trait::async_trait;
use es_gateway_core::prelude::*;
use es_gateway_core::{BatchStream, EnvelopeStream};
use futures::{stream, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

/// NATS JetStream source configuration
#[derive(Debug, Clone)]
pub struct NatsJetStreamSourceConfig {
    /// NATS server URL
    pub url: String,
    /// Stream name
    pub stream: String,
    /// Consumer name
    pub consumer: String,
    /// Filter subjects
    pub filter_subjects: Vec<String>,
    /// Max pending acknowledgements
    pub max_ack_pending: u64,
    /// Ack wait duration
    pub ack_wait: Duration,
    /// Batch size for pull
    pub batch_size: usize,
    /// Connection name
    pub connection_name: String,
}

impl Default for NatsJetStreamSourceConfig {
    fn default() -> Self {
        Self {
            url: "nats://localhost:4222".to_string(),
            stream: "ES_CHANGES".to_string(),
            consumer: "es-writer".to_string(),
            filter_subjects: Vec::new(),
            max_ack_pending: 10000,
            ack_wait: Duration::from_secs(30),
            batch_size: 100,
            connection_name: "es-writer".to_string(),
        }
    }
}

/// NATS JetStream message source
pub struct NatsJetStreamSource {
    config: NatsJetStreamSourceConfig,
    client: RwLock<Option<async_nats::Client>>,
    consumer: RwLock<Option<PullConsumer>>,
    running: AtomicBool,
}

impl NatsJetStreamSource {
    /// Create new source
    pub fn new(config: NatsJetStreamSourceConfig) -> Self {
        Self {
            config,
            client: RwLock::new(None),
            consumer: RwLock::new(None),
            running: AtomicBool::new(false),
        }
    }

    /// Get or create consumer
    async fn ensure_consumer(&self, js: &Context) -> Result<PullConsumer> {
        let stream = js
            .get_stream(&self.config.stream)
            .await
            .map_err(|e| GatewayError::nats_with_source("Stream not found", e))?;

        // Try to get existing consumer
        match stream.get_consumer(&self.config.consumer).await {
            Ok(consumer) => {
                debug!(consumer = %self.config.consumer, "Using existing consumer");
                Ok(consumer)
            }
            Err(_) => {
                // Create new consumer
                let mut config = jetstream::consumer::pull::Config {
                    durable_name: Some(self.config.consumer.clone()),
                    ack_policy: jetstream::consumer::AckPolicy::Explicit,
                    ack_wait: self.config.ack_wait,
                    max_ack_pending: self.config.max_ack_pending as i64,
                    ..Default::default()
                };

                // Add filter subjects if specified
                if !self.config.filter_subjects.is_empty() {
                    config.filter_subjects = self.config.filter_subjects.clone();
                }

                let consumer = stream
                    .create_consumer(config)
                    .await
                    .map_err(|e| GatewayError::nats_with_source("Failed to create consumer", e))?;

                info!(consumer = %self.config.consumer, "Created consumer");
                Ok(consumer)
            }
        }
    }
}

#[async_trait]
impl Lifecycle for NatsJetStreamSource {
    async fn start(&self) -> Result<()> {
        info!(url = %self.config.url, "Connecting to NATS");

        let client = async_nats::ConnectOptions::new()
            .name(&self.config.connection_name)
            .connect(&self.config.url)
            .await
            .map_err(|e| GatewayError::nats_with_source("Failed to connect", e))?;

        let js = jetstream::new(client.clone());
        let consumer = self.ensure_consumer(&js).await?;

        *self.client.write().await = Some(client);
        *self.consumer.write().await = Some(consumer);
        self.running.store(true, Ordering::SeqCst);

        info!("NATS JetStream source started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        *self.consumer.write().await = None;
        *self.client.write().await = None;

        info!("NATS JetStream source stopped");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HealthCheck for NatsJetStreamSource {
    async fn health_check(&self) -> Result<()> {
        let client = self.client.read().await;
        match client.as_ref() {
            Some(c) if c.connection_state() == async_nats::connection::State::Connected => Ok(()),
            Some(_) => Err(GatewayError::nats("Not connected")),
            None => Err(GatewayError::nats("Client not initialized")),
        }
    }

    fn component_name(&self) -> &'static str {
        "nats_jetstream_source"
    }
}

#[async_trait]
impl MessageSource for NatsJetStreamSource {
    async fn messages(&self) -> Result<EnvelopeStream> {
        let consumer = self.consumer.read().await;
        let consumer = consumer
            .as_ref()
            .ok_or_else(|| GatewayError::nats("Consumer not initialized"))?
            .clone();

        let batch_size = self.config.batch_size;

        // Create message stream
        let stream = async_stream::stream! {
            loop {
                match consumer.fetch().max_messages(batch_size).messages().await {
                    Ok(mut messages) => {
                        while let Some(msg_result) = messages.next().await {
                            match msg_result {
                                Ok(msg) => {
                                    // Parse envelope
                                    match Envelope::from_bytes(&msg.payload) {
                                        Ok(mut envelope) => {
                                            // Store message for ack/nack
                                            // In real impl, we'd track this
                                            yield Ok(envelope);
                                        }
                                        Err(e) => {
                                            error!(error = %e, "Failed to parse message");
                                            // Ack bad message to prevent redelivery
                                            let _ = msg.ack().await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!(error = %e, "Error fetching message");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Error fetching batch");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn batches(&self) -> Result<BatchStream> {
        let consumer = self.consumer.read().await;
        let consumer = consumer
            .as_ref()
            .ok_or_else(|| GatewayError::nats("Consumer not initialized"))?
            .clone();

        let batch_size = self.config.batch_size;

        let stream = async_stream::stream! {
            loop {
                match consumer.fetch().max_messages(batch_size).messages().await {
                    Ok(mut messages) => {
                        let mut batch = EnvelopeBatch::with_capacity(batch_size);

                        while let Some(msg_result) = messages.next().await {
                            match msg_result {
                                Ok(msg) => {
                                    match Envelope::from_bytes(&msg.payload) {
                                        Ok(envelope) => {
                                            batch.push(envelope);
                                        }
                                        Err(e) => {
                                            error!(error = %e, "Failed to parse message");
                                            let _ = msg.ack().await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!(error = %e, "Error fetching message");
                                }
                            }
                        }

                        if !batch.is_empty() {
                            yield Ok(batch);
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Error fetching batch");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn ack(&self, envelope: &Envelope) -> Result<()> {
        // In a full implementation, we'd track the original message
        // and call msg.ack() here
        trace!(doc_id = %envelope.event.doc_id, "Acked");
        Ok(())
    }

    async fn nack(&self, envelope: &Envelope) -> Result<()> {
        // In a full implementation, we'd track the original message
        // and call msg.ack_with(AckKind::Nak) here
        trace!(doc_id = %envelope.event.doc_id, "Nacked");
        Ok(())
    }

    async fn pending_count(&self) -> Result<Option<u64>> {
        let consumer = self.consumer.read().await;
        if let Some(c) = consumer.as_ref() {
            match c.info().await {
                Ok(info) => Ok(Some(info.num_pending as u64)),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }
}
