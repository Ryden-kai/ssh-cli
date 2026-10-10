//! Cross-platform config path resolution.
//!
//! Linux/WSL/macOS:  `$XDG_CONFIG_HOME/ssh-cli/config.toml` (default `~/.config/ssh-cli/config.toml`)
//! Windows:         `%APPDATA%\ssh-cli\config.toml` (i.e. `Roaming\ssh-cli\config.toml`)
//!
//! `directories::ProjectDirs` handles the platform conventions; we keep a manual
//! fallback in case `directories` returns `None` on some unusual setup.

use std::path::PathBuf;

const QUALIFIER: &str = "com";
const ORG: &str = "ssh-cli";
const APP: &str = "ssh-cli";

/// Returns the config directory, creating the chain if the crate couldn't resolve it.
pub fn config_dir() -> PathBuf {
    // Explicit override wins — useful for tests, CI, and users who keep multiple configs.
    if let Some(p) = std::env::var_os("SSH_CLI_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    directories::ProjectDirs::from(QUALIFIER, ORG, APP)
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(fallback_config_dir)
}

pub fn config_file() -> PathBuf {
    // Explicit override for the exact file path.
    if let Some(p) = std::env::var_os("SSH_CLI_CONFIG") {
        return PathBuf::from(p);
    }
    config_dir().join("config.toml")
}

fn fallback_config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("ssh-cli");
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("ssh-cli");
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let mut p = PathBuf::from(home);
        if cfg!(windows) {
            p.push("AppData");
            p.push("Roaming");
            p.push("ssh-cli");
        } else {
            p.push(".config");
            p.push("ssh-cli");
        }
        return p;
    }
    PathBuf::from(".ssh-cli")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_is_inside_config_dir() {
        let d = config_dir();
        let f = config_file();
        assert!(f.starts_with(&d), "config_file must live under config_dir");
        assert_eq!(f.file_name().unwrap(), "config.toml");
    }
}
