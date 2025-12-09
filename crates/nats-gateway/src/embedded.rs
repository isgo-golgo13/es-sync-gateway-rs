//! Embedded NATS Server for Firecracker deployment
//!
//! Spawns `nats-server` as a subprocess with JetStream enabled.
//! Our Rust binary is PID 1, owns the full lifecycle.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

/// Embedded NATS server configuration
#[derive(Debug, Clone)]
pub struct EmbeddedNatsConfig {
    /// Path to nats-server binary
    pub binary_path: PathBuf,
    /// Listen address for client connections
    pub client_listen: String,
    /// Listen address for cluster connections (optional)
    pub cluster_listen: Option<String>,
    /// Listen address for HTTP monitoring
    pub http_listen: String,
    /// JetStream storage directory
    pub store_dir: PathBuf,
    /// Max memory for JetStream (e.g., "64MB")
    pub max_memory: String,
    /// Max file storage for JetStream (e.g., "1GB")
    pub max_file: String,
    /// Server name
    pub server_name: String,
    /// Startup timeout
    pub startup_timeout: Duration,
    /// Health check interval during startup
    pub health_check_interval: Duration,
}

impl Default for EmbeddedNatsConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("/usr/bin/nats-server"),
            client_listen: "127.0.0.1:4222".to_string(),
            cluster_listen: None,
            http_listen: "127.0.0.1:8222".to_string(),
            store_dir: PathBuf::from("/data/jetstream"),
            max_memory: "64MB".to_string(),
            max_file: "1GB".to_string(),
            server_name: "es-sync-gateway".to_string(),
            startup_timeout: Duration::from_secs(30),
            health_check_interval: Duration::from_millis(100),
        }
    }
}

/// Embedded NATS server handle
pub struct EmbeddedNats {
    config: EmbeddedNatsConfig,
    child: Option<Child>,
}

impl EmbeddedNats {
    /// Create new embedded NATS server (not yet started)
    pub fn new(config: EmbeddedNatsConfig) -> Self {
        Self {
            config,
            child: None,
        }
    }

    /// Start the embedded NATS server
    pub async fn start(&mut self) -> Result<(), EmbeddedNatsError> {
        if self.child.is_some() {
            return Err(EmbeddedNatsError::AlreadyRunning);
        }

        // Ensure store directory exists
        tokio::fs::create_dir_all(&self.config.store_dir)
            .await
            .map_err(|e| EmbeddedNatsError::Io(e.to_string()))?;

        // Build command
        let mut cmd = Command::new(&self.config.binary_path);
        
        cmd.args([
            // JetStream enabled with storage
            "--jetstream",
            "--store_dir", self.config.store_dir.to_str().unwrap(),
            
            // Limits
            "--js_max_memory", &self.config.max_memory,
            "--js_max_file", &self.config.max_file,
            
            // Listen addresses
            "--addr", &self.config.client_listen.split(':').next().unwrap_or("127.0.0.1"),
            "--port", &self.config.client_listen.split(':').last().unwrap_or("4222"),
            
            // HTTP monitoring
            "--http_port", &self.config.http_listen.split(':').last().unwrap_or("8222"),
            
            // Server name
            "--name", &self.config.server_name,
            
            // Logging
            "--log_time",
        ]);

        // Add cluster listen if configured
        if let Some(ref cluster) = self.config.cluster_listen {
            cmd.args(["--cluster", cluster]);
        }

        // Capture stdout/stderr for logging
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        info!(
            binary = %self.config.binary_path.display(),
            listen = %self.config.client_listen,
            store = %self.config.store_dir.display(),
            "Starting embedded NATS server"
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| EmbeddedNatsError::SpawnFailed(e.to_string()))?;

        // Spawn log forwarders
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(forward_logs(stdout, "nats-server"));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_logs(stderr, "nats-server"));
        }

        self.child = Some(child);

        // Wait for server to be ready
        self.wait_for_ready().await?;

        info!("Embedded NATS server started successfully");
        Ok(())
    }

    /// Wait for NATS server to accept connections
    async fn wait_for_ready(&self) -> Result<(), EmbeddedNatsError> {
        let addr = &self.config.client_listen;
        let deadline = timeout(self.config.startup_timeout, async {
            loop {
                match TcpStream::connect(addr).await {
                    Ok(_) => {
                        debug!(addr, "NATS server accepting connections");
                        return Ok(());
                    }
                    Err(_) => {
                        debug!(addr, "NATS server not ready, retrying...");
                        sleep(self.config.health_check_interval).await;
                    }
                }
            }
        })
        .await;

        match deadline {
            Ok(result) => result,
            Err(_) => Err(EmbeddedNatsError::StartupTimeout),
        }
    }

    /// Check if the server is healthy
    pub async fn health_check(&self) -> Result<(), EmbeddedNatsError> {
        if self.child.is_none() {
            return Err(EmbeddedNatsError::NotRunning);
        }

        // Check TCP connection
        TcpStream::connect(&self.config.client_listen)
            .await
            .map_err(|_| EmbeddedNatsError::Unhealthy)?;

        Ok(())
    }

    /// Get the client connection URL
    pub fn client_url(&self) -> String {
        format!("nats://{}", self.config.client_listen)
    }

    /// Graceful shutdown
    pub async fn stop(&mut self) -> Result<(), EmbeddedNatsError> {
        let Some(ref mut child) = self.child else {
            return Ok(());
        };

        info!("Stopping embedded NATS server");

        // Send SIGTERM for graceful shutdown
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            if let Some(pid) = child.id() {
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }
        }

        // Wait for graceful shutdown with timeout
        let shutdown_timeout = Duration::from_secs(10);
        match timeout(shutdown_timeout, child.wait()).await {
            Ok(Ok(status)) => {
                info!(?status, "NATS server exited");
            }
            Ok(Err(e)) => {
                warn!(error = %e, "Error waiting for NATS server");
            }
            Err(_) => {
                // Force kill if graceful shutdown timed out
                warn!("NATS server graceful shutdown timed out, killing");
                let _ = child.kill().await;
            }
        }

        self.child = None;
        info!("Embedded NATS server stopped");
        Ok(())
    }

    /// Check if the server process is running
    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for EmbeddedNats {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            // Best-effort kill on drop
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                if let Some(pid) = child.id() {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }
            }
        }
    }
}

/// Forward subprocess logs to tracing
async fn forward_logs<R: tokio::io::AsyncRead + Unpin>(reader: R, name: &'static str) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // Parse NATS server log level if possible
        if line.contains("[ERR]") || line.contains("[FTL]") {
            error!(target: name, "{}", line);
        } else if line.contains("[WRN]") {
            warn!(target: name, "{}", line);
        } else if line.contains("[DBG]") || line.contains("[TRC]") {
            debug!(target: name, "{}", line);
        } else {
            info!(target: name, "{}", line);
        }
    }
}

/// Errors from embedded NATS server
#[derive(Debug, thiserror::Error)]
pub enum EmbeddedNatsError {
    #[error("NATS server already running")]
    AlreadyRunning,

    #[error("NATS server not running")]
    NotRunning,

    #[error("Failed to spawn NATS server: {0}")]
    SpawnFailed(String),

    #[error("NATS server startup timed out")]
    StartupTimeout,

    #[error("NATS server unhealthy")]
    Unhealthy,

    #[error("I/O error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EmbeddedNatsConfig::default();
        assert_eq!(config.client_listen, "127.0.0.1:4222");
        assert_eq!(config.max_memory, "64MB");
    }

    #[test]
    fn test_client_url() {
        let config = EmbeddedNatsConfig {
            client_listen: "0.0.0.0:4223".to_string(),
            ..Default::default()
        };
        let server = EmbeddedNats::new(config);
        assert_eq!(server.client_url(), "nats://0.0.0.0:4223");
    }
}
