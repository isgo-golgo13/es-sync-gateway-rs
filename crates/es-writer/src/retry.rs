//! Retry policy for ES Writer

use std::time::Duration;

/// Retry policy with exponential backoff
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            multiplier: 2.0,
        }
    }
}

impl RetryPolicy {
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            ..Default::default()
        }
    }

    pub fn should_retry(&self, attempt: u32) -> bool {
        attempt < self.max_attempts
    }

    pub fn delay(&self, attempt: u32) -> Duration {
        let delay = self.initial_delay.as_millis() as f64 * self.multiplier.powi(attempt as i32 - 1);
        let delay = Duration::from_millis(delay as u64);
        std::cmp::min(delay, self.max_delay)
    }
}
