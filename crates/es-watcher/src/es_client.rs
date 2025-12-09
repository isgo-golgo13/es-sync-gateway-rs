//! Elasticsearch client wrapper
//!
//! Provides a clean interface to ES APIs needed for change detection.

use es_gateway_core::prelude::*;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, trace};

/// Elasticsearch client configuration
#[derive(Debug, Clone)]
pub struct EsClientConfig {
    pub hosts: Vec<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub api_key: Option<String>,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
}

impl Default for EsClientConfig {
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

/// Elasticsearch client
pub struct EsClient {
    client: Client,
    config: EsClientConfig,
    host_index: std::sync::atomic::AtomicUsize,
}

impl EsClient {
    /// Create new ES client
    pub fn new(config: EsClientConfig) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| GatewayError::elasticsearch_with_source("Failed to create client", e))?;

        Ok(Self {
            client,
            config,
            host_index: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Get the current host (round-robin for load balancing)
    fn get_host(&self) -> &str {
        let idx = self
            .host_index
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.config.hosts.len();
        &self.config.hosts[idx]
    }

    /// Build request with authentication
    fn build_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.get_host(), path);
        let mut req = self.client.request(method, &url);

        if let Some(ref api_key) = self.config.api_key {
            req = req.header("Authorization", format!("ApiKey {}", api_key));
        } else if let (Some(ref user), Some(ref pass)) =
            (&self.config.username, &self.config.password)
        {
            req = req.basic_auth(user, Some(pass));
        }

        req.header("Content-Type", "application/json")
    }

    /// Ping the cluster
    pub async fn ping(&self) -> Result<()> {
        let resp = self
            .build_request(reqwest::Method::GET, "/")
            .send()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Ping failed", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(GatewayError::elasticsearch(format!(
                "Ping returned {}",
                resp.status()
            )))
        }
    }

    /// Get cluster info
    pub async fn cluster_info(&self) -> Result<Value> {
        let resp = self
            .build_request(reqwest::Method::GET, "/")
            .send()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Failed to get cluster info", e))?;

        resp.json()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Failed to parse response", e))
    }

    /// Fetch changes using search with seq_no tracking
    ///
    /// Returns tuples of (doc, seq_no, primary_term)
    pub async fn fetch_changes(
        &self,
        index_pattern: &str,
        from_seq: u64,
        batch_size: usize,
    ) -> Result<Vec<(Value, u64, u64)>> {
        let query = json!({
            "size": batch_size,
            "query": {
                "range": {
                    "_seq_no": {
                        "gt": from_seq
                    }
                }
            },
            "sort": [
                { "_seq_no": "asc" }
            ],
            "seq_no_primary_term": true
        });

        let path = format!("/{}/_search", index_pattern);
        let resp = self
            .build_request(reqwest::Method::POST, &path)
            .json(&query)
            .send()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Search failed", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GatewayError::elasticsearch(format!(
                "Search failed: {} - {}",
                status, body
            )));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Failed to parse response", e))?;

        let hits = body["hits"]["hits"]
            .as_array()
            .ok_or_else(|| GatewayError::elasticsearch("Invalid response format"))?;

        let results: Vec<(Value, u64, u64)> = hits
            .iter()
            .filter_map(|hit| {
                let seq_no = hit["_seq_no"].as_u64()?;
                let primary_term = hit["_primary_term"].as_u64().unwrap_or(1);
                Some((hit.clone(), seq_no, primary_term))
            })
            .collect();

        trace!(count = results.len(), from_seq, "Fetched changes");
        Ok(results)
    }

    /// Get document by ID
    pub async fn get_document(&self, index: &str, id: &str) -> Result<Option<Value>> {
        let path = format!("/{}/_doc/{}", index, id);
        let resp = self
            .build_request(reqwest::Method::GET, &path)
            .send()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Get failed", e))?;

        match resp.status() {
            StatusCode::OK => {
                let doc: Value = resp.json().await.map_err(|e| {
                    GatewayError::elasticsearch_with_source("Failed to parse response", e)
                })?;
                Ok(Some(doc))
            }
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(GatewayError::elasticsearch(format!(
                "Get failed: {}",
                status
            ))),
        }
    }

    /// Bulk index documents
    pub async fn bulk(&self, operations: &[BulkOperation]) -> Result<BulkResponse> {
        if operations.is_empty() {
            return Ok(BulkResponse {
                took: 0,
                errors: false,
                items: Vec::new(),
            });
        }

        // Build NDJSON body
        let mut body = String::new();
        for op in operations {
            match op {
                BulkOperation::Index { index, id, source } => {
                    body.push_str(&serde_json::to_string(&json!({
                        "index": {
                            "_index": index,
                            "_id": id
                        }
                    }))?);
                    body.push('\n');
                    body.push_str(&serde_json::to_string(source)?);
                    body.push('\n');
                }
                BulkOperation::Update {
                    index,
                    id,
                    doc,
                    doc_as_upsert,
                } => {
                    body.push_str(&serde_json::to_string(&json!({
                        "update": {
                            "_index": index,
                            "_id": id
                        }
                    }))?);
                    body.push('\n');
                    body.push_str(&serde_json::to_string(&json!({
                        "doc": doc,
                        "doc_as_upsert": doc_as_upsert
                    }))?);
                    body.push('\n');
                }
                BulkOperation::Delete { index, id } => {
                    body.push_str(&serde_json::to_string(&json!({
                        "delete": {
                            "_index": index,
                            "_id": id
                        }
                    }))?);
                    body.push('\n');
                }
            }
        }

        let resp = self
            .build_request(reqwest::Method::POST, "/_bulk")
            .header("Content-Type", "application/x-ndjson")
            .body(body)
            .send()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Bulk request failed", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GatewayError::elasticsearch(format!(
                "Bulk failed: {} - {}",
                status, body
            )));
        }

        let response: BulkResponse = resp
            .json()
            .await
            .map_err(|e| GatewayError::elasticsearch_with_source("Failed to parse response", e))?;

        debug!(
            took = response.took,
            errors = response.errors,
            items = response.items.len(),
            "Bulk operation completed"
        );

        Ok(response)
    }
}

/// Bulk operation
#[derive(Debug, Clone)]
pub enum BulkOperation {
    Index {
        index: String,
        id: String,
        source: Value,
    },
    Update {
        index: String,
        id: String,
        doc: Value,
        doc_as_upsert: bool,
    },
    Delete {
        index: String,
        id: String,
    },
}

/// Bulk response
#[derive(Debug, Clone, Deserialize)]
pub struct BulkResponse {
    pub took: u64,
    pub errors: bool,
    #[serde(default)]
    pub items: Vec<BulkResponseItem>,
}

/// Bulk response item
#[derive(Debug, Clone, Deserialize)]
pub struct BulkResponseItem {
    #[serde(flatten)]
    pub operation: BulkResponseOperation,
}

/// Bulk response operation
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BulkResponseOperation {
    Index(BulkResponseResult),
    Update(BulkResponseResult),
    Delete(BulkResponseResult),
}

impl BulkResponseOperation {
    pub fn result(&self) -> &BulkResponseResult {
        match self {
            Self::Index(r) | Self::Update(r) | Self::Delete(r) => r,
        }
    }

    pub fn is_success(&self) -> bool {
        self.result().is_success()
    }
}

/// Bulk response result
#[derive(Debug, Clone, Deserialize)]
pub struct BulkResponseResult {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    pub status: u16,
    #[serde(default)]
    pub error: Option<BulkError>,
}

impl BulkResponseResult {
    pub fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// Bulk error
#[derive(Debug, Clone, Deserialize)]
pub struct BulkError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bulk_operation_serialization() {
        let op = BulkOperation::Index {
            index: "test".to_string(),
            id: "1".to_string(),
            source: json!({"field": "value"}),
        };

        match op {
            BulkOperation::Index { index, id, .. } => {
                assert_eq!(index, "test");
                assert_eq!(id, "1");
            }
            _ => panic!("Expected Index operation"),
        }
    }
}
