//! Dead Letter Queue for ES Writer

use es_gateway_core::prelude::*;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Dead letter queue entry
#[derive(Debug, Clone)]
pub struct DlqEntry {
    pub envelope: Envelope,
    pub error: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub attempts: u32,
}

/// File-backed dead letter queue
pub struct DeadLetterQueue {
    path: PathBuf,
    max_size: usize,
    entries: RwLock<Vec<DlqEntry>>,
}

impl DeadLetterQueue {
    pub fn new(path: impl Into<PathBuf>, max_size: usize) -> Self {
        Self {
            path: path.into(),
            max_size,
            entries: RwLock::new(Vec::new()),
        }
    }

    pub async fn add(&self, envelope: Envelope, error: String) -> Result<()> {
        let mut entries = self.entries.write().await;
        
        if entries.len() >= self.max_size {
            warn!(max_size = self.max_size, "DLQ full, dropping oldest entry");
            entries.remove(0);
        }

        entries.push(DlqEntry {
            envelope,
            error,
            timestamp: chrono::Utc::now(),
            attempts: 1,
        });

        Ok(())
    }

    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }

    pub async fn drain(&self) -> Vec<DlqEntry> {
        std::mem::take(&mut *self.entries.write().await)
    }

    pub async fn peek(&self, count: usize) -> Vec<DlqEntry> {
        self.entries.read().await.iter().take(count).cloned().collect()
    }
}
