use rust_network_mgr::{
    nftables::NftablesManager,
    types::{InterfaceConfig, NetworkState}
};

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::runtime::Runtime;

/// Helper to create a mock interface configuration.
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

/// Helper to create a test network state with IPs.
fn create_test_network_state() -> NetworkState {
    let mut state = NetworkState::default();
    state.interface_ips.insert(
        "eth0".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 100))],
    );
    state.interface_ips.insert(
        "eth1".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))],
    );

    state.zone_ips.insert(
        "wan".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 100))],
    );
    state.zone_ips.insert(
        "lan".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))],
    );

    state
}

// Note: These tests are marked with #[ignore] because they interact with
// the live nftables system and require root privileges to run.
//
// To run these tests:
// 1. Ensure that nftables is installed and available
// 2. Run as root: sudo cargo test --test nftables_manager_tests -- --ignored
//
// These tests assume the following setup (which the test itself attempts to create):
// - Table: inet filter
// - Sets: wan_ips, lan_ips (type ipv4_addr)

#[test]
#[ignore]
fn test_nftables_manager_init() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let config = create_mock_config();
        let manager_result = NftablesManager::new(config).await;
        assert!(manager_result.is_ok(), "Failed to initialize NftablesManager: {:?}", manager_result.err());
    });
}

#[test]
#[ignore]
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
#[ignore]
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

// Add more tests as needed for specific functionality 