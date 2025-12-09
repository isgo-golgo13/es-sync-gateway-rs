//! NATS Gateway CLI
//!
//! Supports two modes:
//! - External: Connect to existing NATS servers
//! - Embedded: Spawn local nats-server (for Firecracker)

use clap::Parser;
use nats_gateway::{
    build_filters, EmbeddedNats, EmbeddedNatsConfig, FilterEngineConfig, Gateway,
    GatewayServerConfig,
};
use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(name = "nats-gateway")]
#[command(about = "NATS message gateway with filtering for ES Sync Gateway")]
#[command(version)]
struct Args {
    // ─────────────────────────────────────────────────────────────────────────
    // Embedded mode (Firecracker)
    // ─────────────────────────────────────────────────────────────────────────
    /// Run with embedded NATS server (Firecracker mode)
    #[arg(long, env = "EMBEDDED")]
    embedded: bool,

    /// Path to nats-server binary (embedded mode)
    #[arg(long, env = "NATS_BINARY", default_value = "/usr/bin/nats-server")]
    nats_binary: PathBuf,

    /// JetStream storage directory (embedded mode)
    #[arg(long, env = "STORE_DIR", default_value = "/data/jetstream")]
    store_dir: PathBuf,

    /// Max memory for JetStream (embedded mode)
    #[arg(long, env = "JS_MAX_MEMORY", default_value = "64MB")]
    js_max_memory: String,

    /// Max file storage for JetStream (embedded mode)
    #[arg(long, env = "JS_MAX_FILE", default_value = "1GB")]
    js_max_file: String,

    /// NATS client listen address (embedded mode)
    #[arg(long, env = "NATS_LISTEN", default_value = "0.0.0.0:4222")]
    nats_listen: String,

    /// NATS HTTP monitoring port (embedded mode)
    #[arg(long, env = "NATS_HTTP", default_value = "0.0.0.0:8222")]
    nats_http: String,

    // ─────────────────────────────────────────────────────────────────────────
    // External mode (default)
    // ─────────────────────────────────────────────────────────────────────────
    /// Upstream NATS URL (external mode, ignored in embedded mode)
    #[arg(long, env = "UPSTREAM_URL", default_value = "nats://localhost:4222")]
    upstream_url: String,

    /// Downstream NATS URL (external mode, defaults to upstream)
    #[arg(long, env = "DOWNSTREAM_URL")]
    downstream_url: Option<String>,

    // ─────────────────────────────────────────────────────────────────────────
    // Stream configuration (both modes)
    // ─────────────────────────────────────────────────────────────────────────
    /// Stream to consume from
    #[arg(long, env = "UPSTREAM_STREAM", default_value = "ES_CHANGES")]
    upstream_stream: String,

    /// Consumer name
    #[arg(long, env = "CONSUMER", default_value = "nats-gateway")]
    consumer: String,

    /// Stream to publish to
    #[arg(long, env = "DOWNSTREAM_STREAM", default_value = "ES_CHANGES_FILTERED")]
    downstream_stream: String,

    /// Downstream subject prefix
    #[arg(long, env = "DOWNSTREAM_PREFIX", default_value = "es.filtered")]
    downstream_prefix: String,

    // ─────────────────────────────────────────────────────────────────────────
    // Filter configuration
    // ─────────────────────────────────────────────────────────────────────────
    /// Include indices (comma-separated)
    #[arg(long, env = "INCLUDE_INDICES")]
    include_indices: Option<String>,

    /// Exclude patterns (comma-separated)
    #[arg(long, env = "EXCLUDE_PATTERNS")]
    exclude_patterns: Option<String>,

    /// Include operations (comma-separated: index,update,delete)
    #[arg(long, env = "INCLUDE_OPERATIONS")]
    include_operations: Option<String>,

    // ─────────────────────────────────────────────────────────────────────────
    // Admin API
    // ─────────────────────────────────────────────────────────────────────────
    /// Admin API listen address
    #[arg(long, env = "ADMIN_LISTEN", default_value = "0.0.0.0:8080")]
    admin_listen: String,

    /// Disable admin API
    #[arg(long, env = "ADMIN_DISABLED")]
    admin_disabled: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Logging
    // ─────────────────────────────────────────────────────────────────────────
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(fmt::layer().json())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)))
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        embedded = args.embedded,
        "Starting nats-gateway"
    );

    // ─────────────────────────────────────────────────────────────────────────
    // Embedded mode: spawn nats-server
    // ─────────────────────────────────────────────────────────────────────────
    let mut embedded_nats = if args.embedded {
        let config = EmbeddedNatsConfig {
            binary_path: args.nats_binary,
            client_listen: args.nats_listen,
            http_listen: args.nats_http,
            store_dir: args.store_dir,
            max_memory: args.js_max_memory,
            max_file: args.js_max_file,
            server_name: "es-sync-gateway".to_string(),
            ..Default::default()
        };

        let mut nats = EmbeddedNats::new(config);
        if let Err(e) = nats.start().await {
            error!(error = %e, "Failed to start embedded NATS server");
            return Err(e.into());
        }

        Some(nats)
    } else {
        None
    };

    // Determine NATS URLs
    let (upstream_url, downstream_url) = if let Some(ref nats) = embedded_nats {
        let url = nats.client_url();
        (url.clone(), url)
    } else {
        let upstream = args.upstream_url.clone();
        let downstream = args.downstream_url.unwrap_or_else(|| upstream.clone());
        (upstream, downstream)
    };

    // ─────────────────────────────────────────────────────────────────────────
    // Build gateway configuration
    // ─────────────────────────────────────────────────────────────────────────
    let config = GatewayServerConfig {
        upstream_url,
        upstream_stream: args.upstream_stream,
        consumer_name: args.consumer,
        downstream_url,
        downstream_stream: args.downstream_stream,
        downstream_prefix: args.downstream_prefix,
        admin_listen: args.admin_listen,
        admin_enabled: !args.admin_disabled,
    };

    let gateway = Gateway::new(config);

    // Build filters
    let filter_config = FilterEngineConfig {
        include_indices: args
            .include_indices
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        exclude_patterns: args
            .exclude_patterns
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        include_operations: args
            .include_operations
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        ..Default::default()
    };

    let filters = build_filters(&filter_config);
    gateway.set_filters(filters).await;

    // ─────────────────────────────────────────────────────────────────────────
    // Run gateway
    // ─────────────────────────────────────────────────────────────────────────
    let result = gateway.run().await;

    // ─────────────────────────────────────────────────────────────────────────
    // Shutdown embedded NATS
    // ─────────────────────────────────────────────────────────────────────────
    if let Some(ref mut nats) = embedded_nats {
        info!("Shutting down embedded NATS server");
        if let Err(e) = nats.stop().await {
            error!(error = %e, "Error stopping embedded NATS server");
        }
    }

    result?;
    info!("nats-gateway stopped");
    Ok(())
}
