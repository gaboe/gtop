//! Per-process GPU usage monitoring
//!
//! Linux DRM drivers publish per-client engine busy times through
//! `/proc/<pid>/fdinfo/<fd>`, for every file descriptor pointing at a render
//! node. The counters are cumulative nanoseconds, so a usage percentage comes
//! from how much they grow between two samples, divided by the wall time that
//! passed in between.
//!
//! Only descriptors the current user is allowed to inspect can be read, so
//! processes belonging to other users are reported as idle rather than
//! unknown. On macOS the GPU time comes from the IOKit user clients each
//! process opens, also in cumulative nanoseconds. Other platforms report no
//! usage at all, which the frontend shows as a dash.

use std::collections::HashMap;
use std::time::Instant;

/// Tracks how much GPU engine time each process is using
#[derive(Debug)]
pub struct GpuProcessMonitor {
    /// Whether this machine exposes DRM devices at all
    supported: bool,
    /// Cumulative engine nanoseconds per process at the previous sample
    previous: HashMap<u32, u64>,
    /// When that sample was taken
    sampled_at: Instant,
    /// The GPUs whose clients are sampled
    #[cfg(target_os = "macos")]
    accelerators: Vec<super::iokit::Accelerator>,
}

impl GpuProcessMonitor {
    /// Creates a monitor, noting whether per-process accounting is possible
    pub fn new() -> Self {
        Self {
            #[cfg(not(target_os = "macos"))]
            supported: super::drm::is_supported(),
            #[cfg(target_os = "macos")]
            supported: true,
            #[cfg(target_os = "macos")]
            accelerators: super::iokit::Accelerator::all(),
            previous: HashMap::new(),
            sampled_at: Instant::now(),
        }
    }

    /// Whether this machine can account GPU time per process
    pub fn is_supported(&self) -> bool {
        self.supported
    }

    /// Samples every process and returns its GPU usage as a percentage
    ///
    /// The first call has no earlier sample to compare against and so reports
    /// nothing; usage appears from the second refresh onwards. A process can
    /// exceed 100% by keeping several engines busy at once, in the same way
    /// CPU usage exceeds 100% across cores.
    pub fn collect(&mut self) -> HashMap<u32, f32> {
        if !self.supported {
            return HashMap::new();
        }

        let current = self.engine_times();
        let now = Instant::now();
        let elapsed = now.duration_since(self.sampled_at).as_nanos();

        // Two samples taken within the same instant carry no information
        let usage = if elapsed == 0 {
            HashMap::new()
        } else {
            current
                .iter()
                .filter_map(|(pid, total)| {
                    let previous = self.previous.get(pid)?;
                    // The counters only ever climb, but a recycled pid can
                    // look like a decrease, so those are dropped instead of
                    // being reported as negative usage
                    let delta = total.checked_sub(*previous)?;
                    let percentage = (delta as f64 / elapsed as f64) * 100.0;
                    Some((*pid, percentage as f32))
                })
                .collect()
        };

        self.previous = current;
        self.sampled_at = now;

        usage
    }
}

impl GpuProcessMonitor {
    /// Cumulative GPU nanoseconds per process
    #[cfg(not(target_os = "macos"))]
    fn engine_times(&self) -> HashMap<u32, u64> {
        super::drm::collect_engine_times()
    }

    /// Cumulative GPU nanoseconds per process
    #[cfg(target_os = "macos")]
    fn engine_times(&self) -> HashMap<u32, u64> {
        let mut totals = HashMap::new();
        for accelerator in &self.accelerators {
            accelerator.gpu_time_per_process(&mut totals);
        }
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single fdinfo block is reduced to its client id and total engine time
    /// Capacity lines describe how many engines exist, not how busy they were
    /// Descriptors that are not DRM clients carry no client id
    /// The first sample only establishes a baseline, so it reports nothing
    #[test]
    fn test_first_sample_has_no_baseline() {
        let mut monitor = GpuProcessMonitor::new();
        assert!(monitor.collect().is_empty());
    }

    /// Percentages must stay non-negative and finite
    #[test]
    fn test_collected_usage_is_plausible() {
        let mut monitor = GpuProcessMonitor::new();
        monitor.collect();

        for (_, usage) in monitor.collect() {
            assert!(usage.is_finite());
            assert!(usage >= 0.0);
        }
    }
}
