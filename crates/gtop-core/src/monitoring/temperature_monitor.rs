//! Temperature monitoring
//!
//! Machines expose dozens of sensors with driver-specific labels, such as
//! "PMU tdie3" on Apple silicon or "coretemp Package id 0" on Linux. Each one
//! is sorted into a component group by its label, and the panel shows the
//! hottest sensor of every group.

use super::Temperature;
use std::time::{Duration, Instant};
use sysinfo::Components;

/// Reading a sensor takes most of a millisecond on macOS, so sensors are read
/// less often than processes
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Component groups and the label fragments that identify their sensors,
/// matched case-insensitively
const GROUPS: [(&str, &[&str]); 4] = [
    ("CPU", &["tdie", "package id", "tctl", "cpu"]),
    ("GPU", &["gpu", "amdgpu", "nouveau"]),
    ("SSD", &["nand", "nvme", "ssd"]),
    ("Battery", &["battery"]),
];

/// Monitors the temperature sensors that belong to a known group
#[derive(Debug)]
pub struct TemperatureMonitor {
    components: Components,
    /// Index into `GROUPS` for each sensor in `components`
    groups: Vec<Option<usize>>,
    refreshed_at: Option<Instant>,
    readings: Vec<Temperature>,
    #[cfg(target_os = "macos")]
    gpu: Option<super::smc::GpuTemperature>,
}

impl Default for TemperatureMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl TemperatureMonitor {
    /// Lists the sensors once; they do not change while the application runs
    pub fn new() -> Self {
        let components = Components::new_with_refreshed_list();
        let groups = components
            .iter()
            .map(|component| group_of(component.label()))
            .collect();
        Self {
            components,
            groups,
            refreshed_at: None,
            readings: Vec::new(),
            #[cfg(target_os = "macos")]
            gpu: super::smc::GpuTemperature::new(),
        }
    }

    /// Returns the hottest reading of every group that has a sensor
    pub fn collect(&mut self) -> Vec<Temperature> {
        if self
            .refreshed_at
            .is_some_and(|at| at.elapsed() < REFRESH_INTERVAL)
        {
            return self.readings.clone();
        }

        let mut hottest: [Option<f32>; GROUPS.len()] = [None; GROUPS.len()];
        for (component, group) in self.components.iter_mut().zip(&self.groups) {
            let Some(group) = *group else { continue };
            component.refresh();
            if let Some(celsius) = component.temperature().filter(|c| c.is_finite()) {
                hottest[group] = Some(hottest[group].map_or(celsius, |h| h.max(celsius)));
            }
        }

        #[cfg(target_os = "macos")]
        if let Some(celsius) = self.gpu.as_ref().and_then(|gpu| gpu.hottest()) {
            hottest[1] = Some(hottest[1].map_or(celsius, |h| h.max(celsius)));
        }

        self.readings = GROUPS
            .iter()
            .zip(hottest)
            .filter_map(|((name, _), celsius)| {
                Some(Temperature {
                    name: name.to_string(),
                    celsius: celsius?,
                })
            })
            .collect();
        self.refreshed_at = Some(Instant::now());
        self.readings.clone()
    }
}

fn group_of(label: &str) -> Option<usize> {
    let label = label.to_lowercase();
    GROUPS
        .iter()
        .position(|(_, fragments)| fragments.iter().any(|f| label.contains(f)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_sensors_into_groups_by_label() {
        let name = |label| group_of(label).map(|i| GROUPS[i].0);
        assert_eq!(name("PMU tdie3"), Some("CPU"));
        assert_eq!(name("coretemp Package id 0"), Some("CPU"));
        assert_eq!(name("amdgpu edge"), Some("GPU"));
        assert_eq!(name("NAND CH0 temp"), Some("SSD"));
        assert_eq!(name("gas gauge battery"), Some("Battery"));
        assert_eq!(name("PMU tdev1"), None);
    }

    #[test]
    fn readings_are_plausible() {
        for reading in TemperatureMonitor::new().collect() {
            assert!(
                (-40.0..150.0).contains(&reading.celsius),
                "{} reads {}",
                reading.name,
                reading.celsius
            );
        }
    }
}
