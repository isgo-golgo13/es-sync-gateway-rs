//! Filter Strategy for ES Sync Gateway
//!
//! Provides composable filtering strategies for message routing and selection.
//!
//! ## Filter Types
//!
//! - **SubjectFilter**: Pattern-based subject matching (NATS wildcards)
//! - **IndexFilter**: Include/exclude by ES index name
//! - **OperationFilter**: Filter by document operation type
//! - **FieldFilter**: JSONPath-based field predicates
//! - **CompositeFilter**: AND/OR combinations of filters
//!
//! ## Example
//!
//! ```rust,ignore
//! let filter = FilterChain::new()
//!     .with(IndexFilter::include(vec!["assets", "metadata"]))
//!     .with(IndexFilter::exclude(vec!["logs", "temp_*"]))
//!     .with(OperationFilter::exclude(vec![DocumentOperation::Delete]));
//! ```

use crate::error::{GatewayError, Result};
use crate::message::{DocumentOperation, Envelope};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Result of filter evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterResult {
    /// Message passes the filter
    Pass,
    /// Message is filtered out
    Reject,
    /// Filter cannot determine (pass to next filter)
    Abstain,
}

impl FilterResult {
    /// Combine with AND semantics (both must pass)
    pub fn and(self, other: FilterResult) -> FilterResult {
        match (self, other) {
            (FilterResult::Reject, _) | (_, FilterResult::Reject) => FilterResult::Reject,
            (FilterResult::Pass, FilterResult::Pass) => FilterResult::Pass,
            (FilterResult::Pass, FilterResult::Abstain) => FilterResult::Pass,
            (FilterResult::Abstain, FilterResult::Pass) => FilterResult::Pass,
            (FilterResult::Abstain, FilterResult::Abstain) => FilterResult::Abstain,
        }
    }

    /// Combine with OR semantics (either can pass)
    pub fn or(self, other: FilterResult) -> FilterResult {
        match (self, other) {
            (FilterResult::Pass, _) | (_, FilterResult::Pass) => FilterResult::Pass,
            (FilterResult::Reject, FilterResult::Reject) => FilterResult::Reject,
            (FilterResult::Reject, FilterResult::Abstain) => FilterResult::Abstain,
            (FilterResult::Abstain, FilterResult::Reject) => FilterResult::Abstain,
            (FilterResult::Abstain, FilterResult::Abstain) => FilterResult::Abstain,
        }
    }

    /// Check if passes
    pub fn passes(self) -> bool {
        matches!(self, FilterResult::Pass | FilterResult::Abstain)
    }
}

/// Core filter trait - Strategy pattern for message filtering
pub trait Filter: Send + Sync {
    /// Evaluate the filter against an envelope
    fn evaluate(&self, envelope: &Envelope) -> FilterResult;

    /// Get filter name for debugging/metrics
    fn name(&self) -> &'static str;

    /// Check if filter is enabled
    fn is_enabled(&self) -> bool {
        true
    }
}

// ============================================================================
// Index Filter
// ============================================================================

/// Filter by Elasticsearch index name
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexFilter {
    /// Indices to include (if non-empty, only these pass)
    #[serde(default)]
    include: HashSet<String>,

    /// Indices to exclude (these always fail)
    #[serde(default)]
    exclude: HashSet<String>,

    /// Wildcard patterns to include (e.g., "iconik-*")
    #[serde(default)]
    include_patterns: Vec<String>,

    /// Wildcard patterns to exclude (e.g., "temp_*")
    #[serde(default)]
    exclude_patterns: Vec<String>,
}

impl IndexFilter {
    /// Create empty filter (passes all)
    pub fn new() -> Self {
        Self::default()
    }

    /// Create include-only filter
    pub fn include(indices: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            include: indices.into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Create exclude-only filter
    pub fn exclude(indices: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            exclude: indices.into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Add include pattern
    pub fn with_include_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.include_patterns.push(pattern.into());
        self
    }

    /// Add exclude pattern
    pub fn with_exclude_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.exclude_patterns.push(pattern.into());
        self
    }

    /// Check if index matches a wildcard pattern
    fn matches_pattern(index: &str, pattern: &str) -> bool {
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            index.starts_with(prefix)
        } else if pattern.starts_with('*') {
            let suffix = &pattern[1..];
            index.ends_with(suffix)
        } else if pattern.contains('*') {
            // Simple glob: split on * and check contains
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                index.starts_with(parts[0]) && index.ends_with(parts[1])
            } else {
                pattern == index
            }
        } else {
            pattern == index
        }
    }
}

impl Default for IndexFilter {
    fn default() -> Self {
        Self {
            include: HashSet::new(),
            exclude: HashSet::new(),
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
        }
    }
}

impl Filter for IndexFilter {
    fn evaluate(&self, envelope: &Envelope) -> FilterResult {
        let index = &envelope.event.index;

        // Check explicit excludes first
        if self.exclude.contains(index) {
            return FilterResult::Reject;
        }

        // Check exclude patterns
        for pattern in &self.exclude_patterns {
            if Self::matches_pattern(index, pattern) {
                return FilterResult::Reject;
            }
        }

        // If we have includes, check them
        if !self.include.is_empty() || !self.include_patterns.is_empty() {
            // Check explicit includes
            if self.include.contains(index) {
                return FilterResult::Pass;
            }

            // Check include patterns
            for pattern in &self.include_patterns {
                if Self::matches_pattern(index, pattern) {
                    return FilterResult::Pass;
                }
            }

            // Has includes but didn't match any
            return FilterResult::Reject;
        }

        // No includes defined, passed excludes
        FilterResult::Pass
    }

    fn name(&self) -> &'static str {
        "index_filter"
    }
}

// ============================================================================
// Operation Filter
// ============================================================================

/// Filter by document operation type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationFilter {
    /// Operations to include (if non-empty, only these pass)
    #[serde(default)]
    include: HashSet<DocumentOperation>,

    /// Operations to exclude
    #[serde(default)]
    exclude: HashSet<DocumentOperation>,
}

impl OperationFilter {
    /// Create filter that includes only specified operations
    pub fn include(ops: impl IntoIterator<Item = DocumentOperation>) -> Self {
        Self {
            include: ops.into_iter().collect(),
            exclude: HashSet::new(),
        }
    }

    /// Create filter that excludes specified operations
    pub fn exclude(ops: impl IntoIterator<Item = DocumentOperation>) -> Self {
        Self {
            include: HashSet::new(),
            exclude: ops.into_iter().collect(),
        }
    }

    /// Create filter for write operations only (no deletes)
    pub fn writes_only() -> Self {
        Self::include(vec![
            DocumentOperation::Index,
            DocumentOperation::Update,
            DocumentOperation::PartialUpdate,
        ])
    }
}

impl Default for OperationFilter {
    fn default() -> Self {
        Self {
            include: HashSet::new(),
            exclude: HashSet::new(),
        }
    }
}

impl Filter for OperationFilter {
    fn evaluate(&self, envelope: &Envelope) -> FilterResult {
        let op = envelope.event.operation;

        if self.exclude.contains(&op) {
            return FilterResult::Reject;
        }

        if !self.include.is_empty() && !self.include.contains(&op) {
            return FilterResult::Reject;
        }

        FilterResult::Pass
    }

    fn name(&self) -> &'static str {
        "operation_filter"
    }
}

// ============================================================================
// Subject Filter (NATS-style)
// ============================================================================

/// Filter by NATS subject pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectFilter {
    /// Subject patterns to match (NATS wildcard syntax)
    patterns: Vec<String>,

    /// Tenant for subject generation
    tenant: String,
}

impl SubjectFilter {
    /// Create filter with patterns
    pub fn new(tenant: impl Into<String>, patterns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            tenant: tenant.into(),
            patterns: patterns.into_iter().map(Into::into).collect(),
        }
    }

    /// Check if subject matches NATS pattern
    ///
    /// Supports:
    /// - `*` matches a single token
    /// - `>` matches one or more tokens (only at end)
    fn matches_nats_pattern(subject: &str, pattern: &str) -> bool {
        let subject_parts: Vec<&str> = subject.split('.').collect();
        let pattern_parts: Vec<&str> = pattern.split('.').collect();

        let mut si = 0;
        let mut pi = 0;

        while pi < pattern_parts.len() {
            let pp = pattern_parts[pi];

            if pp == ">" {
                // > matches everything remaining
                return si < subject_parts.len();
            }

            if si >= subject_parts.len() {
                return false;
            }

            if pp != "*" && pp != subject_parts[si] {
                return false;
            }

            si += 1;
            pi += 1;
        }

        si == subject_parts.len()
    }
}

impl Filter for SubjectFilter {
    fn evaluate(&self, envelope: &Envelope) -> FilterResult {
        let subject = envelope.subject(&self.tenant);

        for pattern in &self.patterns {
            if Self::matches_nats_pattern(&subject, pattern) {
                return FilterResult::Pass;
            }
        }

        if self.patterns.is_empty() {
            FilterResult::Abstain
        } else {
            FilterResult::Reject
        }
    }

    fn name(&self) -> &'static str {
        "subject_filter"
    }
}

// ============================================================================
// Field Filter (JSONPath-like)
// ============================================================================

/// Predicate for field comparison
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldPredicate {
    /// Field exists
    Exists,
    /// Field equals value
    Equals(serde_json::Value),
    /// Field not equals value
    NotEquals(serde_json::Value),
    /// Field contains string
    Contains(String),
    /// Field matches regex pattern
    Matches(String),
    /// Field greater than numeric value
    GreaterThan(f64),
    /// Field less than numeric value
    LessThan(f64),
    /// Field is in list of values
    In(Vec<serde_json::Value>),
}

/// Filter by document field values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldFilter {
    /// JSONPath to the field (simple dot notation: "metadata.type")
    path: String,

    /// Predicate to apply
    predicate: FieldPredicate,
}

impl FieldFilter {
    /// Create field filter
    pub fn new(path: impl Into<String>, predicate: FieldPredicate) -> Self {
        Self {
            path: path.into(),
            predicate,
        }
    }

    /// Get field value by path
    fn get_field<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = value;

        for part in parts {
            match current {
                serde_json::Value::Object(map) => {
                    current = map.get(part)?;
                }
                serde_json::Value::Array(arr) => {
                    let idx: usize = part.parse().ok()?;
                    current = arr.get(idx)?;
                }
                _ => return None,
            }
        }

        Some(current)
    }

    /// Evaluate predicate against value
    fn eval_predicate(value: &serde_json::Value, predicate: &FieldPredicate) -> bool {
        match predicate {
            FieldPredicate::Exists => true,
            FieldPredicate::Equals(expected) => value == expected,
            FieldPredicate::NotEquals(expected) => value != expected,
            FieldPredicate::Contains(needle) => {
                value.as_str().map(|s| s.contains(needle)).unwrap_or(false)
            }
            FieldPredicate::Matches(pattern) => {
                // Simple contains for now, could use regex crate
                value.as_str().map(|s| s.contains(pattern)).unwrap_or(false)
            }
            FieldPredicate::GreaterThan(threshold) => {
                value.as_f64().map(|n| n > *threshold).unwrap_or(false)
            }
            FieldPredicate::LessThan(threshold) => {
                value.as_f64().map(|n| n < *threshold).unwrap_or(false)
            }
            FieldPredicate::In(values) => values.contains(value),
        }
    }
}

impl Filter for FieldFilter {
    fn evaluate(&self, envelope: &Envelope) -> FilterResult {
        let Some(ref source) = envelope.event.source else {
            // No source (e.g., delete) - can't evaluate field
            return if matches!(self.predicate, FieldPredicate::Exists) {
                FilterResult::Reject
            } else {
                FilterResult::Abstain
            };
        };

        match Self::get_field(source, &self.path) {
            Some(value) => {
                if Self::eval_predicate(value, &self.predicate) {
                    FilterResult::Pass
                } else {
                    FilterResult::Reject
                }
            }
            None => {
                if matches!(self.predicate, FieldPredicate::Exists) {
                    FilterResult::Reject
                } else {
                    FilterResult::Abstain
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "field_filter"
    }
}

// ============================================================================
// Filter Chain (Composite Pattern)
// ============================================================================

/// Chain of filters with configurable combination logic
#[derive(Default)]
pub struct FilterChain {
    filters: Vec<Box<dyn Filter>>,
    mode: FilterChainMode,
}

/// How to combine filter results
#[derive(Debug, Clone, Copy, Default)]
pub enum FilterChainMode {
    /// All filters must pass (AND)
    #[default]
    All,
    /// Any filter must pass (OR)
    Any,
    /// First non-abstain result wins
    FirstMatch,
}

impl FilterChain {
    /// Create empty filter chain
    pub fn new() -> Self {
        Self::default()
    }

    /// Create chain with specific mode
    pub fn with_mode(mode: FilterChainMode) -> Self {
        Self {
            filters: Vec::new(),
            mode,
        }
    }

    /// Add filter to chain (builder pattern)
    pub fn with<F: Filter + 'static>(mut self, filter: F) -> Self {
        self.filters.push(Box::new(filter));
        self
    }

    /// Add filter to chain
    pub fn add<F: Filter + 'static>(&mut self, filter: F) {
        self.filters.push(Box::new(filter));
    }

    /// Evaluate all filters
    pub fn evaluate(&self, envelope: &Envelope) -> FilterResult {
        if self.filters.is_empty() {
            return FilterResult::Pass;
        }

        match self.mode {
            FilterChainMode::All => {
                let mut result = FilterResult::Pass;
                for filter in &self.filters {
                    if !filter.is_enabled() {
                        continue;
                    }
                    result = result.and(filter.evaluate(envelope));
                    if result == FilterResult::Reject {
                        return result;
                    }
                }
                result
            }
            FilterChainMode::Any => {
                let mut result = FilterResult::Reject;
                for filter in &self.filters {
                    if !filter.is_enabled() {
                        continue;
                    }
                    result = result.or(filter.evaluate(envelope));
                    if result == FilterResult::Pass {
                        return result;
                    }
                }
                result
            }
            FilterChainMode::FirstMatch => {
                for filter in &self.filters {
                    if !filter.is_enabled() {
                        continue;
                    }
                    let result = filter.evaluate(envelope);
                    if result != FilterResult::Abstain {
                        return result;
                    }
                }
                FilterResult::Abstain
            }
        }
    }

    /// Check if envelope passes the filter chain
    pub fn passes(&self, envelope: &Envelope) -> bool {
        self.evaluate(envelope).passes()
    }

    /// Get number of filters
    pub fn len(&self) -> usize {
        self.filters.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }
}

// ============================================================================
// Filter Configuration
// ============================================================================

/// Serializable filter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FilterConfig {
    Index(IndexFilter),
    Operation(OperationFilter),
    Subject {
        tenant: String,
        patterns: Vec<String>,
    },
    Field {
        path: String,
        predicate: FieldPredicate,
    },
}

impl FilterConfig {
    /// Convert to boxed filter
    pub fn into_filter(self) -> Result<Box<dyn Filter>> {
        match self {
            Self::Index(f) => Ok(Box::new(f)),
            Self::Operation(f) => Ok(Box::new(f)),
            Self::Subject { tenant, patterns } => {
                Ok(Box::new(SubjectFilter::new(tenant, patterns)))
            }
            Self::Field { path, predicate } => Ok(Box::new(FieldFilter::new(path, predicate))),
        }
    }
}

/// Build filter chain from configuration
pub fn build_filter_chain(
    configs: Vec<FilterConfig>,
    mode: FilterChainMode,
) -> Result<FilterChain> {
    let mut chain = FilterChain::with_mode(mode);
    for config in configs {
        chain.filters.push(config.into_filter()?);
    }
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ChangeEvent, Metadata};

    fn test_envelope(index: &str, op: DocumentOperation) -> Envelope {
        Envelope {
            metadata: Metadata::new("test", 1),
            event: ChangeEvent {
                operation: op,
                index: index.to_string(),
                doc_id: "doc1".to_string(),
                doc_type: "_doc".to_string(),
                version: None,
                routing: None,
                pipeline: None,
                source: Some(serde_json::json!({"field": "value"})),
            },
        }
    }

    #[test]
    fn test_index_filter() {
        let filter = IndexFilter::include(vec!["assets", "users"])
            .with_exclude_pattern("temp_*");

        assert_eq!(
            filter.evaluate(&test_envelope("assets", DocumentOperation::Index)),
            FilterResult::Pass
        );
        assert_eq!(
            filter.evaluate(&test_envelope("logs", DocumentOperation::Index)),
            FilterResult::Reject
        );
        assert_eq!(
            filter.evaluate(&test_envelope("temp_cache", DocumentOperation::Index)),
            FilterResult::Reject
        );
    }

    #[test]
    fn test_operation_filter() {
        let filter = OperationFilter::writes_only();

        assert_eq!(
            filter.evaluate(&test_envelope("test", DocumentOperation::Index)),
            FilterResult::Pass
        );
        assert_eq!(
            filter.evaluate(&test_envelope("test", DocumentOperation::Delete)),
            FilterResult::Reject
        );
    }

    #[test]
    fn test_filter_chain() {
        let chain = FilterChain::new()
            .with(IndexFilter::include(vec!["assets"]))
            .with(OperationFilter::writes_only());

        assert!(chain.passes(&test_envelope("assets", DocumentOperation::Index)));
        assert!(!chain.passes(&test_envelope("assets", DocumentOperation::Delete)));
        assert!(!chain.passes(&test_envelope("logs", DocumentOperation::Index)));
    }

    #[test]
    fn test_subject_filter() {
        let filter = SubjectFilter::new("iconik", vec!["es.iconik.assets.*", "es.iconik.users.>"]);

        assert_eq!(
            SubjectFilter::matches_nats_pattern("es.iconik.assets.index", "es.iconik.assets.*"),
            true
        );
        assert_eq!(
            SubjectFilter::matches_nats_pattern("es.iconik.users.index.batch", "es.iconik.users.>"),
            true
        );
    }
}
