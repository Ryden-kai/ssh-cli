//! Config file types, load/save, validation.

use crate::error::{Error, Result};
use crate::paths;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_ALIAS_LEN: usize = 64;
pub const MAX_HOST_LEN: usize = 253; // RFC 1035 max domain length; also covers IPv6 literals.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_opts: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_opts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Top-level on-disk schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub hosts: BTreeMap<String, Host>,
}

impl Config {
    /// Load from disk, returning an empty in-memory config if the file is absent.
    pub fn load_or_default() -> Result<Self> {
        let path = paths::config_file();
        if !path.exists() {
            return Ok(Self::empty());
        }
        let text = std::fs::read_to_string(&path)?;
        let cfg: Self = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Load from disk, erroring if the file is absent.
    pub fn load() -> Result<Self> {
        let path = paths::config_file();
        if !path.exists() {
            return Err(Error::ConfigNotFound(path.display().to_string()));
        }
        let text = std::fs::read_to_string(&path)?;
        let cfg: Self = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::config_file();
        save_atomic(&path, self)
    }

    pub fn empty() -> Self {
        Self {
            version: SCHEMA_VERSION,
            defaults: Defaults::default(),
            hosts: BTreeMap::new(),
        }
    }

    /// Resolve an alias to the effective ssh arguments, merging host-level over defaults.
    pub fn effective<'a>(&'a self, alias: &'a str) -> Result<EffectiveHost<'a>> {
        let host = self
            .hosts
            .get(alias)
            .ok_or_else(|| Error::HostNotFound(alias.to_string()))?;

        let user = host
            .user
            .as_deref()
            .or(self.defaults.user.as_deref())
            .ok_or_else(|| Error::MissingUser(alias.to_string()))?
            .to_string();

        let port = host.port.or(self.defaults.port).unwrap_or(22);

        let identity_file = host
            .identity_file
            .as_deref()
            .or(self.defaults.identity_file.as_deref())
            .map(str::to_string);

        let mut extra_opts = self.defaults.extra_opts.clone();
        extra_opts.extend(host.extra_opts.iter().cloned());

        Ok(EffectiveHost {
            alias,
            host: &host.host,
            user,
            port,
            identity_file,
            extra_opts,
            desc: host.desc.as_deref(),
            tags: &host.tags,
        })
    }

    fn validate(&self) -> Result<()> {
        if self.version != SCHEMA_VERSION {
            return Err(Error::TomlParse(toml::de::Error::custom(format!(
                "unsupported schema version {} (expected {})",
                self.version, SCHEMA_VERSION
            ))));
        }
        for alias in self.hosts.keys() {
            validate_alias(alias)?;
        }
        Ok(())
    }
}

/// Resolved host ready to be turned into ssh argv.
#[derive(Debug, Clone)]
pub struct EffectiveHost<'a> {
    pub alias: &'a str,
    pub host: &'a str,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    pub extra_opts: Vec<String>,
    pub desc: Option<&'a str>,
    pub tags: &'a [String],
}

impl<'a> EffectiveHost<'a> {
    /// Format as `user@host`. For IPv6 literals, the host is bracketed.
    pub fn destination(&self) -> String {
        if self.host.contains(':') {
            format!("{}@[{}]", self.user, self.host)
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }
}

/// Validate an alias name. Restrictive on purpose so it can never be a path or shell metacharacter.
pub fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty() || alias.len() > MAX_ALIAS_LEN {
        return Err(Error::InvalidAlias(alias.to_string()));
    }
    let bytes = alias.as_bytes();
    if bytes[0] == b'.' || bytes[0] == b'-' {
        return Err(Error::InvalidAlias(alias.to_string()));
    }
    for &b in bytes {
        let ok = b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
        if !ok {
            return Err(Error::InvalidAlias(alias.to_string()));
        }
    }
    Ok(())
}

pub fn save_atomic(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(cfg)?;
    let tmp = path.with_extension("toml.tmp");
    // Best-effort cleanup of an old tmp file.
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, body)?;
    // On Windows, std::fs::rename fails if dest exists; remove then rename.
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Expand a leading `~` or `~/...` to the user's home directory.
/// Returns the input unchanged if no expansion applies.
pub fn expand_home(p: &str) -> PathBuf {
    if p == "~" {
        return home_dir().join("");
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(p)
}

fn home_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h);
    }
    if let Some(h) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(h);
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_validator_accepts_normal() {
        for a in ["arch-prod", "dev_box", "foo.bar", "a", "A-1.b_2"] {
            validate_alias(a).unwrap();
        }
    }

    #[test]
    fn alias_validator_rejects_bad() {
        for a in ["", ".hidden", "-leading-dash", "has space", "has/slash", "a*b", &"x".repeat(65)] {
            assert!(validate_alias(a).is_err(), "expected {a:?} to be rejected");
        }
    }

    #[test]
    fn effective_merges_host_over_defaults() {
        let mut cfg = Config::empty();
        cfg.defaults.user = Some("default-user".into());
        cfg.defaults.port = Some(22);
        cfg.defaults.extra_opts = vec!["-o".into(), "ServerAliveInterval=60".into()];
        cfg.hosts.insert(
            "box".into(),
            Host {
                host: "10.0.0.5".into(),
                port: Some(2222),
                extra_opts: vec!["-o".into(), "ForwardAgent=yes".into()],
                ..Default::default()
            },
        );
        let eff = cfg.effective("box").unwrap();
        assert_eq!(eff.user, "default-user");
        assert_eq!(eff.port, 2222);
        assert_eq!(eff.destination(), "default-user@10.0.0.5");
        assert!(eff.extra_opts.iter().any(|s| s == "ServerAliveInterval=60"));
        assert!(eff.extra_opts.iter().any(|s| s == "ForwardAgent=yes"));
    }

    #[test]
    fn effective_ipv6_is_bracketed() {
        let mut cfg = Config::empty();
        cfg.hosts.insert(
            "v6".into(),
            Host {
                host: "::1".into(),
                user: Some("root".into()),
                ..Default::default()
            },
        );
        let eff = cfg.effective("v6").unwrap();
        assert_eq!(eff.destination(), "root@[::1]");
    }
}
