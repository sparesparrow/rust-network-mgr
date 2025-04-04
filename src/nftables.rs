//! NFTables management module using the nftables-rs crate (JSON API)

use crate::types::{AppError, InterfaceConfig, NetworkState};
use log::{debug, info};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use std::borrow::Cow;

// Corrected nftables-rs imports based on common structure and errors
use nftables::{
    batch::Batch,
    helper, // NftablesError is now here
    // Import base types from nftables crate directly
    expr::Expression, // Need this for elements
    schema::{NfCmd, NfListObject, NfObject, Table, Set, Element, FlushObject, SetTypeValue, SetFlag, Nftables}, // Nftables struct is in schema
    types::NfFamily, // Keep NfFamily here
};

/// Manages nftables rules using the nftables-rs crate
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
        info!("[NFTABLES-RS] Ensuring base nftables structure");
        let mut batch = Batch::new();

        // 1. Ensure Table Exists
        batch.add(NfListObject::Table(Table {
            family: NfFamily::INet,
            name: Cow::Borrowed(&self.table_name),
            handle: None, // Explicitly set handle if necessary, often optional
        }));

        // 2. Calculate unique zones from config
        let config_lock = self.config.lock().await;
        let unique_zones: HashSet<String> = config_lock.iter()
            .filter_map(|iface| iface.nftables_zone.clone())
            .collect();
        // Drop the lock explicitly after use
        drop(config_lock);

        // 3. Ensure Sets Exist for each unique zone
        for zone_name in unique_zones {
            // --- IPv4 Set Definition ---
            let ipv4_set_name = format!("{}_ips", zone_name);
            batch.add(NfListObject::Set(Box::new(Set {
                family: NfFamily::INet,
                table: Cow::Borrowed(&self.table_name),
                name: Cow::Owned(ipv4_set_name),
                handle: None,
                set_type: nftables::schema::SetTypeValue::Single(nftables::schema::SetType::Ipv4Addr),
                policy: None,
                flags: Some(HashSet::from([nftables::schema::SetFlag::Dynamic])),
                comment: None,
                elem: None,
                gc_interval: None,
                size: None,
                timeout: None,
            })));

            // --- IPv6 Set Definition ---
            let ipv6_set_name = format!("{}_ipv6", zone_name);
            batch.add(NfListObject::Set(Box::new(Set {
                family: NfFamily::INet,
                table: Cow::Borrowed(&self.table_name),
                name: Cow::Owned(ipv6_set_name),
                handle: None,
                set_type: nftables::schema::SetTypeValue::Single(nftables::schema::SetType::Ipv6Addr),
                policy: None,
                flags: Some(HashSet::from([nftables::schema::SetFlag::Dynamic])),
                comment: None,
                elem: None,
                gc_interval: None,
                size: None,
                timeout: None,
            })));
        }

        let ruleset = batch.to_nftables();
        debug!("[NFTABLES-RS] Load ruleset generated: {:?}", ruleset);

        // Use the helper error type from nftables::helper
        // apply_ruleset takes only one argument
        helper::apply_ruleset(&ruleset).map_err(AppError::NftablesError)?;

        info!("[NFTABLES-RS] Base table '{}' and required sets ensured.", self.table_name);
        Ok(())
    }

    /// Apply rules based on the current network state
    pub async fn apply_rules(&self, network_state: &NetworkState) -> Result<(), AppError> {
         info!("[NFTABLES-RS] Applying nftables rules (flush and add elements)");

         // Calculate zone_to_ips based on current network state and config
         let config_lock = self.config.lock().await;
         let mut zone_to_ips: HashMap<String, HashSet<IpAddr>> = HashMap::new();
         // Correct access to network_state fields
         // No need for another lock if network_state is &NetworkState
         for interface_config in config_lock.iter() {
             if let Some(zone) = &interface_config.nftables_zone {
                 // Access interface_ips directly on network_state
                 if let Some(ips) = network_state.interface_ips.get(&interface_config.name) {
                     let zone_ips = zone_to_ips.entry(zone.clone()).or_default();
                     for ip in ips {
                         zone_ips.insert(*ip); // Insert directly into HashSet
                     }
                 }
             }
         }
         // Drop lock
         drop(config_lock);


         // --- Flushing Phase --- Execute Flush commands directly
         let mut flush_commands: Vec<NfObject> = Vec::new();
         for zone_name in zone_to_ips.keys() {
              // --- IPv4: Flush Set --- //
              let ipv4_set_name = format!("{}_ips", zone_name);
              // Use NfObject::CmdObject with NfCmd::Flush(FlushObject::Set(...))
              // FlushObject::Set is a tuple variant expecting Box<Set>
              let set_to_flush_v4 = Box::new(Set {
                   family: NfFamily::INet,
                   table: Cow::Borrowed(&self.table_name),
                   name: Cow::Owned(ipv4_set_name),
                   // Only include fields needed for identification
                   handle: None,
                   set_type: nftables::schema::SetTypeValue::Single(nftables::schema::SetType::Ipv4Addr),
                   policy: None,
                   flags: None,
                   comment: None,
                   elem: None,
                   gc_interval: None,
                   size: None,
                   timeout: None,
              });
              flush_commands.push(NfObject::CmdObject(NfCmd::Flush(FlushObject::Set(set_to_flush_v4))));

              // --- IPv6: Flush Set --- //
              let ipv6_set_name = format!("{}_ipv6", zone_name);
              let set_to_flush_v6 = Box::new(Set {
                   family: NfFamily::INet,
                   table: Cow::Borrowed(&self.table_name),
                   name: Cow::Owned(ipv6_set_name),
                   handle: None,
                   set_type: nftables::schema::SetTypeValue::Single(nftables::schema::SetType::Ipv6Addr),
                   policy: None,
                   flags: None,
                   comment: None,
                   elem: None,
                   gc_interval: None,
                   size: None,
                   timeout: None,
              });
              flush_commands.push(NfObject::CmdObject(NfCmd::Flush(FlushObject::Set(set_to_flush_v6))));
         }

         // Apply flush commands if any
         if !flush_commands.is_empty() {
            let flush_nftables = Nftables { objects: Cow::Owned(flush_commands) };
            debug!("[NFTABLES-RS] Flush commands generated: {:?}", flush_nftables);
            // apply_ruleset takes only one argument
            helper::apply_ruleset(&flush_nftables).map_err(AppError::NftablesError)?;
            info!("[NFTABLES-RS] Successfully flushed nftables sets");
         } else {
            info!("[NFTABLES-RS] No sets to flush.");
         }


         // --- Adding Elements Phase ---
         let mut add_batch = Batch::new();
         for (zone_name, ips) in zone_to_ips {
             // --- IPv4: Add Elements --- //
             let ipv4_set_name = format!("{}_ips", zone_name);
             let ipv4_elements: Vec<Element> = ips.iter()
                 .filter_map(|ip| match ip {
                     IpAddr::V4(v4) => Some(Element {
                         family: NfFamily::INet,
                         table: Cow::Borrowed(&self.table_name),
                         name: Cow::Owned(ipv4_set_name.clone()),
                         elem: Cow::Owned(vec![Expression::String(v4.to_string().into())]),
                     }),
                     _ => None,
                 })
                 .collect();

             if !ipv4_elements.is_empty() {
                 add_batch.add(NfListObject::Element(Element {
                    family: NfFamily::INet,
                    table: Cow::Borrowed(&self.table_name),
                    name: Cow::Owned(ipv4_set_name),
                    elem: Cow::Owned(ipv4_elements.into_iter().flat_map(|e| e.elem.into_owned()).collect()),
                }));
             }


             // --- IPv6: Add Elements --- //
             let ipv6_set_name = format!("{}_ipv6", zone_name);
             let ipv6_elements: Vec<Element> = ips.iter()
                 .filter_map(|ip| match ip {
                     IpAddr::V6(v6) => Some(Element {
                         family: NfFamily::INet,
                         table: Cow::Borrowed(&self.table_name),
                         name: Cow::Owned(ipv6_set_name.clone()),
                         elem: Cow::Owned(vec![Expression::String(v6.to_string().into())]),
                     }),
                     _ => None,
                 })
                 .collect();

             if !ipv6_elements.is_empty() {
                 add_batch.add(NfListObject::Element(Element {
                    family: NfFamily::INet,
                    table: Cow::Borrowed(&self.table_name),
                    name: Cow::Owned(ipv6_set_name),
                    elem: Cow::Owned(ipv6_elements.into_iter().flat_map(|e| e.elem.into_owned()).collect()),
                 }));
             }
         }

         // Check internal vector for emptiness using to_nftables() and checking the result
         let add_ruleset = add_batch.to_nftables();
         if !add_ruleset.objects.is_empty() {
            debug!("[NFTABLES-RS] Add elements ruleset generated: {:?}", add_ruleset);
            // apply_ruleset takes only one argument
            helper::apply_ruleset(&add_ruleset).map_err(AppError::NftablesError)?;
            info!("[NFTABLES-RS] Successfully added elements to nftables sets");
         } else {
             info!("[NFTABLES-RS] No elements to add.");
         }

         Ok(())
    }
}

// Note: ip_to_expr helper is no longer needed as conversion happens inline.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{InterfaceConfig, AppStateShared}; // Need AppStateShared for NetworkState access
    use std::net::{Ipv4Addr, IpAddr};
    use std::sync::Arc;
    use tokio::runtime::Runtime;
    use tokio::sync::Mutex as AsyncMutex;
    use std::collections::HashMap; // Need HashMap for test state

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
        // NetworkState is now nested in AppStateShared, create that instead
        let mut state = NetworkState::default(); // Use default
        let wan_ips = vec![
            IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
            IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)),
        ];
        // Use interface_ips map directly
        state.interface_ips.insert("eth0".to_string(), wan_ips);
        let lan_ips = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))]; // Example LAN IP
        state.interface_ips.insert("eth1".to_string(), lan_ips);
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

            // Create the state within the shared structure for the test
            let network_state = create_test_network_state();
            // Need to pass the NetworkState struct directly
            let apply_result = manager.apply_rules(&network_state).await;
            assert!(apply_result.is_ok(), "apply_rules should succeed: {:?}", apply_result.err());
        });
    }
}
