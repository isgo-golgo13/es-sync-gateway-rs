//! # ES Gateway Core
//!
//! Core types, strategy traits, and utilities for the ES Sync Gateway system.
//!
//! This crate defines the fundamental abstractions using the Strategy pattern,
//! enabling pluggable implementations for:
//! - Change detection (polling, streaming, webhook)
//! - Message transport (NATS, Kafka, direct)
//! - Filtering (subject-based, content-based, composite)
//! - Writing (bulk, streaming, buffered)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │ ChangeSource│────►│  Transport  │────►│   Writer    │
//! │  Strategy   │     │  Strategy   │     │  Strategy   │
//! └─────────────┘     └─────────────┘     └─────────────┘
//!                           │
//!                     ┌─────┴─────┐
//!                     │  Filter   │
//!                     │ Strategy  │
//!                     └───────────┘
//! ```

pub mod config;
pub mod error;
pub mod filter;
pub mod message;
pub mod metrics;
pub mod strategy;

pub use config::*;
pub use error::*;
pub use filter::*;
pub use message::*;
pub use metrics::*;
pub use strategy::*;

/// Prelude for convenient imports
pub mod prelude {
    pub use crate::config::GatewayConfig;
    pub use crate::error::{GatewayError, Result};
    pub use crate::filter::{Filter, FilterChain, FilterResult};
    pub use crate::message::{ChangeEvent, DocumentOperation, Envelope, Metadata};
    pub use crate::strategy::{
        ChangeSource, HealthCheck, Lifecycle, MessageSink, MessageSource, Writer,
    };
}
