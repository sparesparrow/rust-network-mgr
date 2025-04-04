use crate::types::{AppConfig, AppError, Result};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_PATH: &str = "/etc/rust-network-manager/config.yaml";
const PKG_DEFAULT_CONFIG_PATH_FALLBACK: &str = "pkg-files/config/default.yaml";

/// Gets the path to the default configuration file packaged with the application.
/// In debug builds, it resolves relative to the Cargo manifest directory.
/// In release builds, it uses a predefined path relative to the expected installation.
fn get_pkg_default_config_path() -> PathBuf {
    if cfg!(debug_assertions) {
        // In debug builds, find it relative to the project root
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PKG_DEFAULT_CONFIG_PATH_FALLBACK)
    } else {
        // In release builds, assume it's in a standard location relative
        // to the binary or a system path. This might need adjustment based
        // on the actual installation procedure.
        PathBuf::from(PKG_DEFAULT_CONFIG_PATH_FALLBACK) // Or perhaps /usr/share/rust-network-manager/default.yaml?
    }
}

/// Loads configuration from the specified path, or falls back to defaults.
pub fn load_config(config_path_opt: Option<&Path>) -> Result<AppConfig> {
    let config_path = config_path_opt
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH).to_path_buf());

    tracing::info!("Attempting to load configuration from: {:?}", config_path);

    let config_str = match fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(e) => {
            let pkg_default_path = get_pkg_default_config_path();
            tracing::warn!(
                "Failed to read config file {:?}: {}. Trying package default at {:?}.",
                config_path,
                e,
                pkg_default_path
            );

            fs::read_to_string(&pkg_default_path).map_err(|e_fallback| {
                AppError::Config(format!(
                    "Failed to read both {:?} and {:?}: {} (fallback error: {})",
                    config_path,
                    pkg_default_path,
                    e,
                    e_fallback
                ))
            })?
        }
    };

    serde_yaml::from_str(&config_str)
        .map_err(|e| AppError::Config(format!("Failed to parse YAML: {}", e)))
}

// Basic validation (can be expanded)
pub fn validate_config(config: &AppConfig) -> Result<()> {
    if config.interfaces.is_empty() {
        return Err(AppError::Config(
            "Configuration must define at least one interface.".to_string(),
        ));
    }
    // Add more specific validation rules as needed
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::types::InterfaceConfig;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_valid_config() {
        let yaml = r#"
interfaces:
  - name: eth0
    dhcp: true
    nftables_zone: wan
  - name: eth1
    address: 192.168.1.1/24
    nftables_zone: lan
socket_path: /tmp/test.sock
"#;
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "{}", yaml).unwrap();

        let config = load_config(Some(file.path())).unwrap();

        assert_eq!(config.interfaces.len(), 2);
        assert_eq!(config.interfaces[0].name, "eth0");
        assert_eq!(config.interfaces[0].dhcp, Some(true));
        assert_eq!(config.interfaces[0].nftables_zone, Some("wan".to_string()));
        assert_eq!(config.interfaces[1].name, "eth1");
        assert_eq!(config.interfaces[1].address, Some("192.168.1.1/24".to_string()));
        assert_eq!(config.interfaces[1].nftables_zone, Some("lan".to_string()));
        assert_eq!(config.socket_path, Some("/tmp/test.sock".to_string()));
    }

    #[test]
    fn test_load_fallback_config() {
        // Ensure the test doesn't find a config at the non-existent path
        let non_existent_path = Path::new("/tmp/non_existent_config_for_test.yaml");
        let _ = std::fs::remove_file(&non_existent_path); // Clean up if it exists

        // Create the fallback file temporarily (relative to manifest dir)
        let fallback_path = get_pkg_default_config_path();
        let fallback_dir = fallback_path.parent().unwrap();
        std::fs::create_dir_all(fallback_dir).unwrap();
        let fallback_yaml = r#"
interfaces:
  - name: "fallback0"
    dhcp: true
socket_path: "/tmp/fallback.sock"
"#;
        std::fs::write(&fallback_path, fallback_yaml).unwrap();

        // Attempt to load using the non-existent path, expecting fallback
        let config = load_config(Some(&non_existent_path)).unwrap();

        // Verify fallback content is loaded
        assert_eq!(config.interfaces.len(), 1);
        assert_eq!(config.interfaces[0].name, "fallback0");
        assert_eq!(config.socket_path, Some("/tmp/fallback.sock".to_string()));

        // Clean up the temporary fallback file
        let _ = std::fs::remove_file(&fallback_path);
        let _ = std::fs::remove_dir(fallback_dir); // Remove dir only if empty
    }

    #[test]
    fn test_load_invalid_yaml() {
        // Use YAML with definitively incorrect syntax (bad indentation)
        let yaml = "interfaces:\n  - name: eth0\n invalid_indent: true"; 
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "{}", yaml).unwrap();

        let result = load_config(Some(file.path()));
        assert!(result.is_err());
        if let Err(AppError::Config(msg)) = result {
            assert!(msg.contains("Failed to parse YAML"));
        } else {
            panic!("Expected Config error");
        }
    }

    #[test]
    fn test_validate_empty_interfaces() {
        let config = AppConfig {
            interfaces: vec![],
            socket_path: None,
            nftables_rules_path: None,
        };
        let result = validate_config(&config);
        assert!(result.is_err());
        if let Err(AppError::Config(msg)) = result {
            assert!(msg.contains("at least one interface"));
        } else {
            panic!("Expected Config error");
        }
    }
}
