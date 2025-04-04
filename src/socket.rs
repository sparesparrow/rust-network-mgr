use crate::types::{AppError, ControlCommand, ControlCommandSender, Result};
use directories::ProjectDirs; // Changed from BaseDirs to ProjectDirs for runtime path
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

const SOCKET_FILE: &str = "rust-network-manager.sock";

/// Gets the recommended path for the Unix domain socket.
/// Prefers /run/ RUST_PROJECT_NAME if possible, otherwise falls back to /tmp.
fn get_socket_path(config_path: Option<&str>) -> Result<PathBuf> {
    if let Some(path_str) = config_path {
        return Ok(PathBuf::from(path_str));
    }

    // Try to use /run/rust-network-manager/
    let run_dir = Path::new("/run");
    if run_dir.exists() && run_dir.is_dir() {
        // Basic check for write access might be needed in a real scenario
        let app_run_dir = run_dir.join("rust-network-manager");
        if std::fs::create_dir_all(&app_run_dir).is_ok() {
             // Set appropriate permissions if needed (e.g., only accessible by root/group)
             // For simplicity, we don't do this here.
             return Ok(app_run_dir.join(SOCKET_FILE));
        }
    }
    
    // Fallback using ProjectDirs (might place it in user's runtime dir)
    if let Some(proj_dirs) = ProjectDirs::from("", "", "RustNetworkManager") { // qualifier, organization, application
        if let Some(runtime_dir) = proj_dirs.runtime_dir() {
            if std::fs::create_dir_all(runtime_dir).is_ok() {
                return Ok(runtime_dir.join(SOCKET_FILE));
            }
        }
    }

    // Absolute fallback to /tmp (less ideal for system services)
    Ok(Path::new("/tmp").join(SOCKET_FILE))
}

pub struct SocketHandler {
    listener: UnixListener,
    command_sender: ControlCommandSender,
}

impl SocketHandler {
    pub async fn new(config_socket_path: Option<&str>, command_sender: ControlCommandSender) -> Result<Self> {
        let socket_path = get_socket_path(config_socket_path)?;
        tracing::info!("Attempting to bind control socket at: {:?}", socket_path);

        // Ensure the path exists and clean up old socket if present
        if socket_path.exists() {
            tracing::warn!("Existing socket file found at {:?}. Removing.", socket_path);
            std::fs::remove_file(&socket_path)
                .map_err(|e| AppError::Init(format!("Failed to remove old socket: {}", e)))?;
        }
        if let Some(parent) = socket_path.parent() {
             if !parent.exists() {
                 std::fs::create_dir_all(parent).map_err(|e| AppError::Init(format!("Failed to create socket directory: {}", e)))?;
             }
        }

        let listener = UnixListener::bind(&socket_path)
            .map_err(|e| AppError::Socket(e))?;

        // TODO: Set permissions on the socket file (e.g., only allow specific user/group)

        tracing::info!("Control socket listening at: {:?}", socket_path);
        Ok(SocketHandler { listener, command_sender })
    }

    pub async fn start(self) {
        tracing::info!("Starting socket command listener loop...");
        loop {
            match self.listener.accept().await {
                Ok((stream, _addr)) => {
                    tracing::debug!("Accepted new socket connection");
                    let sender = self.command_sender.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, sender).await {
                            tracing::error!("Error handling socket connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to accept socket connection: {}. Stopping listener.", e);
                    break; // Stop listening on error
                }
            }
        }
    }

    async fn handle_connection(stream: UnixStream, sender: ControlCommandSender) -> Result<()> {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => { // Connection closed
                    tracing::debug!("Socket connection closed by peer.");
                    break;
                }
                Ok(_) => {
                    let command_str = line.trim();
                    tracing::info!("Received command via socket: {}", command_str);
                    let command = match command_str {
                        "reload" => Some(ControlCommand::Reload),
                        "status" => Some(ControlCommand::Status),
                        "ping" => Some(ControlCommand::Ping),
                        "shutdown" => Some(ControlCommand::Shutdown),
                        _ => {
                            tracing::warn!("Received unknown command: {}", command_str);
                             let stream_ref = reader.get_mut(); // Get ref to write response
                             stream_ref.write_all(b"ERROR: Unknown command\n").await?;
                            None
                        }
                    };

                    if let Some(cmd) = command {
                         let stream_ref = reader.get_mut(); // Get ref to write response
                         match sender.send(cmd.clone()).await {
                            Ok(_) => {
                                tracing::debug!("Sent command {:?} to main loop", cmd);
                                // Simple ACK for most commands
                                let response_str: &'static str = match cmd {
                                    ControlCommand::Ping => "PONG\n",
                                    ControlCommand::Status => "STATUS command received (response handled by main loop)\n", // Status response is async
                                    _ => "OK\n",
                                };
                                stream_ref.write_all(response_str.as_bytes()).await?;
                                if matches!(cmd, ControlCommand::Shutdown) {
                                     tracing::info!("Shutdown command received, closing connection.");
                                     break; // Close connection after shutdown cmd
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to send command {:?} to main loop: {}", cmd, e);
                                stream_ref.write_all(b"ERROR: Failed to process command internally\n").await?;
                            }
                         }
                    }
                }
                Err(e) => { // Read error
                    tracing::error!("Error reading from socket: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }
}

// Note: Testing socket interaction often requires integration tests or mocking frameworks.
// Basic unit tests might focus on command parsing if extracted.
