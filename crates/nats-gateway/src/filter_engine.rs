//! Filter engine for NATS Gateway

use es_gateway_core::filter::{FilterChain, IndexFilter, OperationFilter, SubjectFilter};
use es_gateway_core::prelude::*;

/// Build filter chain from configuration
pub fn build_filters(config: &FilterEngineConfig) -> FilterChain {
    let mut chain = FilterChain::new();

    if !config.include_indices.is_empty() || !config.exclude_indices.is_empty() {
        let mut filter = IndexFilter::new();
        if !config.include_indices.is_empty() {
            filter = IndexFilter::include(config.include_indices.clone());
        }
        for pattern in &config.exclude_patterns {
            filter = filter.with_exclude_pattern(pattern);
        }
        chain.add(filter);
    }

    if !config.include_operations.is_empty() {
        let ops: Vec<DocumentOperation> = config
            .include_operations
            .iter()
            .filter_map(|s| match s.as_str() {
                "index" => Some(DocumentOperation::Index),
                "update" => Some(DocumentOperation::Update),
                "delete" => Some(DocumentOperation::Delete),
                _ => None,
            })
            .collect();
        chain.add(OperationFilter::include(ops));
    }

    chain
}

/// Filter engine configuration
#[derive(Debug, Clone, Default)]
pub struct FilterEngineConfig {
    pub include_indices: Vec<String>,
    pub exclude_indices: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub include_operations: Vec<String>,
}
