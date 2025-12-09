//! Checkpoint storage strategies
//!
//! Provides pluggable checkpoint persistence for resumable streaming.

use async_trait::async_trait;
use es_gateway_core::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Checkpoint storage trait (Strategy pattern)
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Load the last checkpoint
    async fn load(&self) -> Result<u64>;

    /// Save checkpoint
    async fn save(&self, sequence: u64) -> Result<()>;

    /// Get store name
    fn name(&self) -> &'static str;
}

// ============================================================================
// File-based Checkpoint
// ============================================================================

/// File-based checkpoint storage
pub struct FileCheckpoint {
    path: PathBuf,
    cached: AtomicU64,
}

impl FileCheckpoint {
    /// Create new file checkpoint
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            cached: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl CheckpointStore for FileCheckpoint {
    async fn load(&self) -> Result<u64> {
        match fs::read_to_string(&self.path).await {
            Ok(content) => {
                let seq: u64 = content.trim().parse().map_err(|e| {
                    GatewayError::Checkpoint {
                        message: format!("Failed to parse checkpoint: {}", e),
                        source: None,
                    }
                })?;
                self.cached.store(seq, Ordering::SeqCst);
                info!(checkpoint = seq, path = ?self.path, "Loaded checkpoint");
                Ok(seq)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!(path = ?self.path, "No checkpoint file, starting from 0");
                Ok(0)
            }
            Err(e) => Err(GatewayError::Checkpoint {
                message: format!("Failed to read checkpoint: {}", e),
                source: Some(Box::new(e)),
            }),
        }
    }

    async fn save(&self, sequence: u64) -> Result<()> {
        // Only write if changed
        if self.cached.load(Ordering::SeqCst) == sequence {
            return Ok(());
        }

        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                GatewayError::Checkpoint {
                    message: format!("Failed to create checkpoint directory: {}", e),
                    source: Some(Box::new(e)),
                }
            })?;
        }

        // Write atomically via temp file
        let temp_path = self.path.with_extension("tmp");
        fs::write(&temp_path, sequence.to_string()).await.map_err(|e| {
            GatewayError::Checkpoint {
                message: format!("Failed to write checkpoint: {}", e),
                source: Some(Box::new(e)),
            }
        })?;

        fs::rename(&temp_path, &self.path).await.map_err(|e| {
            GatewayError::Checkpoint {
                message: format!("Failed to rename checkpoint: {}", e),
                source: Some(Box::new(e)),
            }
        })?;

        self.cached.store(sequence, Ordering::SeqCst);
        debug!(checkpoint = sequence, "Saved checkpoint");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "file_checkpoint"
    }
}

// ============================================================================
// Memory Checkpoint (for testing)
// ============================================================================

/// In-memory checkpoint storage
pub struct MemoryCheckpoint {
    sequence: RwLock<u64>,
}

impl MemoryCheckpoint {
    /// Create new memory checkpoint
    pub fn new() -> Self {
        Self {
            sequence: RwLock::new(0),
        }
    }

    /// Create with initial value
    pub fn with_value(value: u64) -> Self {
        Self {
            sequence: RwLock::new(value),
        }
    }
}

impl Default for MemoryCheckpoint {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CheckpointStore for MemoryCheckpoint {
    async fn load(&self) -> Result<u64> {
        Ok(*self.sequence.read().await)
    }

    async fn save(&self, sequence: u64) -> Result<()> {
        *self.sequence.write().await = sequence;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "memory_checkpoint"
    }
}

// ============================================================================
// Batched Checkpoint (write-behind)
// ============================================================================

/// Batched checkpoint that flushes periodically
pub struct BatchedCheckpoint {
    inner: Arc<dyn CheckpointStore>,
    pending: AtomicU64,
    last_saved: AtomicU64,
    threshold: u64,
}

impl BatchedCheckpoint {
    /// Create batched checkpoint
    ///
    /// Will only persist when `threshold` commits have accumulated.
    pub fn new(inner: Arc<dyn CheckpointStore>, threshold: u64) -> Self {
        Self {
            inner,
            pending: AtomicU64::new(0),
            last_saved: AtomicU64::new(0),
            threshold,
        }
    }

    /// Force flush
    pub async fn flush(&self) -> Result<()> {
        let pending = self.pending.load(Ordering::SeqCst);
        if pending > self.last_saved.load(Ordering::SeqCst) {
            self.inner.save(pending).await?;
            self.last_saved.store(pending, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[async_trait]
impl CheckpointStore for BatchedCheckpoint {
    async fn load(&self) -> Result<u64> {
        let seq = self.inner.load().await?;
        self.pending.store(seq, Ordering::SeqCst);
        self.last_saved.store(seq, Ordering::SeqCst);
        Ok(seq)
    }

    async fn save(&self, sequence: u64) -> Result<()> {
        self.pending.store(sequence, Ordering::SeqCst);

        // Only persist if threshold reached
        let last = self.last_saved.load(Ordering::SeqCst);
        if sequence - last >= self.threshold {
            self.inner.save(sequence).await?;
            self.last_saved.store(sequence, Ordering::SeqCst);
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "batched_checkpoint"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_memory_checkpoint() {
        let cp = MemoryCheckpoint::new();

        assert_eq!(cp.load().await.unwrap(), 0);
        cp.save(100).await.unwrap();
        assert_eq!(cp.load().await.unwrap(), 100);
    }

    #[tokio::test]
    async fn test_file_checkpoint() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("checkpoint");

        let cp = FileCheckpoint::new(&path);

        // Initial load returns 0
        assert_eq!(cp.load().await.unwrap(), 0);

        // Save and reload
        cp.save(42).await.unwrap();
        assert_eq!(cp.load().await.unwrap(), 42);

        // New instance should load persisted value
        let cp2 = FileCheckpoint::new(&path);
        assert_eq!(cp2.load().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_batched_checkpoint() {
        let inner = Arc::new(MemoryCheckpoint::new());
        let cp = BatchedCheckpoint::new(inner.clone(), 10);

        cp.load().await.unwrap();

        // Save below threshold - shouldn't persist
        for i in 1..10 {
            cp.save(i).await.unwrap();
        }
        assert_eq!(inner.load().await.unwrap(), 0);

        // Save at threshold - should persist
        cp.save(10).await.unwrap();
        assert_eq!(inner.load().await.unwrap(), 10);
    }
}
