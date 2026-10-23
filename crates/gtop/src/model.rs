use gtop_core::ProcessInfo;
use regex::{Regex, RegexBuilder};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessKey {
    pub pid: u32,
    pub start_time: u64,
}

impl From<&ProcessInfo> for ProcessKey {
    fn from(process: &ProcessInfo) -> Self {
        Self {
            pid: process.pid,
            start_time: process.start_time,
        }
    }
}

pub enum SearchTerm {
    Port(u16),
    Text {
        text: String,
        pattern: Option<Regex>,
    },
}

#[derive(Default)]
pub struct Search {
    terms: Vec<SearchTerm>,
}

pub fn parse_port(term: &str) -> Option<u16> {
    let term = term.trim();
    let digits = term.strip_prefix(':').or_else(|| {
        term.get(..5)
            .filter(|prefix| prefix.eq_ignore_ascii_case("port:"))
            .and_then(|_| term.get(5..))
    })?;
    if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u16>().ok().filter(|port| *port != 0)
}

impl Search {
    pub fn new(query: &str) -> Self {
        if query.is_empty() {
            return Self::default();
        }
        Self {
            terms: query
                .split(',')
                .map(str::trim)
                .map(|term| {
                    parse_port(term)
                        .map(SearchTerm::Port)
                        .unwrap_or_else(|| SearchTerm::Text {
                            text: term.to_lowercase(),
                            pattern: RegexBuilder::new(term).case_insensitive(true).build().ok(),
                        })
                })
                .collect(),
        }
    }

    pub fn needs_ports(&self) -> bool {
        self.terms
            .iter()
            .any(|term| matches!(term, SearchTerm::Port(_)))
    }

    pub fn matches(&self, process: &ProcessInfo) -> bool {
        self.terms.is_empty()
            || self.terms.iter().any(|term| match term {
                SearchTerm::Port(port) => process.ports.contains(port),
                SearchTerm::Text { text, pattern } => {
                    process.name.to_lowercase().contains(text)
                        || process.command.to_lowercase().contains(text)
                        || process.pid.to_string().contains(text)
                        || pattern
                            .as_ref()
                            .is_some_and(|pattern| pattern.is_match(&process.name))
                }
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessColumn {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    Gpu,
    Memory,
    VirtualMemory,
    Disk,
    Runtime,
}

impl ProcessColumn {
    pub const ALL: [Self; 10] = [
        Self::Name,
        Self::Pid,
        Self::Status,
        Self::User,
        Self::Cpu,
        Self::Gpu,
        Self::Memory,
        Self::VirtualMemory,
        Self::Disk,
        Self::Runtime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Process name",
            Self::Pid => "PID",
            Self::Status => "Status",
            Self::User => "User",
            Self::Cpu => "CPU %",
            Self::Gpu => "GPU %",
            Self::Memory => "RAM",
            Self::VirtualMemory => "VIRT",
            Self::Disk => "Disk R / W",
            Self::Runtime => "Run time",
        }
    }

    pub fn compare(self, a: &ProcessInfo, b: &ProcessInfo) -> Ordering {
        match self {
            Self::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            Self::Pid => a.pid.cmp(&b.pid),
            Self::Status => a.status.cmp(&b.status),
            Self::User => a.user.cmp(&b.user),
            Self::Cpu => a.cpu_usage.total_cmp(&b.cpu_usage),
            Self::Gpu => a
                .gpu_usage
                .partial_cmp(&b.gpu_usage)
                .unwrap_or(Ordering::Equal),
            Self::Memory => a.memory_usage.cmp(&b.memory_usage),
            Self::VirtualMemory => a.virtual_memory.cmp(&b.virtual_memory),
            Self::Disk => (a.disk_usage.0.saturating_add(a.disk_usage.1))
                .cmp(&b.disk_usage.0.saturating_add(b.disk_usage.1)),
            Self::Runtime => a.run_time.cmp(&b.run_time),
        }
    }

    pub fn text(self, process: &ProcessInfo) -> String {
        match self {
            Self::Name => process.name.clone(),
            Self::Pid => process.pid.to_string(),
            Self::Status => process.status.clone(),
            Self::User => process.user.clone(),
            Self::Cpu => format!("{:.1}%", process.cpu_usage),
            Self::Gpu => process
                .gpu_usage
                .map(|value| format!("{value:.1}%"))
                .unwrap_or_else(|| "Unavailable".into()),
            Self::Memory => bytes(process.memory_usage),
            Self::VirtualMemory => bytes(process.virtual_memory),
            Self::Disk => format!(
                "{} / {}",
                bytes(process.disk_usage.0),
                bytes(process.disk_usage.1)
            ),
            Self::Runtime => duration(process.run_time),
        }
    }
}

pub fn process_order(
    processes: &[ProcessInfo],
    search: &Search,
    column: ProcessColumn,
    descending: bool,
) -> Vec<usize> {
    let mut order: Vec<_> = processes
        .iter()
        .enumerate()
        .filter_map(|(index, process)| search.matches(process).then_some(index))
        .collect();
    order.sort_unstable_by(|a, b| {
        let comparison = column.compare(&processes[*a], &processes[*b]);
        (if descending {
            comparison.reverse()
        } else {
            comparison
        })
        .then_with(|| processes[*a].pid.cmp(&processes[*b].pid))
    });
    order
}

pub fn bytes(value: u64) -> String {
    let mut amount = value as f64;
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut unit = 0;
    while amount >= 1024. && unit < units.len() - 1 {
        amount /= 1024.;
        unit += 1;
    }
    format!("{amount:.1} {}", units[unit])
}

pub fn duration(seconds: u64) -> String {
    if seconds >= 86400 {
        format!(
            "{}d {}h {}m",
            seconds / 86400,
            seconds / 3600 % 24,
            seconds / 60 % 60
        )
    } else {
        format!(
            "{}h {}m {}s",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, cpu: f32, ports: Vec<u16>) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: 0,
            name: name.into(),
            cpu_usage: cpu,
            gpu_usage: None,
            memory_usage: 0,
            status: "Running".into(),
            user: "501".into(),
            command: format!("/bin/{name} --server"),
            threads: None,
            environ: vec![],
            root: "/".into(),
            virtual_memory: 0,
            start_time: 10,
            run_time: 60,
            disk_usage: (0, 0),
            session_id: None,
            ports,
        }
    }

    #[test]
    fn ports_follow_svelte_parse_rules() {
        for term in [":1", ":65535", "port:443", " PORT:03000 "] {
            assert!(parse_port(term).is_some(), "{term}");
        }
        for term in [
            "3000",
            ":0",
            "port:65536",
            ":000001",
            ":+80",
            ":１２",
            "port: 80",
            "port:",
            "",
        ] {
            assert_eq!(parse_port(term), None, "{term}");
        }
    }

    #[test]
    fn search_matches_any_term_and_only_valid_port_terms_need_collection() {
        let process = process(12345, "Worker", 5., vec![3000]);
        for query in [
            "work",
            "--server",
            "234",
            "absent,worker",
            ":3000",
            "port:3000",
            "^work.*",
        ] {
            assert!(Search::new(query).matches(&process), "{query}");
        }
        assert!(!Search::new(":8080").matches(&process));
        assert!(!Search::new("[").matches(&process));
        assert!(Search::new("name,PORT:3000").needs_ports());
        assert!(!Search::new("3000,port:65536").needs_ports());
    }

    #[test]
    fn sorting_reverses_and_keeps_ties_stable_by_pid() {
        let processes = vec![
            process(20, "b", 2., vec![]),
            process(10, "a", 20., vec![]),
            process(30, "c", 2., vec![]),
        ];
        assert_eq!(
            process_order(&processes, &Search::default(), ProcessColumn::Cpu, true),
            vec![1, 0, 2]
        );
        assert_eq!(
            process_order(&processes, &Search::default(), ProcessColumn::Cpu, false),
            vec![0, 2, 1]
        );
        assert_eq!(
            process_order(&processes, &Search::new("c"), ProcessColumn::Cpu, true),
            vec![2]
        );
    }
}
