# ES Sync Gateway Service (Rust) 
Cross-Cloud Elasticsearch (Self-Hosted) to AWS Elasticsearch Doc Indexing Sync Service using Rust, NATS Jetstream and Firecracker VMM


## Architectural Workflow

```shell
┌─────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                              PROPOSED: 4-STEP NATS STREAMING PIPELINE                               │
└─────────────────────────────────────────────────────────────────────────────────────────────────────┘

    ┌──────────────────────────────────────┐              ┌──────────────────────────────────────┐
    │         GCP eu-west-1                │              │         AWS eu-north-1               │
    │                                      │              │                                      │
    │  ┌─────────────┐                     │              │                     ┌─────────────┐  │
    │  │   Iconik    │                     │              │                     │   Iconik    │  │
    │  │  Workload   │                     │              │                     │  Workload   │  │
    │  └──────┬──────┘                     │              │                     └─────────────┘  │
    │         │ 1. API Write               │              │                                      │
    │         ▼                            │              │                                      │
    │  ┌─────────────┐                     │              │                                      │
    │  │ Dual Write  │                     │              │                                      │
    │  │   Proxy     │                     │              │                                      │
    │  └──┬──────┬───┘                     │              │                                      │
    │     │      │                         │              │                                      │
    │     │ Sync │                         │              │                                      │
    │     ▼      │                         │              │                                      │
    │  ┌─────┐   │                         │              │                                      │
    │  │ C*  │   │ 2. Change Event         │              │                                      │
    │  └──┬──┘   │                         │              │                                      │
    │     │      │                         │              │                                      │
    │     │ Read │                         │              │                                      │
    │     ▼      ▼                         │              │                                      │
    │  ┌──────────────┐                    │              │                                      │
    │  │ ES-WATCHER   │ ◄─── Rust Binary   │              │                                      │
    │  |(es-sync-gtwy │      Sidecar       │              │                                      │
    │  └──────┬───────┘                    │              │                                      │
    │         │                            │              │                                      │
    │         │ 3. NATS Publish            │              │                                      │
    │         │    (JetStream)             │              │                                      │
    │         ▼                            │              │                                      │
    │     ════════════════════════════════════════════════════════════════════                   │
    │                                      │              │                                      │
    └──────────────────────────────────────┘              └──────────────────────────────────────┘
                                           │              │
                                           ▼              ▼
                              ┌─────────────────────────────────────────┐
                              │                                         │
                              │    ┌─────────────────────────────────┐  │
                              │    │     AWS FIRECRACKER microVM     │  │
                              │    │                                 │  │
                              │    │  ┌───────────────────────────┐  │  │
                              │    │  │      NATS-BRIDGE          │  │  │
                              │    │  │      (es-sync-gateway)    │  │  │
                              │    │  │                           │  │  │
                              │    │  │  ┌─────────────────────┐  │  │  │
                              │    │  │  │  Subject Filtering  │  │  │  │
                              │    │  │  │  ─────────────────  │  │  │  │
                              │    │  │  │  • es.iconik.assets │  │  │  │
                              │    │  │  │  • es.iconik.meta   │  │  │  │
                              │    │  │  │  • es.iconik.users  │  │  │  │
                              │    │  │  │  ✗ es.iconik.logs   │  │  │  │
                              │    │  │  │  ✗ es.iconik.temp   │  │  │  │
                              │    │  │  └─────────────────────┘  │  │  │
                              │    │  │                           │  │  │
                              │    │  │  ┌─────────────────────┐  │  │  │
                              │    │  │  │  Transform/Enrich   │  │  │  │
                              │    │  │  └─────────────────────┘  │  │  │
                              │    │  │                           │  │  │
                              │    │  └───────────────────────────┘  │  │
                              │    │                                 │  │
                              │    │  Memory: 128MB | CPU: 0.5 vCPU  │  │
                              │    │  Cold Start: <125ms             │  │
                              │    │                                 │  │
                              │    └─────────────────────────────────┘  │
                              │                                         │
                              │              NATS JetStream             │
                              │         (Cross-Cloud Replication)       │
                              │                                         │
                              └─────────────────────────────────────────┘
                                           │              │
                                           │              │
    ┌──────────────────────────────────────┘              └──────────────────────────────────────┐
    │                                      │              │                                      │
    │         GCP eu-west-1                │              │         AWS eu-north-1               │
    │                                      │              │                                      │
    │     ════════════════════════════════════════════════════════════════════                   │
    │                                      │              │         │                            │
    │                                      │              │         │ 4. NATS Subscribe          │
    │                                      │              │         │    (Filtered Stream)       │
    │                                      │              │         ▼                            │
    │                                      │              │  ┌──────────────┐                    │
    │                                      │              │  │  ES-WRITER   │ ◄─── Rust Binary   │
    │                                      │              │  │  (es-sync-gateway) │      Sidecar │
    │                                      │              │  └──────┬───────┘                    │
    │                                      │              │         │                            │
    │     ┌──────────────┐                 │              │         │ Bulk Index                 │
    │     │Elasticsearch │                 │              │         ▼                            │
    │     │    (GCP)     │                 │              │  ┌──────────────┐                    │
    │     └──────────────┘                 │              │  │Elasticsearch │                    │
    │                                      │              │  │    (AWS)     │                    │
    │                                      │              │  └──────────────┘                    │
    │                                      │              │                                      │
    └──────────────────────────────────────┘              └──────────────────────────────────────┘

    Components: 3 (single binary) | Queue Hops: 1 | Latency: <10ms | Language: Rust
```



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
    │       ├── strategy.rs             # Strategy traits (ChangeSource, MessageSink, Writer, etc.)
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
    │       ├── embedded.rs             # ★ NEW: Embedded NATS server subprocess
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


## Building the Project

```shell
make help              # Show all targets

# Development
make build             # Debug build
make release           # Release build (native)
make check             # Cargo check
make test              # Run tests

# Code Quality
make fmt               # Format code
make clippy            # Run lints
make lint              # fmt-check + clippy
make audit             # Security audit

# Static Builds (Firecracker)
make musl              # x86_64 static binary
make musl-arm          # aarch64 static binary

# Distribution
make dist              # Create release package
make dist-musl         # Create musl package
make tarball           # Create .tar.gz release

# Docker
make docker-build      # Build all images
make docker-push       # Push to registry

# Firecracker
make firecracker-rootfs
make firecracker-kernel

# Local Dev
make dev-up            # Start NATS + ES stack
make dev-down          # Stop stack
make run-watcher       # Run watcher (debug)
make run-gateway       # Run gateway (debug)
make run-gateway-embedded  # Run with embedded NATS

# Install
make install           # Install to /usr/local/bin
make uninstall

# CI
make ci                # fmt-check + clippy + test
make ci-full           # ci + audit
```



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
