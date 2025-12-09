//! Error types for ES Sync Gateway
//!
//! Uses `thiserror` for ergonomic error handling with full context preservation.

use std::fmt;
use thiserror::Error;

/// Result type alias for Gateway operations
pub type Result<T> = std::result::Result<T, GatewayError>;

/// Primary error type for all Gateway operations
#[derive(Error, Debug)]
pub enum GatewayError {
    /// Elasticsearch connection or query errors
    #[error("Elasticsearch error: {message}")]
    Elasticsearch {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// NATS connection or messaging errors
    #[error("NATS error: {message}")]
    Nats {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Message serialization/deserialization errors
    #[error("Serialization error: {message}")]
    Serialization {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Configuration errors
    #[error("Configuration error: {message}")]
    Configuration { message: String },

    /// Filter evaluation errors
    #[error("Filter error: {message}")]
    Filter { message: String },

    /// Checkpoint/resume errors
    #[error("Checkpoint error: {message}")]
    Checkpoint {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Rate limiting or backpressure
    #[error("Backpressure: {message}")]
    Backpressure { message: String },

    /// Operation timeout
    #[error("Timeout: {operation} exceeded {duration_ms}ms")]
    Timeout { operation: String, duration_ms: u64 },

    /// Resource exhaustion
    #[error("Resource exhausted: {resource}")]
    ResourceExhausted { resource: String },

    /// Retry limit exceeded
    #[error("Retry exhausted after {attempts} attempts: {message}")]
    RetryExhausted { attempts: u32, message: String },

    /// Graceful shutdown requested
    #[error("Shutdown requested")]
    Shutdown,

    /// Generic internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl GatewayError {
    /// Create an Elasticsearch error
    pub fn elasticsearch(message: impl Into<String>) -> Self {
        Self::Elasticsearch {
            message: message.into(),
            source: None,
        }
    }

    /// Create an Elasticsearch error with source
    pub fn elasticsearch_with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Elasticsearch {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a NATS error
    pub fn nats(message: impl Into<String>) -> Self {
        Self::Nats {
            message: message.into(),
            source: None,
        }
    }

    /// Create a NATS error with source
    pub fn nats_with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Nats {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a configuration error
    pub fn config(message: impl Into<String>) -> Self {
        Self::Configuration {
            message: message.into(),
        }
    }

    /// Create a filter error
    pub fn filter(message: impl Into<String>) -> Self {
        Self::Filter {
            message: message.into(),
        }
    }

    /// Create a timeout error
    pub fn timeout(operation: impl Into<String>, duration_ms: u64) -> Self {
        Self::Timeout {
            operation: operation.into(),
            duration_ms,
        }
    }

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Elasticsearch { .. }
                | Self::Nats { .. }
                | Self::Backpressure { .. }
                | Self::Timeout { .. }
        )
    }

    /// Check if error is transient (may resolve on its own)
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Backpressure { .. } | Self::Timeout { .. } | Self::ResourceExhausted { .. }
        )
    }
}

/// Error context for enhanced debugging
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub component: &'static str,
    pub operation: String,
    pub document_id: Option<String>,
    pub index: Option<String>,
    pub sequence: Option<u64>,
}

impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}::{}]", self.component, self.operation)?;
        if let Some(ref id) = self.document_id {
            write!(f, " doc={}", id)?;
        }
        if let Some(ref idx) = self.index {
            write!(f, " index={}", idx)?;
        }
        if let Some(seq) = self.sequence {
            write!(f, " seq={}", seq)?;
        }
        Ok(())
    }
}

/// Extension trait for adding context to errors
pub trait ErrorContextExt<T> {
    fn with_context(self, ctx: ErrorContext) -> Result<T>;
}

impl<T> ErrorContextExt<T> for Result<T> {
    fn with_context(self, ctx: ErrorContext) -> Result<T> {
        self.map_err(|e| {
            tracing::error!(
                error = %e,
                component = ctx.component,
                operation = %ctx.operation,
                document_id = ?ctx.document_id,
                index = ?ctx.index,
                sequence = ?ctx.sequence,
                "Operation failed"
            );
            e
        })
    }
}
