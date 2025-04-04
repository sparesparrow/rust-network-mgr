use crate::types::{AppError, NetworkEvent, Result, NetworkState, EventSender, SystemEvent};
use futures::stream::{StreamExt, TryStreamExt};
// Import only the minimum necessary from rtnetlink
use rtnetlink::{
    new_connection,
    constants::*, // Use correct path for constants
};
// Import the netlink_packet_core crate directly for the message types
use netlink_packet_core::{
    NetlinkMessage, NetlinkPayload,
};
// Import the netlink_packet_route crate directly for the route-specific types
use netlink_packet_route::{
    address::AddressMessage,
    link::{LinkMessage, LinkFlags},
    RouteNetlinkMessage,
};
use std::collections::HashMap;
use std::net::IpAddr;
use log::{info, debug, warn, error}; // Import log macros

/// Monitors network interface and address changes using rtnetlink.
pub struct NetworkMonitor {
    event_sender: EventSender, // Use the SystemEvent sender
    // Store interface index to name mapping for easier lookup
    if_index_to_name: HashMap<u32, String>,
    // Store current IPs per interface index
    current_ips: HashMap<u32, Vec<IpAddr>>,
}

impl NetworkMonitor {
    pub fn new(event_sender: EventSender) -> Self {
        NetworkMonitor {
            event_sender,
            if_index_to_name: HashMap::new(),
            current_ips: HashMap::new(),
        }
    }

    /// Starts the monitoring loop.
    /// This function will run indefinitely until an error occurs or the stream ends.
    pub async fn start(mut self) -> Result<()> { // Correct Result type
        info!("Starting NetworkMonitor task");

        let (connection, handle, mut messages) =
            rtnetlink::new_connection().map_err(|e| {
                AppError::Netlink(format!("Failed to create netlink connection: {}", e))
            })?;
        tokio::spawn(connection); // Spawn the connection task

        debug!("Gathering initial network state...");

        // 1. Get Interfaces to map index to name
        let mut links = handle.link().get().execute();
        let mut initial_if_index_to_name = HashMap::new();
        while let Some(link) = links.try_next().await.map_err(AppError::RtNetlink)? {
            if let Some(name) = link.attributes.iter().find_map(|nla| {
                if let netlink_packet_route::link::LinkAttribute::IfName(name) = nla {
                    Some(name.clone())
                } else {
                    None
                }
            }) {
                debug!("Found interface: index={}, name={}", link.header.index, name);
                initial_if_index_to_name.insert(link.header.index, name);
            }
        }
        self.if_index_to_name = initial_if_index_to_name.clone(); // Store initial map
        debug!("Interface map populated: {:?}", self.if_index_to_name);

        // 2. Get Addresses for initial state
        let mut initial_ips: HashMap<u32, Vec<IpAddr>> = HashMap::new();
        let mut addresses = handle.address().get().execute();
        while let Some(msg) = addresses.try_next().await.map_err(AppError::RtNetlink)? {
            let if_index = msg.header.index;
             if let Some(ip_addr) = msg.attributes.iter().find_map(|attr| {
                 if let netlink_packet_route::address::AddressAttribute::Address(ip) = attr {
                     Some(*ip)
                 } else {
                     None
                 }
             }) {
                 if let Some(if_name) = self.if_index_to_name.get(&if_index) {
                    info!("Initial state: Found IP {} for interface {} ({})", ip_addr, if_name, if_index);
                    let ips = initial_ips.entry(if_index).or_default();
                    if !ips.contains(&ip_addr) {
                        ips.push(ip_addr);
                    }
                 } else {
                     warn!("Found IP for unknown interface index {} during init", if_index);
                 }
             }
        }
        self.current_ips = initial_ips;
        debug!("Initial IP state populated: {:?}", self.current_ips);

        info!("Listening for netlink address and link events...");

        // --- Listen for Events ---
        loop {
             match messages.next().await {
                 Some((message, _addr)) => {
                     if let Err(e) = self.handle_netlink_message(message).await {
                         error!("Error handling netlink message: {}", e);
                     }
                 }
                 None => {
                      warn!("Netlink message stream ended unexpectedly.");
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
                error!("Received netlink error message: {:?}", err);
            }
            _ => { /* Ignore */ }
         }
         Ok(())
    }

    async fn handle_address_change(&mut self, msg: AddressMessage, is_add: bool) -> Result<()> {
        let if_index = msg.header.index;

        if let Some(if_name) = self.if_index_to_name.get(&if_index).cloned() {
            let mut changed = false;
            if let Some(ip_addr) = msg.attributes.iter().find_map(|attr| {
                 if let netlink_packet_route::address::AddressAttribute::Address(ip) = attr {
                     Some(*ip)
                 } else {
                     None
                 }
             }) {
                if is_add {
                    let ips = self.current_ips.entry(if_index).or_default();
                    if !ips.contains(&ip_addr) {
                         info!("Detected IP Added: {} on {}", ip_addr, if_name);
                         ips.push(ip_addr);
                         changed = true;
                    }
                } else {
                    if let Some(ips) = self.current_ips.get_mut(&if_index) {
                         if let Some(pos) = ips.iter().position(|&x| x == ip_addr) {
                            info!("Detected IP Removed: {} from {}", ip_addr, if_name);
                            ips.remove(pos);
                            changed = true;
                         }
                    }
                }
            }

            if changed {
                let current_ips_for_if = self.current_ips.get(&if_index).cloned().unwrap_or_default();
                self.send_event(NetworkEvent::IpUpdate {
                    interface: if_name.clone(),
                    ips: current_ips_for_if,
                }).await?;
            }
        } else {
            warn!("Received address event for unknown interface index: {}", if_index);
        }
        Ok(())
    }

     async fn handle_link_change(&mut self, msg: LinkMessage, is_add: bool) -> Result<()> {
        let if_index = msg.header.index;
        if is_add {
             if let Some(name) = msg.attributes.iter().find_map(|nla| {
                 if let netlink_packet_route::link::LinkAttribute::IfName(name) = nla {
                     Some(name.clone())
                 } else {
                     None
                 }
             }) {
                 info!("Detected Interface Added/Updated: index={}, name={}", if_index, name);
                 let old_name = self.if_index_to_name.insert(if_index, name.clone());
                 if old_name.is_none() || old_name.as_ref() != Some(&name) {
                    // Fix LinkFlags case
                    let is_up = msg.header.flags.contains(LinkFlags::Up);
                     self.send_event(NetworkEvent::LinkChanged { name, is_up }).await?;
                 }
             }
        } else {
            if let Some(removed_name) = self.if_index_to_name.remove(&if_index) {
                 info!("Detected Interface Removed: index={}, name={}", if_index, removed_name);
                 if self.current_ips.remove(&if_index).is_some() {
                     self.send_event(NetworkEvent::IpUpdate{
                         interface: removed_name.clone(), // Use correct field name
                         ips: vec![],
                     }).await?;
                 }
                 self.send_event(NetworkEvent::LinkChanged { name: removed_name, is_up: false }).await?;
            } else {
                debug!("Ignoring DelLink for unknown index: {}", if_index);
            }
        }
         Ok(())
     }

    async fn send_event(&self, event: NetworkEvent) -> Result<()> {
        // Send the specific NetworkEvent wrapped in SystemEvent::Network
        self.event_sender.send(SystemEvent::Network(event)).await
            .map_err(|e| AppError::MpscSendError(format!("Failed to send NetworkEvent: {}", e)))?;
        Ok(())
    }
}

// Testing rtnetlink still requires specific setup (like network namespaces) or root privileges.
