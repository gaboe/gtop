//! Vendored disk scanning and pure layout. See UPSTREAM.md for migration limits.

pub mod access;
pub mod classify;
pub mod filter;
pub mod identity;
pub mod insights;
#[cfg(target_os = "macos")]
mod macos;
pub mod removal;
pub mod scan;
pub mod scan_preview;
pub mod scan_threads;
pub mod size;
pub mod space;
pub mod tree;
pub mod treemap;

#[cfg(test)]
mod contract_tests;
