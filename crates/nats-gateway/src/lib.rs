//! # NATS Gateway
//!
//! Lightweight NATS message router with filtering for ES Sync Gateway.
//! Designed to run on AWS Firecracker microVMs.
//!
//! ## Features
//!
//! - Subject-based message filtering
//! - Message transformation
//! - Admin API for runtime configuration
//! - Prometheus metrics
//! - Embedded NATS server mode for Firecracker
//!
//! ## Deployment Modes
//!
//! **External NATS (default):**
//! ```bash
//! nats-gateway --upstream-url nats://external:4222
//! ```
//!
//! **Embedded NATS (Firecracker):**
//! ```bash
//! nats-gateway --embedded --store-dir /data/jetstream
//! ```

pub mod admin_api;
pub mod embedded;
pub mod filter_engine;
pub mod server;
pub mod transform;

pub use admin_api::*;
pub use embedded::*;
pub use filter_engine::*;
pub use server::*;
pub use transform::*;

use es_gateway_core::prelude::*;
use es_gateway_core::{FilterChain, GatewayMetrics};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace};

/// Gateway configuration
#[derive(Debug, Clone)]
pub struct GatewayServerConfig {
    /// NATS server URL (upstream)
    pub upstream_url: String,
    /// Stream to consume from
    pub upstream_stream: String,
    /// Consumer name
    pub consumer_name: String,
    /// NATS server URL (downstream) - can be same as upstream
    pub downstream_url: String,
    /// Stream to publish to
    pub downstream_stream: String,
    /// Subject prefix for downstream
    pub downstream_prefix: String,
    /// Admin API listen address
    pub admin_listen: String,
    /// Enable admin API
    pub admin_enabled: bool,
}

impl Default for GatewayServerConfig {
    fn default() -> Self {
        Self {
            upstream_url: "nats://localhost:4222".to_string(),
            upstream_stream: "ES_CHANGES".to_string(),
            consumer_name: "nats-gateway".to_string(),
            downstream_url: "nats://localhost:4222".to_string(),
            downstream_stream: "ES_CHANGES_FILTERED".to_string(),
            downstream_prefix: "es.filtered".to_string(),
            admin_listen: "0.0.0.0:8080".to_string(),
            admin_enabled: true,
        }
    }
}

/// Main gateway server
pub struct Gateway {
    config: GatewayServerConfig,
    filter_chain: Arc<RwLock<FilterChain>>,
    transformer: Arc<RwLock<Option<Box<dyn MessageTransformer>>>>,
    metrics: GatewayMetrics,
    running: AtomicBool,
    received: AtomicU64,
    filtered: AtomicU64,
    forwarded: AtomicU64,
}

impl Gateway {
    /// Create new gateway
    pub fn new(config: GatewayServerConfig) -> Self {
        Self {
            config,
            filter_chain: Arc::new(RwLock::new(FilterChain::new())),
            transformer: Arc::new(RwLock::new(None)),
            metrics: GatewayMetrics::new("nats_gateway"),
            running: AtomicBool::new(false),
            received: AtomicU64::new(0),
            filtered: AtomicU64::new(0),
            forwarded: AtomicU64::new(0),
        }
    }

    /// Set filter chain
    pub async fn set_filters(&self, chain: FilterChain) {
        *self.filter_chain.write().await = chain;
    }

    /// Set transformer
    pub async fn set_transformer(&self, transformer: Box<dyn MessageTransformer>) {
        *self.transformer.write().await = Some(transformer);
    }

    /// Get statistics
    pub fn stats(&self) -> GatewayStats {
        GatewayStats {
            received: self.received.load(Ordering::Relaxed),
            filtered: self.filtered.load(Ordering::Relaxed),
            forwarded: self.forwarded.load(Ordering::Relaxed),
        }
    }

    /// Run the gateway
    pub async fn run(&self) -> Result<()> {
        info!("Starting NATS gateway");
        self.running.store(true, Ordering::SeqCst);

        // Start admin API if enabled
        let admin_handle = if self.config.admin_enabled {
            let state = AdminState {
                filter_chain: self.filter_chain.clone(),
                stats: Arc::new(|| self.stats()),
            };
            Some(tokio::spawn(run_admin_server(
                self.config.admin_listen.clone(),
                state,
            )))
        } else {
            None
        };

        // Connect to NATS
        let upstream = async_nats::connect(&self.config.upstream_url)
            .await
            .map_err(|e| GatewayError::nats_with_source("Failed to connect upstream", e))?;

        let downstream = if self.config.downstream_url == self.config.upstream_url {
            upstream.clone()
        } else {
            async_nats::connect(&self.config.downstream_url)
                .await
                .map_err(|e| GatewayError::nats_with_source("Failed to connect downstream", e))?
        };

        let js_up = async_nats::jetstream::new(upstream);
        let js_down = async_nats::jetstream::new(downstream);

        // Get consumer
        let stream = js_up
            .get_stream(&self.config.upstream_stream)
            .await
            .map_err(|e| GatewayError::nats_with_source("Stream not found", e))?;

        let consumer = stream
            .get_or_create_consumer(
                &self.config.consumer_name,
                async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some(self.config.consumer_name.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| GatewayError::nats_with_source("Failed to create consumer", e))?;

        info!("Gateway connected, starting message loop");

        // Message loop
        use futures::StreamExt;
        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            let mut messages = consumer
                .fetch()
                .max_messages(100)
                .messages()
                .await
                .map_err(|e| GatewayError::nats_with_source("Fetch failed", e))?;

            while let Some(msg_result) = messages.next().await {
                let msg = match msg_result {
                    Ok(m) => m,
                    Err(e) => {
                        error!(error = %e, "Error receiving message");
                        continue;
                    }
                };

                self.received.fetch_add(1, Ordering::Relaxed);
                self.metrics.record_received(&msg.subject);

                // Parse envelope
                let envelope = match Envelope::from_bytes(&msg.payload) {
                    Ok(e) => e,
                    Err(e) => {
                        error!(error = %e, "Failed to parse envelope");
                        let _ = msg.ack().await;
                        continue;
                    }
                };

                // Apply filters
                let filter_chain = self.filter_chain.read().await;
                if !filter_chain.passes(&envelope) {
                    self.filtered.fetch_add(1, Ordering::Relaxed);
                    self.metrics.record_filtered("chain");
                    let _ = msg.ack().await;
                    trace!(doc_id = %envelope.event.doc_id, "Message filtered");
                    continue;
                }

                // Apply transformation
                let envelope = {
                    let transformer = self.transformer.read().await;
                    if let Some(ref t) = *transformer {
                        t.transform(envelope)?
                    } else {
                        envelope
                    }
                };

                // Forward to downstream
                let subject = format!(
                    "{}.{}",
                    self.config.downstream_prefix,
                    envelope.event.index.replace('-', "_")
                );
                let payload = envelope.to_bytes().map_err(|e| {
                    GatewayError::Serialization {
                        message: e.to_string(),
                        source: None,
                    }
                })?;

                js_down
                    .publish(subject.clone(), payload)
                    .await
                    .map_err(|e| GatewayError::nats_with_source("Publish failed", e))?
                    .await
                    .map_err(|e| GatewayError::nats_with_source("Ack failed", e))?;

                let _ = msg.ack().await;
                self.forwarded.fetch_add(1, Ordering::Relaxed);
                self.metrics.record_forwarded();

                trace!(doc_id = %envelope.event.doc_id, subject, "Message forwarded");
            }

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Shutdown signal received");
                    break;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            }
        }

        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = admin_handle {
            handle.abort();
        }

        info!(stats = ?self.stats(), "Gateway stopped");
        Ok(())
    }

    /// Stop the gateway
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// Gateway statistics
#[derive(Debug, Clone, Copy)]
pub struct GatewayStats {
    pub received: u64,
    pub filtered: u64,
    pub forwarded: u64,
}
