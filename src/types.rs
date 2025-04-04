use serde::Deserialize;
use std::net::IpAddr;
use thiserror::Error;
use tokio::sync::mpsc; // For channels
use rustables; // ADDED

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Network monitoring error: {0}")]
    Network(#[from] rtnetlink::Error),
    #[error("NFTables error: {0}")]
    Nftables(#[from] rustables::error::QueryError),
    #[error("Socket error: {0}")]
    Socket(#[from] std::io::Error),
    #[error("Initialization error: {0}")]
    Init(String),
    #[error("Channel send error: {0}")]
    ChannelSend(String),
    #[error("Anyhow error: {0}")]
    Anyhow(#[from] anyhow::Error), // General errors
}

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkState {
    // Example: Store current IPs per interface managed
    pub interface_ips: std::collections::HashMap<String, Vec<IpAddr>>,
    // Maps nftables_zone to its current list of IP addresses (Calculated)
    pub zone_ips: std::collections::HashMap<String, Vec<IpAddr>>,
}

impl NetworkState {
    pub fn new() -> Self {
        Self::default()
    }
}

// --- Events and Commands ---

#[derive(Debug, Clone)]
pub enum NetworkEvent {
    IpAdded(String, IpAddr),   // interface_name, ip_address
    IpRemoved(String, IpAddr), // interface_name, ip_address
    // Add other events like InterfaceUp, InterfaceDown if needed
}

#[derive(Debug, Clone)]
pub enum ControlCommand {
    Reload,
    Status,
    Ping,
    Shutdown, // Graceful shutdown command
}

// --- Type Aliases ---
pub type Result<T> = std::result::Result<T, AppError>;
pub type NetworkEventSender = mpsc::Sender<NetworkEvent>;
pub type ControlCommandReceiver = mpsc::Receiver<ControlCommand>;
pub type ControlCommandSender = mpsc::Sender<ControlCommand>;

#[derive(thiserror::Error, Debug)]
pub enum NetworkError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Network manager error: {0}")]
    NetworkManager(String),
    #[error("Rustables error: {0}")]
    Rustables(#[from] rustables::error::QueryError),
    #[error("Rustables builder error: {0}")]
    RustablesBuilder(#[from] rustables::error::BuilderError),
    // ... existing code ...
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub network_state: NetworkState,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        AppState {
            config,
            network_state: NetworkState::new(),
        }
    }
}
