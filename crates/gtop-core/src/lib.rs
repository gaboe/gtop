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
    fast: u128,
    slow: u128,
    /// First letters of the macOS perflevel names, fast bank first: P and E,
    /// or S (Super) and P (Performance) on M5.
    letters: [char; 2],
}

pub enum CpuBanks<'a> {
    Hybrid {
        fast: Vec<(usize, f32)>,
        slow: Vec<(usize, f32)>,
        letters: [char; 2],
    },
    Cores(&'a [f32]),
}

impl CpuTopology {
    /// Cluster `P` is the fast bank. `E` (M1 to M4) or `M` (M5) is the slow bank.
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
                    topology.fast |= bit;
                } else if cluster.contains("\"E\"") || cluster.contains("\"M\"") {
                    topology.slow |= bit;
                }
            }
        }
        topology
    }

    #[cfg(target_os = "macos")]
    fn detect() -> Self {
        let perflevels = std::process::Command::new("/usr/sbin/sysctl")
            .args([
                "-n",
                "hw.perflevel0.logicalcpu",
                "hw.perflevel1.logicalcpu",
                "hw.perflevel0.name",
                "hw.perflevel1.name",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success());
        let Some(perflevels) = perflevels else {
            return Self::default();
        };
        let topology = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPlatformDevice"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| Self::from_device_tree(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default();
        topology.with_perflevels(&String::from_utf8_lossy(&perflevels.stdout))
    }

    /// Keeps the device tree banks only when they match the macOS perflevel
    /// counts, and takes the bank letters from the perflevel names.
    fn with_perflevels(self, perflevels: &str) -> Self {
        let lines: Vec<_> = perflevels.lines().map(str::trim).collect();
        let [fast, slow, fast_name, slow_name] = lines[..] else {
            return Self::default();
        };
        let letter = |name: &str| {
            name.chars()
                .next()
                .filter(char::is_ascii_alphabetic)
                .map(|letter| letter.to_ascii_uppercase())
        };
        match (letter(fast_name), letter(slow_name)) {
            (Some(fast_letter), Some(slow_letter))
                if fast.parse() == Ok(self.fast.count_ones())
                    && slow.parse() == Ok(self.slow.count_ones())
                    && fast_letter != slow_letter =>
            {
                Self {
                    letters: [fast_letter, slow_letter],
                    ..self
                }
            }
            _ => Self::default(),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn detect() -> Self {
        Self::default()
    }

    fn complete(&self, count: usize) -> bool {
        count < 128
            && self.fast != 0
            && self.slow != 0
            && self.fast & self.slow == 0
            && self.fast | self.slow == (1u128 << count) - 1
            && self.letters.iter().all(char::is_ascii_uppercase)
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
            fast: bank(self.fast),
            slow: bank(self.slow),
            letters: self.letters,
        }
    }

    pub fn label(&self, usage_len: usize) -> String {
        if self.complete(usage_len) {
            let [fast, slow] = self.letters;
            format!(
                "CPU · {} {fast} + {} {slow} cores",
                self.fast.count_ones(),
                self.slow.count_ones()
            )
        } else {
            format!("CPU · {usage_len} cores")
        }
    }
}

pub fn perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| perf_requested(std::env::var_os("GTOP_PERF")))
}

/// Any `GTOP_PERF` value turns perf logging on, an empty one included.
fn perf_requested(value: Option<std::ffi::OsString>) -> bool {
    value.is_some()
}

#[cfg(test)]
mod perf_tests {
    #[test]
    fn any_gtop_perf_value_enables_perf_logging() {
        assert!(!super::perf_requested(None));
        assert!(super::perf_requested(Some("".into())));
        assert!(super::perf_requested(Some("1".into())));
    }
}

#[cfg(test)]
mod topology_tests {
    use super::{CpuBanks, CpuTopology};

    fn tree(cores: &[(&str, u32)]) -> String {
        cores
            .iter()
            .map(|(cluster, id)| {
                format!(
                    "+-o cpu{id}\n\"cluster-type\" = <\"{cluster}\">\n\"logical-cpu-id\" = {id}\n"
                )
            })
            .collect()
    }

    #[test]
    fn splits_performance_then_efficiency_cores() {
        let topology =
            CpuTopology::from_device_tree(&tree(&[("E", 0), ("E", 1), ("P", 2), ("P", 3)]))
                .with_perflevels("2\n2\nPerformance\nEfficiency\n");
        let usage = [10., 20., 30., 40.];
        match topology.banks(&usage) {
            CpuBanks::Hybrid {
                fast,
                slow,
                letters,
            } => {
                assert_eq!(fast, vec![(2, 30.), (3, 40.)]);
                assert_eq!(slow, vec![(0, 10.), (1, 20.)]);
                assert_eq!(letters, ['P', 'E']);
            }
            CpuBanks::Cores(_) => panic!("expected P and E banks"),
        }
        assert_eq!(topology.label(usage.len()), "CPU · 2 P + 2 E cores");
    }

    #[test]
    fn splits_m5_super_then_performance_cores() {
        let topology =
            CpuTopology::from_device_tree(&tree(&[("M", 0), ("M", 1), ("M", 2), ("P", 3)]))
                .with_perflevels("1\n3\nSuper\nPerformance\n");
        let usage = [10., 20., 30., 40.];
        match topology.banks(&usage) {
            CpuBanks::Hybrid {
                fast,
                slow,
                letters,
            } => {
                assert_eq!(fast, vec![(3, 40.)]);
                assert_eq!(slow, vec![(0, 10.), (1, 20.), (2, 30.)]);
                assert_eq!(letters, ['S', 'P']);
            }
            CpuBanks::Cores(_) => panic!("expected S and P banks"),
        }
        assert_eq!(topology.label(usage.len()), "CPU · 1 S + 3 P cores");
    }

    #[test]
    fn validates_device_tree_banks_against_macos_perflevels() {
        let topology = CpuTopology {
            fast: 0b1100,
            slow: 0b0011,
            letters: ['\0'; 2],
        };
        assert_eq!(
            topology.with_perflevels("2\n2\nPerformance\nEfficiency\n"),
            CpuTopology {
                letters: ['P', 'E'],
                ..topology
            }
        );
        for perflevels in [
            "",
            "2\n2",
            "3\n1\nPerformance\nEfficiency",
            "2\n2\nPerformance\nEfficiency\nExtra",
            "invalid\n2\nPerformance\nEfficiency",
            "2\n2\nPerformance\nPerformance",
            "2\n2\n\nEfficiency",
        ] {
            assert_eq!(topology.with_perflevels(perflevels), CpuTopology::default());
        }
    }

    #[test]
    fn falls_back_to_one_cores_bank_when_counts_are_missing_or_mismatched() {
        let usage = [10., 20., 30.];
        for topology in [
            CpuTopology::default(),
            CpuTopology {
                fast: 3,
                slow: 0,
                letters: ['P', 'E'],
            },
            CpuTopology {
                fast: 3,
                slow: 12,
                letters: ['P', 'E'],
            },
            CpuTopology {
                fast: 3,
                slow: 4,
                letters: ['\0'; 2],
            },
        ] {
            match topology.banks(&usage) {
                CpuBanks::Cores(cores) => assert_eq!(cores, &usage),
                CpuBanks::Hybrid { .. } => panic!("expected one Cores bank"),
            }
            assert_eq!(topology.label(usage.len()), "CPU · 3 cores");
        }
    }

    #[test]
    fn shows_one_cores_bank_beyond_the_128_core_mask() {
        let topology = CpuTopology {
            fast: u128::MAX >> 64,
            slow: u128::MAX << 64,
            letters: ['P', 'E'],
        };
        let usage = [1.; 128];
        assert!(matches!(topology.banks(&usage), CpuBanks::Cores(cores) if cores.len() == 128));
        assert_eq!(topology.label(usage.len()), "CPU · 128 cores");
    }

    #[test]
    fn empty_usage_is_one_cores_bank() {
        let topology = CpuTopology {
            fast: 1,
            slow: 2,
            letters: ['P', 'E'],
        };
        assert!(matches!(topology.banks(&[]), CpuBanks::Cores([])));
        assert_eq!(topology.label(0), "CPU · 0 cores");
    }
}
