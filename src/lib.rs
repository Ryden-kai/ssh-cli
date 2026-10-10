//! Library entry point for `ssh-cli`.
//!
//! Exposes the public modules so future callers (e.g. an MCP wrapper) can drive
//! the same logic without going through the CLI binary.

pub mod cli;
pub mod config;
pub mod error;
pub mod exec;
pub mod paths;

pub use error::{Error, Result};
