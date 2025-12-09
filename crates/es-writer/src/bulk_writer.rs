//! Bulk writer strategy for Elasticsearch

use async_trait::async_trait;
use es_gateway_core::prelude::*;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, error, trace};

use crate::retry::RetryPolicy;

/// Bulk writer configuration
#[derive(Debug, Clone)]
pub struct BulkWriterConfig {
    pub hosts: Vec<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub api_key: Option<String>,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
}

impl Default for BulkWriterConfig {
    fn default() -> Self {
        Self {
            hosts: vec!["http://localhost:9200".to_string()],
            username: None,
            password: None,
            api_key: None,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// Elasticsearch bulk writer
pub struct BulkWriter {
    client: Client,
    config: BulkWriterConfig,
    retry_policy: RetryPolicy,
    host_index: std::sync::atomic::AtomicUsize,
    running: AtomicBool,
}

impl BulkWriter {
    pub fn new(config: BulkWriterConfig, retry_policy: RetryPolicy) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| GatewayError::elasticsearch_with_source("Failed to create client", e))?;

        Ok(Self {
            client,
            config,
            retry_policy,
            host_index: std::sync::atomic::AtomicUsize::new(0),
            running: AtomicBool::new(false),
        })
    }

    fn get_host(&self) -> &str {
        let idx = self.host_index.fetch_add(1, Ordering::Relaxed) % self.config.hosts.len();
        &self.config.hosts[idx]
    }

    fn build_request(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.get_host(), path);
        let mut req = self.client.post(&url);

        if let Some(ref api_key) = self.config.api_key {
            req = req.header("Authorization", format!("ApiKey {}", api_key));
        } else if let (Some(ref user), Some(ref pass)) = (&self.config.username, &self.config.password) {
            req = req.basic_auth(user, Some(pass));
        }

        req.header("Content-Type", "application/x-ndjson")
    }

    fn envelope_to_bulk_line(&self, envelope: &Envelope) -> String {
        let mut lines = String::new();
        
        match envelope.event.operation {
            DocumentOperation::Index | DocumentOperation::Update => {
                let action = json!({
                    "index": {
                        "_index": envelope.event.index,
                        "_id": envelope.event.doc_id
                    }
                });
                lines.push_str(&serde_json::to_string(&action).unwrap());
                lines.push('\n');
                
                if let Some(ref source) = envelope.event.source {
                    lines.push_str(&serde_json::to_string(source).unwrap());
                    lines.push('\n');
                }
            }
            DocumentOperation::Delete => {
                let action = json!({
                    "delete": {
                        "_index": envelope.event.index,
                        "_id": envelope.event.doc_id
                    }
                });
                lines.push_str(&serde_json::to_string(&action).unwrap());
                lines.push('\n');
            }
            DocumentOperation::PartialUpdate => {
                let action = json!({
                    "update": {
                        "_index": envelope.event.index,
                        "_id": envelope.event.doc_id
                    }
                });
                lines.push_str(&serde_json::to_string(&action).unwrap());
                lines.push('\n');
                
                if let Some(ref source) = envelope.event.source {
                    let doc = json!({"doc": source, "doc_as_upsert": true});
                    lines.push_str(&serde_json::to_string(&doc).unwrap());
                    lines.push('\n');
                }
            }
        }
        
        lines
    }
}

#[async_trait]
impl Lifecycle for BulkWriter {
    async fn start(&self) -> Result<()> {
        // Test connection
        let resp = self.client
            .get(format!("{}/", self.get_host()))
            .send()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Connection failed", e))?;

        if !resp.status().is_success() {
            return Err(GatewayError::elasticsearch("Failed to connect to Elasticsearch"));
        }

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
impl HealthCheck for BulkWriter {
    async fn health_check(&self) -> Result<()> {
        let resp = self.client
            .get(format!("{}/_cluster/health", self.get_host()))
            .send()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Health check failed", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(GatewayError::elasticsearch("Cluster unhealthy"))
        }
    }

    fn component_name(&self) -> &'static str {
        "bulk_writer"
    }
}

#[async_trait]
impl Writer for BulkWriter {
    async fn write(&self, envelope: &Envelope) -> Result<WriteResult> {
        let batch = std::iter::once(envelope.clone()).collect::<EnvelopeBatch>();
        self.write_batch(&batch).await
    }

    async fn write_batch(&self, batch: &EnvelopeBatch) -> Result<WriteResult> {
        if batch.is_empty() {
            return Ok(WriteResult::success(0, 0));
        }

        let start = Instant::now();
        
        // Build NDJSON body
        let mut body = String::new();
        for envelope in batch.iter() {
            body.push_str(&self.envelope_to_bulk_line(envelope));
        }

        // Execute with retry
        let mut attempts = 0;
        let result = loop {
            attempts += 1;
            
            let resp = self.build_request("/_bulk")
                .body(body.clone())
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => {
                    let body: Value = r.json().await.map_err(|e| {
                        GatewayError::elasticsearch_with_source("Failed to parse response", e)
                    })?;

                    let took = body["took"].as_u64().unwrap_or(0);
                    let has_errors = body["errors"].as_bool().unwrap_or(false);

                    if !has_errors {
                        break WriteResult::success(batch.len(), start.elapsed().as_millis() as u64);
                    }

                    // Parse individual failures
                    let mut failures = Vec::new();
                    if let Some(items) = body["items"].as_array() {
                        for item in items {
                            let op = item.as_object().and_then(|o| o.values().next());
                            if let Some(op) = op {
                                if let Some(error) = op.get("error") {
                                    let id = op["_id"].as_str().unwrap_or("unknown").to_string();
                                    let reason = error["reason"].as_str().unwrap_or("unknown").to_string();
                                    failures.push((id, reason));
                                }
                            }
                        }
                    }

                    break WriteResult {
                        success_count: batch.len() - failures.len(),
                        failure_count: failures.len(),
                        failures,
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
                Ok(r) => {
                    let status = r.status();
                    if self.retry_policy.should_retry(attempts) && status.is_server_error() {
                        tokio::time::sleep(self.retry_policy.delay(attempts)).await;
                        continue;
                    }
                    return Err(GatewayError::elasticsearch(format!("Bulk failed: {}", status)));
                }
                Err(e) => {
                    if self.retry_policy.should_retry(attempts) {
                        tokio::time::sleep(self.retry_policy.delay(attempts)).await;
                        continue;
                    }
                    return Err(GatewayError::elasticsearch_with_source("Bulk request failed", e));
                }
            }
        };

        debug!(
            success = result.success_count,
            failed = result.failure_count,
            duration_ms = result.duration_ms,
            "Bulk write completed"
        );

        Ok(result)
    }

    async fn flush(&self) -> Result<()> {
        // Bulk writer doesn't buffer
        Ok(())
    }
}
