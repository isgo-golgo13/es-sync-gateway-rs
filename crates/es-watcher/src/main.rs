//! ES Watcher CLI
//!
//! Watches Elasticsearch for changes and publishes to NATS JetStream.

use clap::Parser;
use es_gateway_core::config::GatewayConfig;
use es_gateway_core::prelude::*;
use es_watcher::{
    checkpoint::FileCheckpoint, es_client::EsClient, source::PollingSource,
    publisher::NatsJetStreamSink, Watcher, EsClientConfig, NatsJetStreamSinkConfig,
    PollingSourceConfig,
};
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(name = "es-watcher")]
#[command(about = "Elasticsearch change watcher for ES Sync Gateway")]
#[command(version)]
struct Args {
    /// Configuration file path
    #[arg(short, long, env = "ES_GATEWAY_CONFIG")]
    config: Option<String>,

    /// Elasticsearch hosts (comma-separated)
    #[arg(long, env = "ES_HOSTS", default_value = "http://localhost:9200")]
    es_hosts: String,

    /// NATS server URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// Index patterns to watch (comma-separated)
    #[arg(long, env = "INDEX_PATTERNS", default_value = "*")]
    index_patterns: String,

    /// Tenant identifier
    #[arg(long, env = "TENANT", default_value = "default")]
    tenant: String,

    /// Checkpoint file path
    #[arg(long, env = "CHECKPOINT_PATH", default_value = "/var/lib/es-sync-gateway/checkpoint")]
    checkpoint_path: String,

    /// Log level
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer().json())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(&args.log_level)
        }))
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting es-watcher"
    );

    // Parse configuration
    let es_hosts: Vec<String> = args.es_hosts.split(',').map(|s| s.trim().to_string()).collect();
    let index_patterns: Vec<String> = args
        .index_patterns
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    // Build ES client
    let es_config = EsClientConfig {
        hosts: es_hosts,
        ..Default::default()
    };
    let es_client = EsClient::new(es_config)?;

    // Build checkpoint store
    let checkpoint = Arc::new(FileCheckpoint::new(&args.checkpoint_path));

    // Build source
    let source_config = PollingSourceConfig {
        index_patterns,
        tenant: args.tenant.clone(),
        source_id: format!("es-watcher-{}", hostname()),
        ..Default::default()
    };
    let source = PollingSource::new(es_client, checkpoint, source_config);

    // Build sink
    let sink_config = NatsJetStreamSinkConfig {
        url: args.nats_url,
        tenant: args.tenant,
        ..Default::default()
    };
    let sink = NatsJetStreamSink::new(sink_config);

    // Build and run watcher
    let watcher = Watcher::new(source, sink);

    info!("Watcher initialized, starting main loop");

    if let Err(e) = watcher.run().await {
        error!(error = %e, "Watcher failed");
        return Err(e.into());
    }

    info!("Watcher stopped gracefully");
    Ok(())
}

/// Get hostname for source identification
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".to_string())
}
