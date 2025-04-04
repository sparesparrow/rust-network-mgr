use crate::types::{AppConfig, AppError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use directories::ProjectDirs;
use log::{info, warn};

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

/// Determines the configuration file path to use.
/// Order: Override > Default System Path > Packaged Fallback > User Config Dir
fn get_config_path(config_path_override: Option<&str>) -> Result<PathBuf> {
    // 1. Use override if provided
    if let Some(path_str) = config_path_override {
        let path = PathBuf::from(path_str);
        if path.exists() {
            return Ok(path);
        } else {
            // If override is given but doesn't exist, it's an error
            return Err(AppError::ConfigIo(format!("Specified config file not found: {}", path_str)));
        }
    }

    // 2. Check default system path
    let default_path = Path::new(DEFAULT_CONFIG_PATH);
    if default_path.exists() {
        return Ok(default_path.to_path_buf());
    }

    // 3. Check packaged default path (for initial setup/fallback)
    let pkg_default_path = get_pkg_default_config_path();
    if pkg_default_path.exists() {
         warn!("System config not found at {}, using packaged default: {}",
               DEFAULT_CONFIG_PATH, pkg_default_path.display());
        return Ok(pkg_default_path);
    }

    // 4. Check user config directory as last resort
    if let Some(proj_dirs) = ProjectDirs::from("", "", "RustNetworkManager") {
        // proj_dirs is now ProjectDirs, config_dir() returns &Path directly
        let config_dir: &Path = proj_dirs.config_dir();
        let user_config_path = config_dir.join("config.yaml");
        if user_config_path.exists() {
            warn!("System config not found, using user config: {}", user_config_path.display());
            return Ok(user_config_path);
        }
    }

    // 5. If none found, return an error indicating where it looked
    Err(AppError::ConfigIo(format!(
        "Configuration file not found. Looked in: override ({:?}), {}, {}, and user config dir.",
        config_path_override,
        DEFAULT_CONFIG_PATH,
        pkg_default_path.display()
    )))
}

/// Loads configuration from the determined path.
pub fn load_config(config_path_override: Option<&str>) -> Result<AppConfig> {
    let config_path = get_config_path(config_path_override)?;
    info!("Loading configuration from: {}", config_path.display());

    match std::fs::read_to_string(&config_path) {
        Ok(content) => {
            let config: AppConfig = serde_yaml::from_str(&content)
                .map_err(AppError::ConfigParse)?;
            validate_config(&config)?;
            Ok(config)
        }
        Err(e) => {
            // Use ConfigIo for file read errors
            Err(AppError::ConfigIo(format!(
                "Failed to read configuration file '{}': {}",
                config_path.display(),
                e
            )))
        }
    }
}

// Make validate_config public so it can be re-exported
pub(crate) fn validate_config(config: &AppConfig) -> Result<()> {
    if config.interfaces.is_empty() {
        // Use ConfigValidation
        return Err(AppError::ConfigValidation(
            "Configuration must include at least one interface.".to_string(),
        ));
    }
    for interface in &config.interfaces {
        if interface.name.is_empty() {
            // Use ConfigValidation
            return Err(AppError::ConfigValidation(
                "Interface name cannot be empty".to_string(),
            ));
        }
        // Add more specific validation rules as needed
        // e.g., check format of static address, ensure zone name isn't empty if present
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
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

        let config = load_config(Some(file.path().to_str().unwrap())).unwrap();

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
        let config = load_config(Some(fallback_path.to_str().unwrap())).unwrap();

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

        let result = load_config(Some(file.path().to_str().unwrap()));
        assert!(result.is_err());
        // Expect ConfigParse error for bad YAML
        match result {
            Err(AppError::ConfigParse(_)) => { /* Expected */ }
            _ => panic!("Expected ConfigParse error, got {:?}", result),
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
        // Expect ConfigValidation error
        match result {
            Err(AppError::ConfigValidation(msg)) => {
                assert!(msg.contains("at least one interface"));
            }
            _ => panic!("Expected ConfigValidation error, got {:?}", result),
        }
    }
}
