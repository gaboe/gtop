use gtop_core::ProcessInfo;
use regex::{Regex, RegexBuilder};
use std::{cmp::Ordering, sync::OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub pid: u32,
    pub start_time: u64,
}

static AVAILABLE_LOGICAL_CPUS: OnceLock<usize> = OnceLock::new();

pub fn logical_cpu_count(snapshot: Option<&gtop_core::Snapshot>) -> usize {
    snapshot
        .map(|snapshot| snapshot.stats.cpu_usage.len())
        .unwrap_or_else(|| {
            *AVAILABLE_LOGICAL_CPUS.get_or_init(|| {
                std::thread::available_parallelism()
                    .map(|count| count.get())
                    .unwrap_or(1)
            })
        })
}

pub fn format_cpu_share(
    per_core: f64,
    logical_cpus: usize,
    scale: crate::palette::CpuScale,
) -> String {
    let value = match scale {
        crate::palette::CpuScale::Mac => per_core / logical_cpus.max(1) as f64,
        crate::palette::CpuScale::Core => per_core,
    };
    let rounded = (value * 100.).round() / 100.;
    if rounded < 10. {
        format!("{rounded:.2}")
    } else {
        format!("{value:.1}")
    }
}

pub fn running_history_process<'a>(
    name: &str,
    processes: &'a [ProcessInfo],
) -> Option<&'a ProcessInfo> {
    processes
        .iter()
        .filter(|process| process.name == name)
        .max_by(|a, b| {
            a.cpu_usage
                .total_cmp(&b.cpu_usage)
                .then_with(|| b.pid.cmp(&a.pid))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryRowAction {
    SelectOnly,
    OpenDetail(ProcessKey),
}

pub fn history_row_action(name: &str, processes: &[ProcessInfo]) -> HistoryRowAction {
    running_history_process(name, processes)
        .map(ProcessKey::from)
        .map(HistoryRowAction::OpenDetail)
        .unwrap_or(HistoryRowAction::SelectOnly)
}

impl From<&ProcessInfo> for ProcessKey {
    fn from(process: &ProcessInfo) -> Self {
        Self {
            pid: process.pid,
            start_time: process.start_time,
        }
    }
}

#[derive(Clone, Copy)]
pub enum ProcessAction {
    Detail,
    Kill,
}

pub fn action_target(
    action: ProcessAction,
    details: Option<ProcessKey>,
    selected: Option<ProcessKey>,
    first: Option<ProcessKey>,
) -> Option<ProcessKey> {
    match action {
        ProcessAction::Detail => selected.or(first),
        ProcessAction::Kill => details.or(selected).or(first),
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
                        || process
                            .ports
                            .iter()
                            .any(|port| port.to_string().contains(text))
                        || pattern
                            .as_ref()
                            .is_some_and(|pattern| pattern.is_match(&process.name))
                }
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessColumn {
    Lamp,
    Name,
    Pid,
    User,
    Cpu,
    Memory,
    Age,
    Gpu,
    Ports,
    Command,
}
impl ProcessColumn {
    pub const ALL: [Self; 10] = [
        Self::Lamp,
        Self::Name,
        Self::Pid,
        Self::User,
        Self::Cpu,
        Self::Memory,
        Self::Age,
        Self::Gpu,
        Self::Ports,
        Self::Command,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Lamp => "",
            Self::Name => "Process",
            Self::Pid => "PID",
            Self::User => "User",
            Self::Cpu => "CPU %",
            Self::Memory => "Memory",
            Self::Age => "Age",
            Self::Gpu => "GPU %",
            Self::Ports => "Ports",
            Self::Command => "Command",
        }
    }
    pub fn compare(self, a: &ProcessInfo, b: &ProcessInfo) -> Ordering {
        match self {
            Self::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            Self::Pid => a.pid.cmp(&b.pid),
            Self::User => a.user.cmp(&b.user),
            Self::Cpu | Self::Lamp => a.cpu_usage.total_cmp(&b.cpu_usage),
            Self::Memory => a.memory_usage.cmp(&b.memory_usage),
            Self::Age => b.start_time.cmp(&a.start_time),
            Self::Gpu => match (a.gpu_usage, b.gpu_usage) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(a), Some(b)) if a.is_nan() && !b.is_nan() => Ordering::Less,
                (Some(a), Some(b)) if !a.is_nan() && b.is_nan() => Ordering::Greater,
                (Some(a), Some(b)) => a.total_cmp(&b),
            },
            Self::Ports => a.ports.cmp(&b.ports),
            Self::Command => a.command.cmp(&b.command),
        }
    }
    pub fn text(
        self,
        p: &ProcessInfo,
        logical_cpus: usize,
        scale: crate::palette::CpuScale,
    ) -> String {
        match self {
            Self::Lamp => String::new(),
            Self::Name => p.name.clone(),
            Self::Pid => p.pid.to_string(),
            Self::User => p.user.clone(),
            Self::Cpu => format_cpu_share(p.cpu_usage as f64, logical_cpus, scale),
            Self::Memory => bytes(p.memory_usage),
            Self::Age => {
                if p.start_time == 0 {
                    "·".into()
                } else {
                    compact_duration(p.run_time)
                }
            }
            Self::Gpu => p
                .gpu_usage
                .filter(|v| *v >= 0.05)
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "·".into()),
            Self::Ports => p
                .ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            Self::Command => p.command.clone(),
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
        let a = &processes[*a];
        let b = &processes[*b];
        let comparison = match (column, a.start_time == 0, b.start_time == 0) {
            (ProcessColumn::Age, true, false) => Ordering::Greater,
            (ProcessColumn::Age, false, true) => Ordering::Less,
            _ => {
                let comparison = column.compare(a, b);
                if descending {
                    comparison.reverse()
                } else {
                    comparison
                }
            }
        };
        comparison.then_with(|| a.pid.cmp(&b.pid))
    });
    order
}

pub fn bytes(value: u64) -> String {
    let mut amount = value as f64;
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut unit = 0;
    while amount >= 1000. && unit < units.len() - 1 {
        amount /= 1000.;
        unit += 1;
    }
    format!("{amount:.1} {}", units[unit])
}

#[cfg(test)]
mod history_process_tests {
    use super::*;

    #[test]
    fn history_process_match_prefers_the_highest_cpu_instance_and_marks_missing_names() {
        let processes = crate::test_data::snapshot().processes;
        let name = processes[0].name.clone();
        let best = processes
            .iter()
            .filter(|p| p.name == name)
            .max_by(|a, b| a.cpu_usage.total_cmp(&b.cpu_usage))
            .unwrap();
        assert_eq!(
            running_history_process(&name, &processes).map(ProcessKey::from),
            Some(ProcessKey::from(best))
        );
        assert!(running_history_process("process-that-exited", &processes).is_none());
    }

    #[test]
    fn history_row_action_opens_running_processes_and_only_selects_exited_processes() {
        let processes = crate::test_data::snapshot().processes;
        let running_name = processes[0].name.clone();
        let running_key = running_history_process(&running_name, &processes)
            .map(ProcessKey::from)
            .unwrap();
        assert_eq!(
            history_row_action(&running_name, &processes),
            HistoryRowAction::OpenDetail(running_key)
        );
        assert_eq!(
            history_row_action("process-that-exited", &processes),
            HistoryRowAction::SelectOnly
        );
    }
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

fn compact_duration(seconds: u64) -> String {
    if seconds >= 86400 {
        format!("{}d {:02}h", seconds / 86400, seconds / 3600 % 24)
    } else if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
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
            cpu_sample_seconds: Some(3.),
            gpu_usage: None,
            memory_usage: 0,
            status: "Running".into(),
            user: "501".into(),
            executable: format!("/bin/{name}"),
            argv: vec![format!("/bin/{name}")],
            command: format!("/bin/{name} --server"),
            threads: None,
            environ: vec![],
            root: "/".into(),
            virtual_memory: 0,
            start_time: 10,
            run_time: 60,
            disk_usage: (0, 0),
            disk_sample_seconds: None,
            session_id: None,
            ports,
        }
    }

    #[test]
    fn byte_labels_use_decimal_units() {
        assert_eq!(bytes(1_000_000), "1.0 MB");
        assert_eq!(bytes(1_000_000_000), "1.0 GB");
        assert_eq!(bytes(16_000_000_000), "16.0 GB");
    }

    #[test]
    fn age_labels_use_compact_duration_units() {
        for (seconds, expected) in [
            (0, "0s"),
            (45, "45s"),
            (59, "59s"),
            (60, "1m"),
            (720, "12m"),
            (3599, "59m"),
            (3600, "1h 00m"),
            (11040, "3h 04m"),
            (86399, "23h 59m"),
            (86400, "1d 00h"),
            (439200, "5d 02h"),
        ] {
            let mut process = process(1, "age", 0., vec![]);
            process.run_time = seconds;
            assert_eq!(
                ProcessColumn::Age.text(&process, 1, crate::palette::CpuScale::Mac),
                expected
            );
        }
    }

    #[test]
    fn cpu_column_and_formatter_follow_the_selected_scale() {
        let process = process(1, "builder", 350., vec![]);
        assert_eq!(
            ProcessColumn::Cpu.text(&process, 14, crate::palette::CpuScale::Mac),
            "25.0"
        );
        assert_eq!(
            ProcessColumn::Cpu.text(&process, 14, crate::palette::CpuScale::Core),
            "350.0"
        );
        assert_eq!(
            format_cpu_share(2.5, 14, crate::palette::CpuScale::Mac),
            "0.18"
        );
        assert_eq!(
            format_cpu_share(2.5, 14, crate::palette::CpuScale::Core),
            "2.50"
        );
        assert_eq!(
            format_cpu_share(350., 0, crate::palette::CpuScale::Mac),
            "350.0"
        );
        assert_eq!(
            format_cpu_share(9.996, 1, crate::palette::CpuScale::Mac),
            "10.0"
        );
    }

    #[test]
    fn history_peak_without_snapshot_uses_available_cpu_count() {
        let expected = format_cpu_share(
            350.,
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1),
            crate::palette::CpuScale::Mac,
        );
        assert_eq!(
            logical_cpu_count(None),
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1)
        );
        assert_eq!(
            format_cpu_share(350., logical_cpu_count(None), crate::palette::CpuScale::Mac),
            expected
        );
    }

    #[test]
    fn gpu_sort_orders_nan_zero_and_positive_values_in_both_directions() {
        let mut processes = vec![
            process(4, "nan-later", 0., vec![]),
            process(3, "positive", 0., vec![]),
            process(2, "zero", 0., vec![]),
            process(1, "nan-earlier", 0., vec![]),
        ];
        processes[0].gpu_usage = Some(f32::NAN);
        processes[1].gpu_usage = Some(12.5);
        processes[2].gpu_usage = Some(0.);
        processes[3].gpu_usage = Some(f32::NAN);

        let ascending = process_order(&processes, &Search::default(), ProcessColumn::Gpu, false);
        assert_eq!(
            ascending
                .iter()
                .map(|&index| processes[index].pid)
                .collect::<Vec<_>>(),
            [1, 4, 2, 3]
        );

        let descending = process_order(&processes, &Search::default(), ProcessColumn::Gpu, true);
        assert_eq!(
            descending
                .iter()
                .map(|&index| processes[index].pid)
                .collect::<Vec<_>>(),
            [3, 2, 1, 4]
        );
    }

    #[test]
    fn detail_uses_the_filtered_row_while_kill_uses_the_shown_detail() {
        let old = ProcessKey {
            pid: 1,
            start_time: 10,
        };
        let found = ProcessKey {
            pid: 2,
            start_time: 20,
        };
        assert_eq!(
            action_target(ProcessAction::Detail, Some(old), None, Some(found)),
            Some(found)
        );
        assert_eq!(
            action_target(ProcessAction::Kill, Some(old), None, Some(found)),
            Some(old)
        );
        assert_eq!(
            action_target(ProcessAction::Detail, Some(old), None, None),
            None
        );
    }
    #[test]
    fn port_terms_follow_parse_rules() {
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

    #[test]
    fn age_sort_uses_start_time_and_keeps_pid_ties_in_both_directions() {
        let mut old = process(30, "old", 0., vec![]);
        old.start_time = 5;
        old.run_time = 1;
        let mut same_age = process(20, "same-age", 0., vec![]);
        same_age.start_time = 5;
        same_age.run_time = 10_000;
        let mut recent = process(10, "recent", 0., vec![]);
        recent.start_time = 100;
        recent.run_time = 1_000_000;
        let processes = vec![recent, old, same_age];

        assert_eq!(
            process_order(&processes, &Search::default(), ProcessColumn::Age, true),
            vec![2, 1, 0]
        );
        assert_eq!(
            process_order(&processes, &Search::default(), ProcessColumn::Age, false),
            vec![0, 2, 1]
        );
    }

    #[test]
    fn unknown_age_is_unavailable() {
        let mut unknown = process(20, "unknown", 0., vec![]);
        unknown.start_time = 0;
        unknown.run_time = 0;

        assert_eq!(
            ProcessColumn::Age.text(&unknown, 1, crate::palette::CpuScale::Mac),
            "·"
        );
    }

    #[test]
    fn unknown_age_sorts_after_known_processes_in_both_directions() {
        let mut old = process(30, "old", 0., vec![]);
        old.start_time = 5;
        old.run_time = 999_999;
        let mut unknown_later_pid = process(40, "unknown-later", 0., vec![]);
        unknown_later_pid.start_time = 0;
        unknown_later_pid.run_time = 0;
        let mut recent = process(10, "recent", 0., vec![]);
        recent.start_time = 100;
        let mut unknown_earlier_pid = process(20, "unknown-earlier", 0., vec![]);
        unknown_earlier_pid.start_time = 0;
        unknown_earlier_pid.run_time = 0;
        let processes = vec![unknown_later_pid, recent, unknown_earlier_pid, old];

        assert_eq!(
            process_order(&processes, &Search::default(), ProcessColumn::Age, true),
            vec![3, 1, 2, 0]
        );
        assert_eq!(
            process_order(&processes, &Search::default(), ProcessColumn::Age, false),
            vec![1, 3, 2, 0]
        );
    }
}
