//! System monitoring functionality
//!
//! This module provides types and functionality for monitoring system resources
//! and processes. It includes process monitoring, system statistics collection,
//! and data structures for representing system state.

mod battery;
#[cfg(not(target_os = "macos"))]
mod drm;
mod gpu_monitor;
mod gpu_process_monitor;
#[cfg(target_os = "macos")]
mod iokit;
mod memory;
#[cfg(target_os = "macos")]
mod memory_macos;
#[cfg(target_os = "macos")]
mod native_processes;
mod port_monitor;
mod process_monitor;
#[cfg(target_os = "macos")]
mod smc;
mod system_monitor;
mod temperature_monitor;
mod types;

pub(crate) use battery::PowerSourceTimeRead;
pub use gpu_monitor::GpuMonitor;
pub use gpu_process_monitor::GpuProcessMonitor;
pub use port_monitor::collect_ports_by_pid;
pub use process_monitor::ProcessMonitor;
pub use system_monitor::SystemMonitor;
pub use temperature_monitor::TemperatureMonitor;
pub use types::*; // Re-export all types
