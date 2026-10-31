pub mod monitoring;
mod state;

pub use monitoring::{ProcessInfo, SystemStats};
pub use state::AppState;

#[derive(Debug)]
pub struct Snapshot {
    pub processes: std::sync::Arc<Vec<ProcessInfo>>,
    pub process_data: ProcessData,
    /// Unix time of the last valid process collection.
    pub process_observed_at: u64,
    pub stats: SystemStats,
    pub cpu_topology: CpuTopology,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessData {
    Fresh,
    /// Rows repeat the last emitted collection. Age starts at the first failure.
    Stale {
        since: std::time::Instant,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTopology {
    performance: u128,
    efficiency: u128,
}

pub enum CpuBanks<'a> {
    Hybrid {
        performance: Vec<(usize, f32)>,
        efficiency: Vec<(usize, f32)>,
    },
    Cores(&'a [f32]),
}

impl CpuTopology {
    fn from_device_tree(tree: &str) -> Self {
        let mut topology = Self::default();
        for node in tree.split("+-o ") {
            let cluster = node.lines().find(|line| line.contains("\"cluster-type\""));
            let id = node
                .lines()
                .find(|line| line.contains("\"logical-cpu-id\""))
                .and_then(|line| line.split('=').nth(1))
                .and_then(|value| value.trim().parse::<u32>().ok());
            if let (Some(cluster), Some(id)) = (cluster, id) {
                let Some(bit) = 1u128.checked_shl(id) else {
                    return Self::default();
                };
                if cluster.contains("\"P\"") {
                    topology.performance |= bit;
                } else if cluster.contains("\"E\"") {
                    topology.efficiency |= bit;
                }
            }
        }
        topology
    }

    #[cfg(target_os = "macos")]
    fn detect() -> Self {
        let counts = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.perflevel0.logicalcpu", "hw.perflevel1.logicalcpu"])
            .output()
            .ok()
            .filter(|output| output.status.success());
        let Some(counts) = counts else {
            return Self::default();
        };
        let topology = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPlatformDevice"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| Self::from_device_tree(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default();
        topology.with_perflevel_counts(&String::from_utf8_lossy(&counts.stdout))
    }

    fn with_perflevel_counts(self, counts: &str) -> Self {
        let counts: Vec<_> = counts
            .lines()
            .map(|line| line.trim().parse::<u32>())
            .collect();
        if counts.len() == 2
            && counts[0] == Ok(self.performance.count_ones())
            && counts[1] == Ok(self.efficiency.count_ones())
        {
            self
        } else {
            Self::default()
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn detect() -> Self {
        Self::default()
    }

    fn complete(&self, count: usize) -> bool {
        count > 0
            && count < 128
            && self.performance != 0
            && self.efficiency != 0
            && self.performance & self.efficiency == 0
            && self.performance | self.efficiency == (1u128 << count) - 1
    }

    pub fn banks<'a>(&self, usage: &'a [f32]) -> CpuBanks<'a> {
        if !self.complete(usage.len()) {
            return CpuBanks::Cores(usage);
        }
        let bank = |mask| {
            usage
                .iter()
                .copied()
                .enumerate()
                .filter(|(index, _)| mask & (1u128 << index) != 0)
                .collect()
        };
        CpuBanks::Hybrid {
            performance: bank(self.performance),
            efficiency: bank(self.efficiency),
        }
    }

    pub fn label(&self, usage_len: usize) -> String {
        if self.complete(usage_len) {
            format!(
                "CPU · {} P + {} E cores",
                self.performance.count_ones(),
                self.efficiency.count_ones()
            )
        } else {
            format!("CPU · {usage_len} cores")
        }
    }
}

pub fn perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("GTOP_PERF").is_some())
}

#[cfg(test)]
mod topology_tests {
    use super::{CpuBanks, CpuTopology};

    #[test]
    fn splits_performance_then_efficiency_cores() {
        let topology = CpuTopology::from_device_tree("+-o cpu0\n\"cluster-type\" = <\"E\">\n\"logical-cpu-id\" = 0\n+-o cpu1\n\"cluster-type\" = <\"E\">\n\"logical-cpu-id\" = 1\n+-o cpu2\n\"cluster-type\" = <\"P\">\n\"logical-cpu-id\" = 2\n+-o cpu3\n\"cluster-type\" = <\"P\">\n\"logical-cpu-id\" = 3");
        let usage = [10., 20., 30., 40.];
        match topology.banks(&usage) {
            CpuBanks::Hybrid {
                performance,
                efficiency,
            } => {
                assert_eq!(performance, vec![(2, 30.), (3, 40.)]);
                assert_eq!(efficiency, vec![(0, 10.), (1, 20.)]);
            }
            CpuBanks::Cores(_) => panic!("expected P and E banks"),
        }
        assert_eq!(topology.label(usage.len()), "CPU · 2 P + 2 E cores");
    }

    #[test]
    fn validates_device_tree_banks_against_macos_perflevel_counts() {
        let topology = CpuTopology {
            performance: 0b1100,
            efficiency: 0b0011,
        };
        assert_eq!(topology.with_perflevel_counts("2\n2\n"), topology);
        for counts in ["", "2", "3\n1", "2\n2\n2", "invalid\n2"] {
            assert_eq!(
                topology.with_perflevel_counts(counts),
                CpuTopology::default()
            );
        }
    }

    #[test]
    fn falls_back_to_one_cores_bank_when_counts_are_missing_or_mismatched() {
        let usage = [10., 20., 30.];
        for topology in [
            CpuTopology::default(),
            CpuTopology {
                performance: 3,
                efficiency: 0,
            },
            CpuTopology {
                performance: 3,
                efficiency: 12,
            },
        ] {
            match topology.banks(&usage) {
                CpuBanks::Cores(cores) => assert_eq!(cores, &usage),
                CpuBanks::Hybrid { .. } => panic!("expected one Cores bank"),
            }
            assert_eq!(topology.label(usage.len()), "CPU · 3 cores");
        }
    }
}
