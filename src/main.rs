// Use the library crate
use rust_network_mgr::config::{load_config, validate_config};
use rust_network_mgr::network::NetworkMonitor;
use rust_network_mgr::nftables::NftablesManager;
use rust_network_mgr::socket::SocketHandler;
use rust_network_mgr::types::{AppConfig, AppError, ControlCommand, NetworkEvent, Result, InterfaceConfig, AppState};

use std::collections::HashMap; // Keep this if AppState uses it directly
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::signal::unix::{signal, SignalKind};
use log::{info, error}; // Removed warn
use tracing; // Assuming tracing is used

// Channel buffer sizes
const EVENT_CHANNEL_SIZE: usize = 100;
const COMMAND_CHANNEL_SIZE: usize = 10;

#[tokio::main]
async fn main() -> Result<()> {
    // Basic logging setup (consider a more robust solution like tracing-subscriber)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    info!("Starting rust-network-mgr...");

    let initial_config = load_initial_config()?;

    let (network_tx, mut network_rx) = mpsc::channel::<NetworkEvent>(EVENT_CHANNEL_SIZE);
    let (control_tx, mut control_rx) = mpsc::channel::<ControlCommand>(COMMAND_CHANNEL_SIZE);

    // Create and initialize components
    // Assume NetworkMonitor::new returns Self directly now based on test errors
    let network_monitor = NetworkMonitor::new(network_tx.clone());

    let interface_config_arc = Arc::new(Mutex::new(initial_config.interfaces.clone()));
    let nftables_manager = Arc::new(NftablesManager::new(interface_config_arc.clone()).await?);
    let socket_handler = SocketHandler::new(initial_config.socket_path.as_deref(), control_tx.clone()).await?;
    let initial_state = AppState::new(initial_config.clone()); 
    let app_state = Arc::new(Mutex::new(initial_state));

    // Load initial NFT rules
    info!("Loading initial nftables rules...");
    if let Err(e) = nftables_manager.load_rules().await {
        error!("Failed to load initial nftables rules: {}", e);
        // Consider if this is fatal
    }

    // Apply initial rules based on current (empty) state if needed
    {
        let state_guard = app_state.lock().await;
        info!("Applying initial nftables rules based on state: {:?}", state_guard.network_state);
        if let Err(e) = nftables_manager.apply_rules(&state_guard.network_state).await {
            error!("Failed to apply initial nftables rules: {}", e);
            // Consider if fatal
        }
    }

    // Start tasks
    info!("Starting network monitor...");
    let monitor_handle = tokio::spawn(async move {
        if let Err(e) = network_monitor.start().await {
            error!("Network monitor failed: {}", e);
        }
    });

    info!("Starting socket handler...");
    let socket_handle = tokio::spawn(async move {
        // Call start directly, assuming it handles errors internally or returns ()
        socket_handler.start().await;
        info!("Socket handler task finished (assuming clean exit or internal error handling).");
    });

    info!("Application initialized successfully. Waiting for events...");

    // Setup signal handling for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    loop {
        tokio::select! {
            Some(event) = network_rx.recv() => {
                info!("Received network event: {:?}", event);
                // Spawn a task to handle the network event asynchronously
                let state_clone = app_state.clone();
                let nft_manager_clone = nftables_manager.clone(); // Clone Arc
                let config_clone = interface_config_arc.clone(); // Clone Arc
                tokio::spawn(async move {
                    handle_network_event(event, nft_manager_clone, state_clone, config_clone).await;
                });
            }
            Some(command) = control_rx.recv() => {
                info!("Received control command: {:?}", command);
                match command {
                    ControlCommand::Reload => {
                        info!("Reload command received. Reloading configuration...");
                        match load_config(None) { // Assuming None uses default path
                            Ok(new_config) => {
                                let mut state = app_state.lock().await;
                                state.config = new_config.clone(); // Update config in state
                                *interface_config_arc.lock().await = new_config.interfaces; // Update Arc for nftables
                                info!("Configuration reloaded.");
                                // Optionally re-apply rules based on new config/state
                                if let Err(e) = nftables_manager.apply_rules(&state.network_state).await {
                                    error!("Failed to apply rules after reload: {}", e);
                                }
                            }
                            Err(e) => {
                                error!("Failed to reload configuration: {}", e);
                            }
                        }
                    }
                    ControlCommand::Status => {
                        let state = app_state.lock().await;
                        info!("Current Status: Network State: {:?}, Config: {:?}", state.network_state, state.config);
                        // Respond via socket if needed
                    }
                    ControlCommand::Ping => {
                         info!("Ping received");
                         // Respond via socket
                    }
                    ControlCommand::Shutdown => {
                        info!("Shutdown command received. Initiating graceful shutdown...");
                        break; // Exit the loop
                    }
                }
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM. Initiating graceful shutdown...");
                break;
            }
            _ = sigint.recv() => {
                info!("Received SIGINT. Initiating graceful shutdown...");
                break;
            }
        }
    }

    info!("Shutting down gracefully...");
    // Add any cleanup logic here (e.g., stopping tasks, cleaning up resources)
    monitor_handle.abort();
    socket_handle.abort();
    info!("Shutdown complete.");

    Ok(())
}

fn load_initial_config() -> Result<AppConfig> {
    let config = load_config(None).map_err(|e| AppError::Config(e.to_string()))?;
    validate_config(&config).map_err(|e| AppError::Config(e.to_string()))?; // Use imported validate_config
    Ok(config)
}

/// Updates the network state based on an event.
async fn handle_network_event(
    event: NetworkEvent,
    nft_manager: Arc<NftablesManager>,
    shared_state: Arc<Mutex<AppState>>,
    _config: Arc<Mutex<Vec<InterfaceConfig>>> // Prefix unused parameter
) {
    tracing::debug!("Handling network event: {:?}", event);
    let mut state_guard = shared_state.lock().await;
    let if_name_for_removal: Option<String> = match event {
        NetworkEvent::IpAdded(if_name, ip) => {
            let ips = state_guard.network_state.interface_ips.entry(if_name.clone()).or_default();
            if !ips.contains(&ip) {
                ips.push(ip);
                tracing::debug!("State updated: Added IP {} to {}", ip, if_name);
            }
            None // No interface to remove
        }
        NetworkEvent::IpRemoved(if_name, ip) => {
            let mut should_remove_entry = false;
            if let Some(ips) = state_guard.network_state.interface_ips.get_mut(&if_name) {
                if let Some(pos) = ips.iter().position(|&x| x == ip) {
                    ips.remove(pos);
                    tracing::debug!("State updated: Removed IP {} from {}", ip, if_name);
                    if ips.is_empty() {
                        should_remove_entry = true;
                    }
                }
            }
            if should_remove_entry {
                Some(if_name) // Return the name to remove after the borrow
            } else {
                None
            }
        }
    };

    // Remove the interface entry outside the main borrow if necessary
    if let Some(if_name_to_remove) = if_name_for_removal {
        state_guard.network_state.interface_ips.remove(&if_name_to_remove);
        tracing::debug!("Removed interface {} from state as it has no IPs.", if_name_to_remove);
    }

    // Recalculate zone IPs based on the updated interface IPs
    let mut new_zone_ips: HashMap<String, Vec<IpAddr>> = HashMap::new();
    let locked_config = _config.lock().await; // Lock config needed for zone mapping
    for iface_config in locked_config.iter() {
        if let Some(zone) = &iface_config.nftables_zone {
            if let Some(ips) = state_guard.network_state.interface_ips.get(&iface_config.name) {
                 let zone_ips_entry = new_zone_ips.entry(zone.clone()).or_default();
                 for ip in ips {
                     if !zone_ips_entry.contains(ip) {
                         zone_ips_entry.push(*ip);
                     }
                 }
            }
        }
    }
    state_guard.network_state.zone_ips = new_zone_ips;

    // Clone the relevant state *before* dropping the lock
    let current_network_state = state_guard.network_state.clone();
    drop(state_guard); // Drop the lock before await

    // Apply nftables rules based on the updated state
    tracing::debug!("Applying NFT rules for state: {:?}", current_network_state);
    if let Err(e) = nft_manager.apply_rules(&current_network_state).await {
        error!("Failed to apply nftables rules after IP update: {}", e);
        // Handle error appropriately
    }
}
