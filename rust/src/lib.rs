//! Experimental Rust core for the CAU e-Class MCP.

pub mod aside;
mod browser;
pub mod catalog;
mod cdp;
pub mod cli;
pub mod documents;
pub mod error;
pub mod http;
pub mod mcp;
mod oz;
pub mod service;
pub mod session;

pub use error::EclassError;
