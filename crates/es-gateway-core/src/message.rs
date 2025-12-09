//! Message types for ES Sync Gateway
//!
//! Defines the canonical message envelope and change event structures
//! that flow through the entire pipeline.

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Document operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentOperation {
    /// New document indexed
    Index,
    /// Existing document updated
    Update,
    /// Document deleted
    Delete,
    /// Partial update (script or partial doc)
    PartialUpdate,
}

impl DocumentOperation {
    /// Returns the NATS subject suffix for this operation
    pub fn as_subject_suffix(&self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::PartialUpdate => "partial",
        }
    }

    /// Check if this operation requires document body
    pub fn requires_body(&self) -> bool {
        !matches!(self, Self::Delete)
    }
}

impl std::fmt::Display for DocumentOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_subject_suffix())
    }
}

/// Message metadata for tracking and observability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// Unique message identifier (UUIDv7 for time-ordering)
    pub msg_id: Uuid,

    /// Timestamp when the change was detected
    pub timestamp: DateTime<Utc>,

    /// Source identifier (e.g., "gcp-eu-west-1")
    pub source: String,

    /// Sequence number for ordering (ES _seq_no or synthetic)
    pub sequence: u64,

    /// Primary term for optimistic concurrency (ES _primary_term)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_term: Option<u64>,

    /// Correlation ID for request tracing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,

    /// Custom headers for extensibility
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl Metadata {
    /// Create new metadata with current timestamp
    pub fn new(source: impl Into<String>, sequence: u64) -> Self {
        Self {
            msg_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: source.into(),
            sequence,
            primary_term: None,
            correlation_id: None,
            headers: HashMap::new(),
        }
    }

    /// Builder pattern: set primary term
    pub fn with_primary_term(mut self, term: u64) -> Self {
        self.primary_term = Some(term);
        self
    }

    /// Builder pattern: set correlation ID
    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }

    /// Builder pattern: add header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

impl Default for Metadata {
    fn default() -> Self {
        Self::new("unknown", 0)
    }
}

/// Elasticsearch change event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// The operation type
    pub operation: DocumentOperation,

    /// Index name (without alias resolution)
    pub index: String,

    /// Document ID
    pub doc_id: String,

    /// Document type (deprecated in ES 7+, usually "_doc")
    #[serde(default = "default_doc_type")]
    pub doc_type: String,

    /// Document version for optimistic concurrency
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,

    /// Routing value if document uses custom routing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<String>,

    /// Pipeline to use for indexing (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<String>,

    /// Document source/body (None for deletes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
}

fn default_doc_type() -> String {
    "_doc".to_string()
}

impl ChangeEvent {
    /// Create a new index event
    pub fn index(index: impl Into<String>, doc_id: impl Into<String>, source: serde_json::Value) -> Self {
        Self {
            operation: DocumentOperation::Index,
            index: index.into(),
            doc_id: doc_id.into(),
            doc_type: default_doc_type(),
            version: None,
            routing: None,
            pipeline: None,
            source: Some(source),
        }
    }

    /// Create a new update event
    pub fn update(index: impl Into<String>, doc_id: impl Into<String>, source: serde_json::Value) -> Self {
        Self {
            operation: DocumentOperation::Update,
            index: index.into(),
            doc_id: doc_id.into(),
            doc_type: default_doc_type(),
            version: None,
            routing: None,
            pipeline: None,
            source: Some(source),
        }
    }

    /// Create a new delete event
    pub fn delete(index: impl Into<String>, doc_id: impl Into<String>) -> Self {
        Self {
            operation: DocumentOperation::Delete,
            index: index.into(),
            doc_id: doc_id.into(),
            doc_type: default_doc_type(),
            version: None,
            routing: None,
            pipeline: None,
            source: None,
        }
    }

    /// Builder: set version
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = Some(version);
        self
    }

    /// Builder: set routing
    pub fn with_routing(mut self, routing: impl Into<String>) -> Self {
        self.routing = Some(routing.into());
        self
    }

    /// Builder: set pipeline
    pub fn with_pipeline(mut self, pipeline: impl Into<String>) -> Self {
        self.pipeline = Some(pipeline.into());
        self
    }

    /// Generate NATS subject for this event
    ///
    /// Format: `es.{tenant}.{index}.{operation}`
    pub fn to_subject(&self, tenant: &str) -> String {
        format!(
            "es.{}.{}.{}",
            tenant,
            self.index.replace('-', "_"),
            self.operation.as_subject_suffix()
        )
    }

    /// Get document size in bytes (approximate)
    pub fn size_bytes(&self) -> usize {
        self.source
            .as_ref()
            .map(|s| s.to_string().len())
            .unwrap_or(0)
            + self.index.len()
            + self.doc_id.len()
    }
}

/// Message envelope wrapping change events
///
/// This is the canonical wire format for all messages in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Message metadata
    pub metadata: Metadata,

    /// The change event payload
    pub event: ChangeEvent,
}

impl Envelope {
    /// Create a new envelope
    pub fn new(metadata: Metadata, event: ChangeEvent) -> Self {
        Self { metadata, event }
    }

    /// Create envelope with auto-generated metadata
    pub fn wrap(event: ChangeEvent, source: impl Into<String>, sequence: u64) -> Self {
        Self {
            metadata: Metadata::new(source, sequence),
            event,
        }
    }

    /// Serialize to JSON bytes
    pub fn to_bytes(&self) -> Result<Bytes, serde_json::Error> {
        serde_json::to_vec(self).map(Bytes::from)
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Get the NATS subject for this envelope
    pub fn subject(&self, tenant: &str) -> String {
        self.event.to_subject(tenant)
    }

    /// Total size in bytes (approximate)
    pub fn size_bytes(&self) -> usize {
        self.event.size_bytes() + 200 // ~200 bytes for metadata overhead
    }
}

/// Batch of envelopes for bulk operations
#[derive(Debug, Clone, Default)]
pub struct EnvelopeBatch {
    envelopes: Vec<Envelope>,
    total_size: usize,
}

impl EnvelopeBatch {
    /// Create empty batch
    pub fn new() -> Self {
        Self::default()
    }

    /// Create batch with capacity hint
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            envelopes: Vec::with_capacity(capacity),
            total_size: 0,
        }
    }

    /// Add envelope to batch
    pub fn push(&mut self, envelope: Envelope) {
        self.total_size += envelope.size_bytes();
        self.envelopes.push(envelope);
    }

    /// Number of envelopes in batch
    pub fn len(&self) -> usize {
        self.envelopes.len()
    }

    /// Check if batch is empty
    pub fn is_empty(&self) -> bool {
        self.envelopes.is_empty()
    }

    /// Total size in bytes
    pub fn size_bytes(&self) -> usize {
        self.total_size
    }

    /// Drain all envelopes
    pub fn drain(&mut self) -> impl Iterator<Item = Envelope> + '_ {
        self.total_size = 0;
        self.envelopes.drain(..)
    }

    /// Iterate over envelopes
    pub fn iter(&self) -> impl Iterator<Item = &Envelope> {
        self.envelopes.iter()
    }

    /// Convert to vec
    pub fn into_vec(self) -> Vec<Envelope> {
        self.envelopes
    }
}

impl IntoIterator for EnvelopeBatch {
    type Item = Envelope;
    type IntoIter = std::vec::IntoIter<Envelope>;

    fn into_iter(self) -> Self::IntoIter {
        self.envelopes.into_iter()
    }
}

impl FromIterator<Envelope> for EnvelopeBatch {
    fn from_iter<T: IntoIterator<Item = Envelope>>(iter: T) -> Self {
        let mut batch = Self::new();
        for envelope in iter {
            batch.push(envelope);
        }
        batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_envelope_serialization_roundtrip() {
        let event = ChangeEvent::index(
            "iconik-assets",
            "doc123",
            serde_json::json!({"title": "Test Asset", "size": 1024}),
        );
        let envelope = Envelope::wrap(event, "gcp-eu-west-1", 42);

        let bytes = envelope.to_bytes().unwrap();
        let restored = Envelope::from_bytes(&bytes).unwrap();

        assert_eq!(restored.event.doc_id, "doc123");
        assert_eq!(restored.metadata.sequence, 42);
    }

    #[test]
    fn test_subject_generation() {
        let event = ChangeEvent::index("iconik-assets", "doc123", serde_json::json!({}));
        assert_eq!(event.to_subject("iconik"), "es.iconik.iconik_assets.index");

        let delete = ChangeEvent::delete("users", "user456");
        assert_eq!(delete.to_subject("tenant1"), "es.tenant1.users.delete");
    }
}
