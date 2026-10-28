use gtop_core::Snapshot;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};

#[path = "history/store.rs"]
mod store;
pub use store::Result;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistorySortColumn {
    Name,
    CoreTime,
    Share,
    CpuShare,
    GpuTime,
    PeakMemory,
    PeakCpu,
}

impl HistorySortColumn {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "PROCESS",
            Self::CoreTime => "CORE TIME",
            Self::Share => "PEAK CPU GRAPH",
            Self::CpuShare => "OF ALL CPU",
            Self::GpuTime => "GPU TIME",
            Self::PeakMemory => "PEAK MEMORY",
            Self::PeakCpu => "PEAK CPU %",
        }
    }

    fn is_text(self) -> bool {
        self == Self::Name
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    fn reverse(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistorySort {
    pub column: HistorySortColumn,
    pub direction: SortDirection,
}

impl Default for HistorySort {
    fn default() -> Self {
        Self {
            column: HistorySortColumn::CoreTime,
            direction: SortDirection::Descending,
        }
    }
}

impl HistorySort {
    pub fn click(&mut self, column: HistorySortColumn) {
        self.direction = if self.column == column {
            self.direction.reverse()
        } else if column.is_text() {
            SortDirection::Ascending
        } else {
            SortDirection::Descending
        };
        self.column = column;
    }

    pub fn order(self, rows: &[(String, ProcessTotal)]) -> Vec<usize> {
        let mut order: Vec<_> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (name_a, total_a) = &rows[a];
            let (name_b, total_b) = &rows[b];
            let value_order = match self.column {
                HistorySortColumn::Name => name_a.cmp(name_b),
                HistorySortColumn::CoreTime | HistorySortColumn::CpuShare => {
                    total_a.cpu_seconds.total_cmp(&total_b.cpu_seconds)
                }
                HistorySortColumn::Share | HistorySortColumn::PeakCpu => {
                    total_a.peak_cpu.total_cmp(&total_b.peak_cpu)
                }
                HistorySortColumn::GpuTime => total_a.gpu_seconds.total_cmp(&total_b.gpu_seconds),
                HistorySortColumn::PeakMemory => total_a.peak_memory.cmp(&total_b.peak_memory),
            };
            let value_order = if self.direction == SortDirection::Descending {
                value_order.reverse()
            } else {
                value_order
            };
            value_order.then_with(|| name_a.cmp(name_b))
        });
        order
    }

    pub fn selected_index_in_order(
        self,
        rows: &[(String, ProcessTotal)],
        order: &[usize],
        selected: Option<&str>,
    ) -> usize {
        selected
            .and_then(|name| order.iter().position(|&index| rows[index].0 == name))
            .unwrap_or(0)
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

pub fn newest_moment(view: &HistoryView) -> Option<MomentSelection> {
    let step = if view.columns.len() > 480 {
        10
    } else if view.columns.len() > 60 {
        5
    } else {
        1
    };
    (!view.columns.is_empty()).then(|| MomentSelection {
        start_minute: view.start_minute + ((view.columns.len() - 1) / step * step) as u64,
        step,
    })
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
    store: Option<store::Store>,
    #[serde(skip)]
    dirty: std::collections::BTreeSet<u64>,
    #[serde(skip)]
    last_sample: Option<f64>,
    #[serde(skip)]
    last_process_sample: Option<f64>,
    #[serde(skip)]
    process_recording_since: Option<f64>,
    #[serde(skip)]
    pub touched: std::ops::Range<u64>,
}
impl History {
    pub fn open(path: &std::path::Path, legacy: &std::path::Path, now: u64) -> store::Result<Self> {
        let store = store::Store::open(path, legacy, now / 60)?;
        let buckets = store.buckets(now / 60..now / 60 + 1, None)?;
        Ok(Self {
            buckets,
            store: Some(store),
            ..Default::default()
        })
    }

    #[cfg(feature = "history-benchmark")]
    pub fn benchmark_minute(&mut self, bucket: Bucket) {
        self.dirty.insert(bucket.minute);
        self.buckets.retain(|b| b.minute != bucket.minute);
        self.buckets.push_back(bucket);
    }

    pub fn persist(&mut self, now: u64) -> store::Result<()> {
        let store = self
            .store
            .as_mut()
            .ok_or("History database is unavailable")?;
        store.write_minutes(
            self.buckets
                .iter()
                .filter(|b| self.dirty.contains(&b.minute)),
        )?;
        self.dirty.clear();
        store.append(std::iter::empty(), now / 60)?;
        self.buckets.retain(|b| b.minute >= now / 60);
        Ok(())
    }

    fn read_buckets(
        &self,
        range: std::ops::Range<u64>,
        name: Option<&str>,
    ) -> Result<VecDeque<Bucket>> {
        let mut buckets: BTreeMap<u64, Bucket> = self
            .store
            .as_ref()
            .map(|s| s.buckets(range.clone(), name))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|b| (b.minute, b))
            .collect();
        for bucket in self.buckets.iter().filter(|b| range.contains(&b.minute)) {
            buckets.insert(bucket.minute, bucket.clone());
        }
        Ok(buckets.into_values().collect())
    }

    pub fn analyze_process(
        &self,
        name: &str,
        command: &str,
        now_minute: u64,
    ) -> Result<crate::analysis::ProcessAnalysis> {
        let buckets =
            self.read_buckets(now_minute.saturating_sub(1439)..now_minute + 1, Some(name))?;
        Ok(crate::analysis::analyze(
            name, command, now_minute, &buckets,
        ))
    }

    #[cfg(test)]
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
    #[cfg(test)]
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
        self.buckets.retain(|b| {
            (self.store.is_some() && self.dirty.contains(&b.minute))
                || (b.minute <= minute && b.minute > minute.saturating_sub(MINUTES))
        });
    }
    pub fn suspend(&mut self) {
        self.last_sample = None;
        self.last_process_sample = None;
        self.process_recording_since = None;
    }
    pub fn record(&mut self, snapshot: &Snapshot, now: f64, interval: f64) -> Result<bool> {
        if self.last_sample.is_none_or(|at| now < at) {
            let minute = now as u64 / 60;
            if let Some(store) = &mut self.store {
                store.discard_future(minute)?;
            }
            self.buckets.retain(|b| b.minute <= minute);
            self.dirty.retain(|m| *m <= minute);
        }
        self.prepare_sample(snapshot, now, interval)?;
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
        if seconds == 0. {
            self.last_process_sample = None;
            self.process_recording_since = None;
        }
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
        if matches!(snapshot.process_data, gtop_core::ProcessData::Fresh) {
            let process_seconds = self
                .last_process_sample
                .replace(now)
                .map_or(0., |at| now - at);
            let recording_since = *self.process_recording_since.get_or_insert(now);
            let oldest = minute.saturating_sub(MINUTES - 1) as f64 * 60.;
            let cpu_start = recording_since.max(oldest);
            let gpu_start = (now - process_seconds).max(oldest);
            let cpu_seconds = snapshot
                .processes
                .iter()
                .filter_map(|p| p.cpu_sample_seconds)
                .fold(0., f64::max);
            let mut cursor = (now - cpu_seconds).max(cpu_start).min(gpu_start);
            self.touched.start = self.touched.start.min(cursor as u64 / 60);
            while cursor < now {
                let part_minute = cursor as u64 / 60;
                let end = now.min((part_minute + 1) as f64 * 60.);
                self.record_processes(
                    snapshot,
                    part_minute,
                    cursor..end,
                    now,
                    cpu_start,
                    gpu_start,
                );
                cursor = end;
            }
            self.record_processes(snapshot, minute, now..now, now, cpu_start, gpu_start);
        }
        self.dirty.extend(self.touched.clone());
        Ok(changed_minute)
    }
    fn prepare_sample(
        &mut self,
        snapshot: &Snapshot,
        now: f64,
        interval: f64,
    ) -> store::Result<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let minute = now as u64 / 60;
        let elapsed = self.last_sample.map_or(0., |at| now - at);
        let mut start = if elapsed > 0. && elapsed <= interval * 2.5 {
            now - elapsed
        } else {
            now
        };
        if elapsed > 0.
            && elapsed <= interval * 2.5
            && matches!(snapshot.process_data, gtop_core::ProcessData::Fresh)
        {
            let span = snapshot
                .processes
                .iter()
                .filter_map(|p| p.cpu_sample_seconds)
                .fold(0., f64::max);
            let oldest = minute.saturating_sub(MINUTES - 1) as f64 * 60.;
            let cpu_start = self.process_recording_since.unwrap_or(now).max(oldest);
            let gpu_start = self.last_process_sample.unwrap_or(now).max(oldest);
            start = start.min((now - span).max(cpu_start).min(gpu_start));
        }
        let range = start as u64 / 60..minute + 1;
        if range
            .clone()
            .all(|minute| self.buckets.iter().any(|b| b.minute == minute))
        {
            return Ok(());
        }
        let buckets = store.buckets(range, None)?;
        for bucket in buckets {
            if !self.buckets.iter().any(|b| b.minute == bucket.minute) {
                let index = self.buckets.partition_point(|b| b.minute < bucket.minute);
                self.buckets.insert(index, bucket);
            }
        }
        Ok(())
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
    }

    fn record_processes(
        &mut self,
        snapshot: &Snapshot,
        minute: u64,
        part: std::ops::Range<f64>,
        now: f64,
        cpu_start: f64,
        gpu_start: f64,
    ) {
        let index = self
            .buckets
            .iter()
            .position(|b| b.minute >= minute)
            .unwrap_or(self.buckets.len());
        if self.buckets.get(index).is_none_or(|b| b.minute != minute) {
            self.buckets.insert(
                index,
                Bucket {
                    minute,
                    ..Default::default()
                },
            );
        }
        let b = &mut self.buckets[index];
        let mut sampled_processes = BTreeMap::<String, ProcessTotal>::new();
        let mut distinct_names = b.processes.len();
        for process in snapshot.processes.iter() {
            let process_start = process.start_time as f64;
            if part.end < process_start {
                continue;
            }
            let cpu_seconds = process.cpu_sample_seconds.map_or(0., |span| {
                (part.end - part.start.max(now - span).max(cpu_start).max(process_start)).max(0.)
            });
            let gpu_seconds = (part.end - part.start.max(gpu_start).max(process_start)).max(0.);
            if part.end > part.start && cpu_seconds == 0. && gpu_seconds == 0. {
                continue;
            }

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
            total.cpu_seconds += process.cpu_usage as f64 / 100. * cpu_seconds;
            total.gpu_seconds += process.gpu_usage.unwrap_or(0.) as f64 / 100. * gpu_seconds;
            total.peak_memory += process.memory_usage;
            total.peak_cpu += process.cpu_usage;
        }
        for (name, total) in sampled_processes {
            b.processes.entry(name).or_default().merge(&total);
        }
    }

    pub fn view(&self, hours: usize, now: u64) -> Result<HistoryView> {
        let minute = now / 60;
        let start = minute.saturating_sub((hours * 60 - 1) as u64);
        let mut result = HistoryView {
            start_minute: start,
            ..Default::default()
        };
        let mut totals: BTreeMap<String, ProcessTotal> = BTreeMap::new();
        let mut stored = VecDeque::new();
        if let Some(store) = &self.store {
            let (buckets, rows) = store.view_data(start..minute + 1, &self.buckets)?;
            stored = buckets;
            totals = rows;
        }
        let by_minute: BTreeMap<_, _> = stored
            .iter()
            .chain(self.buckets.iter())
            .map(|b| (b.minute, b))
            .collect();
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
        Ok(result)
    }

    pub fn moment(&self, selection: MomentSelection) -> Result<MomentDetail> {
        let end = selection
            .start_minute
            .saturating_add(selection.step.max(1) as u64);
        let buckets = self.read_buckets(selection.start_minute..end, None)?;
        let span: Vec<_> = buckets.iter().collect();
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
        Ok(detail)
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

    fn sort_rows() -> Vec<(String, ProcessTotal)> {
        vec![
            (
                "beta".into(),
                ProcessTotal {
                    cpu_seconds: 2.,
                    gpu_seconds: 3.,
                    peak_memory: 20,
                    peak_cpu: 30.,
                },
            ),
            (
                "alpha".into(),
                ProcessTotal {
                    cpu_seconds: 1.,
                    gpu_seconds: 1.,
                    peak_memory: 10,
                    peak_cpu: 10.,
                },
            ),
            (
                "gamma".into(),
                ProcessTotal {
                    cpu_seconds: 3.,
                    gpu_seconds: 2.,
                    peak_memory: 30,
                    peak_cpu: 20.,
                },
            ),
        ]
    }

    fn sorted_names(sort: HistorySort, rows: &[(String, ProcessTotal)]) -> Vec<&str> {
        sort.order(rows)
            .into_iter()
            .map(|index| rows[index].0.as_str())
            .collect()
    }

    #[test]
    fn history_sort_orders_each_column_in_its_initial_direction() {
        let rows = sort_rows();
        let expected = [
            (HistorySortColumn::Name, vec!["alpha", "beta", "gamma"]),
            (HistorySortColumn::CoreTime, vec!["gamma", "beta", "alpha"]),
            (HistorySortColumn::Share, vec!["beta", "gamma", "alpha"]),
            (HistorySortColumn::CpuShare, vec!["gamma", "beta", "alpha"]),
            (HistorySortColumn::GpuTime, vec!["beta", "gamma", "alpha"]),
            (
                HistorySortColumn::PeakMemory,
                vec!["gamma", "beta", "alpha"],
            ),
            (HistorySortColumn::PeakCpu, vec!["beta", "gamma", "alpha"]),
        ];
        for (column, names) in expected {
            let mut sort = HistorySort::default();
            if column != HistorySortColumn::CoreTime {
                sort.click(column);
            }
            assert_eq!(sorted_names(sort, &rows), names);
            sort.click(column);
            let reversed = match column {
                HistorySortColumn::Name => vec!["gamma", "beta", "alpha"],
                HistorySortColumn::CoreTime | HistorySortColumn::CpuShare => {
                    vec!["alpha", "beta", "gamma"]
                }
                HistorySortColumn::Share
                | HistorySortColumn::GpuTime
                | HistorySortColumn::PeakCpu => vec!["alpha", "gamma", "beta"],
                HistorySortColumn::PeakMemory => vec!["alpha", "beta", "gamma"],
            };
            assert_eq!(sorted_names(sort, &rows), reversed);
        }
    }

    #[test]
    fn history_sort_click_sets_initial_direction_and_toggles_active_column() {
        let mut sort = HistorySort::default();
        sort.click(HistorySortColumn::Name);
        assert_eq!(sort.direction, SortDirection::Ascending);
        sort.click(HistorySortColumn::Name);
        assert_eq!(sort.direction, SortDirection::Descending);
        sort.click(HistorySortColumn::PeakMemory);
        assert_eq!(sort.direction, SortDirection::Descending);
        sort.click(HistorySortColumn::PeakMemory);
        assert_eq!(sort.direction, SortDirection::Ascending);
    }

    #[test]
    fn history_sort_breaks_ties_by_name_and_keeps_selection_after_resort() {
        let rows = vec![
            (
                "zulu".into(),
                ProcessTotal {
                    cpu_seconds: 3.,
                    ..Default::default()
                },
            ),
            (
                "alpha".into(),
                ProcessTotal {
                    cpu_seconds: 1.,
                    ..Default::default()
                },
            ),
            (
                "mike".into(),
                ProcessTotal {
                    cpu_seconds: 2.,
                    ..Default::default()
                },
            ),
        ];
        let mut sort = HistorySort::default();
        assert_eq!(sorted_names(sort, &rows), ["zulu", "mike", "alpha"]);
        let first_order = sort.order(&rows);
        let selected_before = sort.selected_index_in_order(&rows, &first_order, Some("zulu"));
        sort.click(HistorySortColumn::Name);
        let sorted_order = sort.order(&rows);
        let selected_after = sort.selected_index_in_order(&rows, &sorted_order, Some("zulu"));
        assert_eq!(selected_before, 0);
        assert_eq!(selected_after, 2);
        assert_eq!(rows[sorted_order[selected_after]].0, "zulu");
    }

    #[test]
    fn history_sort_breaks_equal_numeric_values_by_name_in_both_directions() {
        let rows = vec![
            (
                "zulu".into(),
                ProcessTotal {
                    cpu_seconds: 2.,
                    ..Default::default()
                },
            ),
            (
                "alpha".into(),
                ProcessTotal {
                    cpu_seconds: 2.,
                    ..Default::default()
                },
            ),
        ];
        let mut sort = HistorySort::default();
        assert_eq!(sorted_names(sort, &rows), ["alpha", "zulu"]);
        sort.click(HistorySortColumn::CoreTime);
        assert_eq!(sorted_names(sort, &rows), ["alpha", "zulu"]);
    }

    #[test]
    fn keyboard_entry_selects_newest_displayed_column() {
        for (minutes, step) in [(60, 1), (480, 5), (1440, 10)] {
            let view = HistoryView {
                start_minute: 91,
                columns: vec![None; minutes],
                ..Default::default()
            };
            assert_eq!(
                newest_moment(&view),
                Some(MomentSelection {
                    start_minute: 91 + (minutes - step) as u64,
                    step
                })
            );
        }
        assert!(newest_moment(&HistoryView::default()).is_none());
    }

    #[test]
    fn name_limit_counts_the_distinct_union() {
        let mut history = History::default();
        let mut snapshot = crate::test_data::snapshot();
        snapshot.processes = (1..=MAX_NAMES as u32)
            .map(|pid| crate::test_data::process(pid, &format!("process-{pid}"), 1.))
            .collect::<Vec<_>>()
            .into();
        let rest = std::sync::Arc::make_mut(&mut snapshot.processes).split_off(2048);
        history.record(&snapshot, 6000., 3.).unwrap();
        std::sync::Arc::make_mut(&mut snapshot.processes).extend(rest);
        std::sync::Arc::make_mut(&mut snapshot.processes)
            .push(crate::test_data::process(9999, "extra", 1.));
        history.record(&snapshot, 6003., 3.).unwrap();
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
        let detail = history
            .moment(MomentSelection {
                start_minute: 100,
                step: 5,
            })
            .unwrap();
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
        history.record(&snapshot, 6000., 3.).unwrap();
        history.record(&snapshot, 6003., 3.).unwrap();
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
        history.record(&snapshot, 6006., 3.).unwrap();
        assert_eq!(history.buckets[0].gpu, Some(80.));
        assert_eq!(history.buckets[0].gpu_seconds, Some(3.));
        assert_eq!(history.buckets[0].temperature, Some(80.));
        assert_eq!(history.buckets[0].temperature_seconds, Some(3.));
    }

    #[test]
    fn rollover_marks_the_closed_minute_changed() {
        let snapshot = crate::test_data::snapshot();
        let mut history = History::default();
        history.record(&snapshot, 6057., 3.).unwrap();
        history.record(&snapshot, 6060., 3.).unwrap();
        assert!(history.touched.contains(&100));
        assert!(history.touched.contains(&101));
        assert_eq!(
            history
                .moment(MomentSelection {
                    start_minute: 100,
                    step: 1
                })
                .unwrap()
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
            history.record(&snapshot, crate::rack::now(), 1.).unwrap();
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
        h.record(&s, 6000., 3.).unwrap();
        h.record(&s, 6003., 3.).unwrap();
        let v = h.view(1, 6003).unwrap();
        assert_eq!(v.covered, 1);
        assert_eq!(v.rows[0].0, "rustc");
        assert_eq!(v.rows[0].1.cpu_seconds, 10.5);
        assert_eq!(v.rows[0].1.peak_memory, 3072);
        assert_eq!(
            v.cached_leaders[Metric::Cpu as usize],
            v.leaders(Metric::Cpu)
        );
        assert_eq!(v.leaders(Metric::Temperature)[0], ("Peak".into(), 80.));
        h.suspend();
        h.record(&s, 6006., 3.).unwrap();
        assert_eq!(h.view(1, 6006).unwrap().rows[0].1.cpu_seconds, 10.5);
        h.record(&s, 6180., 3.).unwrap();
        let v = h.view(1, 6180).unwrap();
        assert_eq!(v.covered, 2);
        assert!(v.columns[58].is_none());
        assert_eq!(v.rows[0].1.cpu_seconds, 10.5);
        let path =
            std::env::temp_dir().join(format!("gtop-history-test-{}.json", std::process::id()));
        h.save(&path).unwrap();
        let loaded = History::load(&path, 6180);
        std::fs::remove_file(path).unwrap();
        assert_eq!(loaded.view(1, 6180).unwrap().covered, 2);
        assert!(loaded.last_sample.is_none());
    }

    #[test]
    fn stale_rows_do_not_add_work_or_peaks_and_recovery_spans_missed_samples() {
        let mut s = crate::test_data::snapshot();
        s.processes = std::sync::Arc::new(vec![crate::test_data::process(1, "owned", 100.)]);
        let mut h = History::default();
        h.record(&s, 6054., 3.).unwrap();
        h.record(&s, 6057., 3.).unwrap();
        s.process_data = gtop_core::ProcessData::Stale {
            since: std::time::Instant::now(),
            reason: "failed".into(),
        };
        std::sync::Arc::make_mut(&mut s.processes)[0].memory_usage = 99999;
        s.stats.memory_used = 900;
        for now in [6060., 6063.] {
            h.record(&s, now, 3.).unwrap();
            let total = &h.view(1, now as u64).unwrap().rows[0].1;
            assert_eq!(total.cpu_seconds, 3.);
            assert_eq!(total.gpu_seconds, 1.5);
            assert_eq!(total.peak_memory, 1024);
        }
        assert_eq!(h.buckets.back().unwrap().peak_memory, 900.);
        s.process_data = gtop_core::ProcessData::Fresh;
        std::sync::Arc::make_mut(&mut s.processes)[0].cpu_usage = 100. / 3.;
        std::sync::Arc::make_mut(&mut s.processes)[0].cpu_sample_seconds = Some(9.);
        std::sync::Arc::make_mut(&mut s.processes)[0].gpu_usage = Some(100. / 6.);
        std::sync::Arc::make_mut(&mut s.processes)[0].memory_usage = 2048;
        h.record(&s, 6066., 3.).unwrap();
        let total = &h.view(1, 6066).unwrap().rows[0].1;
        assert!((total.cpu_seconds - 6.).abs() < 0.00001);
        assert!((total.gpu_seconds - 3.).abs() < 0.00001);
        assert_eq!(total.peak_memory, 2048);
        assert!((h.buckets[0].processes["owned"].cpu_seconds - 4.).abs() < 0.00001);
        assert_eq!(h.touched, 100..102);
        h.suspend();
        h.record(&s, 6100., 3.).unwrap();
        assert!((h.view(1, 6100).unwrap().rows[0].1.cpu_seconds - 6.).abs() < 0.00001);
    }

    #[test]
    fn recovery_does_not_record_a_process_before_its_start_minute() {
        let mut snapshot = crate::test_data::snapshot();
        snapshot.processes = vec![crate::test_data::process(1, "old", 0.)].into();
        let mut history = History::default();
        for now in [6054., 6057.] {
            history.record(&snapshot, now, 3.).unwrap();
        }
        snapshot.process_data = gtop_core::ProcessData::Stale {
            since: std::time::Instant::now(),
            reason: "failed".into(),
        };
        for now in [6060., 6063.] {
            history.record(&snapshot, now, 3.).unwrap();
        }

        let mut born = crate::test_data::process(2, "born-after-minute-boundary", 0.);
        born.start_time = 6064;
        born.cpu_sample_seconds = None;
        born.memory_usage = 16 * 1024 * 1024;
        born.gpu_usage = Some(50.);
        std::sync::Arc::make_mut(&mut snapshot.processes).push(born);
        snapshot.process_data = gtop_core::ProcessData::Fresh;
        history.record(&snapshot, 6066., 3.).unwrap();

        assert!(!history.buckets[0]
            .processes
            .contains_key("born-after-minute-boundary"));
        let current = &history.buckets[1].processes["born-after-minute-boundary"];
        assert_eq!(current.peak_memory, 16 * 1024 * 1024);
        assert!((current.gpu_seconds - 1.).abs() < 0.00001);
    }

    #[test]
    fn recovery_does_not_restore_process_buckets_outside_retained_history() {
        let mut s = crate::test_data::snapshot();
        s.processes = vec![crate::test_data::process(1, "owned", 100.)].into();
        let mut h = History::default();
        h.record(&s, 6000., 60.).unwrap();
        s.process_data = gtop_core::ProcessData::Stale {
            since: std::time::Instant::now(),
            reason: "failed".into(),
        };
        for tick in 1..=MINUTES + 2 {
            h.record(&s, (6000 + tick * 60) as f64, 60.).unwrap();
        }
        s.process_data = gtop_core::ProcessData::Fresh;
        let now = 6000 + (MINUTES + 3) * 60;
        std::sync::Arc::make_mut(&mut s.processes)[0].cpu_sample_seconds =
            Some((now - 6000) as f64);
        h.record(&s, now as f64, 60.).unwrap();
        assert_eq!(h.buckets.len(), MINUTES as usize);
        assert_eq!(h.buckets.front().unwrap().minute, now / 60 - MINUTES + 1);
        assert_eq!(
            h.view(24, now).unwrap().total_cpu,
            (MINUTES - 1) as f64 * 60.
        );
    }

    #[test]
    fn partial_cpu_recovery_charges_each_rows_valid_span() {
        let mut snapshot = crate::test_data::snapshot();
        snapshot.processes = vec![
            crate::test_data::process(1, "recovering", 0.),
            crate::test_data::process(2, "steady", 100.),
        ]
        .into();
        let mut history = History::default();
        history.record(&snapshot, 6054., 3.).unwrap();
        history.record(&snapshot, 6057., 3.).unwrap();
        for now in [6060., 6063.] {
            std::sync::Arc::make_mut(&mut snapshot.processes)[0].cpu_sample_seconds = None;
            history.record(&snapshot, now, 3.).unwrap();
        }
        let row = &mut std::sync::Arc::make_mut(&mut snapshot.processes)[0];
        row.cpu_usage = 100. / 3.;
        row.cpu_sample_seconds = Some(9.);
        std::sync::Arc::make_mut(&mut snapshot.processes)
            .push(crate::test_data::process(3, "new", 0.));
        history.record(&snapshot, 6066., 3.).unwrap();
        assert!(!history.buckets[0].processes.contains_key("new"));
        let view = history.view(1, 6066).unwrap();
        let recovered = view.rows.iter().find(|r| r.0 == "recovering").unwrap();
        assert!((recovered.1.cpu_seconds - 3.).abs() < 0.00001);
        let steady = view.rows.iter().find(|r| r.0 == "steady").unwrap();
        assert_eq!(steady.1.cpu_seconds, 12.);
        assert!((history.buckets[0].processes["recovering"].cpu_seconds - 1.).abs() < 0.00001);
        history.suspend();
        history.record(&snapshot, 6070., 3.).unwrap();
        assert!((history.view(1, 6070).unwrap().total_cpu - 15.).abs() < 0.00001);
    }

    #[test]
    fn process_peak_cpu_sums_same_name_processes_in_each_sample() {
        let snapshot = crate::test_data::snapshot();
        let mut history = History::default();
        history.record(&snapshot, 6000., 3.).unwrap();
        assert_eq!(history.buckets[0].processes["rustc"].peak_cpu, 350.);
        assert_eq!(history.buckets[0].processes["rustc"].peak_memory, 3072);
        assert_eq!(history.buckets[0].processes["cargo"].peak_cpu, 110.);
    }
    #[test]
    fn elapsed_time_splits_at_minute_boundaries() {
        let s = crate::test_data::snapshot();
        let mut h = History::default();
        h.record(&s, 6059., 3.).unwrap();
        h.record(&s, 6062., 3.).unwrap();
        let v = h.view(1, 6062).unwrap();
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
        let view = h.view(24, 1500 * 60).unwrap();
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
        let detail = h
            .moment(MomentSelection {
                start_minute: 10,
                step: 2,
            })
            .unwrap();
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
        let detail = History::default()
            .moment(MomentSelection {
                start_minute: 42,
                step: 10,
            })
            .unwrap();
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
        let detail = history
            .moment(MomentSelection {
                start_minute: 100,
                step: 1,
            })
            .unwrap();
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
        let before = history.view(24, 2000 * 60).unwrap();
        history.buckets.push_back(Bucket {
            minute: 2001,
            ..Default::default()
        });
        let after = history.view(24, 2001 * 60).unwrap();
        assert_ne!(before.start_minute, after.start_minute);
        assert_eq!(selection.start_minute, 600);
        assert!(selection_in_window(
            selection,
            after.start_minute,
            after.start_minute + after.columns.len() as u64
        ));
    }
}
