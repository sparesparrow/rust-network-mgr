//! NFTables management module using rustables crate

use crate::types::{AppError, InterfaceConfig, NetworkState};
use log::{debug, error, info};
use rustables::{
    Batch, MsgType, ProtocolFamily, Table,
    set::SetBuilder,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

/// Manages nftables rules using the rustables crate
pub struct NftablesManager {
    #[allow(dead_code)] // Allow config field to be unused for now
    config: Arc<AsyncMutex<Vec<InterfaceConfig>>>,
    table_name: String,
}

impl NftablesManager {
    /// Create a new NftablesManager instance
    pub async fn new(config: Arc<AsyncMutex<Vec<InterfaceConfig>>>) -> Result<Self, AppError> {
        let manager = Self {
            config,
            table_name: "filter".to_string(),
        };
        Ok(manager)
    }

    /// Ensures the base nftables structure exists (inet table)
    pub async fn load_rules(&self) -> Result<(), AppError> {
        info!("Ensuring base nftables structure exists");
        let table = Table::new(ProtocolFamily::Inet).with_name(&self.table_name);
        
        let mut batch = Batch::new();
        table.add_to_batch(&mut batch);
        
        // Send the batch to apply changes
        batch.send().map_err(AppError::Nftables)?;
        
        info!("Base table 'inet filter' created or ensured.");
        Ok(())
    }

    /// Apply rules based on the current network state
    pub async fn apply_rules(&self, network_state: &NetworkState) -> Result<(), AppError> {
        info!("Applying nftables rules based on network state");
        
        // Create a batch for atomic operations
        let mut batch = Batch::new();
        
        // Get zone IPs from network state directly
        for (zone_name, ips) in &network_state.zone_ips {
            debug!("Updating set for zone: {}", zone_name);
            
            // Create table reference
            let table = Table::new(ProtocolFamily::Inet).with_name(&self.table_name);
            
            // Set name based on zone (e.g., "wan_ips", "lan_ips")
            let set_name = format!("{}_ips", zone_name);
            
            // Process IPv4 and IPv6 addresses separately
            // First handle IPv4 addresses
            let ipv4_addresses: Vec<_> = ips.iter()
                .filter_map(|ip| match ip {
                    IpAddr::V4(ipv4) => Some(*ipv4),
                    _ => None,
                })
                .collect();
                
            if !ipv4_addresses.is_empty() {
                let mut set_builder = match SetBuilder::<Ipv4Addr>::new(&set_name, &table) {
                    Ok(builder) => builder,
                    Err(e) => {
                        error!("Failed to create IPv4 set builder for {}: {}", set_name, e);
                        return Err(AppError::Nftables(
                            rustables::error::QueryError::BuilderError(e)
                        ));
                    }
                };
                
                // Add IPs to the set
                for ipv4 in &ipv4_addresses {
                    set_builder.add(ipv4);
                }
                
                // Finish building the set and add to batch
                let (set, elements) = set_builder.finish();
                
                // Add the set to the batch (create/update)
                batch.add(&set, MsgType::Add);
                
                // Add the elements to the batch
                batch.add(&elements, MsgType::Add);
            }
            
            // Handle IPv6 addresses if there are any
            let ipv6_addresses: Vec<_> = ips.iter()
                .filter_map(|ip| match ip {
                    IpAddr::V6(ipv6) => Some(*ipv6),
                    _ => None,
                })
                .collect();
                
            if !ipv6_addresses.is_empty() {
                let set_name_v6 = format!("{}_ipv6", zone_name); // Use a different name for IPv6 sets
                let mut set_builder = match SetBuilder::<Ipv6Addr>::new(&set_name_v6, &table) {
                    Ok(builder) => builder,
                    Err(e) => {
                        error!("Failed to create IPv6 set builder for {}: {}", set_name_v6, e);
                        return Err(AppError::Nftables(
                            rustables::error::QueryError::BuilderError(e)
                        ));
                    }
                };
                
                // Add IPs to the set
                for ipv6 in &ipv6_addresses {
                    set_builder.add(ipv6);
                }
                
                // Finish building the set and add to batch
                let (set, elements) = set_builder.finish();
                
                // Add the set to the batch (create/update)
                batch.add(&set, MsgType::Add);
                
                // Add the elements to the batch
                batch.add(&elements, MsgType::Add);
            }
        }
        
        // Send the batch to apply changes atomically
        batch.send().map_err(AppError::Nftables)?;
        
        info!("Successfully applied nftables rules");
        Ok(())
    }
}

// Note: ip_to_expr helper is no longer needed as conversion happens inline.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::InterfaceConfig; // Make sure this is imported for the test mock
    use std::net::Ipv4Addr;
    use std::sync::Arc;
    use tokio::runtime::Runtime;
    use tokio::sync::Mutex as AsyncMutex;

    // Mock config now uses Vec<InterfaceConfig>
    fn create_mock_config() -> Arc<AsyncMutex<Vec<InterfaceConfig>>> {
        Arc::new(AsyncMutex::new(vec![
            InterfaceConfig {
                name: "eth0".to_string(),
                dhcp: Some(true),
                address: None,
                nftables_zone: Some("wan".to_string()),
            },
            InterfaceConfig {
                name: "eth1".to_string(),
                dhcp: None,
                address: Some("192.168.1.1/24".to_string()),
                nftables_zone: Some("lan".to_string()),
            },
        ]))
    }

    fn create_test_network_state() -> NetworkState {
        let mut state = NetworkState::new();
        let wan_ips = vec![
            IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
            IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)),
        ];
        state.zone_ips.insert("wan".to_string(), wan_ips);
        let lan_ips = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))]; // Example LAN IP
        state.zone_ips.insert("lan".to_string(), lan_ips);
        state
    }

    #[test]
    #[ignore] // Requires root privileges
    fn test_nftables_manager_init() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let config = create_mock_config();
            let manager_result = NftablesManager::new(config).await;
            assert!(manager_result.is_ok(), "NftablesManager::new should succeed: {:?}", manager_result.err());
        });
    }

    #[test]
    #[ignore] // Requires root privileges and nftables installed
    fn test_nftables_table_creation() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let config = create_mock_config();
            let manager = NftablesManager::new(config).await.expect("Failed to create NftablesManager");
            let result = manager.load_rules().await;
            assert!(result.is_ok(), "load_rules should succeed: {:?}", result.err());
        });
    }

    #[test]
    #[ignore] // Requires root privileges and nftables installed
    fn test_nftables_set_management() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let config = create_mock_config();
            let manager = NftablesManager::new(config).await.expect("Failed to create NftablesManager");
            let result = manager.load_rules().await;
            assert!(result.is_ok(), "load_rules should succeed: {:?}", result.err());

            let network_state = create_test_network_state();
            let apply_result = manager.apply_rules(&network_state).await;
            assert!(apply_result.is_ok(), "apply_rules should succeed: {:?}", apply_result.err());
        });
    }
}
