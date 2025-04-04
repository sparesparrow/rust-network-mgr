//! Core types for the application, including configuration, errors, and events.

use serde::Deserialize;
use std::net::IpAddr;
use thiserror::Error;
use tokio::sync::mpsc; // For channels
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use std::path::PathBuf;
use tokio::sync::oneshot;
use bollard;

/// Central application error type.
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Configuration parsing error: {0}")]
    ConfigParse(#[from] serde_yaml::Error),
    #[error("Configuration file IO error: {0}")]
    ConfigIo(String),
    #[error("Configuration validation error: {0}")]
    ConfigValidation(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Nftables helper error: {0}")]
    NftablesError(#[from] nftables::helper::NftablesError),
    #[error("Docker interaction error: {0}")]
    DockerError(String),
    #[error("Error receiving Docker event stream: {0}")]
    DockerStream(#[from] bollard::errors::Error),
    #[error("Netlink error: {0}")]
    Netlink(String),
    #[error("RtNetlink error: {0}")]
    RtNetlink(#[from] rtnetlink::Error),
    #[error("Tokio MPSC channel send error: {0}")]
    MpscSendError(String),
    #[error("Channel receive error: {0}")]
    ChannelRecvError(String),
    #[error("Oneshot channel send error: {0}")]
    OneshotSendError(String),
    #[error("Anyhow error: {0}")]
    Anyhow(#[from] anyhow::Error),
}

// Define Result type alias correctly
pub type Result<T> = std::result::Result<T, AppError>;

// --- Configuration Types ---

#[derive(Debug, Deserialize, Clone)]
pub struct InterfaceConfig {
    pub name: String,
    pub dhcp: Option<bool>, // Use Option for flexibility
    pub address: Option<String>, // e.g., "192.168.1.1/24"
    pub nftables_zone: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub interfaces: Vec<InterfaceConfig>,
    // Add other global settings if needed, e.g., log level, socket path
    pub socket_path: Option<String>,
    pub nftables_rules_path: Option<String>, // Path to rule templates/scripts
}

// --- Network State ---

/// Represents the overall network state, including interface IPs.
#[derive(Debug, Default, Clone)]
pub struct NetworkState {
    pub interface_ips: HashMap<String, Vec<IpAddr>>, // Interface name -> IPs
    pub if_index_to_name: HashMap<u32, String>,
    // Potentially add container IPs here later if needed directly for rules
}

/// Represents the shared application state.
#[derive(Debug, Default)] // Removed AppConfig/AppState structs from here as they are separate concerns
pub struct AppStateShared {
    pub config: Arc<AsyncMutex<Vec<InterfaceConfig>>>,
    pub network_state: Arc<AsyncMutex<NetworkState>>,
    pub container_ips: Arc<AsyncMutex<HashMap<String, Option<IpAddr>>>>,
}

// --- Events and Commands ---

#[derive(Debug, Clone)]
pub enum NetworkEvent {
    IpUpdate { interface: String, ips: Vec<IpAddr> },
    LinkChanged { name: String, is_up: bool },
}

#[derive(Debug)]
pub enum ControlCommand {
    Reload,
    Status { response_tx: oneshot::Sender<String> },
    Ping { response_tx: oneshot::Sender<String> },
    Shutdown, // Graceful shutdown command
}

// Define EventSender type alias correctly
pub type EventSender = mpsc::Sender<SystemEvent>;
pub type EventReceiver = mpsc::Receiver<SystemEvent>; // Keep receiver alias

/// Events related to Docker containers.
#[derive(Debug, Clone)]
pub enum DockerEvent {
    ContainerStarted(String, Option<IpAddr>), // Container ID, Optional IP Address
    ContainerStopped(String),                // Container ID
}

/// Events processed by the main application loop.
#[derive(Debug)]
pub enum SystemEvent {
    Network(NetworkEvent),
    Docker(DockerEvent),
    Control(ControlCommand),
    Signal(i32),
}

// From impl might need adjustment if ControlCommand is no longer Clone
// but it's consumed here, so should be okay.
impl From<ControlCommand> for SystemEvent {
    fn from(cmd: ControlCommand) -> Self {
        SystemEvent::Control(cmd)
    }
}
