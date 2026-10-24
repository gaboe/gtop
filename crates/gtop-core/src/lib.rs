pub mod monitoring;
mod state;

pub use monitoring::{ProcessInfo, SystemStats};
pub use state::AppState;

#[derive(Debug)]
pub struct Snapshot {
    pub processes: Vec<ProcessInfo>,
    pub stats: SystemStats,
    pub cpu_topology: CpuTopology,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTopology {
    performance: Option<usize>,
    efficiency: Option<usize>,
}

pub enum CpuBanks<'a> {
    Hybrid {
        performance: &'a [f32],
        efficiency: &'a [f32],
    },
    Cores(&'a [f32]),
}

impl CpuTopology {
    fn from_counts(performance: Option<usize>, efficiency: Option<usize>) -> Self {
        Self {
            performance: performance.filter(|count| *count > 0),
            efficiency: efficiency.filter(|count| *count > 0),
        }
    }

    #[cfg(target_os = "macos")]
    fn detect() -> Self {
        use std::process::Command;

        let output = Command::new("sysctl")
            .args(["-n", "hw.perflevel0.logicalcpu", "hw.perflevel1.logicalcpu"])
            .output();
        let Ok(output) = output else {
            return Self::default();
        };
        if !output.status.success() {
            return Self::default();
        }
        let counts = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|value| value.parse::<usize>().ok())
            .collect::<Vec<_>>();
        match counts.as_slice() {
            [performance, efficiency] => Self::from_counts(Some(*performance), Some(*efficiency)),
            _ => Self::default(),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn detect() -> Self {
        Self::default()
    }

    pub fn banks<'a>(&self, usage: &'a [f32]) -> CpuBanks<'a> {
        match (self.performance, self.efficiency) {
            (Some(performance), Some(efficiency)) if performance + efficiency == usage.len() => {
                let (performance_values, efficiency_values) = usage.split_at(performance);
                CpuBanks::Hybrid {
                    performance: performance_values,
                    efficiency: efficiency_values,
                }
            }
            _ => CpuBanks::Cores(usage),
        }
    }

    pub fn label(&self, usage_len: usize) -> String {
        match (self.performance, self.efficiency) {
            (Some(performance), Some(efficiency)) if performance + efficiency == usage_len => {
                format!("CPU · {performance} P + {efficiency} E cores")
            }
            _ => format!("CPU · {usage_len} cores"),
        }
    }
}

pub fn perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("NEOHTOP_PERF").is_some())
}

#[cfg(test)]
mod topology_tests {
    use super::{CpuBanks, CpuTopology};

    #[test]
    fn splits_performance_then_efficiency_cores() {
        let topology = CpuTopology::from_counts(Some(2), Some(2));
        let usage = [10., 20., 30., 40.];
        match topology.banks(&usage) {
            CpuBanks::Hybrid {
                performance,
                efficiency,
            } => {
                assert_eq!(performance, &[10., 20.]);
                assert_eq!(efficiency, &[30., 40.]);
            }
            CpuBanks::Cores(_) => panic!("expected P and E banks"),
        }
        assert_eq!(topology.label(usage.len()), "CPU · 2 P + 2 E cores");
    }

    #[test]
    fn falls_back_to_one_cores_bank_when_counts_are_missing_or_mismatched() {
        let usage = [10., 20., 30.];
        for topology in [
            CpuTopology::default(),
            CpuTopology::from_counts(Some(2), None),
            CpuTopology::from_counts(Some(2), Some(2)),
        ] {
            match topology.banks(&usage) {
                CpuBanks::Cores(cores) => assert_eq!(cores, &usage),
                CpuBanks::Hybrid { .. } => panic!("expected one Cores bank"),
            }
            assert_eq!(topology.label(usage.len()), "CPU · 3 cores");
        }
    }
}
