use crate::types::{AppConfig, AppError, Result};
use std::fs;
use std::path::Path;

const DEFAULT_CONFIG_PATH: &str = "/etc/rust-network-manager/config.yaml";
const PKG_DEFAULT_CONFIG_PATH: &str = "pkg-files/config/default.yaml";

/// Loads configuration from the specified path, or falls back to defaults.
pub fn load_config(config_path_opt: Option<&Path>) -> Result<AppConfig> {
    let config_path = config_path_opt
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH).to_path_buf());

tracing::info!("Attempting to load configuration from: {:?}", config_path);

    let config_str = match fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "Failed to read config file {:?}: {}. Trying package default.",
                config_path,
                e
            );
            // Fallback to reading the default config packaged with the application
            fs::read_to_string(PKG_DEFAULT_CONFIG_PATH).map_err(|e| {
                AppError::Config(format!(
                    "Failed to read both {:?} and {}: {}",
                    config_path,
                    PKG_DEFAULT_CONFIG_PATH,
                    e
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
mod tests {
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
    fn test_load_invalid_yaml() {
        let yaml = "interfaces: [ name: eth0 ]"; // Invalid YAML
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
