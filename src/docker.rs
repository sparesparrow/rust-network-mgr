use bollard::Docker;
// Revert wildcard, use specific imports
use bollard::models::EventMessageTypeEnum;
use bollard::service::EventMessage; // Use this type for events
// use bollard::models::*; // Removed wildcard
use bollard::system::EventsOptions;
use futures_util::stream::StreamExt;
use tokio::sync::mpsc;
use crate::types::{Result, AppError, DockerEvent, EventSender, SystemEvent};
use log::{info, error, warn};
use bollard::container::InspectContainerOptions;
use std::collections::HashMap;
use std::net::IpAddr; // Import IpAddr for parsing

/// Monitors Docker events via the Docker daemon socket.
pub struct DockerMonitor {
    docker: Docker,
    event_tx: mpsc::Sender<SystemEvent>,
}

impl DockerMonitor {
    /// Creates a new DockerMonitor and connects to the Docker daemon.
    /// Defaults to the standard Unix socket path `/var/run/docker.sock`.
    pub fn new(event_tx: mpsc::Sender<SystemEvent>) -> Result<Self> {
        // Attempt to connect via the default Unix socket
        let docker = Docker::connect_with_unix_defaults()
            .map_err(|e| AppError::DockerError(format!("Failed to connect to Docker socket: {}", e)))?;
        info!("Successfully connected to Docker daemon.");
        Ok(Self { docker, event_tx })
    }

    /// Starts the Docker event monitoring loop.
    /// Listens for container start/stop events and sends DockerEvents.
    pub async fn start(self) -> Result<()> {
        info!("Starting Docker event listener...");

        // Filter for specific container events
        let mut filters = HashMap::new();
        filters.insert("type".to_string(), vec!["container".to_string()]);
        filters.insert("event".to_string(), vec!["start".to_string(), "stop".to_string(), "die".to_string()]);

        let options = EventsOptions::<String> {
            since: None,
            until: None,
            filters,
        };

        let mut event_stream = self.docker.events(Some(options));

        while let Some(event_result) = event_stream.next().await {
            match event_result {
                Ok(event) => {
                    if let Err(e) = self.handle_event(event).await {
                         warn!("Error handling Docker event: {}", e); // Log error but continue
                    }
                }
                Err(e) => {
                    error!("Error receiving Docker event stream: {}", e);
                    return Err(AppError::DockerError(format!("Docker event stream error: {}", e)));
                }
            }
        }

        warn!("Docker event stream ended unexpectedly.");
        Ok(())
    }

    /// Processes a single Docker event.
    async fn handle_event(&self, event: EventMessage) -> Result<()> {
        match (event.typ, event.action.as_deref()) {
            (Some(EventMessageTypeEnum::CONTAINER), Some("start")) => {
                if let Some(actor) = event.actor {
                    let container_id = actor.id.unwrap_or_else(|| "Unknown".to_string());
                    info!("Docker container started: {}", container_id);

                    let ip_address_str = match self.get_container_ip(&container_id).await {
                        Ok(ip) => ip,
                        Err(e) => {
                            warn!("Failed to inspect container {} for IP: {}", container_id, e);
                            None
                        }
                    };

                    // Parse the string IP into Option<IpAddr>
                    let ip_address: Option<IpAddr> = ip_address_str
                        .clone()
                        .and_then(|ip_str| ip_str.parse().ok());

                    if ip_address.is_none() && ip_address_str.is_some() {
                        warn!("Failed to parse IP address string: {}", ip_address_str.unwrap());
                    }

                    self.event_tx.send(SystemEvent::Docker(crate::types::DockerEvent::ContainerStarted(container_id, ip_address))).await
                        .map_err(|e| AppError::MpscSendError(format!("Failed to send DockerEvent::ContainerStarted: {}", e)))?;
                }
            }
            (Some(EventMessageTypeEnum::CONTAINER), Some("stop")) | (Some(EventMessageTypeEnum::CONTAINER), Some("die")) => {
                 if let Some(actor) = event.actor {
                     let container_id = actor.id.unwrap_or_else(|| "Unknown".to_string());
                     info!("Docker container stopped/died: {}", container_id);
                     self.event_tx.send(SystemEvent::Docker(crate::types::DockerEvent::ContainerStopped(container_id))).await
                         .map_err(|e| AppError::MpscSendError(format!("Failed to send DockerEvent::ContainerStopped: {}", e)))?;
                 }
            }
             // TODO: Handle other events like network connect/disconnect if needed for IP discovery
            _ => {
                 // Ignore other event types/actions for now
            }
        }
        Ok(())
    }

    /// Inspects a container and retrieves its primary IP address.
    /// Returns the IP address as a String if found, otherwise None.
    async fn get_container_ip(&self, container_id: &str) -> Result<Option<String>> {
        info!("Inspecting container {} for IP address...", container_id);
        let options = InspectContainerOptions { size: false };
        match self.docker.inspect_container(container_id, Some(options)).await {
            Ok(inspect_info) => {
                // Try to find an IP address in the network settings
                if let Some(network_settings) = inspect_info.network_settings {
                    // Check the default network IP first
                    if let Some(ip) = network_settings.ip_address {
                        if !ip.is_empty() {
                            info!("Found default IP {} for container {}", ip, container_id);
                            return Ok(Some(ip));
                        }
                    }
                    // If not found, check connected networks
                    if let Some(networks) = network_settings.networks {
                        // Iterate through connected networks (e.g., bridge, host, custom)
                        for (network_name, network_data) in networks {
                            if let Some(ip) = network_data.ip_address {
                                if !ip.is_empty() {
                                    info!("Found IP {} for container {} in network \"{}\"", ip, container_id, network_name);
                                    return Ok(Some(ip));
                                }
                            }
                        }
                    }
                }
                warn!("No IP address found for container {} in inspect details.", container_id);
                Ok(None) // No IP found in the expected places
            }
            Err(e) => {
                error!("Failed to inspect container {}: {}", container_id, e);
                Err(AppError::DockerError(format!("Bollard inspect error: {}", e)))
            }
        }
    }
} 