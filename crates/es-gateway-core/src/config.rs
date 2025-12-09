//! Configuration types for ES Sync Gateway
//!
//! Uses the `config` crate for layered configuration from files and environment.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

/// Root configuration for the entire gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Watcher (source) configuration
    #[serde(default)]
    pub watcher: WatcherConfig,

    /// Gateway (NATS bridge) configuration
    #[serde(default)]
    pub gateway: NatsGatewayConfig,

    /// Writer (target) configuration
    #[serde(default)]
    pub writer: WriterConfig,

    /// Observability configuration
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            watcher: WatcherConfig::default(),
            gateway: NatsGatewayConfig::default(),
            writer: WriterConfig::default(),
            observability: ObservabilityConfig::default(),
        }
    }
}

// ============================================================================
// Elasticsearch Configuration
// ============================================================================

/// Elasticsearch connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElasticsearchConfig {
    /// Elasticsearch hosts
    #[serde(default = "default_es_hosts")]
    pub hosts: Vec<String>,

    /// Optional username for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Optional password for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Optional API key for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Connection timeout
    #[serde(with = "humantime_serde", default = "default_connect_timeout")]
    pub connect_timeout: Duration,

    /// Request timeout
    #[serde(with = "humantime_serde", default = "default_request_timeout")]
    pub request_timeout: Duration,

    /// Enable TLS certificate verification
    #[serde(default = "default_true")]
    pub verify_certs: bool,

    /// CA certificate path (for self-signed certs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert_path: Option<String>,
}

fn default_es_hosts() -> Vec<String> {
    vec!["http://localhost:9200".to_string()]
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_request_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_true() -> bool {
    true
}

impl Default for ElasticsearchConfig {
    fn default() -> Self {
        Self {
            hosts: default_es_hosts(),
            username: None,
            password: None,
            api_key: None,
            connect_timeout: default_connect_timeout(),
            request_timeout: default_request_timeout(),
            verify_certs: true,
            ca_cert_path: None,
        }
    }
}

// ============================================================================
// NATS Configuration
// ============================================================================

/// NATS connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsConfig {
    /// NATS server URL
    #[serde(default = "default_nats_url")]
    pub url: String,

    /// Optional credentials file path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials_path: Option<String>,

    /// Optional JWT token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt: Option<String>,

    /// Optional seed for NKey authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nkey_seed: Option<String>,

    /// Connection name (for monitoring)
    #[serde(default = "default_connection_name")]
    pub connection_name: String,

    /// Reconnect attempts (-1 for infinite)
    #[serde(default = "default_reconnect_attempts")]
    pub reconnect_attempts: i32,

    /// Reconnect delay
    #[serde(with = "humantime_serde", default = "default_reconnect_delay")]
    pub reconnect_delay: Duration,
}

fn default_nats_url() -> String {
    "nats://localhost:4222".to_string()
}

fn default_connection_name() -> String {
    "es-sync-gateway".to_string()
}

fn default_reconnect_attempts() -> i32 {
    -1 // infinite
}

fn default_reconnect_delay() -> Duration {
    Duration::from_secs(2)
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            url: default_nats_url(),
            credentials_path: None,
            jwt: None,
            nkey_seed: None,
            connection_name: default_connection_name(),
            reconnect_attempts: default_reconnect_attempts(),
            reconnect_delay: default_reconnect_delay(),
        }
    }
}

/// JetStream configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JetStreamConfig {
    /// Stream name
    #[serde(default = "default_stream_name")]
    pub stream: String,

    /// Consumer name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer: Option<String>,

    /// Subject prefix for publishing
    #[serde(default = "default_subject_prefix")]
    pub subject_prefix: String,

    /// Subjects to filter on consumption
    #[serde(default)]
    pub filter_subjects: Vec<String>,

    /// Maximum pending acknowledgements
    #[serde(default = "default_max_ack_pending")]
    pub max_ack_pending: u64,

    /// Acknowledgement wait time
    #[serde(with = "humantime_serde", default = "default_ack_wait")]
    pub ack_wait: Duration,
}

fn default_stream_name() -> String {
    "ES_CHANGES".to_string()
}

fn default_subject_prefix() -> String {
    "es".to_string()
}

fn default_max_ack_pending() -> u64 {
    10000
}

fn default_ack_wait() -> Duration {
    Duration::from_secs(30)
}

impl Default for JetStreamConfig {
    fn default() -> Self {
        Self {
            stream: default_stream_name(),
            consumer: None,
            subject_prefix: default_subject_prefix(),
            filter_subjects: Vec::new(),
            max_ack_pending: default_max_ack_pending(),
            ack_wait: default_ack_wait(),
        }
    }
}

// ============================================================================
// Component Configurations
// ============================================================================

/// Watcher (source) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatcherConfig {
    /// Source Elasticsearch configuration
    #[serde(default)]
    pub elasticsearch: ElasticsearchConfig,

    /// NATS publisher configuration
    #[serde(default)]
    pub nats: NatsConfig,

    /// JetStream configuration
    #[serde(default)]
    pub jetstream: JetStreamConfig,

    /// Index patterns to watch (glob patterns)
    #[serde(default)]
    pub index_patterns: Vec<String>,

    /// Polling interval for change detection
    #[serde(with = "humantime_serde", default = "default_poll_interval")]
    pub poll_interval: Duration,

    /// Batch size for fetching changes
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Tenant identifier for subject generation
    #[serde(default = "default_tenant")]
    pub tenant: String,

    /// Checkpoint storage configuration
    #[serde(default)]
    pub checkpoint: CheckpointConfig,
}

fn default_poll_interval() -> Duration {
    Duration::from_millis(100)
}

fn default_batch_size() -> usize {
    1000
}

fn default_tenant() -> String {
    "default".to_string()
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            elasticsearch: ElasticsearchConfig::default(),
            nats: NatsConfig::default(),
            jetstream: JetStreamConfig::default(),
            index_patterns: vec!["*".to_string()],
            poll_interval: default_poll_interval(),
            batch_size: default_batch_size(),
            tenant: default_tenant(),
            checkpoint: CheckpointConfig::default(),
        }
    }
}

/// Checkpoint storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    /// Storage type: "file", "redis", "memory"
    #[serde(default = "default_checkpoint_type")]
    pub storage_type: String,

    /// Path for file-based checkpoint
    #[serde(default = "default_checkpoint_path")]
    pub path: String,

    /// Redis URL for redis-based checkpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redis_url: Option<String>,

    /// Flush interval
    #[serde(with = "humantime_serde", default = "default_flush_interval")]
    pub flush_interval: Duration,
}

fn default_checkpoint_type() -> String {
    "file".to_string()
}

fn default_checkpoint_path() -> String {
    "/var/lib/es-sync-gateway/checkpoint".to_string()
}

fn default_flush_interval() -> Duration {
    Duration::from_secs(1)
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            storage_type: default_checkpoint_type(),
            path: default_checkpoint_path(),
            redis_url: None,
            flush_interval: default_flush_interval(),
        }
    }
}

/// NATS Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsGatewayConfig {
    /// NATS server listen address
    #[serde(default = "default_listen_addr")]
    pub listen: String,

    /// Cluster name
    #[serde(default = "default_cluster_name")]
    pub cluster_name: String,

    /// JetStream storage directory
    #[serde(default = "default_store_dir")]
    pub store_dir: String,

    /// Maximum memory for JetStream
    #[serde(default = "default_max_memory")]
    pub max_memory: String,

    /// Filter configurations
    #[serde(default)]
    pub filters: Vec<crate::filter::FilterConfig>,

    /// Admin API configuration
    #[serde(default)]
    pub admin: AdminApiConfig,
}

fn default_listen_addr() -> String {
    "0.0.0.0:4222".to_string()
}

fn default_cluster_name() -> String {
    "es-sync-gateway".to_string()
}

fn default_store_dir() -> String {
    "/data/jetstream".to_string()
}

fn default_max_memory() -> String {
    "64MB".to_string()
}

impl Default for NatsGatewayConfig {
    fn default() -> Self {
        Self {
            listen: default_listen_addr(),
            cluster_name: default_cluster_name(),
            store_dir: default_store_dir(),
            max_memory: default_max_memory(),
            filters: Vec::new(),
            admin: AdminApiConfig::default(),
        }
    }
}

/// Admin API configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminApiConfig {
    /// Enable admin API
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Listen address
    #[serde(default = "default_admin_listen")]
    pub listen: String,
}

fn default_admin_listen() -> String {
    "0.0.0.0:8080".to_string()
}

impl Default for AdminApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: default_admin_listen(),
        }
    }
}

/// Writer (target) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriterConfig {
    /// NATS consumer configuration
    #[serde(default)]
    pub nats: NatsConfig,

    /// JetStream configuration
    #[serde(default)]
    pub jetstream: JetStreamConfig,

    /// Target Elasticsearch configuration
    #[serde(default)]
    pub elasticsearch: ElasticsearchConfig,

    /// Bulk operation configuration
    #[serde(default)]
    pub bulk: BulkConfig,

    /// Dead letter queue configuration
    #[serde(default)]
    pub dlq: DlqConfig,

    /// Retry configuration
    #[serde(default)]
    pub retry: RetryConfig,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            nats: NatsConfig::default(),
            jetstream: JetStreamConfig::default(),
            elasticsearch: ElasticsearchConfig::default(),
            bulk: BulkConfig::default(),
            dlq: DlqConfig::default(),
            retry: RetryConfig::default(),
        }
    }
}

/// Bulk operation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkConfig {
    /// Maximum documents per bulk request
    #[serde(default = "default_bulk_size")]
    pub size: usize,

    /// Maximum bytes per bulk request
    #[serde(default = "default_bulk_bytes")]
    pub max_bytes: usize,

    /// Flush interval
    #[serde(with = "humantime_serde", default = "default_bulk_flush_interval")]
    pub flush_interval: Duration,

    /// Concurrent bulk requests
    #[serde(default = "default_bulk_concurrency")]
    pub concurrency: usize,
}

fn default_bulk_size() -> usize {
    1000
}

fn default_bulk_bytes() -> usize {
    5 * 1024 * 1024 // 5MB
}

fn default_bulk_flush_interval() -> Duration {
    Duration::from_millis(100)
}

fn default_bulk_concurrency() -> usize {
    4
}

impl Default for BulkConfig {
    fn default() -> Self {
        Self {
            size: default_bulk_size(),
            max_bytes: default_bulk_bytes(),
            flush_interval: default_bulk_flush_interval(),
            concurrency: default_bulk_concurrency(),
        }
    }
}

/// Dead letter queue configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqConfig {
    /// Enable DLQ
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// DLQ path
    #[serde(default = "default_dlq_path")]
    pub path: String,

    /// Maximum DLQ size
    #[serde(default = "default_dlq_max_size")]
    pub max_size: usize,
}

fn default_dlq_path() -> String {
    "/data/dlq".to_string()
}

fn default_dlq_max_size() -> usize {
    100_000
}

impl Default for DlqConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_dlq_path(),
            max_size: default_dlq_max_size(),
        }
    }
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum retry attempts
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Initial backoff delay
    #[serde(with = "humantime_serde", default = "default_initial_backoff")]
    pub initial_backoff: Duration,

    /// Maximum backoff delay
    #[serde(with = "humantime_serde", default = "default_max_backoff")]
    pub max_backoff: Duration,

    /// Backoff multiplier
    #[serde(default = "default_backoff_multiplier")]
    pub multiplier: f64,
}

fn default_max_attempts() -> u32 {
    3
}

fn default_initial_backoff() -> Duration {
    Duration::from_millis(100)
}

fn default_max_backoff() -> Duration {
    Duration::from_secs(10)
}

fn default_backoff_multiplier() -> f64 {
    2.0
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            initial_backoff: default_initial_backoff(),
            max_backoff: default_max_backoff(),
            multiplier: default_backoff_multiplier(),
        }
    }
}

/// Observability configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    /// Metrics configuration
    #[serde(default)]
    pub metrics: MetricsExporterConfig,

    /// Log level
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Log format: "json" or "pretty"
    #[serde(default = "default_log_format")]
    pub log_format: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            metrics: MetricsExporterConfig::default(),
            log_level: default_log_level(),
            log_format: default_log_format(),
        }
    }
}

/// Metrics exporter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsExporterConfig {
    /// Enable metrics
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Prometheus endpoint
    #[serde(default = "default_metrics_endpoint")]
    pub endpoint: String,
}

fn default_metrics_endpoint() -> String {
    "0.0.0.0:9090".to_string()
}

impl Default for MetricsExporterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endpoint: default_metrics_endpoint(),
        }
    }
}

// ============================================================================
// Configuration Loading
// ============================================================================

impl GatewayConfig {
    /// Load configuration from file and environment
    pub fn load(path: Option<&str>) -> Result<Self, config::ConfigError> {
        let mut builder = config::Config::builder();

        // Add default values
        builder = builder.add_source(config::Config::try_from(&Self::default())?);

        // Add config file if specified
        if let Some(path) = path {
            builder = builder.add_source(config::File::with_name(path));
        }

        // Add environment variables with prefix ES_GATEWAY_
        builder = builder.add_source(
            config::Environment::with_prefix("ES_GATEWAY")
                .separator("__")
                .try_parsing(true),
        );

        builder.build()?.try_deserialize()
    }
}
