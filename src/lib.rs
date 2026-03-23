// Declare the modules that form the library's structure
pub mod api;
pub mod cli;
pub mod config;
pub mod docker;
pub mod network;
pub mod nftables;
pub mod socket;
pub mod types;

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

// HTTP API
pub use api::{ApiState, build_router, spawn_http_server};