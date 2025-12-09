# ES Sync Gateway Service (Rust) 
Cross-Cloud Elasticsearch (Self-Hosted) to AWS Elasticsearch Doc Indexing Sync Service using Rust, NATS Jetstream and Firecracker VMM. Designed for AWS Firecracker MicroVMs, this service provides sub-ms latency with low resource footprint.


## Project Structure

```shell
es-sync-gateway-rs/
├── Cargo.toml                          # Workspace root (orchestrates all crates)
├── README.md
├── config/
│   └── example.yaml
│
└── crates/
    ├── es-gateway-core/                # Shared abstractions
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # Exports + prelude
    │       ├── error.rs                # GatewayError enum
    │       ├── message.rs              # Envelope, ChangeEvent, DocumentOperation
    │       ├── strategy.rs             # Strategy traits (ChangeSource, MessageSink, Writer, ...)
    │       ├── filter.rs               # IndexFilter, OperationFilter, FilterChain
    │       ├── config.rs               # All configuration structs
    │       └── metrics.rs              # Prometheus metrics
    │
    ├── es-watcher/                      # Binary: ES → NATS publisher
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # Watcher orchestrator
    │       ├── main.rs                 # CLI entry point
    │       ├── source.rs               # PollingSource (ChangeSource impl)
    │       ├── es_client.rs            # Elasticsearch HTTP client
    │       ├── publisher.rs            # NatsJetStreamSink (MessageSink impl)
    │       └── checkpoint.rs           # FileCheckpoint, MemoryCheckpoint
    │
    ├── nats-gateway/                    # Binary: Filter/router (Firecracker)
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # Gateway orchestrator
    │       ├── main.rs                 # CLI with --embedded flag
    │       ├── embedded.rs             # Embedded NATS server subprocess
    │       ├── filter_engine.rs        # Filter chain builder
    │       ├── transform.rs            # Message transformers
    │       ├── admin_api.rs            # Axum HTTP admin API
    │       └── server.rs               # (placeholder for future embedded NATS)
    │
    └── es-writer/                       # Binary: NATS → ES bulk writer
        ├── Cargo.toml
        └── src/
            ├── lib.rs                  # EsWriter orchestrator
            ├── main.rs                 # CLI entry point
            ├── consumer.rs             # NatsJetStreamSource (MessageSource impl)
            ├── batcher.rs              # Time/size triggered batching
            ├── bulk_writer.rs          # BulkWriter (Writer impl)
            ├── retry.rs                # Exponential backoff
            └── dlq.rs                  # Dead letter queue
```





```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   ES-WATCHER    │────▶│  NATS-GATEWAY   │────▶│   ES-WRITER     │
│  (GCP Source)   │     │  (Firecracker)  │     │  (AWS Target)   │
└─────────────────┘     └─────────────────┘     └─────────────────┘
```

## Features

- **Strategy Pattern Architecture**: Pluggable components for sources, sinks, filters, and writers
- **NATS JetStream**: Durable, exactly-once message delivery with subject-based filtering
- **Firecracker Ready**: <125ms cold start, 128MB RAM footprint
- **Embedded Mode**: Run NATS server as subprocess (single-binary deployment)
- **Comprehensive Filtering**: Index, operation, subject, and field-level filters
- **Bulk Operations**: Batched ES writes with retry and dead-letter queue
- **Observability**: Prometheus metrics, structured logging, health endpoints

## Service Components

| Binary | Description | Deployment |
|--------|-------------|------------|
| `es-watcher` | Watches source ES for changes, publishes to NATS | Source datacenter (GCP) |
| `nats-gateway` | Filters and routes messages | Edge / Firecracker microVM |
| `es-writer` | Consumes from NATS, bulk writes to target ES | Target datacenter (AWS) |

| Library | Description |
|---------|-------------|
| `es-gateway-core` | Shared traits, types, Strategy pattern abstractions |



## Starter Kit

### Prerequisites

- Rust 1.83+
- NATS Server with JetStream (or use embedded mode)
- Elasticsearch 7.x or 8.x (source and target)

### Build

```bash
# Debug build
make build

# Release build (optimized)
make release

# Static musl build (for Firecracker/Alpine)
make musl
```

---

## Deployment Modes

### Mode 1: External NATS (Recommended for Production)

Use managed NATS (Synadia Cloud) or self-hosted NATS cluster.

```
┌──────────┐     ┌──────────────────┐     ┌──────────┐
│es-watcher│────▶│ NATS JetStream   │────▶│es-writer │
│  (GCP)   │     │ (Synadia/EC2)    │     │  (AWS)   │
└──────────┘     └──────────────────┘     └──────────┘
```

**Startup Order:**

```bash
# 1. Ensure NATS JetStream is running
nats-server -js -sd /data/jetstream

# 2. Start es-writer (consumer) - waits for messages
./es-writer \
  --nats-url nats://nats-server:4222 \
  --es-hosts http://target-es:9200 \
  --consumer es-writer-aws

# 3. Start es-watcher (producer) - begins publishing
./es-watcher \
  --es-hosts http://source-es:9200 \
  --nats-url nats://nats-server:4222 \
  --index-patterns "assets,metadata,users" \
  --tenant iconik
```

### Mode 2: With Gateway Filtering

Add `nats-gateway` between source and target for filtering/transformation.

```
┌──────────┐     ┌─────────────┐     ┌──────────────┐     ┌──────────┐
│es-watcher│────▶│nats-gateway │────▶│NATS JetStream│────▶│es-writer │
└──────────┘     │  (filter)   │     └──────────────┘     └──────────┘
                 └─────────────┘
```

**Startup Order:**

```bash
# 1. NATS JetStream running

# 2. Start nats-gateway (connects to NATS, applies filters)
./nats-gateway \
  --upstream-url nats://nats-server:4222 \
  --include-indices "assets,metadata" \
  --exclude-patterns "temp_*,logs"

# 3. Start es-writer

# 4. Start es-watcher
```

### Mode 3: Embedded NATS (Firecracker / Edge)

Run NATS server as a subprocess inside `nats-gateway`. Single binary, no external dependencies.

```
┌──────────┐     ┌─────────────────────────┐     ┌──────────┐
│es-watcher│────▶│     Firecracker VM      │────▶│es-writer │
│  (GCP)   │     │ ┌─────────┬───────────┐ │     │  (AWS)   │
└──────────┘     │ │  NATS   │  Gateway  │ │     └──────────┘
                 │ │ Server  │  Filter   │ │
                 │ └─────────┴───────────┘ │
                 └─────────────────────────┘
```

**Startup Order:**

```bash
# 1. Start nats-gateway with --embedded (spawns nats-server internally)
./nats-gateway \
  --embedded \
  --nats-listen 0.0.0.0:4222 \
  --store-dir /data/jetstream \
  --js-max-memory 64MB \
  --include-indices "assets,metadata"

# 2. Start es-writer (connects to gateway's embedded NATS)
./es-writer \
  --nats-url nats://gateway-host:4222 \
  --es-hosts http://target-es:9200

# 3. Start es-watcher (connects to gateway's embedded NATS)
./es-watcher \
  --es-hosts http://source-es:9200 \
  --nats-url nats://gateway-host:4222 \
  --index-patterns "assets,metadata" \
  --tenant iconik
```

---

## Local Development

```bash
# Start local NATS + Elasticsearch stack
make dev-up

# In terminal 1: Run es-writer
make run-writer

# In terminal 2: Run es-watcher  
make run-watcher

# Or with embedded gateway
make run-gateway-embedded

# Stop stack
make dev-down
```

---

## CLI Reference

### es-watcher

Watches source Elasticsearch and publishes changes to NATS.

```
USAGE:
    es-watcher [OPTIONS]

OPTIONS:
    --es-hosts <HOSTS>          Elasticsearch hosts (comma-separated)
                                [env: ES_HOSTS] [default: http://localhost:9200]
    --nats-url <URL>            NATS server URL
                                [env: NATS_URL] [default: nats://localhost:4222]
    --index-patterns <PATTERNS> Index patterns to watch (comma-separated)
                                [env: INDEX_PATTERNS] [default: *]
    --tenant <TENANT>           Tenant identifier for NATS subjects
                                [env: TENANT] [default: default]
    --poll-interval <MS>        Polling interval in milliseconds
                                [env: POLL_INTERVAL] [default: 100]
    --checkpoint-path <PATH>    Checkpoint file path
                                [env: CHECKPOINT_PATH] [default: /data/checkpoint]
    --log-level <LEVEL>         Log level
                                [env: LOG_LEVEL] [default: info]
```

### nats-gateway

Filters and routes messages. Can run with external or embedded NATS.

```
USAGE:
    nats-gateway [OPTIONS]

EMBEDDED MODE:
    --embedded                  Run with embedded NATS server
    --nats-binary <PATH>        Path to nats-server binary
                                [default: /usr/bin/nats-server]
    --nats-listen <ADDR>        NATS listen address
                                [default: 0.0.0.0:4222]
    --store-dir <PATH>          JetStream storage directory
                                [default: /data/jetstream]
    --js-max-memory <SIZE>      JetStream max memory (e.g., 64MB)
                                [default: 64MB]
    --js-max-file <SIZE>        JetStream max file storage
                                [default: 1GB]

EXTERNAL MODE:
    --upstream-url <URL>        Upstream NATS URL
                                [default: nats://localhost:4222]
    --downstream-url <URL>      Downstream NATS URL (defaults to upstream)

FILTERING:
    --include-indices <LIST>    Include indices (comma-separated)
    --exclude-patterns <LIST>   Exclude patterns (comma-separated)
    --include-operations <LIST> Include operations (index,update,delete)

ADMIN API:
    --admin-listen <ADDR>       Admin API listen address
                                [default: 0.0.0.0:8080]
    --admin-disabled            Disable admin API
```

### es-writer

Consumes from NATS and writes to target Elasticsearch.

```
USAGE:
    es-writer [OPTIONS]

OPTIONS:
    --nats-url <URL>            NATS server URL
                                [env: NATS_URL] [default: nats://localhost:4222]
    --es-hosts <HOSTS>          Target Elasticsearch hosts
                                [env: ES_HOSTS] [default: http://localhost:9200]
    --stream <NAME>             JetStream stream name
                                [env: STREAM] [default: ES_CHANGES]
    --consumer <NAME>           Consumer name
                                [env: CONSUMER] [default: es-writer]
    --filter-subjects <LIST>    Filter subjects (comma-separated)
                                [env: FILTER_SUBJECTS]
    --batch-size <N>            Batch size for bulk writes
                                [env: BATCH_SIZE] [default: 1000]
    --log-level <LEVEL>         Log level
                                [env: LOG_LEVEL] [default: info]
```

---

## Configuration

### Environment Variables

All CLI options can be set via environment variables:

| Variable | Binary | Description |
|----------|--------|-------------|
| `ES_HOSTS` | watcher, writer | Elasticsearch hosts |
| `NATS_URL` | all | NATS server URL |
| `TENANT` | watcher | Tenant identifier |
| `INDEX_PATTERNS` | watcher | Indices to watch |
| `STREAM` | writer | JetStream stream name |
| `CONSUMER` | writer | Consumer name |
| `LOG_LEVEL` | all | Log level |
| `EMBEDDED` | gateway | Enable embedded mode |
| `STORE_DIR` | gateway | JetStream storage path |

### Configuration File

See `config/example.yaml` for full configuration options.

---

## Architecture

### Strategy Pattern

All major components implement strategy traits for pluggability:

```rust
// Source strategies
trait ChangeSource { async fn changes(&self) -> EnvelopeStream; }

// Sink strategies  
trait MessageSink { async fn publish(&self, envelope: Envelope); }

// Writer strategies
trait Writer { async fn write_batch(&self, batch: &EnvelopeBatch); }

// Filter strategies
trait Filter { fn evaluate(&self, envelope: &Envelope) -> FilterResult; }
```

### Message Flow

1. **es-watcher** polls source ES for changes (via `_seq_no`)
2. Wraps changes in `Envelope` with metadata, publishes to NATS JetStream
3. **nats-gateway** (optional) applies filter chain, forwards matching messages
4. **es-writer** consumes batches, writes to target ES via `_bulk` API
5. Acknowledges messages on success, sends failures to DLQ

### NATS Subject Hierarchy

```
es.{tenant}.{index}.{operation}

Examples:
  es.iconik.assets.index
  es.iconik.metadata.update  
  es.iconik.users.delete
```

---

## Performance

| Metric | Value |
|--------|-------|
| Throughput | ~100K docs/sec |
| End-to-end latency | <10ms |
| Memory (per component) | 128-256MB |
| Cold start (Firecracker) | <125ms |

---

## Makefile Targets

```bash
make help           # Show all targets

# Build
make build          # Debug build
make release        # Release build
make musl           # Static musl build

# Test
make test           # Run tests
make lint           # Run fmt + clippy

# Docker
make docker-build   # Build all images
make docker-push    # Push to registry

# Development
make dev-up         # Start local stack
make dev-down       # Stop local stack
make run-watcher    # Run es-watcher
make run-writer     # Run es-writer
make run-gateway-embedded  # Run gateway with embedded NATS

# Distribution
make dist           # Create release package
make tarball        # Create .tar.gz
```

---

## Firecracker Deployment

```bash
# Build static binaries
make musl

# Build rootfs
make firecracker-rootfs

# Download kernel
make firecracker-kernel

# Start microVM
firectl --kernel firecracker/vmlinux \
  --root-drive firecracker/rootfs.ext4 \
  --kernel-opts "console=ttyS0 reboot=k panic=1 pci=off" \
  --socket-path /tmp/firecracker.sock
```

---

## Health Endpoints

### nats-gateway Admin API (port 8080)

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health status |
| `GET /health/live` | Liveness probe |
| `GET /health/ready` | Readiness probe |
| `GET /stats` | Gateway statistics |
| `GET /metrics` | Prometheus metrics |

---




## Embedded Mode Configuration

```shell
# External NATS (default)
./nats-gateway --upstream-url nats://external:4222

# Embedded NATS (Firecracker)
./nats-gateway --embedded \
  --store-dir /data/jetstream \
  --js-max-memory 64MB \
  --nats-listen 0.0.0.0:4222
```

