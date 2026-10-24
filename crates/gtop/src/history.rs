use gtop_core::Snapshot;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};

const MINUTES: u64 = 1440;
const MAX_NAMES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Metric {
    #[default]
    Cpu,
    Memory,
    Gpu,
    Temperature,
}
impl Metric {
    pub const ALL: [Self; 4] = [Self::Cpu, Self::Memory, Self::Gpu, Self::Temperature];
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Gpu => "GPU",
            Self::Temperature => "CPU temperature",
        }
    }
    pub fn next(self) -> Self {
        Self::ALL[(Self::ALL.iter().position(|v| *v == self).unwrap() + 1) % 4]
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProcessTotal {
    pub cpu_seconds: f64,
    pub gpu_seconds: f64,
    pub peak_memory: u64,
    pub peak_cpu: f32,
}
impl ProcessTotal {
    fn merge(&mut self, other: &Self) {
        self.cpu_seconds += other.cpu_seconds;
        self.gpu_seconds += other.gpu_seconds;
        self.peak_memory = self.peak_memory.max(other.peak_memory);
        self.peak_cpu = self.peak_cpu.max(other.peak_cpu);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bucket {
    pub minute: u64,
    pub seconds: f64,
    pub cpu: f64,
    pub peak_cpu: f64,
    pub memory: f64,
    pub peak_memory: f64,
    pub gpu: Option<f64>,
    pub peak_gpu: Option<f64>,
    pub gpu_seconds: Option<f64>,
    pub temperature: Option<f64>,
    pub peak_temperature: Option<f64>,
    pub temperature_seconds: Option<f64>,
    pub processes: BTreeMap<String, ProcessTotal>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MomentSelection {
    pub start_minute: u64,
    pub step: usize,
}

#[derive(Clone, Debug, Default)]
pub struct MomentDetail {
    pub selection: MomentSelection,
    pub recorded_minutes: usize,
    pub cpu_average: Option<f64>,
    pub cpu_peak: Option<f64>,
    pub memory_average: Option<f64>,
    pub memory_peak: Option<f64>,
    pub gpu_average: Option<f64>,
    pub gpu_peak: Option<f64>,
    pub temperature_average: Option<f64>,
    pub temperature_peak: Option<f64>,
    pub rows: Vec<(String, ProcessTotal)>,
}

impl Default for MomentSelection {
    fn default() -> Self {
        Self {
            start_minute: 0,
            step: 1,
        }
    }
}

impl MomentDetail {
    pub fn ranked(&self, metric: Metric) -> Vec<(String, ProcessTotal, f64)> {
        let mut rows: Vec<_> = self
            .rows
            .iter()
            .map(|(name, total)| {
                let value = match metric {
                    Metric::Cpu | Metric::Temperature => total.cpu_seconds,
                    Metric::Memory => total.peak_memory as f64,
                    Metric::Gpu => total.gpu_seconds,
                };
                (name.clone(), total.clone(), value)
            })
            .collect();
        rows.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
        rows.truncate(8);
        rows
    }
}

pub fn step_moment(
    selection: MomentSelection,
    direction: i8,
    start: u64,
    end: u64,
) -> MomentSelection {
    let step = selection.step.max(1) as u64;
    let offset = selection.start_minute.saturating_sub(start);
    let index = if direction < 0 {
        offset.div_ceil(step).saturating_sub(1)
    } else {
        offset / step + 1
    };
    let last = end.saturating_sub(start).saturating_sub(1) / step;
    MomentSelection {
        start_minute: start + index.min(last) * step,
        step: step as usize,
    }
}

pub fn selection_in_window(selection: MomentSelection, start: u64, end: u64) -> bool {
    selection.start_minute >= start
        && selection.start_minute.saturating_add(selection.step as u64) <= end
}
#[derive(Clone, Debug)]
pub struct Column {
    pub minute: u64,
    pub seconds: f64,
    pub cpu: f64,
    pub memory: f64,
    pub gpu: Option<f64>,
    pub temperature: Option<f64>,
    pub peak_temperature: Option<f64>,
    pub gpu_seconds: f64,
    pub temperature_seconds: f64,
}
impl From<&Bucket> for Column {
    fn from(b: &Bucket) -> Self {
        Self {
            minute: b.minute,
            seconds: b.seconds,
            cpu: b.cpu,
            memory: b.memory,
            gpu: b.gpu,
            temperature: b.temperature,
            peak_temperature: b.peak_temperature,
            gpu_seconds: b.gpu_seconds.unwrap_or(b.seconds),
            temperature_seconds: b.temperature_seconds.unwrap_or(b.seconds),
        }
    }
}
pub fn weighted_average(values: impl IntoIterator<Item = (f64, f64)>) -> Option<f64> {
    let (sum, seconds) = values
        .into_iter()
        .fold((0., 0.), |(sum, seconds), (value, weight)| {
            (sum + value * weight, seconds + weight)
        });
    (seconds > 0.).then(|| sum / seconds)
}

impl Column {
    pub fn weight(&self, metric: Metric) -> f64 {
        match metric {
            Metric::Gpu => self.gpu_seconds,
            Metric::Temperature => self.temperature_seconds,
            _ => self.seconds,
        }
    }
    pub fn value(&self, metric: Metric) -> Option<f64> {
        match metric {
            Metric::Cpu => Some(self.cpu),
            Metric::Memory => Some(self.memory),
            Metric::Gpu => self.gpu,
            Metric::Temperature => self.temperature,
        }
    }
}

#[derive(Clone, Default)]
pub struct HistoryView {
    pub columns: Vec<Option<Column>>,
    pub start_minute: u64,
    pub rows: Vec<(String, ProcessTotal)>,
    pub total_cpu: f64,
    pub covered: usize,
    pub cached_leaders: [Vec<(String, f64)>; 4],
}
impl HistoryView {
    pub fn leaders(&self, metric: Metric) -> Vec<(String, f64)> {
        if metric == Metric::Temperature {
            let live: Vec<_> = self
                .columns
                .iter()
                .flatten()
                .filter(|b| b.temperature.is_some())
                .collect();
            if live.is_empty() {
                return vec![];
            }
            let peak = live
                .iter()
                .filter_map(|b| b.peak_temperature)
                .fold(0., f64::max);
            let sum: f64 = live
                .iter()
                .map(|b| b.temperature.unwrap() * b.temperature_seconds)
                .sum();
            let seconds: f64 = live.iter().map(|b| b.temperature_seconds).sum();
            return vec![
                ("Peak".into(), peak),
                ("Average".into(), sum / seconds.max(0.001)),
            ];
        }
        if metric == Metric::Gpu && self.columns.iter().flatten().all(|b| b.gpu.is_none()) {
            return vec![];
        }
        let mut rows: Vec<_> = self
            .rows
            .iter()
            .map(|(name, p)| {
                (
                    name.clone(),
                    match metric {
                        Metric::Cpu => p.cpu_seconds,
                        Metric::Gpu => p.gpu_seconds,
                        Metric::Memory => p.peak_memory as f64,
                        Metric::Temperature => unreachable!(),
                    },
                )
            })
            .collect();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        rows.truncate(3);
        rows
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct History {
    buckets: VecDeque<Bucket>,
    #[serde(skip)]
    last_sample: Option<f64>,
    #[serde(skip)]
    pub touched: std::ops::Range<u64>,
}
impl History {
    pub fn analyze_process(
        &self,
        name: &str,
        command: &str,
        current_cpu: f32,
        now_minute: u64,
    ) -> crate::analysis::ProcessAnalysis {
        crate::analysis::analyze(name, command, current_cpu, now_minute, self.buckets.iter())
    }

    pub fn load(path: &std::path::Path, now: u64) -> Self {
        let mut history: Self = std::fs::metadata(path)
            .ok()
            .filter(|m| m.len() < 512 * 1024 * 1024)
            .and_then(|_| std::fs::read(path).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        for bucket in &mut history.buckets {
            if bucket.peak_cpu == 0. {
                bucket.peak_cpu = bucket.cpu;
            }
            if bucket.peak_memory == 0. {
                bucket.peak_memory = bucket.memory;
            }
            if bucket.peak_gpu.is_none() {
                bucket.peak_gpu = bucket.gpu;
            }
        }
        history.prune(now / 60);
        history
    }
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        static NEXT_WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT_WRITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = path.with_extension(format!("{}.{}.tmp", std::process::id(), sequence));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| {
            let mut writer = std::io::BufWriter::new(file);
            serde_json::to_writer(&mut writer, self)?;
            std::io::Write::flush(&mut writer)?;
            writer.get_ref().sync_all()?;
            std::fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    fn prune(&mut self, minute: u64) {
        self.buckets
            .retain(|b| b.minute <= minute && b.minute > minute.saturating_sub(MINUTES));
        while self.buckets.len() > MINUTES as usize {
            self.buckets.pop_front();
        }
    }
    pub fn suspend(&mut self) {
        self.last_sample = None;
    }
    pub fn record(&mut self, snapshot: &Snapshot, now: f64, interval: f64) -> bool {
        let previous = self.last_sample.replace(now);
        let elapsed = previous.map(|v| now - v).unwrap_or(0.);
        let minute = now as u64 / 60;
        self.prune(minute);
        let changed_minute = self.buckets.back().is_none_or(|b| b.minute != minute);
        // Do not charge time while paused, asleep, or between application runs.
        let seconds = if elapsed > 0. && elapsed <= interval * 2.5 {
            elapsed
        } else {
            0.
        };
        self.touched = ((now - seconds) as u64 / 60)..minute + 1;
        if seconds == 0. {
            self.record_part(snapshot, minute, 0.);
        } else {
            let mut cursor = now - seconds;
            while cursor < now {
                let part_minute = cursor as u64 / 60;
                let end = now.min((part_minute + 1) as f64 * 60.);
                self.record_part(snapshot, part_minute, end - cursor);
                cursor = end;
            }
            if self.buckets.back().is_none_or(|b| b.minute != minute) {
                self.record_part(snapshot, minute, 0.);
            }
        }
        changed_minute
    }
    fn record_part(&mut self, snapshot: &Snapshot, minute: u64, seconds: f64) {
        if self.buckets.back().is_none_or(|b| b.minute != minute) {
            self.buckets.push_back(Bucket {
                minute,
                ..Default::default()
            });
        }
        let b = self.buckets.back_mut().unwrap();
        let old = b.seconds;
        let weight = if seconds > 0. {
            seconds
        } else if old == 0. {
            0.001
        } else {
            0.
        };
        b.seconds += weight;
        let mean =
            |old_value: f64, value: f64| (old_value * old + value * weight) / b.seconds.max(0.001);
        let cpu = snapshot
            .stats
            .cpu_usage
            .iter()
            .map(|v| *v as f64)
            .sum::<f64>()
            / snapshot.stats.cpu_usage.len().max(1) as f64;
        b.cpu = mean(b.cpu, cpu);
        b.peak_cpu = b.peak_cpu.max(cpu);
        b.memory = mean(b.memory, snapshot.stats.memory_used as f64);
        b.peak_memory = b.peak_memory.max(snapshot.stats.memory_used as f64);
        if let Some(v) = snapshot.stats.gpus.first().and_then(|g| g.utilization) {
            let observed = b
                .gpu_seconds
                .unwrap_or(if b.gpu.is_some() { old } else { 0. });
            let total = observed + weight;
            if total > 0. {
                b.gpu = Some((b.gpu.unwrap_or(0.) * observed + v as f64 * weight) / total);
            }
            b.gpu_seconds = Some(total);
            b.peak_gpu = Some(b.peak_gpu.unwrap_or(v as f64).max(v as f64));
        }
        if let Some(v) = snapshot
            .stats
            .temperatures
            .iter()
            .find(|t| t.name == "CPU")
            .map(|t| t.celsius as f64)
        {
            let observed =
                b.temperature_seconds
                    .unwrap_or(if b.temperature.is_some() { old } else { 0. });
            let total = observed + weight;
            if total > 0. {
                b.temperature = Some((b.temperature.unwrap_or(0.) * observed + v * weight) / total);
            }
            b.temperature_seconds = Some(total);
            b.peak_temperature = Some(b.peak_temperature.unwrap_or(v).max(v));
        }
        let mut sampled_processes = BTreeMap::<String, ProcessTotal>::new();
        let mut distinct_names = b.processes.len();
        for process in &snapshot.processes {
            if !b.processes.contains_key(&process.name)
                && !sampled_processes.contains_key(&process.name)
                && distinct_names >= MAX_NAMES
            {
                continue;
            }
            if !b.processes.contains_key(&process.name)
                && !sampled_processes.contains_key(&process.name)
            {
                distinct_names += 1;
            }
            let total = sampled_processes.entry(process.name.clone()).or_default();
            total.cpu_seconds += process.cpu_usage as f64 / 100. * seconds;
            total.gpu_seconds += process.gpu_usage.unwrap_or(0.) as f64 / 100. * seconds;
            total.peak_memory += process.memory_usage;
            total.peak_cpu += process.cpu_usage;
        }
        for (name, total) in sampled_processes {
            b.processes.entry(name).or_default().merge(&total);
        }
    }

    pub fn view(&self, hours: usize, now: u64) -> HistoryView {
        let minute = now / 60;
        let start = minute.saturating_sub((hours * 60 - 1) as u64);
        let mut result = HistoryView {
            start_minute: start,
            ..Default::default()
        };
        let mut totals: BTreeMap<String, ProcessTotal> = BTreeMap::new();
        let by_minute: BTreeMap<_, _> = self.buckets.iter().map(|b| (b.minute, b)).collect();
        for m in start..=minute {
            let bucket = by_minute.get(&m).copied();
            if let Some(b) = bucket {
                result.covered += 1;
                for (name, p) in &b.processes {
                    totals.entry(name.clone()).or_default().merge(p);
                }
            }
            result.columns.push(bucket.map(Column::from));
        }
        result.rows = totals.into_iter().collect();
        result.rows.sort_by(|a, b| {
            b.1.cpu_seconds
                .total_cmp(&a.1.cpu_seconds)
                .then(a.0.cmp(&b.0))
        });
        result.total_cpu = result.rows.iter().map(|r| r.1.cpu_seconds).sum();
        result.cached_leaders = Metric::ALL.map(|metric| result.leaders(metric));
        result
    }

    pub fn moment(&self, selection: MomentSelection) -> MomentDetail {
        let end = selection
            .start_minute
            .saturating_add(selection.step.max(1) as u64);
        let span: Vec<_> = self
            .buckets
            .iter()
            .filter(|b| b.minute >= selection.start_minute && b.minute < end)
            .collect();
        let mut detail = MomentDetail {
            selection,
            recorded_minutes: span.len(),
            ..Default::default()
        };
        let peak = |values: Vec<f64>| values.into_iter().reduce(f64::max);
        detail.cpu_average = weighted_average(span.iter().map(|b| (b.cpu, b.seconds)));
        detail.cpu_peak = peak(span.iter().map(|b| b.peak_cpu).collect());
        detail.memory_average = weighted_average(span.iter().map(|b| (b.memory, b.seconds)));
        detail.memory_peak = peak(span.iter().map(|b| b.peak_memory).collect());
        detail.gpu_average = weighted_average(
            span.iter()
                .filter_map(|b| b.gpu.map(|v| (v, b.gpu_seconds.unwrap_or(b.seconds)))),
        );
        detail.gpu_peak = peak(span.iter().filter_map(|b| b.peak_gpu).collect());
        detail.temperature_average = weighted_average(span.iter().filter_map(|b| {
            b.temperature
                .map(|v| (v, b.temperature_seconds.unwrap_or(b.seconds)))
        }));
        detail.temperature_peak = peak(span.iter().filter_map(|b| b.peak_temperature).collect());
        let mut rows: BTreeMap<String, ProcessTotal> = BTreeMap::new();
        for bucket in span {
            for (name, total) in &bucket.processes {
                rows.entry(name.clone()).or_default().merge(total);
            }
        }
        detail.rows = rows.into_iter().collect();
        detail
    }
}

pub fn support_path(file: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/gtop")
        .join(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn name_limit_counts_the_distinct_union() {
        let mut history = History::default();
        let mut snapshot = crate::test_data::snapshot();
        snapshot.processes = (1..=MAX_NAMES as u32)
            .map(|pid| crate::test_data::process(pid, &format!("process-{pid}"), 1.))
            .collect();
        let rest = snapshot.processes.split_off(2048);
        history.record(&snapshot, 6000., 3.);
        snapshot.processes.extend(rest);
        snapshot
            .processes
            .push(crate::test_data::process(9999, "extra", 1.));
        history.record(&snapshot, 6003., 3.);
        assert_eq!(history.buckets[0].processes.len(), MAX_NAMES);
        assert_eq!(
            history.buckets[0].processes["process-4096"].cpu_seconds,
            0.03
        );
        assert!(!history.buckets[0].processes.contains_key("extra"));
    }

    #[test]
    fn failed_publish_removes_the_temporary_file() {
        let directory =
            std::env::temp_dir().join(format!("gtop-publish-fail-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("history.json")).unwrap();
        assert!(History::default()
            .save(&directory.join("history.json"))
            .is_err());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn partial_minutes_use_observed_time() {
        let mut history = History::default();
        history.buckets.push_back(Bucket {
            minute: 100,
            seconds: 60.,
            cpu: 100.,
            memory: 100.,
            gpu: Some(100.),
            temperature: Some(100.),
            ..Default::default()
        });
        history.buckets.push_back(Bucket {
            minute: 101,
            seconds: 1.,
            gpu: Some(0.),
            temperature: Some(0.),
            ..Default::default()
        });
        let detail = history.moment(MomentSelection {
            start_minute: 100,
            step: 5,
        });
        assert!((detail.cpu_average.unwrap() - 6000. / 61.).abs() < 0.0001);
        assert_eq!(detail.cpu_average, detail.memory_average);
        assert_eq!(detail.gpu_average, detail.cpu_average);
        assert_eq!(detail.temperature_average, detail.cpu_average);
    }

    #[test]
    fn missing_temperature_does_not_count_as_zero() {
        let mut snapshot = crate::test_data::snapshot();
        snapshot.stats.temperatures.clear();
        let mut history = History::default();
        history.record(&snapshot, 6000., 3.);
        history.record(&snapshot, 6003., 3.);
        snapshot.stats.temperatures = crate::test_data::snapshot().stats.temperatures;
        snapshot.stats.gpus.push(gtop_core::monitoring::GpuInfo {
            name: "test".into(),
            vendor: "test".into(),
            utilization: Some(80.),
            memory_total: None,
            memory_used: None,
            temperature: None,
            power_watts: None,
            core_clock_mhz: None,
        });
        history.record(&snapshot, 6006., 3.);
        assert_eq!(history.buckets[0].gpu, Some(80.));
        assert_eq!(history.buckets[0].gpu_seconds, Some(3.));
        assert_eq!(history.buckets[0].temperature, Some(80.));
        assert_eq!(history.buckets[0].temperature_seconds, Some(3.));
    }

    #[test]
    fn rollover_marks_the_closed_minute_changed() {
        let snapshot = crate::test_data::snapshot();
        let mut history = History::default();
        history.record(&snapshot, 6057., 3.);
        history.record(&snapshot, 6060., 3.);
        assert!(history.touched.contains(&100));
        assert!(history.touched.contains(&101));
        assert_eq!(
            history
                .moment(MomentSelection {
                    start_minute: 100,
                    step: 1
                })
                .rows
                .iter()
                .find(|row| row.0 == "rustc")
                .unwrap()
                .1
                .cpu_seconds,
            10.5
        );
    }

    #[test]
    fn stepping_tracks_columns_after_rollover() {
        let selection = MomentSelection {
            start_minute: 100,
            step: 10,
        };
        assert_eq!(step_moment(selection, 1, 91, 1531).start_minute, 101);
        assert_eq!(step_moment(selection, -1, 91, 1531).start_minute, 91);
        let first = MomentSelection {
            start_minute: 91,
            step: 10,
        };
        assert_eq!(step_moment(first, -1, 91, 1531), first);
    }

    #[test]
    fn concurrent_history_writers_publish_complete_files() {
        let directory =
            std::env::temp_dir().join(format!("gtop-history-race-{}", std::process::id()));
        let path = directory.join("nested/history.json");
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..30 {
                        History::default().save(&path).unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        serde_json::from_reader::<_, History>(std::fs::File::open(path).unwrap()).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore]
    fn history_cost() {
        let state = gtop_core::AppState::new();
        let mut history = History::default();
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let snapshot = state.snapshot(false, false).unwrap();
            assert!(snapshot
                .processes
                .iter()
                .any(|p| p.pid == std::process::id()));
            let start = std::time::Instant::now();
            history.record(&snapshot, crate::rack::now(), 1.);
            eprintln!(
                "history record_us={} procs={}",
                start.elapsed().as_micros(),
                snapshot.processes.len()
            );
        }
        assert!(!history.buckets.is_empty());
    }
    #[test]
    fn recording_counts_elapsed_seconds_and_leaves_suspended_minutes_empty() {
        let s = crate::test_data::snapshot();
        let mut h = History::default();
        h.record(&s, 6000., 3.);
        h.record(&s, 6003., 3.);
        let v = h.view(1, 6003);
        assert_eq!(v.covered, 1);
        assert_eq!(v.rows[0].0, "rustc");
        assert_eq!(v.rows[0].1.cpu_seconds, 10.5);
        assert_eq!(v.rows[0].1.peak_memory, 3072);
        assert_eq!(v.cached_leaders[Metric::Cpu as usize], v.leaders(Metric::Cpu));
        assert_eq!(v.leaders(Metric::Temperature)[0], ("Peak".into(), 80.));
        h.suspend();
        h.record(&s, 6006., 3.);
        assert_eq!(h.view(1, 6006).rows[0].1.cpu_seconds, 10.5);
        h.record(&s, 6180., 3.);
        let v = h.view(1, 6180);
        assert_eq!(v.covered, 2);
        assert!(v.columns[58].is_none());
        assert_eq!(v.rows[0].1.cpu_seconds, 10.5);
        let path =
            std::env::temp_dir().join(format!("gtop-history-test-{}.json", std::process::id()));
        h.save(&path).unwrap();
        let loaded = History::load(&path, 6180);
        std::fs::remove_file(path).unwrap();
        assert_eq!(loaded.view(1, 6180).covered, 2);
        assert!(loaded.last_sample.is_none());
    }

    #[test]
    fn process_peak_cpu_sums_same_name_processes_in_each_sample() {
        let snapshot = crate::test_data::snapshot();
        let mut history = History::default();
        history.record(&snapshot, 6000., 3.);
        assert_eq!(history.buckets[0].processes["rustc"].peak_cpu, 350.);
        assert_eq!(history.buckets[0].processes["rustc"].peak_memory, 3072);
        assert_eq!(history.buckets[0].processes["cargo"].peak_cpu, 110.);
    }
    #[test]
    fn elapsed_time_splits_at_minute_boundaries() {
        let s = crate::test_data::snapshot();
        let mut h = History::default();
        h.record(&s, 6059., 3.);
        h.record(&s, 6062., 3.);
        let v = h.view(1, 6062);
        assert_eq!(v.covered, 2);
        let totals: Vec<_> = h
            .buckets
            .iter()
            .map(|b| b.processes["rustc"].cpu_seconds)
            .collect();
        assert_eq!(totals, [3.5, 7.]);
    }
    #[test]
    fn gaps_and_pruning_survive_storage() {
        let mut h = History::default();
        for minute in [1, 2, 100, 1500] {
            h.buckets.push_back(Bucket {
                minute,
                ..Default::default()
            });
        }
        h.prune(1500);
        assert_eq!(
            h.buckets.iter().map(|b| b.minute).collect::<Vec<_>>(),
            [100, 1500]
        );
        let view = h.view(24, 1500 * 60);
        assert_eq!(view.columns.len(), 1440);
        assert_eq!(view.covered, 2);
        assert!(view.columns[1438].is_none());
        assert_eq!(view.columns[1439].as_ref().unwrap().minute, 1500);
    }
    #[test]
    fn metric_leaders_use_their_own_units() {
        let view = HistoryView {
            columns: vec![Some(Column {
                minute: 0,
                seconds: 1.,
                cpu: 0.,
                memory: 0.,
                gpu: Some(1.),
                temperature: None,
                peak_temperature: None,
                gpu_seconds: 1.,
                temperature_seconds: 1.,
            })],
            rows: (0..4)
                .map(|i| {
                    (
                        i.to_string(),
                        ProcessTotal {
                            cpu_seconds: i as f64,
                            gpu_seconds: (4 - i) as f64,
                            peak_memory: (i * 10) as u64,
                            peak_cpu: 0.,
                        },
                    )
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            view.leaders(Metric::Cpu)
                .iter()
                .map(|r| r.0.as_str())
                .collect::<Vec<_>>(),
            ["3", "2", "1"]
        );
        assert_eq!(view.leaders(Metric::Gpu)[0].0, "0");
        assert_eq!(view.leaders(Metric::Memory)[0].1, 30.);
        assert!(view.leaders(Metric::Temperature).is_empty());
    }

    #[test]
    fn moment_aggregates_values_and_ranks_by_each_metric() {
        let mut h = History::default();
        for (
            minute,
            cpu,
            peak_cpu,
            memory,
            peak_memory,
            gpu,
            peak_gpu,
            temperature,
            peak_temperature,
            first_mem,
        ) in [
            (
                10,
                40.,
                47.,
                100.,
                150.,
                Some(12.),
                Some(15.),
                Some(50.),
                Some(55.),
                200,
            ),
            (
                11,
                60.,
                71.,
                300.,
                500.,
                Some(8.),
                Some(10.),
                Some(70.),
                Some(82.),
                400,
            ),
        ] {
            let mut processes = BTreeMap::new();
            processes.insert(
                "cpu-heavy".into(),
                ProcessTotal {
                    cpu_seconds: 9.,
                    gpu_seconds: 1.,
                    peak_memory: first_mem,
                    peak_cpu: 75.,
                },
            );
            processes.insert(
                "gpu-heavy".into(),
                ProcessTotal {
                    cpu_seconds: 3.,
                    gpu_seconds: 10.,
                    peak_memory: 50,
                    peak_cpu: 30.,
                },
            );
            processes.insert(
                "memory-heavy".into(),
                ProcessTotal {
                    cpu_seconds: 1.,
                    gpu_seconds: 2.,
                    peak_memory: 900,
                    peak_cpu: 5.,
                },
            );
            h.buckets.push_back(Bucket {
                minute,
                seconds: 60.,
                cpu,
                peak_cpu,
                memory,
                peak_memory,
                gpu,
                peak_gpu,
                temperature,
                peak_temperature,
                processes,
                ..Default::default()
            });
        }
        let detail = h.moment(MomentSelection {
            start_minute: 10,
            step: 2,
        });
        assert_eq!(detail.recorded_minutes, 2);
        assert_eq!(detail.cpu_average, Some(50.));
        assert_eq!(detail.cpu_peak, Some(71.));
        assert_eq!(detail.memory_average, Some(200.));
        assert_eq!(detail.memory_peak, Some(500.));
        assert_eq!(detail.gpu_average, Some(10.));
        assert_eq!(detail.gpu_peak, Some(15.));
        assert_eq!(detail.temperature_average, Some(60.));
        assert_eq!(detail.temperature_peak, Some(82.));
        assert_eq!(detail.ranked(Metric::Cpu)[0].0, "cpu-heavy");
        assert_eq!(detail.ranked(Metric::Temperature)[0].0, "cpu-heavy");
        assert_eq!(detail.ranked(Metric::Gpu)[0].0, "gpu-heavy");
        assert_eq!(detail.ranked(Metric::Memory)[0].0, "memory-heavy");
    }

    #[test]
    fn empty_moment_is_a_gap() {
        let detail = History::default().moment(MomentSelection {
            start_minute: 42,
            step: 10,
        });
        assert_eq!(detail.recorded_minutes, 0);
        assert!(detail.cpu_average.is_none());
        assert!(detail.rows.is_empty());
    }

    #[test]
    fn moments_keep_peak_readouts_for_existing_history_files() {
        let path =
            std::env::temp_dir().join(format!("gtop-old-history-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"buckets":[{"minute":100,"seconds":30,"cpu":42,"memory":2048,"gpu":12,"temperature":70,"peak_temperature":74,"processes":{}}]}"#).unwrap();
        let history = History::load(&path, 100 * 60);
        std::fs::remove_file(path).unwrap();
        let detail = history.moment(MomentSelection {
            start_minute: 100,
            step: 1,
        });
        assert_eq!(detail.cpu_peak, Some(42.));
        assert_eq!(detail.memory_peak, Some(2048.));
        assert_eq!(detail.gpu_peak, Some(12.));
    }

    #[test]
    fn moment_steps_clamp_to_tape_and_selection_keeps_its_minutes() {
        let selection = MomentSelection {
            start_minute: 100,
            step: 10,
        };
        assert_eq!(step_moment(selection, -1, 100, 150), selection);
        assert_eq!(step_moment(selection, 1, 100, 150).start_minute, 110);
        assert_eq!(
            step_moment(
                MomentSelection {
                    start_minute: 140,
                    step: 10
                },
                1,
                100,
                150
            )
            .start_minute,
            140
        );
        assert!(selection_in_window(selection, 100, 150));
        assert!(!selection_in_window(selection, 101, 150));

        let selection = MomentSelection {
            start_minute: 600,
            step: 10,
        };
        let mut history = History::default();
        history.buckets.push_back(Bucket {
            minute: 600,
            ..Default::default()
        });
        let before = history.view(24, 2000 * 60);
        history.buckets.push_back(Bucket {
            minute: 2001,
            ..Default::default()
        });
        let after = history.view(24, 2001 * 60);
        assert_ne!(before.start_minute, after.start_minute);
        assert_eq!(selection.start_minute, 600);
        assert!(selection_in_window(
            selection,
            after.start_minute,
            after.start_minute + after.columns.len() as u64
        ));
    }
}
