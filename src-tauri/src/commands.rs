//! Tauri command handlers
//!
//! This module contains the command handlers that are exposed to the frontend
//! through Tauri's IPC mechanism. These commands provide the interface between
//! the frontend and the system monitoring functionality.

use crate::monitoring::{ProcessInfo, ProcessMonitor, SystemStats};
use crate::state::AppState;
use std::sync::OnceLock;
use std::time::Instant;
use tauri::State;

/// `NEOHTOP_PERF=1` logs the cost of every refresh tick to stderr.
fn perf_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("NEOHTOP_PERF").is_some())
}

/// Retrieves the current list of processes and system statistics
///
/// # Arguments
///
/// * `state` - The application state containing system monitoring components
///
/// # Returns
///
/// A tuple containing:
/// * A vector of process information
/// * Current system statistics
///
/// # Errors
///
/// Returns an error string if:
/// * Failed to acquire locks on system state
/// * Failed to collect process information
#[tauri::command]
pub async fn get_processes(
    state: State<'_, AppState>,
) -> Result<(Vec<ProcessInfo>, SystemStats), String> {
    let mut sys = state.sys.lock().map_err(|e| e.to_string())?;
    let mut disks = state.disks.lock().map_err(|e| e.to_string())?;
    let mut networks = state.networks.lock().map_err(|e| e.to_string())?;
    let started = Instant::now();
    sys.refresh_all();
    disks.refresh(true);
    networks.refresh(true);
    let refreshed = Instant::now();

    let mut process_monitor = state.process_monitor.lock().map_err(|e| e.to_string())?;
    let mut system_monitor = state.system_monitor.lock().map_err(|e| e.to_string())?;

    let processes = process_monitor.collect_processes(&sys)?;
    let system_stats = system_monitor.collect_stats(&sys, &networks, &disks);
    let collected = Instant::now();

    let result = (processes, system_stats);
    if perf_enabled() {
        let bytes = serde_json::to_vec(&result).map(|v| v.len()).unwrap_or(0);
        eprintln!(
            "perf tick refresh_us={} collect_us={} bytes={} procs={}",
            (refreshed - started).as_micros(),
            (collected - refreshed).as_micros(),
            bytes,
            result.0.len()
        );
    }
    Ok(result)
}

/// Attempts to kill a process with the specified PID
///
/// # Arguments
///
/// * `pid` - Process ID to kill
/// * `state` - The application state
///
/// # Returns
///
/// * `true` if the process was successfully killed
/// * `false` if the process couldn't be killed or wasn't found
///
/// # Errors
///
/// Returns an error string if failed to acquire lock on system state
#[tauri::command]
pub async fn kill_process(pid: u32, state: State<'_, AppState>) -> Result<bool, String> {
    let sys = state.sys.lock().map_err(|e| e.to_string())?;
    Ok(ProcessMonitor::kill_process(&sys, pid))
}
