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
pub struct Bucket {
    pub minute: u64,
    pub seconds: f64,
    pub cpu: f64,
    pub memory: f64,
    pub gpu: Option<f64>,
    pub temperature: Option<f64>,
    pub peak_temperature: Option<f64>,
    pub processes: BTreeMap<String, ProcessTotal>,
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
        }
    }
}
impl Column {
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
    pub rows: Vec<(String, ProcessTotal)>,
    pub total_cpu: f64,
    pub covered: usize,
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
                .map(|b| b.temperature.unwrap() * b.seconds)
                .sum();
            let seconds: f64 = live.iter().map(|b| b.seconds).sum();
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
}
impl History {
    pub fn load(path: &std::path::Path, now: u64) -> Self {
        let mut history: Self = std::fs::metadata(path)
            .ok()
            .filter(|m| m.len() < 512 * 1024 * 1024)
            .and_then(|_| std::fs::read(path).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        history.prune(now / 60);
        history
    }
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("tmp");
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&temp)?);
        serde_json::to_writer(&mut writer, self)?;
        std::io::Write::flush(&mut writer)?;
        std::fs::rename(temp, path)
    }
    fn prune(&mut self, minute: u64) {
        self.buckets
            .retain(|b| b.minute <= minute && b.minute > minute.saturating_sub(MINUTES));
        while self.buckets.len() > MINUTES as usize {
            self.buckets.pop_front();
        }
    }
    pub fn record(&mut self, snapshot: &Snapshot, now: f64, interval: f64) -> bool {
        let previous = self.last_sample.replace(now);
        let elapsed = previous.map(|v| now - v).unwrap_or(0.);
        let minute = now as u64 / 60;
        self.prune(minute);
        let changed_minute = self.buckets.back().is_none_or(|b| b.minute != minute);
        // Do not charge time while paused, covered, asleep, or between application runs.
        let seconds = if elapsed > 0. && elapsed <= interval * 2.5 {
            elapsed
        } else {
            0.
        };
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
        b.cpu = mean(
            b.cpu,
            snapshot
                .stats
                .cpu_usage
                .iter()
                .map(|v| *v as f64)
                .sum::<f64>()
                / snapshot.stats.cpu_usage.len().max(1) as f64,
        );
        b.memory = mean(b.memory, snapshot.stats.memory_used as f64);
        if let Some(v) = snapshot.stats.gpus.first().and_then(|g| g.utilization) {
            b.gpu = Some(mean(b.gpu.unwrap_or(0.), v as f64));
        }
        if let Some(v) = snapshot
            .stats
            .temperatures
            .iter()
            .find(|t| t.name == "CPU")
            .map(|t| t.celsius as f64)
        {
            b.temperature = Some(mean(b.temperature.unwrap_or(0.), v));
            b.peak_temperature = Some(b.peak_temperature.unwrap_or(v).max(v));
        }
        for p in &snapshot.processes {
            if !b.processes.contains_key(&p.name) && b.processes.len() >= MAX_NAMES {
                continue;
            }
            b.processes
                .entry(p.name.clone())
                .or_default()
                .merge(&ProcessTotal {
                    cpu_seconds: p.cpu_usage as f64 / 100. * seconds,
                    gpu_seconds: p.gpu_usage.unwrap_or(0.) as f64 / 100. * seconds,
                    peak_memory: p.memory_usage,
                    peak_cpu: p.cpu_usage,
                });
        }
    }

    pub fn view(&self, hours: usize, now: u64) -> HistoryView {
        let minute = now / 60;
        let start = minute.saturating_sub((hours * 60 - 1) as u64);
        let mut result = HistoryView::default();
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
        result
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
        assert_eq!(v.rows[0].1.peak_memory, 2048);
        assert_eq!(v.leaders(Metric::Temperature)[0], ("Peak".into(), 80.));
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
}
