//! Batching for ES Writer

use es_gateway_core::prelude::*;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Envelope batcher with size and time triggers
pub struct Batcher {
    batch: RwLock<EnvelopeBatch>,
    max_size: usize,
    max_bytes: usize,
    timeout: Duration,
    last_add: RwLock<Instant>,
}

impl Batcher {
    pub fn new(max_size: usize, max_bytes: usize, timeout: Duration) -> Self {
        Self {
            batch: RwLock::new(EnvelopeBatch::with_capacity(max_size)),
            max_size,
            max_bytes,
            timeout,
            last_add: RwLock::new(Instant::now()),
        }
    }

    pub async fn add(&self, envelope: Envelope) {
        self.batch.write().await.push(envelope);
        *self.last_add.write().await = Instant::now();
    }

    pub async fn should_flush(&self) -> bool {
        let batch = self.batch.read().await;
        let last_add = *self.last_add.read().await;
        
        batch.len() >= self.max_size
            || batch.size_bytes() >= self.max_bytes
            || (batch.len() > 0 && last_add.elapsed() >= self.timeout)
    }

    pub async fn drain(&self) -> EnvelopeBatch {
        std::mem::take(&mut *self.batch.write().await)
    }

    pub async fn len(&self) -> usize {
        self.batch.read().await.len()
    }
}
