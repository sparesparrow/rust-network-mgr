mod config;
mod network;
mod nftables;
mod socket;
mod types;

use crate::config::{load_config, validate_config};
use crate::network::NetworkMonitor;
use crate::nftables::NftablesManager;
use crate::socket::SocketHandler;
use crate::types::{AppConfig, AppError, ControlCommand, NetworkEvent, NetworkState, Result};

use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::signal::unix::{signal, SignalKind};

// Channel buffer sizes
const EVENT_CHANNEL_SIZE: usize = 100;
const COMMAND_CHANNEL_SIZE: usize = 10;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Rust Network Manager...");

    // --- Initialization ---
    let app_state = Arc::new(Mutex::new(AppState::new()));
    let initial_config = load_initial_config(None)?; // Load initial config (can take path arg later)

    // Create communication channels
    let (network_tx, mut network_rx) = mpsc::channel::<NetworkEvent>(EVENT_CHANNEL_SIZE);
    let (control_tx, mut control_rx) = mpsc::channel::<ControlCommand>(COMMAND_CHANNEL_SIZE);

    // Create and initialize components
    let network_monitor = NetworkMonitor::new(network_tx);
    let mut nftables_manager = NftablesManager::new(initial_config.clone());
    let socket_handler = SocketHandler::new(initial_config.socket_path.as_deref(), control_tx.clone()).await?;

    // Load initial NFT rules (placeholder)
    nftables_manager.load_rules()?;

    // Apply initial rules based on potentially empty state (or state from monitor startup)
    // Note: NetworkMonitor populates its internal state first.
    // A more robust approach might involve getting initial state from monitor *before* starting its event loop.
    { 
        let state = app_state.lock().await.network_state.clone();
        nftables_manager.apply_rules(&state).await.map_err(|e| {
            tracing::error!("Failed to apply initial NFTables rules: {}", e);
            AppError::Nftables(format!("Initial apply failed: {}", e))
        })?;
    }
    tracing::info!("Initial setup complete.");

    // --- Spawn Tasks ---
    let monitor_handle = tokio::spawn(async move {
        if let Err(e) = network_monitor.start().await {
            tracing::error!("Network monitor failed: {}", e);
        }
        tracing::info!("Network monitor task finished.");
    });

    let socket_handle = tokio::spawn(async move {
        socket_handler.start().await;
        tracing::info!("Socket handler task finished.");
    });

    // --- Signal Handling ---
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    // --- Main Event Loop ---
    tracing::info!("Entering main event loop...");
    loop {
        tokio::select! {
            // Listen for network events
            Some(event) = network_rx.recv() => {
                tracing::debug!("Received network event: {:?}", event);
                let mut state_guard = app_state.lock().await;
                handle_network_event(&mut state_guard.network_state, event);
                // Clone the necessary parts for apply_rules
                let current_state = state_guard.network_state.clone();
                drop(state_guard); // Release lock before await

                if let Err(e) = nftables_manager.apply_rules(&current_state).await {
                    tracing::error!("Failed to apply NFTables rules after network event: {}", e);
                    // Decide on error strategy: continue, retry, shutdown?
                }
            }

            // Listen for control commands
            Some(command) = control_rx.recv() => {
                tracing::info!("Received control command: {:?}", command);
                match command {
                    ControlCommand::Reload => {
                        tracing::info!("Reload command received. Reloading configuration...");
                         match load_initial_config(None) { // Replace None with path if needed
                             Ok(new_config) => {
                                 nftables_manager = NftablesManager::new(new_config);
                                 if let Err(e) = nftables_manager.load_rules() {
                                     tracing::error!("Failed to load NFTables rules during reload: {}", e);
                                     // Potentially revert to old config or state?
                                 } else {
                                     // Re-apply rules with current state and new config
                                     let state_guard = app_state.lock().await;
                                     let current_state = state_guard.network_state.clone();
                                     drop(state_guard);
                                     if let Err(e) = nftables_manager.apply_rules(&current_state).await {
                                         tracing::error!("Failed to apply NFTables rules after reload: {}", e);
                                     }
                                      tracing::info!("Reload complete.");
                                 }
                             }
                             Err(e) => {
                                 tracing::error!("Failed to reload configuration: {}", e);
                                 // Keep using the old configuration
                             }
                         }
                    }
                    ControlCommand::Status => {
                        let state_guard = app_state.lock().await;
                        tracing::info!("Current Network State: {:?}", state_guard.network_state);
                        // TODO: Could send status back via the socket connection if needed (more complex)
                    }
                    ControlCommand::Ping => {
                        // Handled mostly by socket handler, just log here
                        tracing::debug!("Ping command processed.");
                    }
                    ControlCommand::Shutdown => {
                        tracing::info!("Shutdown command received. Initiating graceful shutdown...");
                        break; // Exit the main loop
                    }
                }
            }

            // Listen for termination signals
            _ = sigint.recv() => {
                tracing::info!("Received SIGINT. Initiating graceful shutdown...");
                 // Send shutdown command to self to trigger cleanup
                 if let Err(e) = control_tx.send(ControlCommand::Shutdown).await {
                     tracing::error!("Failed to send shutdown command internally: {}", e);
                     break; // Force break if channel fails
                 }
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM. Initiating graceful shutdown...");
                 // Send shutdown command to self
                 if let Err(e) = control_tx.send(ControlCommand::Shutdown).await {
                     tracing::error!("Failed to send shutdown command internally: {}", e);
                     break; // Force break if channel fails
                 }
            }

            else => {
                tracing::info!("All channels closed or signal handlers errored. Shutting down.");
                break;
            }
        }
    }

    // --- Cleanup --- (Optional: wait for tasks to finish)
    tracing::info!("Shutting down tasks...");
    // monitor_handle.abort(); // Or use a dedicated shutdown signal
    // socket_handle.abort();
    // Add graceful shutdown for tasks if needed
     if let Err(e) = monitor_handle.await {
        tracing::error!("Error joining monitor task: {:?}", e);
    }
     if let Err(e) = socket_handle.await {
        tracing::error!("Error joining socket task: {:?}", e);
    }

    tracing::info!("Rust Network Manager shut down gracefully.");
    Ok(())
}

/// Loads and validates the initial configuration.
fn load_initial_config(path: Option<&Path>) -> Result<AppConfig> {
    let config = load_config(path)?;
    validate_config(&config)?;
    tracing::info!("Configuration loaded and validated successfully.");
    Ok(config)
}

/// Represents the shared state of the application.
pub struct AppState {
    network_state: NetworkState,
    // Potentially add config here if it needs to be mutable and shared
}

impl AppState {
    fn new() -> Self {
        AppState {
            network_state: NetworkState::new(),
        }
    }
}

/// Updates the network state based on an event.
fn handle_network_event(state: &mut NetworkState, event: NetworkEvent) {
    match event {
        NetworkEvent::IpAdded(if_name, ip) => {
            let if_name_clone = if_name.clone(); // Clone before moving into entry()
            let ips = state.interface_ips.entry(if_name).or_default();
            if !ips.contains(&ip) {
                ips.push(ip);
                tracing::debug!("State updated: Added IP {} to {}", ip, if_name_clone);
            }
        }
        NetworkEvent::IpRemoved(if_name, ip) => {
            if let Some(ips) = state.interface_ips.get_mut(&if_name) {
                if let Some(pos) = ips.iter().position(|&x| x == ip) {
                    ips.remove(pos);
                    tracing::debug!("State updated: Removed IP {} from {}", ip, if_name);
                    if ips.is_empty() {
                        state.interface_ips.remove(&if_name);
                        tracing::debug!("Removed interface {} from state as it has no IPs.", if_name);
                    }
                }
            }
        }
    }
}
