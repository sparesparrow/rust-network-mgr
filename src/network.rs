use crate::types::{AppError, NetworkEvent, NetworkEventSender, Result};
use futures::stream::{StreamExt, TryStreamExt};
// Import only the minimum necessary from rtnetlink
use rtnetlink::{
    new_connection,
};
// Import the netlink_packet_core crate directly for the message types
use netlink_packet_core::{
    NetlinkMessage, NetlinkPayload,
};
// Import the netlink_packet_route crate directly for the route-specific types
use netlink_packet_route::{
    address::AddressMessage,
    link::LinkMessage,
    RouteNetlinkMessage,
};
use std::collections::HashMap;
use std::net::IpAddr;

/// Monitors network interface and address changes using rtnetlink.
pub struct NetworkMonitor {
    event_sender: NetworkEventSender,
    // Store interface index to name mapping for easier lookup
    if_index_to_name: HashMap<u32, String>,
    // Store current IPs per interface index
    current_ips: HashMap<u32, Vec<IpAddr>>,
}

impl NetworkMonitor {
    pub fn new(event_sender: NetworkEventSender) -> Self {
        NetworkMonitor {
            event_sender,
            if_index_to_name: HashMap::new(),
            current_ips: HashMap::new(),
        }
    }

    /// Starts the monitoring loop.
    /// This function will run indefinitely until an error occurs or the stream ends.
    pub async fn start(mut self) -> Result<()> {
        tracing::info!("Starting network monitor...");

        // Use new_connection for simple setup
        let (connection, handle, mut messages) = new_connection().map_err(|e| {
             AppError::Init(format!("Failed to create netlink connection: {}", e))
        })?;
        tokio::spawn(connection);
        
        tracing::info!("Listening for netlink address and link events...");

        // --- Initial State Population ---
        tracing::debug!("Gathering initial network state...");

        // 1. Get Interfaces to map index to name
        let mut links = handle.link().get().execute();
        while let Some(link) = links.try_next().await? {
            let mut name = None;
            for nla in link.attributes.iter() {
                if let netlink_packet_route::link::LinkAttribute::IfName(if_name) = nla {
                    name = Some(if_name.clone());
                    break;
                }
            }
            if let Some(name) = name {
                tracing::debug!("Found interface: index={}, name={}", link.header.index, name);
                self.if_index_to_name.insert(link.header.index, name);
            }
        }
        tracing::debug!("Interface map populated: {:?}", self.if_index_to_name);

        // 2. Get Addresses for initial state
        let mut addresses = handle.address().get().execute();
        while let Some(msg) = addresses.try_next().await? {
            let if_index = msg.header.index;
            if let Some(if_name) = self.if_index_to_name.get(&if_index) {
                for nla in msg.attributes.iter() {
                    if let netlink_packet_route::address::AddressAttribute::Address(ip_addr) = nla {
                        let ip = ip_addr;
                        
                        tracing::info!(
                            "Initial state: Found IP {} for interface {} ({})",
                            ip, if_name, if_index
                        );
                        let ips = self.current_ips.entry(if_index).or_default();
                        if !ips.contains(&ip) {
                            ips.push(*ip);
                            // Optionally send initial state as events
                            // self.send_event(NetworkEvent::IpAdded(if_name.clone(), *ip)).await?;
                        }
                    }
                }
            }
        }
         tracing::debug!("Initial IP state populated: {:?}", self.current_ips);

        // --- Listen for Events ---
        loop {
            match messages.next().await {
                Some((message, _addr)) => {
                    if let Err(e) = self.handle_netlink_message(message).await {
                        tracing::error!("Error handling netlink message: {}", e);
                    }
                }
                None => {
                     tracing::warn!("Netlink message stream ended unexpectedly.");
                     break;
                }
            }
        }

        Ok(())
    }

    async fn handle_netlink_message(&mut self, message: NetlinkMessage<RouteNetlinkMessage>) -> Result<()> {
         match message.payload {
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewAddress(msg)) => {
                self.handle_address_change(msg, true).await?;
            }
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelAddress(msg)) => {
                self.handle_address_change(msg, false).await?;
            }
             NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(msg)) => {
                self.handle_link_change(msg, true).await?;
            }
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelLink(msg)) => {
                self.handle_link_change(msg, false).await?;
            }
            NetlinkPayload::Error(err) => {
                tracing::error!("Received netlink error message: {:?}", err);
            }
            _ => {
                // tracing::trace!("Ignoring other netlink message type: {:?}", message.payload);
            }
         }
         Ok(())
    }

    async fn handle_address_change(&mut self, msg: AddressMessage, is_add: bool) -> Result<()> {
        let if_index = msg.header.index;

        if let Some(if_name) = self.if_index_to_name.get(&if_index).cloned() {
            for nla in msg.attributes.iter() {
                if let netlink_packet_route::address::AddressAttribute::Address(ip_addr) = nla {
                    let ip = ip_addr;

                    if is_add {
                        tracing::info!("Detected IP Added: {} on {}", ip, if_name);
                        let ips = self.current_ips.entry(if_index).or_default();
                        if !ips.contains(&ip) {
                             ips.push(*ip);
                             self.send_event(NetworkEvent::IpAdded(if_name.clone(), *ip)).await?;
                        }
                    } else {
                        tracing::info!("Detected IP Removed: {} from {}", ip, if_name);
                        if let Some(ips) = self.current_ips.get_mut(&if_index) {
                             if let Some(pos) = ips.iter().position(|&x| x == *ip) {
                                ips.remove(pos);
                                self.send_event(NetworkEvent::IpRemoved(if_name.clone(), *ip)).await?;
                             }
                        }
                    }
                }
            }
        } else {
            tracing::warn!("Received address event for unknown interface index: {}", if_index);
        }
        Ok(())
    }

     async fn handle_link_change(&mut self, msg: LinkMessage, is_add: bool) -> Result<()> {
        let if_index = msg.header.index;

        if is_add {
            let mut name = None;
            for nla in msg.attributes.iter() {
                if let netlink_packet_route::link::LinkAttribute::IfName(if_name) = nla {
                    name = Some(if_name.clone());
                    break;
                }
            }
            if let Some(name) = name {
                 tracing::info!("Detected Interface Added/Updated: index={}, name={}", if_index, name);
                 self.if_index_to_name.insert(if_index, name);
            }
        } else {
            if let Some(removed_name) = self.if_index_to_name.remove(&if_index) {
                 tracing::info!("Detected Interface Removed: index={}, name={}", if_index, removed_name);
                 if let Some(removed_ips) = self.current_ips.remove(&if_index) {
                     for ip in removed_ips {
                          self.send_event(NetworkEvent::IpRemoved(removed_name.clone(), ip)).await?;
                     }
                 }
            } else {
                tracing::debug!("Ignoring DelLink for unknown index: {}", if_index);
            }
        }
         Ok(())
     }

    async fn send_event(&self, event: NetworkEvent) -> Result<()> {
        self.event_sender
            .send(event.clone())
            .await
            .map_err(|e| AppError::ChannelSend(format!("Failed to send network event {:?}: {}", event, e)))
    }
}

// Testing rtnetlink still requires specific setup (like network namespaces) or root privileges.
