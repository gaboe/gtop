use serde::Serialize;
use std::fmt::Debug;

/// Formatted process data for the monitor UI.
#[derive(Serialize, Debug, Clone, Default)]
pub struct ProcessInfo {
    /// Process ID
    pub pid: u32,
    /// Parent process ID
    pub ppid: u32,
    /// Process name
    pub name: String,
    /// CPU usage as percentage (0-100)
    pub cpu_usage: f32,
    /// Seconds covered by this CPU sample; None when no counter delta is available.
    pub cpu_sample_seconds: Option<f64>,
    /// GPU usage as percentage, or `None` where the platform cannot report it
    pub gpu_usage: Option<f32>,
    /// Physical memory usage in bytes
    pub memory_usage: u64,
    /// Process status as string
    pub status: String,
    /// Username of the process owner
    pub user: String,
    /// Full command string
    pub command: String,
    pub executable: String,
    pub argv: Vec<String>,
    /// Number of threads (if available)
    pub threads: Option<u32>,
    /// Environment variables
    pub environ: Vec<String>,
    /// Root directory of the process
    pub root: String,
    /// Virtual memory usage in bytes
    pub virtual_memory: u64,
    /// Process start time (Unix timestamp)
    pub start_time: u64,
    /// Process running time in seconds
    pub run_time: u64,
    /// Disk I/O statistics (read bytes, written bytes)
    pub disk_usage: (u64, u64),
    /// Seconds between the valid disk samples, when the collector can report them.
    pub disk_sample_seconds: Option<f64>,
    /// Session ID of the process
    pub session_id: Option<u32>,
    /// Local bound ports (TCP LISTEN + UDP). Empty when port collection is skipped.
    pub ports: Vec<u16>,
}

/// Statistics for a single GPU exposed to the frontend
///
/// Every reading is optional: which ones a card actually publishes depends on
/// its driver, and the frontend hides whatever is missing.
#[derive(Serialize, Debug, Clone)]
pub struct GpuInfo {
    /// Graphics card model name
    pub name: String,
    /// Graphics card vendor name
    pub vendor: String,
    /// Core utilization as percentage (0-100)
    pub utilization: Option<f32>,
    /// Total video memory in bytes
    pub memory_total: Option<u64>,
    /// Used video memory in bytes
    pub memory_used: Option<u64>,
    /// Core temperature in degrees Celsius
    pub temperature: Option<f32>,
    /// Board power draw in watts
    pub power_watts: Option<f32>,
    /// Shader clock in MHz
    pub core_clock_mhz: Option<u32>,
}

/// Hottest sensor reading of one component group, such as "CPU"
#[derive(Serialize, Debug, Clone)]
pub struct Temperature {
    /// Component group name
    pub name: String,
    /// Temperature in degrees Celsius
    pub celsius: f32,
}

/// Battery charge and power readings exposed by the platform.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Power {
    /// Battery charge from 0 to 100 percent.
    pub percent: u8,
    /// Whether the battery is charging, discharging, or externally powered.
    pub state: BatteryState,
    /// Current power drawn by the notebook, when available.
    pub system_watts: Option<f32>,
    /// Signed battery power. Positive values charge the battery.
    pub battery_watts: Option<f32>,
    /// Rated adapter power, when available.
    pub adapter_watts: Option<u32>,
}

/// Battery power state from the system's perspective.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    Charging { minutes_to_full: Option<u32> },
    Discharging { minutes_to_empty: Option<u32> },
    Full,
    NotCharging,
}

/// System-wide statistics exposed to the frontend
/// Provides overall system resource usage and performance metrics
#[derive(Serialize, Debug)]
pub struct SystemStats {
    /// CPU usage per core as percentage (0-100)
    pub cpu_usage: Vec<f32>,
    /// Total physical memory in bytes
    pub memory_total: u64,
    /// Used physical memory in bytes
    pub memory_used: u64,
    /// Free physical memory in bytes
    pub memory_free: u64,
    /// Cached memory in bytes
    pub memory_cached: u64,
    /// Exclusive macOS VM page groups in bytes, when the VM API returned a sample.
    pub memory_breakdown: Option<MemoryBreakdown>,
    /// Swap in use in bytes, from the same memory refresh.
    pub swap_used: u64,
    /// System uptime in seconds
    pub uptime: u64,
    /// Load averages for 1, 5, and 15 minutes
    pub load_avg: [f64; 3],
    /// Total bytes received over network
    pub network_rx_bytes: u64,
    /// Total bytes transmitted over network
    pub network_tx_bytes: u64,
    /// Cumulative interface bytes, received and sent.
    pub network_totals: Option<(u64, u64)>,
    /// Cumulative physical internal SSD bytes, read and written.
    pub disk_io_totals: Option<(u64, u64)>,
    /// Total disk space in bytes
    pub disk_total_bytes: u64,
    /// Used disk space in bytes
    pub disk_used_bytes: u64,
    /// Free disk space in bytes
    pub disk_free_bytes: u64,
    /// Space macOS can reclaim, beyond space free now
    pub disk_purgeable_bytes: u64,
    /// Per-GPU statistics; empty when no supported GPU was found
    pub gpus: Vec<GpuInfo>,
    /// Hottest reading per component group; empty when no sensor is readable
    pub temperatures: Vec<Temperature>,
    /// Battery and power readings, or `None` when no battery is available.
    pub power: Option<Power>,
}

/// Physical-memory queues and overlapping attributes used by the Memory window.
#[derive(Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryBreakdown {
    pub wired: u64,
    pub active: u64,
    pub compressed: u64,
    pub inactive: u64,
    /// Overlaps the physical queues. Do not add to the total.
    pub purgeable: u64,
    pub speculative: u64,
    /// Overlaps active, inactive, and speculative pages. Do not add to the total.
    pub file_cache: u64,
    pub free: u64,
    pub other: u64,
}
