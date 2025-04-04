// Use the library crate
use rust_network_mgr::config::load_config;

use rust_network_mgr::network::NetworkMonitor;
use rust_network_mgr::nftables::NftablesManager;
use rust_network_mgr::socket::SocketHandler;
use rust_network_mgr::types::{AppConfig, ControlCommand, NetworkEvent, Result, InterfaceConfig, NetworkState, SystemEvent, EventSender};
use tokio::sync::mpsc::{channel, Sender, Receiver};

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::{Mutex};
use tokio::signal::unix::{signal, SignalKind};
use log::{info, error}; // Removed warn
use tracing; // Assuming tracing is used

// Channel buffer sizes
const EVENT_CHANNEL_SIZE: usize = 100;
const COMMAND_CHANNEL_SIZE: usize = 10;

// Define AppState here since it's not in the types module
struct AppState {
    config: AppConfig,
    network_state: NetworkState,
    container_ips: HashMap<String, IpAddr>,
}

impl AppState {
    fn new(config: AppConfig) -> Self {
        Self {
            config,
            network_state: NetworkState::default(),
            container_ips: HashMap::new(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Basic logging setup (consider a more robust solution like tracing-subscriber)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    info!("Starting rust-network-mgr...");

    // Set up signal handling
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to create SIGTERM signal stream");
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to create SIGINT signal stream");

    let initial_config = load_initial_config()?;

    // -- Create Communication Channels --
    // Channel for system events
    let (event_tx, mut event_rx): (EventSender, Receiver<SystemEvent>) = channel(EVENT_CHANNEL_SIZE);
    
    // Create and initialize components
    let network_monitor = NetworkMonitor::new(event_tx.clone());

    let interface_config_arc = Arc::new(Mutex::new(initial_config.interfaces.clone()));
    let nftables_manager = Arc::new(NftablesManager::new(interface_config_arc.clone()).await?);
    let socket_handler = SocketHandler::new(initial_config.socket_path.as_deref(), event_tx.clone()).await?;
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

    // Docker Monitor (streamlined implementation - all events go through the main event channel)
    let docker_handle = match rust_network_mgr::docker::DockerMonitor::new(event_tx.clone()) {
        Ok(monitor) => {
            info!("Starting Docker monitor...");
            Some(tokio::spawn(async move {
                if let Err(e) = monitor.start().await {
                    error!("Docker monitor failed: {}", e);
                }
            }))
        },
        Err(e) => {
            error!("Failed to initialize Docker Monitor: {}. Docker monitoring disabled.", e);
            None // Continue without Docker monitoring if it fails
        }
    };

    // -- Start Background Tasks --
    info!("Starting background tasks...");

    // Start Network Monitor
    info!("Starting network monitor...");
    let monitor_handle = tokio::spawn(async move {
        if let Err(e) = network_monitor.start().await {
            error!("Network monitor failed: {}", e);
        }
    });

    // Start Socket Handler
    info!("Starting socket handler...");
    let socket_handle = tokio::spawn(async move {
        if let Err(e) = socket_handler.start().await {
            error!("Socket handler failed: {}", e);
        }
    });

    // -- Main Event Loop --
    info!("Starting main event loop...");
    loop {
        tokio::select! {
            // Main event channel handles all events now
            Some(event) = event_rx.recv() => {
                match event {
                    SystemEvent::Network(network_event) => {
                        info!("Received network event: {:?}", network_event);
                        // Spawn a task to handle the network event asynchronously
                        let state_clone = app_state.clone();
                        let nft_manager_clone = nftables_manager.clone(); // Clone Arc
                        let config_clone = interface_config_arc.clone(); // Clone Arc
                        tokio::spawn(async move {
                            handle_network_event(network_event, nft_manager_clone, state_clone, config_clone).await;
                        });
                    },
                    SystemEvent::Docker(docker_event) => {
                        info!("Received Docker event: {:?}", docker_event);
                        // Acquire lock once to update state based on event
                        let mut state = app_state.lock().await;
                        match docker_event {
                            rust_network_mgr::types::DockerEvent::ContainerStarted(id, Some(ip)) => {
                                info!("Handling Docker Container Started: {} (IP: {})", id, ip);
                                state.container_ips.insert(id.clone(), ip);
                                // TODO: Potentially trigger nftables update if container IPs affect rules
                                info!("Updated AppState container IPs: {:?}", state.container_ips);
                            }
                            rust_network_mgr::types::DockerEvent::ContainerStarted(id, None) => {
                                info!("Handling Docker Container Started: {} (No IP found)", id);
                                // No IP to add to state
                            }
                            rust_network_mgr::types::DockerEvent::ContainerStopped(id) => {
                                info!("Handling Docker Container Stopped: {}", id);
                                if state.container_ips.remove(&id).is_some() {
                                    // TODO: Potentially trigger nftables update if container IPs affect rules
                                    info!("Removed container {} from AppState IPs. Current: {:?}", id, state.container_ips);
                                } else {
                                    info!("Container {} not found in AppState IPs.", id);
                                }
                            }
                        }
                        // Lock is released when `state` goes out of scope
                    },
                    SystemEvent::Control(command) => {
                        info!("Received control command: {:?}", command);
                        match command {
                            ControlCommand::Reload => {
                                info!("Reload command received. Reloading configuration and applying rules...");
                                let config_result = load_initial_config(); // Reload config
                                let nft_manager = nftables_manager.clone(); // Clone Arc for async block
                                let state_clone = app_state.clone(); // Clone Arc for async block

                                tokio::spawn(async move {
                                    match config_result {
                                        Ok(new_config) => {
                                            let mut state = state_clone.lock().await;
                                            state.config = new_config;
                                            // Optionally update interface_config_arc if NftablesManager needs it dynamically
                                            // let mut if_config = interface_config_arc.lock().await;
                                            // *if_config = state.config.interfaces.clone();
                                            // drop(if_config);
                                            
                                            // Re-apply rules based on current state
                                            // The NftablesManager uses its internally stored config reference
                                            if let Err(e) = nft_manager.apply_rules(&state.network_state).await {
                                                error!("Error applying rules after reload: {}", e);
                                            }
                                            info!("Configuration reloaded and rules re-applied.");
                                        }
                                        Err(e) => {
                                            error!("Failed to reload configuration: {}", e);
                                        }
                                    }
                                });
                            }
                            ControlCommand::Status { response_tx } => {
                                info!("Status command received.");
                                let state = app_state.lock().await;
                                // Format the status string
                                let interface_status = state.network_state.interface_ips.iter()
                                    .map(|(name, ips)| format!("  {}: {:?}", name, ips))
                                    .collect::<Vec<String>>().join("\n");
                                let container_status = state.container_ips.iter()
                                    .map(|(id, ip)| format!("  {}: {}", id, ip))
                                    .collect::<Vec<String>>().join("\n");
                                
                                let status_report = format!(
                                    "Current Status:\nInterfaces:\n{}\nTracked Containers:\n{}",
                                    if interface_status.is_empty() { "  (None)" } else { &interface_status },
                                    if container_status.is_empty() { "  (None)" } else { &container_status }
                                );
                                
                                // Send the report back to the socket handler
                                if let Err(_) = response_tx.send(status_report) {
                                    error!("Failed to send status response back to socket handler.");
                                }
                            }
                            ControlCommand::Ping { response_tx } => {
                                info!("Ping command received.");
                                // Send pong back
                                if let Err(_) = response_tx.send("PONG".to_string()) {
                                    error!("Failed to send pong response back to socket handler.");
                                }
                            }
                            ControlCommand::Shutdown => { // Added for completeness, might be handled by signals mainly
                                info!("Shutdown command received via socket. Initiating graceful shutdown...");
                                break; // Exit the main loop
                            }
                        }
                    },
                    SystemEvent::Signal(sig) => {
                        info!("Received signal {}, initiating graceful shutdown...", sig);
                        break;
                    }
                }
            }

            // --- Termination Signals --- 
            _ = sigterm.recv() => {
                info!("Received SIGTERM. Initiating graceful shutdown...");
                break;
            }
            _ = sigint.recv() => {
                info!("Received SIGINT. Initiating graceful shutdown...");
                break;
            }

            else => {
                info!("All channels closed, shutting down.");
                break;
            }
        }
    }

    // --- Shutdown Process --- 
    info!("Shutting down background tasks...");
    monitor_handle.abort();
    socket_handle.abort();
    if let Some(handle) = docker_handle {
        handle.abort(); // Abort Docker monitor task
        let _ = handle.await; // Optionally wait, ignoring cancellation error
        info!("Docker monitor task shut down.");
    }

    info!("Shutdown complete.");
    Ok(())
}

fn load_initial_config() -> Result<AppConfig> {
    let config = load_config(None).map_err(|e| 
        rust_network_mgr::types::AppError::ConfigIo(format!("Failed to load configuration: {}", e)))?;
    
    // Validation will happen inside load_config now
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
        NetworkEvent::IpUpdate { interface, ips } => {
            // Update interface IPs directly
            state_guard.network_state.interface_ips.insert(interface.clone(), ips.clone());
            tracing::debug!("State updated for interface {} with IPs {:?}", interface, ips);
            None // No interface to remove
        }
        NetworkEvent::LinkChanged { name, is_up } => {
            tracing::debug!("Interface {} state changed, is_up: {}", name, is_up);
            if !is_up {
                // If interface went down, consider removing its IPs
                Some(name)
            } else {
                None
            }
        }
    };

    // Remove the interface entry outside the main borrow if necessary
    if let Some(if_name_to_remove) = if_name_for_removal {
        state_guard.network_state.interface_ips.remove(&if_name_to_remove);
        tracing::debug!("Removed interface {} from state as it went down.", if_name_to_remove);
    }

    // Clone the relevant state *before* dropping the lock
    let current_network_state = state_guard.network_state.clone();
    drop(state_guard); // Drop the lock before await

    // Apply nftables rules based on the updated state
    tracing::debug!("Applying NFT rules for state: {:?}", current_network_state);
    // Pass only the network state; NftablesManager uses its internal config
    if let Err(e) = nft_manager.apply_rules(&current_network_state).await {
        error!("Failed to apply nftables rules after IP update: {}", e);
        // Handle error appropriately
    }
}
