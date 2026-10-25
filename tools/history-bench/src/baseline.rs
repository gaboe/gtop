use crate::history::{Bucket, Column, HistoryView, Metric, ProcessTotal};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
const MINUTES: u64 = 1440;
#[derive(Default, Serialize, Deserialize)]
pub struct History {
    pub buckets: VecDeque<Bucket>,
}
impl History {
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
                    let total = totals.entry(name.clone()).or_default();
                    total.cpu_seconds += p.cpu_seconds;
                    total.gpu_seconds += p.gpu_seconds;
                    total.peak_memory = total.peak_memory.max(p.peak_memory);
                    total.peak_cpu = total.peak_cpu.max(p.peak_cpu);
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
}
