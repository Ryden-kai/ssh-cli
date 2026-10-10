//! Crate-wide error type.

use thiserror::Error;

/// All errors that can be produced by `ssh-cli`.
///
/// Design choices:
/// - One enum so callers can match exhaustively on a stable shape.
/// - `#[from]` adapters for stdlib errors so `?` keeps working.
/// - No secrets in `Display` output (intentionally low-detail).
#[derive(Debug, Error)]
pub enum Error {
    #[error("config file not found at {0}; run `ssh-cli init` to create one")]
    ConfigNotFound(String),

    #[error("invalid alias `{0}`: aliases must match [A-Za-z0-9._-]+, 1..=64 chars, not starting with `.` or `-`")]
    InvalidAlias(String),

    #[error("host `{0}` not found; run `ssh-cli list` to see known aliases")]
    HostNotFound(String),

    #[error("host `{0}` already exists; remove it first or use `ssh-cli rename`")]
    HostExists(String),

    #[error("no `user` configured for `{0}` (set it in [defaults] or under the host)")]
    MissingUser(String),

    #[error("ssh binary not found on PATH")]
    SshMissing,

    #[error("scp binary not found on PATH")]
    ScpMissing,

    #[error("invalid scp target `{0}` (expected `alias:path`, `user@host:path`, or a local path)")]
    InvalidScpTarget(String),

    #[error("invalid tunnel spec `{0}` (expected `[bind:]port:host:port` for local/remote, or `port` for dynamic)")]
    InvalidTunnelSpec(String),

    #[error("port `{0}` is not a valid u16")]
    InvalidPort(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("config serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
