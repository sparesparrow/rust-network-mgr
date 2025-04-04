// Declare the modules that form the library's structure
pub mod config;
pub mod network;
pub mod nftables;
pub mod socket;
pub mod types;

// Publicly export key types, functions, and modules needed by the binary or tests
pub use config::{load_config, validate_config};
pub use network::NetworkMonitor;
pub use nftables::NftablesManager;
pub use socket::SocketHandler;
pub use types::{AppConfig, AppError, ControlCommand, NetworkEvent, NetworkState, Result};

// You might also want a function to run the main application logic, called by main.rs
// Example (needs more implementation based on main.rs logic):
/*
pub async fn run_daemon(config_path: Option<&std::path::Path>) -> Result<()> {
    // ... initialization code from main.rs ...
    // ... spawn tasks ...
    // ... main event loop ...
    // ... cleanup ...
    Ok(())
}
*/ 