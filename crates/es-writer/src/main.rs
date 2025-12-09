//! ES Writer CLI

use clap::Parser;
use es_gateway_core::prelude::*;
use es_writer::{
    bulk_writer::{BulkWriter, BulkWriterConfig},
    consumer::{NatsJetStreamSource, NatsJetStreamSourceConfig},
    retry::RetryPolicy,
    EsWriter, WriterOrchestratorConfig,
};
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(name = "es-writer")]
#[command(about = "Elasticsearch bulk writer for ES Sync Gateway")]
#[command(version)]
struct Args {
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    #[arg(long, env = "ES_HOSTS", default_value = "http://localhost:9200")]
    es_hosts: String,

    #[arg(long, env = "STREAM", default_value = "ES_CHANGES")]
    stream: String,

    #[arg(long, env = "CONSUMER", default_value = "es-writer")]
    consumer: String,

    #[arg(long, env = "FILTER_SUBJECTS")]
    filter_subjects: Option<String>,

    #[arg(long, env = "BATCH_SIZE", default_value = "1000")]
    batch_size: usize,

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

    info!(version = env!("CARGO_PKG_VERSION"), "Starting es-writer");

    let filter_subjects: Vec<String> = args
        .filter_subjects
        .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let source_config = NatsJetStreamSourceConfig {
        url: args.nats_url,
        stream: args.stream,
        consumer: args.consumer,
        filter_subjects,
        batch_size: 100,
        ..Default::default()
    };
    let source = NatsJetStreamSource::new(source_config);

    let es_hosts: Vec<String> = args.es_hosts.split(',').map(|s| s.trim().to_string()).collect();
    let writer_config = BulkWriterConfig {
        hosts: es_hosts,
        ..Default::default()
    };
    let writer = BulkWriter::new(writer_config, RetryPolicy::default())?;

    let orchestrator_config = WriterOrchestratorConfig {
        batch_size: args.batch_size,
        ..Default::default()
    };

    let es_writer = EsWriter::new(source, writer, orchestrator_config);

    if let Err(e) = es_writer.run().await {
        error!(error = %e, "Writer failed");
        return Err(e.into());
    }

    info!("Writer stopped gracefully");
    Ok(())
}
