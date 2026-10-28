//! System statistics monitoring
//!
//! This module handles collection and monitoring of system-wide statistics
//! including CPU, memory, network, and disk usage.

use super::{GpuMonitor, SystemStats, TemperatureMonitor};
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use sysinfo::{Disk, DiskRefreshKind, Disks, Networks, System};

#[cfg(target_os = "macos")]
use std::{ffi::CString, os::unix::ffi::OsStrExt};

#[derive(Clone, Copy)]
struct DiskCapacity {
    total_bytes: u64,
    free_bytes: u64,
    purgeable_bytes: u64,
}

#[derive(Default)]
struct DiskStats {
    total_bytes: u64,
    used_bytes: u64,
    free_bytes: u64,
    purgeable_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct DiskSample {
    real_free_bytes: Option<u64>,
    purgeable_bytes: Option<u64>,
}

impl DiskSample {
    fn paired(important_free_bytes: u64, real_free_bytes: u64) -> Self {
        Self {
            real_free_bytes: Some(real_free_bytes),
            purgeable_bytes: Some(important_free_bytes.saturating_sub(real_free_bytes)),
        }
    }

    fn refresh_pair(&mut self, important_free_bytes: u64, real_free_bytes: Option<u64>) {
        if let Some(real_free_bytes) = real_free_bytes {
            *self = Self::paired(important_free_bytes, real_free_bytes);
        } else {
            self.purgeable_bytes = None;
        }
    }

    fn live_free(&mut self, reading: Option<u64>) -> Option<u64> {
        if let Some(real_free_bytes) = reading {
            self.real_free_bytes = Some(real_free_bytes);
        }
        self.real_free_bytes
    }
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
    disks_refreshed_at: Option<Instant>,
    disk_samples: HashMap<PathBuf, DiskSample>,
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
            disks_refreshed_at: None,
            disk_samples: HashMap::new(),
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
        self.refresh_disks_with(
            disks,
            || Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage()),
            real_free_bytes,
        );
    }

    fn refresh_disks_with(
        &mut self,
        disks: &mut Disks,
        refresh: impl FnOnce() -> Disks,
        mut read_free: impl FnMut(&Path) -> Option<u64>,
    ) {
        if self
            .disks_refreshed_at
            .is_some_and(|refreshed_at| refreshed_at.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        *disks = refresh();
        let mut samples = HashMap::new();
        for disk in disks.iter().filter(|disk| Self::is_shown(disk)) {
            let path = disk.mount_point().to_path_buf();
            let real_free_bytes = read_free(&path);
            let previous = self.disk_samples.get(&path).copied().unwrap_or_default();
            let mut sample = previous;
            sample.refresh_pair(disk.available_space(), real_free_bytes);
            samples.insert(path, sample);
        }
        self.disk_samples = samples;
        self.disks_refreshed_at = Some(Instant::now());
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
    fn calculate_disk_stats(&mut self, disks: &Disks) -> DiskStats {
        self.calculate_disk_stats_with(disks, real_free_bytes)
    }

    fn calculate_disk_stats_with(
        &mut self,
        disks: &Disks,
        mut read_free: impl FnMut(&Path) -> Option<u64>,
    ) -> DiskStats {
        #[cfg(not(target_os = "macos"))]
        let _ = &mut read_free;

        let mut capacities = Vec::new();
        for disk in disks.iter().filter(|disk| Self::is_shown(disk)) {
            #[cfg(target_os = "macos")]
            {
                let sample = self
                    .disk_samples
                    .entry(disk.mount_point().to_path_buf())
                    .or_default();
                if sample.live_free(read_free(disk.mount_point())).is_none() {
                    return DiskStats::default();
                }
                let Some(capacity) = disk_capacity(disk.total_space(), sample) else {
                    return DiskStats::default();
                };
                capacities.push(capacity);
            };
            #[cfg(not(target_os = "macos"))]
            capacities.push(DiskCapacity {
                total_bytes: disk.total_space(),
                free_bytes: disk.available_space(),
                purgeable_bytes: 0,
            });
        }
        aggregate_disk_capacities(capacities)
    }
}

fn disk_capacity(total_bytes: u64, sample: &DiskSample) -> Option<DiskCapacity> {
    Some(DiskCapacity {
        total_bytes,
        free_bytes: sample.real_free_bytes?,
        purgeable_bytes: sample.purgeable_bytes.unwrap_or_default(),
    })
}

fn aggregate_disk_capacities(capacities: impl IntoIterator<Item = DiskCapacity>) -> DiskStats {
    capacities
        .into_iter()
        .fold(DiskStats::default(), |mut stats, disk| {
            stats.total_bytes += disk.total_bytes;
            stats.free_bytes += disk.free_bytes;
            stats.used_bytes += disk.total_bytes.saturating_sub(disk.free_bytes);
            stats.purgeable_bytes += disk.purgeable_bytes;
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

fn real_free_bytes(mount_point: &Path) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        macos_free_bytes(mount_point)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mount_point;
        None
    }
}

#[cfg(test)]
fn sample_disk_stats(total_bytes: u64, sample: &DiskSample) -> Option<DiskStats> {
    Some(aggregate_disk_capacities([disk_capacity(
        total_bytes,
        sample,
    )?]))
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
        let mut reads = 0;
        monitor.refresh_disks_with(
            &mut disks,
            || {
                reads += 1;
                Disks::new()
            },
            |_| Some(100),
        );
        assert_eq!(reads, 1);
        let first_refresh = monitor.disks_refreshed_at;
        monitor.refresh_disks_with(
            &mut disks,
            || {
                reads += 1;
                Disks::new()
            },
            |_| Some(100),
        );
        assert_eq!(monitor.disks_refreshed_at, first_refresh);
        assert_eq!(reads, 1);

        monitor.disks_refreshed_at = Some(Instant::now() - Duration::from_secs(60));
        monitor.refresh_disks_with(
            &mut disks,
            || {
                reads += 1;
                Disks::new()
            },
            |_| Some(100),
        );
        assert_eq!(reads, 2);
        assert!(monitor.disks_refreshed_at.is_some());
    }

    #[test]
    fn disk_stats_separate_space_free_now_from_purgeable_space() {
        let stats = aggregate_disk_capacities([DiskCapacity {
            total_bytes: 1000,
            free_bytes: 120,
            purgeable_bytes: 180,
        }]);

        assert_eq!(stats.total_bytes, 1000);
        assert_eq!(stats.used_bytes, 880);
        assert_eq!(stats.free_bytes, 120);
        assert_eq!(stats.purgeable_bytes, 180);
    }

    #[test]
    fn paired_purgeable_stays_fixed_between_refreshes() {
        let mut sample = DiskSample::paired(130, 100);
        let first = sample_disk_stats(1000, &sample).unwrap();
        assert_eq!(first.free_bytes, 100);
        assert_eq!(first.purgeable_bytes, 30);

        assert_eq!(sample.live_free(Some(90)), Some(90));
        let after_write = sample_disk_stats(1000, &sample).unwrap();
        assert_eq!(after_write.used_bytes, 910);
        assert_eq!(after_write.free_bytes, 90);
        assert_eq!(after_write.purgeable_bytes, 30);

        sample.refresh_pair(110, Some(90));
        let after_paired_refresh = sample_disk_stats(1000, &sample).unwrap();
        assert_eq!(after_paired_refresh.purgeable_bytes, 20);
    }

    #[test]
    fn failed_free_read_keeps_last_good_capacity_or_reports_unknown() {
        let mut sample = DiskSample::paired(130, 100);
        assert_eq!(sample.live_free(None), Some(100));
        assert_eq!(sample_disk_stats(1000, &sample).unwrap().free_bytes, 100);

        let mut missing = DiskSample::default();
        assert_eq!(missing.live_free(None), None);
        assert!(sample_disk_stats(1000, &missing).is_none());
        assert_eq!(sample.live_free(None), Some(100));
        sample.refresh_pair(130, None);
        assert_eq!(sample.purgeable_bytes, None);
        let after_failed_pair = sample_disk_stats(1000, &sample).unwrap();
        assert_eq!(after_failed_pair.free_bytes, 100);
        assert_eq!(after_failed_pair.purgeable_bytes, 0);
    }

    #[test]
    fn purgeable_saturates_when_real_free_exceeds_important_free() {
        let sample = DiskSample::paired(100, 120);
        assert_eq!(sample_disk_stats(1000, &sample).unwrap().purgeable_bytes, 0);
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
        monitor.disks_refreshed_at = Some(Instant::now() - Duration::from_secs(60));
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
