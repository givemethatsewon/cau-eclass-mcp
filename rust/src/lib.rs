//! Experimental Rust core for the CAU e-Class MCP.

pub mod catalog;
pub mod cli;
pub mod documents;
pub mod error;
pub mod http;
pub mod mcp;
pub mod service;
pub mod session;

pub use error::EclassError;
