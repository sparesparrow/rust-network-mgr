use crate::types::{AppError, ControlCommand, Result, EventSender, SystemEvent};
use directories::ProjectDirs; // Changed from BaseDirs to ProjectDirs for runtime path
use std::path::{Path, PathBuf};
use tokio::io::{AsyncWriteExt, AsyncReadExt, BufReader, AsyncBufReadExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot}; // Import mpsc and oneshot
use log::{info, warn, error}; // Keep log imports

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
    socket_path: PathBuf,
    event_sender: EventSender,
}

impl SocketHandler {
    pub async fn new(config_socket_path: Option<&str>, event_sender: EventSender) -> Result<Self> {
        let socket_path = get_socket_path(config_socket_path)?;
        info!("Attempting to bind control socket at: {:?}", socket_path);

        if socket_path.exists() {
            warn!("Existing socket file found at {:?}. Removing.", socket_path);
            std::fs::remove_file(&socket_path)
                .map_err(|e| AppError::Io(e))?;
        }

        if let Some(parent) = socket_path.parent() {
             if !parent.exists() {
                info!("Creating socket directory: {:?}", parent);
                std::fs::create_dir_all(parent)
                    .map_err(|e| AppError::Io(e))?;
             }
        }

        Ok(SocketHandler { socket_path, event_sender })
    }

    pub async fn start(self) -> Result<()> {
        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|e| AppError::Io(e))?;

        info!("Control socket listening on {}", self.socket_path.display());

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let sender = self.event_sender.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, sender).await {
                            error!("Error handling socket connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    async fn handle_connection(mut stream: UnixStream, sender: EventSender) -> Result<()> {
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();

        match reader.read_line(&mut line).await {
            Ok(0) => Ok(()),
            Ok(_) => {
                let command = line.trim();
                info!("Received command: {}", command);

                match command {
                    "reload" => {
                        sender.send(SystemEvent::Control(ControlCommand::Reload)).await
                            .map_err(|e| AppError::MpscSendError(format!("Failed to send Reload command: {}", e)))?;
                        stream.write_all(b"OK: Reload command sent\n").await
                            .map_err(|e| AppError::Io(e))?;
                    }
                    "status" => {
                        let (tx, rx) = oneshot::channel();
                        sender.send(SystemEvent::Control(ControlCommand::Status { response_tx: tx })).await
                             .map_err(|e| AppError::MpscSendError(format!("Failed to send Status command: {}", e)))?;
                        match rx.await {
                            Ok(status_response) => {
                                stream.write_all(status_response.as_bytes()).await
                                     .map_err(|e| AppError::Io(e))?;
                                stream.write_all(b"\n").await.map_err(|e| AppError::Io(e))?;
                            }
                            Err(e) => {
                                let err_msg = format!("Failed to receive status response: {}", e);
                                error!("{}", err_msg);
                                stream.write_all(format!("ERROR: {}\n", err_msg).as_bytes()).await
                                      .map_err(|e| AppError::Io(e))?;
                                return Err(AppError::ChannelRecvError(err_msg));
                            }
                        }
                    }
                    "ping" => {
                        let (tx, rx) = oneshot::channel();
                        sender.send(SystemEvent::Control(ControlCommand::Ping { response_tx: tx })).await
                             .map_err(|e| AppError::MpscSendError(format!("Failed to send Ping command: {}", e)))?;
                        match rx.await {
                            Ok(ping_response) => {
                                stream.write_all(ping_response.as_bytes()).await
                                     .map_err(|e| AppError::Io(e))?;
                                stream.write_all(b"\n").await.map_err(|e| AppError::Io(e))?;
                            }
                            Err(e) => {
                                let err_msg = format!("Failed to receive ping response: {}", e);
                                error!("{}", err_msg);
                                stream.write_all(format!("ERROR: {}\n", err_msg).as_bytes()).await
                                      .map_err(|e| AppError::Io(e))?;
                                return Err(AppError::ChannelRecvError(err_msg));
                            }
                        }
                    }
                    "shutdown" => {
                        info!("Shutdown command received via socket.");
                         sender.send(SystemEvent::Control(ControlCommand::Shutdown)).await
                             .map_err(|e| AppError::MpscSendError(format!("Failed to send Shutdown command: {}", e)))?;
                         stream.write_all(b"OK: Shutdown command sent\n").await
                             .map_err(|e| AppError::Io(e))?;
                    }
                    _ => {
                        stream.write_all(b"ERROR: Unknown command\n").await
                             .map_err(|e| AppError::Io(e))?;
                    }
                }
                Ok(())
            }
            Err(e) => Err(AppError::Io(e)),
        }
    }
}

// Note: Testing socket interaction often requires integration tests or mocking frameworks.
// Basic unit tests might focus on command parsing if extracted.
