//! Application state management
//!
//! This module handles the global application state, including system monitoring
//! and process tracking capabilities.

use crate::monitoring::{collect_ports_by_pid, ProcessMonitor, SystemMonitor};
use crate::{perf_enabled, CpuTopology, Snapshot};
use std::sync::Mutex;
use std::time::Instant;
use sysinfo::{DiskRefreshKind, Disks, Networks, System};

/// Global application state
///
/// Maintains thread-safe access to system information and monitoring components
#[derive(Debug)]
pub struct AppState {
    /// System information handler
    sys: Mutex<System>,
    /// Networks information handler
    networks: Mutex<Networks>,
    /// Networks information handler
    disks: Mutex<Disks>,
    /// Process monitoring component
    process_monitor: Mutex<ProcessMonitor>,
    /// System statistics monitoring component
    system_monitor: Mutex<SystemMonitor>,
    /// Apple Silicon performance and efficiency core counts, read at startup.
    cpu_topology: CpuTopology,
}

impl AppState {
    /// Creates a new instance of the application state
    ///
    /// Initializes system monitoring and process tracking components
    ///
    /// # Returns
    ///
    /// A new `AppState` instance with initialized monitors
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        #[cfg(not(target_os = "macos"))]
        ProcessMonitor::refresh(&mut sys, false);
        let disks =
            Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
        let networks = Networks::new_with_refreshed_list();

        let process_monitor = ProcessMonitor::new();
        #[cfg(target_os = "macos")]
        let process_monitor = {
            let mut monitor = process_monitor;
            let _ = monitor.collect_native_processes(false, &Default::default());
            monitor
        };
        Self {
            process_monitor: Mutex::new(process_monitor),
            system_monitor: Mutex::new(SystemMonitor::new(&networks)),
            sys: Mutex::new(sys),
            disks: Mutex::new(disks),
            networks: Mutex::new(networks),
            cpu_topology: CpuTopology::detect(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn snapshot(&self, include_environ: bool, include_ports: bool) -> Result<Snapshot, String> {
        let mut sys = self.sys.lock().map_err(|e| e.to_string())?;
        let mut disks = self.disks.lock().map_err(|e| e.to_string())?;
        let mut networks = self.networks.lock().map_err(|e| e.to_string())?;
        let started = Instant::now();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        #[cfg(not(target_os = "macos"))]
        ProcessMonitor::refresh(&mut sys, include_environ);
        SystemMonitor::refresh_disks(&mut disks);
        networks.refresh(true);
        let refreshed = Instant::now();
        let mut process_monitor = self.process_monitor.lock().map_err(|e| e.to_string())?;
        let mut system_monitor = self.system_monitor.lock().map_err(|e| e.to_string())?;
        let ports_by_pid = if include_ports {
            collect_ports_by_pid()
        } else {
            Default::default()
        };
        #[cfg(not(target_os = "macos"))]
        let processes = process_monitor.collect_processes(&sys, include_environ, &ports_by_pid)?;
        #[cfg(target_os = "macos")]
        let processes =
            match process_monitor.collect_native_processes(include_environ, &ports_by_pid) {
                Ok(processes) => processes,
                Err(_) => {
                    ProcessMonitor::refresh(&mut sys, include_environ);
                    process_monitor.collect_processes(&sys, include_environ, &ports_by_pid)?
                }
            };
        let stats = system_monitor.collect_stats(&sys, &networks, &disks);
        if perf_enabled() {
            eprintln!(
                "perf tick refresh_us={} collect_us={} procs={}",
                (refreshed - started).as_micros(),
                refreshed.elapsed().as_micros(),
                processes.len()
            );
        }
        Ok(Snapshot {
            processes,
            stats,
            cpu_topology: self.cpu_topology,
        })
    }

    pub fn process_environ(&self, pid: u32) -> Result<Vec<String>, String> {
        let mut sys = System::new();
        ProcessMonitor::read_environment(&mut sys, pid)
    }

    pub fn kill_process(&self, pid: u32) -> Result<bool, String> {
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        Ok(ProcessMonitor::kill_process(&sys, pid))
    }

    pub fn process_environ_if_matches(
        &self,
        pid: u32,
        start_time: u64,
    ) -> Result<Vec<String>, String> {
        let mut sys = System::new();
        let id = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[id]),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        if !sys
            .process(id)
            .is_some_and(|process| process.start_time() == start_time)
        {
            return Err("The process exited or its PID changed".into());
        }
        ProcessMonitor::read_environment(&mut sys, pid)
    }

    pub fn process_stopped(&self, pid: u32, start_time: u64) -> Result<bool, String> {
        let mut sys = System::new();
        let id = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[id]),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        Ok(sys.process(id).is_none_or(|process| {
            process.start_time() != start_time || process.status() == sysinfo::ProcessStatus::Zombie
        }))
    }

    pub fn kill_process_if_matches(&self, pid: u32, start_time: u64) -> Result<bool, String> {
        let mut sys = System::new();
        let id = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[id]),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        Ok(sys
            .process(id)
            .is_some_and(|process| process.start_time() == start_time && process.kill()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_owned_child_environment_after_normal_refresh() {
        let mut child = std::process::Command::new("/usr/bin/python3")
            .args(["-c", "import time; time.sleep(30)", ""])
            .env("GTOP_ENV_MARKER", "fix2-owned-child")
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let state = AppState::new();
        let snapshot = state.snapshot(false, false).unwrap();
        let process = snapshot
            .processes
            .iter()
            .find(|p| p.pid == child.id())
            .unwrap();
        let result = state.process_environ_if_matches(process.pid, process.start_time);
        child.kill().unwrap();
        child.wait().unwrap();
        #[cfg(target_os = "macos")]
        {
            state.snapshot(false, false).unwrap();
            assert!(
                state
                    .sys
                    .lock()
                    .unwrap()
                    .process(sysinfo::Pid::from_u32(child.id()))
                    .is_none(),
                "exited action metadata must not accumulate between native snapshots"
            );
        }
        let values = result.unwrap();
        assert!(
            values
                .iter()
                .any(|value| value == "GTOP_ENV_MARKER=fix2-owned-child"),
            "owned child marker missing"
        );
        assert!(values.iter().any(|value| value.starts_with("PATH=")));
    }

    #[test]
    fn snapshot_contains_this_process_and_system_stats() {
        let state = AppState::new();
        let snapshot = state.snapshot(false, false).unwrap();
        assert!(snapshot
            .processes
            .iter()
            .any(|p| p.pid == std::process::id()));
        assert!(snapshot
            .processes
            .iter()
            .all(|p| p.environ.is_empty() && p.ports.is_empty()));
        assert!(snapshot.stats.memory_total > 0);
        assert!(!snapshot.stats.cpu_usage.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_failure_falls_back_to_sysinfo_and_retries_native_next_tick() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let state = AppState::new();
        state
            .process_monitor
            .lock()
            .unwrap()
            .fail_next_native_collection();

        let fallback = state.snapshot(false, false).unwrap();
        assert!(fallback.stats.memory_total > 0);
        assert!(fallback
            .processes
            .iter()
            .any(|process| process.pid == child.id()));

        let retry = state.snapshot(false, false).unwrap();
        assert!(retry
            .processes
            .iter()
            .any(|process| process.pid == child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn abandoned_stop_checks_do_not_retain_processes_in_app_state() {
        let state = AppState::new();
        for _ in 0..3 {
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            let snapshot = state.snapshot(false, false).unwrap();
            let process = snapshot
                .processes
                .iter()
                .find(|process| process.pid == child.id())
                .unwrap();
            assert!(!state
                .process_stopped(child.id(), process.start_time)
                .unwrap());
            child.kill().unwrap();
            child.wait().unwrap();
            assert!(state
                .sys
                .lock()
                .unwrap()
                .process(sysinfo::Pid::from_u32(child.id()))
                .is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_kill_rejects_another_process_identity() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let state = AppState::new();
        let snapshot = state.snapshot(false, false).unwrap();
        let process = snapshot
            .processes
            .iter()
            .find(|p| p.pid == child.id())
            .unwrap();
        assert!(!state
            .process_stopped(child.id(), process.start_time)
            .unwrap());
        assert!(state
            .process_stopped(child.id(), process.start_time + 1)
            .unwrap());
        assert!(!state
            .kill_process_if_matches(child.id(), process.start_time + 1)
            .unwrap());
        assert!(child.try_wait().unwrap().is_none());
        assert!(state
            .kill_process_if_matches(child.id(), process.start_time)
            .unwrap());
        assert!(!child.wait().unwrap().success());
        assert!(state
            .process_stopped(child.id(), process.start_time)
            .unwrap());
    }

    #[test]
    fn legacy_kill_uses_the_latest_snapshot_process() {
        let state = AppState::new();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let snapshot = state.snapshot(false, false).unwrap();
        assert!(snapshot.processes.iter().any(|p| p.pid == child.id()));
        let killed = state.kill_process(child.id()).unwrap();
        if !killed {
            let _ = child.kill();
        }
        child.wait().unwrap();
        assert!(killed);
    }

    #[test]
    #[ignore]
    fn tick_cost() {
        let state = AppState::new();
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            state.snapshot(false, false).unwrap();
        }
    }
}
