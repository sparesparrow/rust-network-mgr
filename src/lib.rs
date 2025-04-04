// Declare the modules that form the library's structure
pub mod config;
pub mod network;
pub mod nftables;
pub mod socket;
pub mod types;
pub mod docker;

// Publicly export key types, functions, and modules needed by the binary or tests

// Configuration
pub use config::load_config;

// Core Application Logic
// pub use app::run; // Commented out - module doesn't exist yet

// Monitoring Modules
pub use network::NetworkMonitor;
pub use nftables::NftablesManager;
pub use socket::SocketHandler;
pub use docker::DockerMonitor;

// Core Types (Consolidated)
pub use types::{
    AppConfig,
    AppError,
    Result,
    SystemEvent,
    EventSender,
    EventReceiver,
    NetworkEvent,
    DockerEvent,
    ControlCommand,
    AppStateShared,
    NetworkState,
};

// Other potential public interfaces if needed
// e.g., pub use docker::DockerMonitor;

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