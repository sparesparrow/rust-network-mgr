//! Command-line interface for rust-network-mgr.
//!
//! Provides both the daemon entry-point and a client for sending control
//! commands to a running daemon via the Unix domain socket.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Linux network manager: monitors interfaces, manages nftables, and tracks Docker containers.
#[derive(Parser, Debug)]
#[command(name = "rust-network-mgr", version, about, long_about = None)]
pub struct Cli {
    /// Path to the YAML configuration file.
    #[arg(short, long, env = "RUST_NETWORK_MGR_CONFIG", global = true)]
    pub config: Option<PathBuf>,

    /// Override the Unix socket path used for the control interface.
    #[arg(short, long, global = true)]
    pub socket: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the daemon (default when no subcommand is given).
    Daemon,
    /// Send a reload command to the running daemon.
    Reload,
    /// Query the running daemon for its current status.
    Status,
    /// Send a ping and print the response.
    Ping,
    /// Ask the running daemon to shut down gracefully.
    Shutdown,
}

impl Default for Commands {
    fn default() -> Self {
        Commands::Daemon
    }
}

/// Connect to the Unix socket and send a one-line command, returning the response.
pub async fn send_socket_command(
    socket_path: &std::path::Path,
    command: &str,
) -> crate::types::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| crate::types::AppError::Io(e))?;

    stream
        .write_all(format!("{}\n", command).as_bytes())
        .await
        .map_err(|e| crate::types::AppError::Io(e))?;

    // Signal end-of-write so the server knows we're done sending.
    stream
        .shutdown()
        .await
        .map_err(|e| crate::types::AppError::Io(e))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .map_err(|e| crate::types::AppError::Io(e))?;

    Ok(response)
}

/// Resolve the socket path: prefer explicit override, then config, then default.
pub fn resolve_socket_path(
    cli_override: Option<&std::path::Path>,
    config_path: Option<&str>,
) -> PathBuf {
    if let Some(p) = cli_override {
        return p.to_path_buf();
    }
    if let Some(p) = config_path {
        return PathBuf::from(p);
    }
    PathBuf::from("/run/rust-network-manager/rust-network-manager.sock")
}
