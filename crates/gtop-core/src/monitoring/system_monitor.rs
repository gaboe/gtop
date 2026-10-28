//! System statistics monitoring
//!
//! This module handles collection and monitoring of system-wide statistics
//! including CPU, memory, network, and disk usage.

use super::{GpuMonitor, SystemStats, TemperatureMonitor};
use std::fmt::Debug;
use std::path::Path;
use std::time::{Duration, Instant};
use sysinfo::{Disk, DiskRefreshKind, Disks, Networks, System};

#[cfg(target_os = "macos")]
use std::{ffi::CString, os::unix::ffi::OsStrExt};

#[derive(Clone, Copy)]
struct DiskCapacity {
    total_bytes: u64,
    free_bytes: u64,
    important_free_bytes: u64,
}

#[derive(Default)]
struct DiskStats {
    total_bytes: u64,
    used_bytes: u64,
    free_bytes: u64,
    purgeable_bytes: u64,
}

/// Monitors system-wide statistics
#[derive(Debug)]
pub struct SystemMonitor {
    /// Tracks network usage between updates
    last_network_update: (Instant, u64, u64),
    /// Reads the GPUs found at start-up
    gpu_monitor: GpuMonitor,
    /// Reads the temperature sensors
    temperature_monitor: TemperatureMonitor,
    disks_refreshed_at: Instant,
}

impl SystemMonitor {
    /// Creates a new system monitor instance
    ///
    /// # Arguments
    ///
    /// * `sys` - System information provider for initial readings
    pub fn new(networks: &Networks) -> Self {
        let (initial_rx, initial_tx) =
            networks
                .iter()
                .fold((0, 0), |(initial_rx, initial_tx), (_, data)| {
                    (
                        initial_rx + data.total_received(),
                        initial_tx + data.total_transmitted(),
                    )
                });

        Self {
            last_network_update: (Instant::now(), initial_rx, initial_tx),
            gpu_monitor: GpuMonitor::new(),
            temperature_monitor: TemperatureMonitor::new(),
            disks_refreshed_at: Instant::now(),
        }
    }

    /// Collects current system statistics
    ///
    /// # Arguments
    ///
    /// * `sys` - System information provider
    pub fn collect_stats(
        &mut self,
        sys: &System,
        networks: &Networks,
        disks: &Disks,
    ) -> SystemStats {
        let (network_rx, network_tx) = self.calculate_network_stats(networks);
        let disk_stats = self.calculate_disk_stats(disks);
        let load_average = System::load_average();

        SystemStats {
            cpu_usage: sys.cpus().iter().map(|cpu| cpu.cpu_usage()).collect(),
            memory_total: sys.total_memory(),
            memory_used: sys.used_memory(),
            swap_used: sys.used_swap(),
            memory_free: sys.total_memory() - sys.used_memory(),
            memory_cached: sys.total_memory()
                - (sys.used_memory() + (sys.total_memory() - sys.used_memory())),
            uptime: System::uptime(),
            load_avg: [load_average.one, load_average.five, load_average.fifteen],
            network_rx_bytes: network_rx,
            network_tx_bytes: network_tx,
            disk_total_bytes: disk_stats.total_bytes,
            disk_used_bytes: disk_stats.used_bytes,
            disk_free_bytes: disk_stats.free_bytes,
            disk_purgeable_bytes: disk_stats.purgeable_bytes,
            gpus: self.gpu_monitor.collect(),
            temperatures: self.temperature_monitor.collect(),
        }
    }

    /// Rebuilds disk entries every 60 seconds to read fresh capacity values.
    pub fn refresh_disks(&mut self, disks: &mut Disks) {
        self.refresh_disks_with(disks, || {
            Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage())
        });
    }

    fn refresh_disks_with(&mut self, disks: &mut Disks, refresh: impl FnOnce() -> Disks) {
        if self.disks_refreshed_at.elapsed() < Duration::from_secs(60) {
            return;
        }
        *disks = refresh();
        self.disks_refreshed_at = Instant::now();
    }

    /// Whether a disk counts toward the storage panel
    #[cfg(not(target_os = "windows"))]
    fn is_shown(disk: &Disk) -> bool {
        disk.mount_point() == Path::new("/")
    }

    /// Windows-specific disk filtering
    #[cfg(target_os = "windows")]
    fn is_shown(_disk: &Disk) -> bool {
        true
    }

    /// Calculates network usage rates
    fn calculate_network_stats(&mut self, networks: &Networks) -> (u64, u64) {
        let (current_rx, current_tx) =
            networks
                .iter()
                .fold((0, 0), |(current_rx, current_tx), (_, data)| {
                    (
                        current_rx + data.total_received(),
                        current_tx + data.total_transmitted(),
                    )
                });

        let elapsed = self.last_network_update.0.elapsed().as_secs_f64();
        let rx_rate = ((current_rx - self.last_network_update.1) as f64 / elapsed) as u64;
        let tx_rate = ((current_tx - self.last_network_update.2) as f64 / elapsed) as u64;

        self.last_network_update = (Instant::now(), current_rx, current_tx);
        (rx_rate, tx_rate)
    }

    /// Calculates disk usage statistics for the displayed volumes.
    fn calculate_disk_stats(&self, disks: &Disks) -> DiskStats {
        aggregate_disk_capacities(
            disks
                .iter()
                .filter(|disk| Self::is_shown(disk))
                .map(|disk| {
                    let important_free_bytes = disk.available_space();
                    #[cfg(target_os = "macos")]
                    let free_bytes =
                        macos_free_bytes(disk.mount_point()).unwrap_or(important_free_bytes);
                    #[cfg(not(target_os = "macos"))]
                    let free_bytes = important_free_bytes;

                    DiskCapacity {
                        total_bytes: disk.total_space(),
                        free_bytes,
                        important_free_bytes,
                    }
                }),
        )
    }
}

fn aggregate_disk_capacities(capacities: impl IntoIterator<Item = DiskCapacity>) -> DiskStats {
    capacities
        .into_iter()
        .fold(DiskStats::default(), |mut stats, disk| {
            stats.total_bytes += disk.total_bytes;
            stats.free_bytes += disk.free_bytes;
            stats.used_bytes += disk.total_bytes.saturating_sub(disk.free_bytes);
            stats.purgeable_bytes += disk.important_free_bytes.saturating_sub(disk.free_bytes);
            stats
        })
}

#[cfg(target_os = "macos")]
fn macos_free_bytes(mount_point: &Path) -> Option<u64> {
    let path = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    let result = unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    // SAFETY: `statfs` initializes the structure when it returns success.
    let stats = unsafe { stats.assume_init() };
    let available_blocks = stats.f_bavail;
    let block_size = u64::from(stats.f_bsize);
    Some(available_blocks.saturating_mul(block_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::{io::Write, os::unix::fs::MetadataExt};
    use sysinfo::System;

    #[cfg(target_os = "macos")]
    struct ScratchFile(std::path::PathBuf);

    #[cfg(target_os = "macos")]
    impl Drop for ScratchFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Tests creation of system monitor
    #[test]
    fn test_system_monitor_creation() {
        let networks = Networks::new();
        let monitor = SystemMonitor::new(&networks);
        assert_eq!(monitor.last_network_update.1, 0);
        assert_eq!(monitor.last_network_update.2, 0);
    }

    #[test]
    fn storage_refresh_reads_a_fresh_disk_list_after_sixty_seconds() {
        let mut monitor = SystemMonitor::new(&Networks::new());
        let mut disks = Disks::new();
        let initial = monitor.disks_refreshed_at;
        let mut reads = 0;
        monitor.refresh_disks_with(&mut disks, || {
            reads += 1;
            Disks::new()
        });
        assert_eq!(monitor.disks_refreshed_at, initial);
        assert_eq!(reads, 0);

        monitor.disks_refreshed_at = initial - Duration::from_secs(60);
        monitor.refresh_disks_with(&mut disks, || {
            reads += 1;
            Disks::new()
        });
        assert_eq!(reads, 1);
        assert!(monitor.disks_refreshed_at >= initial);
    }

    #[test]
    fn disk_stats_separate_space_free_now_from_purgeable_space() {
        let stats = aggregate_disk_capacities([DiskCapacity {
            total_bytes: 1000,
            free_bytes: 120,
            important_free_bytes: 300,
        }]);

        assert_eq!(stats.total_bytes, 1000);
        assert_eq!(stats.used_bytes, 880);
        assert_eq!(stats.free_bytes, 120);
        assert_eq!(stats.purgeable_bytes, 180);
    }

    #[test]
    fn purgeable_space_never_goes_below_zero() {
        let stats = aggregate_disk_capacities([DiskCapacity {
            total_bytes: 1000,
            free_bytes: 120,
            important_free_bytes: 100,
        }]);

        assert_eq!(stats.purgeable_bytes, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_volume_free_space_matches_df_avail() {
        let df_free_bytes = || {
            let output = std::process::Command::new("df")
                .args(["-k", "/"])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .nth(1)
                .unwrap()
                .split_whitespace()
                .nth(3)
                .unwrap()
                .parse::<u64>()
                .unwrap()
                * 1024
        };
        let before = df_free_bytes();
        let free_bytes = macos_free_bytes(Path::new("/")).unwrap();
        let after = df_free_bytes();
        let lower = before.min(after).saturating_sub(1024);
        let upper = before.max(after).saturating_add(1024);

        assert!(
            (lower..=upper).contains(&free_bytes),
            "statfs reported {free_bytes} bytes between df readings {before} and {after}"
        );
    }

    #[cfg(target_os = "macos")]
    #[ignore = "manual 512 MiB system-volume capacity probe"]
    #[test]
    fn storage_refresh_reads_new_system_volume_capacity() {
        let mut disks =
            Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
        let system_volume_device = std::fs::metadata("/").unwrap().dev();
        let temp_dir = std::env::temp_dir();
        assert_eq!(
            std::fs::metadata(&temp_dir).unwrap().dev(),
            system_volume_device,
            "temporary directory must be on the system volume"
        );
        let path = std::env::temp_dir().join(format!(
            "gtop-capacity-refresh-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _scratch_file = ScratchFile(path.clone());
        let mut file = std::fs::File::create(&path).unwrap();
        let block = vec![0x6b; 1024 * 1024];
        for _ in 0..512 {
            file.write_all(&block).unwrap();
        }
        file.sync_all().unwrap();
        drop(file);

        let mut monitor = SystemMonitor::new(&Networks::new());
        monitor.disks_refreshed_at = Instant::now() - Duration::from_secs(60);
        monitor.refresh_disks(&mut disks);
        let refreshed_available = disks
            .iter()
            .find(|disk| disk.mount_point() == Path::new("/"))
            .unwrap()
            .available_space();
        let fresh_disks =
            Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
        let fresh_available = fresh_disks
            .iter()
            .find(|disk| disk.mount_point() == Path::new("/"))
            .unwrap()
            .available_space();

        assert!(
            refreshed_available.abs_diff(fresh_available) <= 8 * 1024 * 1024,
            "refreshed capacity {refreshed_available} differs from fresh capacity {fresh_available}"
        );
    }

    /// Tests system statistics collection
    #[test]
    fn test_stats_collection() {
        let mut networks = Networks::new();
        let mut monitor = SystemMonitor::new(&networks);
        networks.refresh(true);
        let sys = System::new_all();
        let disks = Disks::new_with_refreshed_list();

        let stats = monitor.collect_stats(&sys, &networks, &disks);
        assert!(!stats.cpu_usage.is_empty());
        assert!(stats.memory_total > 0);
    }
}
